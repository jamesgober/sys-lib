//! The Linux backend's file-tree logic: everything Linux answers through
//! `/proc`, `/sys`, and `/etc`, read through a [`Root`] so the same code runs
//! against the real filesystem and against captured fixture machines on any
//! host. No unsafe code: the few libc calls live in `crate::sys::linux`.

// On other hosts this module exists for the fixture tests only.
#![cfg_attr(
    not(all(
        any(target_os = "linux", target_os = "android"),
        target_pointer_width = "64"
    )),
    allow(dead_code)
)]

pub(crate) mod cgroup;
pub(crate) mod cpu;
pub(crate) mod disk;
pub(crate) mod host;
pub(crate) mod mem;
pub(crate) mod net;
pub(crate) mod os;
pub(crate) mod process;
pub(crate) mod stat;

use core::ops::ControlFlow;
use std::{
    borrow::Cow,
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use crate::{Error, Result};

/// The largest file read whole (threats T6 and T7).
pub(crate) const FILE_LIMIT: usize = 1 << 20;
/// Files that fit here are read without touching the heap.
const STACK_BUF: usize = 8 << 10;

/// Where the Linux file tree is: the real `/`, or a fixture directory.
#[derive(Clone, Debug)]
pub(crate) struct Root {
    base: Option<PathBuf>,
    /// Every path asked for, in tests that check the set (threat T4).
    #[cfg(test)]
    log: Option<std::sync::Arc<std::sync::Mutex<Vec<String>>>>,
}

impl Root {
    /// The real filesystem.
    pub(crate) const fn system() -> Self {
        Self {
            base: None,
            #[cfg(test)]
            log: None,
        }
    }

    /// A captured tree (`tests/fixtures/<name>`), or a host tree mounted elsewhere.
    #[cfg_attr(not(test), allow(dead_code))] // the fixtures; a host root from 0.4
    pub(crate) fn at(base: impl Into<PathBuf>) -> Self {
        Self {
            base: Some(base.into()),
            #[cfg(test)]
            log: None,
        }
    }

    /// This root, recording every path the backend asks for.
    #[cfg(test)]
    pub(crate) fn recording(mut self) -> (Self, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let log = std::sync::Arc::default();
        self.log = Some(std::sync::Arc::clone(&log));
        (self, log)
    }

    fn resolve<'a>(&self, abs: &'a str) -> Cow<'a, Path> {
        #[cfg(test)]
        if let Some(log) = &self.log {
            log.lock().unwrap().push(abs.to_owned());
        }
        match &self.base {
            None => Cow::Borrowed(Path::new(abs)),
            Some(base) => Cow::Owned(base.join(abs.trim_start_matches('/'))),
        }
    }

    /// Reads a whole file, at most `limit` bytes; longer is `TooLarge`.
    pub(crate) fn read(
        &self,
        abs: &str,
        limit: usize,
        what: &'static str,
        source: &'static str,
    ) -> Result<Vec<u8>> {
        let mut file = File::open(self.resolve(abs)).map_err(|e| Error::io(what, source, e))?;
        let mut data = Vec::new();
        file.by_ref()
            .take(limit as u64 + 1)
            .read_to_end(&mut data)
            .map_err(|e| Error::io(what, source, e))?;
        if data.len() > limit {
            return Err(Error::TooLarge {
                what,
                limit: limit as u64,
            });
        }
        Ok(data)
    }

    /// Reads the first `limit` bytes of a file (for files where a prefix is
    /// enough, such as `/proc/cpuinfo` on a host with thousands of CPUs).
    pub(crate) fn read_prefix(
        &self,
        abs: &str,
        limit: usize,
        what: &'static str,
        source: &'static str,
    ) -> Result<Vec<u8>> {
        let file = File::open(self.resolve(abs)).map_err(|e| Error::io(what, source, e))?;
        let mut data = Vec::new();
        file.take(limit as u64)
            .read_to_end(&mut data)
            .map_err(|e| Error::io(what, source, e))?;
        Ok(data)
    }

    /// Reads a small file into a stack buffer and hands it to `f`; a file that
    /// does not fit is read from the heap, bounded by [`FILE_LIMIT`].
    pub(crate) fn with<T>(
        &self,
        abs: &str,
        what: &'static str,
        source: &'static str,
        f: impl FnOnce(&[u8]) -> T,
    ) -> Result<T> {
        let mut buf = [0u8; STACK_BUF];
        let mut file = File::open(self.resolve(abs)).map_err(|e| Error::io(what, source, e))?;
        let mut len = 0;
        loop {
            match file.read(&mut buf[len..]) {
                Ok(0) => return Ok(f(&buf[..len])),
                Ok(n) => {
                    len += n;
                    if len == buf.len() {
                        break;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::io(what, source, e)),
            }
        }
        // Larger than the stack buffer: read it whole, bounded.
        let data = self.read(abs, FILE_LIMIT, what, source)?;
        Ok(f(&data))
    }

    /// Reads at most the first 8 KiB of a file into a stack buffer and hands
    /// them to `f`: for files where the head is enough (the total line of
    /// `/proc/stat` on a host with hundreds of CPUs, E-27). No heap.
    pub(crate) fn with_head<T>(
        &self,
        abs: &str,
        what: &'static str,
        source: &'static str,
        f: impl FnOnce(&[u8]) -> T,
    ) -> Result<T> {
        let mut buf = [0u8; STACK_BUF];
        let mut file = File::open(self.resolve(abs)).map_err(|e| Error::io(what, source, e))?;
        let mut len = 0;
        while len < buf.len() {
            match file.read(&mut buf[len..]) {
                Ok(0) => break,
                Ok(n) => len += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(Error::io(what, source, e)),
            }
        }
        Ok(f(&buf[..len]))
    }

    /// Calls `f` with each line of a file (without the newline), read through
    /// an 8 KiB stack buffer, so a large file costs no heap. A line longer than
    /// the buffer is skipped. `f` stops early by returning `Break`.
    ///
    /// # Errors
    ///
    /// `TooLarge` when the file is longer than `limit` bytes (T7), checked as
    /// it is read.
    pub(crate) fn for_each_line(
        &self,
        abs: &str,
        limit: usize,
        what: &'static str,
        source: &'static str,
        mut f: impl FnMut(&[u8]) -> ControlFlow<()>,
    ) -> Result<()> {
        let mut buf = [0u8; STACK_BUF];
        let mut file = File::open(self.resolve(abs)).map_err(|e| Error::io(what, source, e))?;
        let (mut start, mut end, mut total) = (0usize, 0usize, 0usize);
        let mut skipping = false;
        loop {
            if start > 0 {
                buf.copy_within(start..end, 0);
                end -= start;
                start = 0;
            }
            if end == buf.len() {
                // A line that fills the whole buffer: drop it, up to its newline.
                skipping = true;
                end = 0;
            }
            let n = match file.read(&mut buf[end..]) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(Error::io(what, source, e)),
            };
            total += n;
            if total > limit {
                return Err(Error::TooLarge {
                    what,
                    limit: limit as u64,
                });
            }
            if n == 0 {
                if end > start && !skipping {
                    let _ = f(strip_cr(&buf[start..end]));
                }
                return Ok(());
            }
            let mut search = end;
            end += n;
            while let Some(pos) = buf[search..end].iter().position(|&b| b == b'\n') {
                let line_end = search + pos;
                if skipping {
                    skipping = false;
                } else if f(strip_cr(&buf[start..line_end])).is_break() {
                    return Ok(());
                }
                start = line_end + 1;
                search = start;
            }
        }
    }

    /// A small file's trimmed text, or `None` if it is missing or unreadable.
    pub(crate) fn text(&self, abs: &str) -> Option<String> {
        self.with(abs, "file", "file", |d| {
            super::parse::lossy(super::parse::trim(d), 4096)
        })
        .ok()
    }

    #[allow(dead_code)] // container and virtualization markers, 0.4
    pub(crate) fn exists(&self, abs: &str) -> bool {
        self.resolve(abs).exists()
    }

    /// The target of a symbolic link. Fixture trees hold no real links; they
    /// record the target in `<path>.link`.
    #[cfg_attr(not(test), allow(dead_code))] // the time zone and block devices, 0.4
    pub(crate) fn link_target(&self, abs: &str) -> Option<String> {
        #[cfg(test)]
        if let Some(log) = &self.log {
            log.lock().unwrap().push(abs.to_owned());
        }
        match &self.base {
            None => std::fs::read_link(abs)
                .ok()
                .and_then(|p| p.to_str().map(str::to_owned)),
            Some(_) => self.text(&format!("{abs}.link")),
        }
    }

    /// The names in a directory, at most `max`, sorted.
    #[allow(dead_code)] // disks, interfaces, and sensors, 0.4
    pub(crate) fn dir_names(&self, abs: &str, max: usize) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.resolve(abs)) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .take(max)
            .collect();
        names.sort();
        names
    }
}

fn strip_cr(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// `prefix` + `name` + `suffix` as a path in `buf`, without the heap: `None`
/// when it does not fit or `name` could escape its directory (a `/`, `..`, or
/// NUL: names from the OS are validated before they build a path).
pub(crate) fn join_name<'b>(
    buf: &'b mut [u8],
    prefix: &str,
    name: &[u8],
    suffix: &str,
) -> Option<&'b str> {
    if name.is_empty() || name == b"." || name == b".." || name.iter().any(|&b| b == b'/' || b == 0)
    {
        return None;
    }
    let len = prefix.len() + name.len() + suffix.len();
    if len > buf.len() {
        return None;
    }
    buf[..prefix.len()].copy_from_slice(prefix.as_bytes());
    buf[prefix.len()..prefix.len() + name.len()].copy_from_slice(name);
    buf[prefix.len() + name.len()..len].copy_from_slice(suffix.as_bytes());
    std::str::from_utf8(&buf[..len]).ok()
}

#[cfg(test)]
pub(crate) mod fixtures {
    //! Helpers for tests that run the Linux backend against captured machines.

    use super::Root;

    pub(crate) fn root(name: &str) -> Root {
        Root::at(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/").to_owned() + name)
    }

    /// A fixture's cgroups, resolved the way the system root's are.
    pub(crate) fn cgroups(name: &str) -> super::cgroup::Cgroups {
        super::cgroup::Cgroups::resolve(&root(name))
    }

    pub(crate) const ALL: [&str; 5] = [
        "wsl2",
        "wsl2-systemd-scope-limited",
        "docker-cgroup2-limited",
        "docker-cgroup1-limited",
        "minimal-scratch",
    ];

    /// The output of a tool recorded in a fixture's `TOOLS.txt`.
    pub(crate) fn tool(name: &str, command: &str) -> Option<String> {
        let text = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/").to_owned()
                + name
                + "/TOOLS.txt",
        )
        .ok()?;
        let marker = format!("## {command}\n");
        let start = text.find(&marker)? + marker.len();
        let rest = &text[start..];
        Some(
            rest[..rest.find("\n\n").unwrap_or(rest.len())]
                .trim()
                .to_owned(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The files the 0.3 readings may open. A new read shows up here as a test
    /// failure, so it gets reviewed (threat T4; PLAN.md section 22).
    fn reviewed(path: &str) -> bool {
        const EXACT: &[&str] = &[
            "/etc/os-release",
            "/usr/lib/os-release",
            "/proc/cpuinfo",
            "/proc/meminfo",
            "/proc/stat",
            "/proc/loadavg",
            "/proc/uptime",
            "/proc/vmstat",
            "/proc/diskstats",
            "/proc/net/dev",
            "/proc/net/snmp",
            "/proc/net/sockstat",
            "/proc/net/sockstat6",
            "/proc/pressure/cpu",
            "/proc/pressure/memory",
            "/proc/pressure/io",
            "/proc/self/cgroup",
            "/proc/self/mountinfo",
            "/proc/self/status",
            "/proc/self/statm",
            "/proc/self/stat",
            "/proc/self/io",
            "/proc/sys/kernel/hostname",
            "/proc/sys/kernel/osrelease",
            "/proc/sys/fs/file-nr",
            "/sys/devices/system/cpu/present",
            "/sys/devices/system/cpu/online",
            "/sys/kernel/mm/transparent_hugepage/enabled",
            "/sys/fs/cgroup",
        ];
        const CGROUP_FILES: &[&str] = &[
            "cpu.max",
            "cpuset.cpus",
            "cpuset.cpus.effective",
            "cpuset.effective_cpus",
            "cpu.cfs_quota_us",
            "cpu.cfs_period_us",
            "memory.max",
            "memory.swap.max",
            "memory.high",
            "memory.current",
            "memory.stat",
            "memory.events",
            "memory.soft_limit_in_bytes",
            "memory.usage_in_bytes",
            "memory.memsw.limit_in_bytes",
            "memory.oom_control",
            "cpu.pressure",
            "memory.pressure",
            "io.pressure",
        ];
        const CPU_FILES: &[&str] = &[
            "topology/core_cpus_list",
            "topology/thread_siblings_list",
            "topology/physical_package_id",
            "cpufreq/base_frequency",
            "cpufreq/cpuinfo_max_freq",
            "cpufreq/scaling_cur_freq",
        ];
        const NET_FILES: &[&str] = &[
            "addr_assign_type",
            "operstate",
            "mtu",
            "speed",
            "type",
            "address",
            "wireless",
            "phy80211",
            "bridge",
            "device",
        ];
        if path.split('/').any(|c| c == "..") {
            return false;
        }
        if EXACT.contains(&path) || crate::parse::release::FILES.iter().any(|(f, _)| *f == path) {
            return true;
        }
        // One validated name, then one of the listed files.
        let named = |prefix: &str, files: &[&str], name_ok: &dyn Fn(&str) -> bool| {
            path.strip_prefix(prefix)
                .is_some_and(|rest| match rest.split_once('/') {
                    Some((name, file)) => name_ok(name) && files.contains(&file),
                    None => name_ok(rest) && files.is_empty(),
                })
        };
        let digits = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
        let iface = |n: &str| !n.is_empty() && n.len() <= 15 && !n.contains('/');
        named("/sys/devices/system/cpu/cpu", CPU_FILES, &digits)
            || named("/sys/class/net/", NET_FILES, &iface)
            || named("/sys/block/", &[], &iface)
            || named("/sys/block/", &["removable"], &iface)
            || (path.starts_with("/sys/fs/cgroup/")
                && path
                    .rsplit('/')
                    .next()
                    .is_some_and(|f| CGROUP_FILES.contains(&f)))
    }

    #[test]
    fn test_linux_backend_opens_only_reviewed_files() {
        // T4: no path comes from the environment or from file content beyond the
        // cgroup path and names the kernel lists, and every file opened is on
        // the reviewed list.
        use crate::disk::ListOptions;
        for name in fixtures::ALL {
            let (root, log) = fixtures::root(name).recording();
            let cg = cgroup::Cgroups::resolve(&root);
            let paths = mem::MemPaths::new(&cg);
            let _ = os::read(&root);
            let facts = cpu::read(&root);
            let _ = cpu::available(&root, &cg, facts.logical.unwrap_or(1));
            let _ = cpu::quota(&root, &cg);
            let _ = cpu::online(&root);
            let _ = cpu::frequency(&root);
            let _ = cpu::current_frequency(&root);
            let _ = host::name(&root);
            let _ = host::open_files(&root);
            let _ = mem::read(&root, paths.as_ref(), false, true);
            let _ = mem::events(&root, paths.as_ref());
            let _ = mem::paging(&root, 4096);
            let _ = mem::huge_pages(&root);
            let mut times = vec![crate::cpu::Times::default(); 64];
            let _ = stat::cpu_times(&root, 100);
            let _ = stat::cpu_times_per_core(&root, 100, &mut times);
            let _ = stat::load(&root);
            let _ = stat::boot_time(&root);
            let _ = stat::uptime(&root);
            let _ = stat::procs_running(&root);
            for r in [
                stat::Resource::Cpu,
                stat::Resource::Memory,
                stat::Resource::Io,
            ] {
                let _ = stat::pressure(&root, &cg, r);
            }
            let _ = disk::mounts(&root, &ListOptions::default());
            let _ = disk::io_totals(&root);
            let _ = disk::io_counters(&root);
            let _ = net::totals(&root);
            let _ = net::counters(&root, 4096);
            let _ = net::connections(&root);
            let _ = net::details(&root, b"eth0", None);
            let _ = process::memory(&root, 4096);
            let _ = process::memory_detail(&root);
            let _ = process::io(&root);
            let opened = log.lock().unwrap().clone();
            assert!(!opened.is_empty());
            for path in &opened {
                assert!(reviewed(path), "{name}: unreviewed read of {path}");
            }
        }
    }

    #[test]
    fn test_linux_backend_reads_each_core_group_once() {
        // wsl2 has 32 logical CPUs in 16 two-thread cores: 16 topology reads.
        let (root, log) = fixtures::root("wsl2").recording();
        let facts = cpu::read(&root);
        let topology = log
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.ends_with("/topology/core_cpus_list"))
            .count();
        assert_eq!(facts.logical, Some(32));
        assert_eq!(topology, facts.physical.unwrap());
    }

    #[test]
    fn test_root_read_enforces_the_limit() {
        let root = fixtures::root("wsl2");
        let data = root
            .read("/proc/cpuinfo", FILE_LIMIT, "cpuinfo", "/proc/cpuinfo")
            .unwrap();
        assert!(data.len() > 1000);
        let err = root
            .read("/proc/cpuinfo", 100, "cpuinfo", "/proc/cpuinfo")
            .unwrap_err();
        assert!(matches!(err, Error::TooLarge { limit: 100, .. }));
        assert_eq!(
            root.read_prefix("/proc/cpuinfo", 100, "cpuinfo", "/proc/cpuinfo")
                .unwrap()
                .len(),
            100
        );
    }

    #[test]
    fn test_root_missing_file_is_not_found() {
        let err = fixtures::root("wsl2")
            .read("/proc/nope", 10, "x", "/proc/nope")
            .unwrap_err();
        assert!(matches!(
            err,
            Error::NotFound {
                source: "/proc/nope",
                ..
            }
        ));
    }

    #[test]
    fn test_root_with_uses_the_heap_past_the_stack_buffer() {
        let root = fixtures::root("wsl2");
        let len = root
            .with("/proc/cpuinfo", "cpuinfo", "/proc/cpuinfo", <[u8]>::len)
            .unwrap();
        assert!(len > STACK_BUF);
        let small = root
            .with("/proc/sys/kernel/hostname", "h", "h", <[u8]>::to_vec)
            .unwrap();
        assert_eq!(small, b"fixture-host\n");
    }

    #[test]
    fn test_root_for_each_line_matches_a_whole_read_and_stops_early() {
        let root = fixtures::root("wsl2");
        let whole = root.read("/proc/cpuinfo", FILE_LIMIT, "c", "c").unwrap();
        let mut lines = Vec::new();
        root.for_each_line("/proc/cpuinfo", FILE_LIMIT, "c", "c", |l| {
            lines.push(l.to_vec());
            ControlFlow::Continue(())
        })
        .unwrap();
        let expected: Vec<Vec<u8>> = whole.split(|&b| b == b'\n').map(<[u8]>::to_vec).collect();
        // A trailing newline gives no extra empty line.
        let expected = &expected[..expected.len() - usize::from(whole.ends_with(b"\n"))];
        assert_eq!(lines, expected);
        let mut seen = 0;
        root.for_each_line("/proc/cpuinfo", FILE_LIMIT, "c", "c", |_| {
            seen += 1;
            if seen == 3 {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        })
        .unwrap();
        assert_eq!(seen, 3);
        assert!(matches!(
            root.for_each_line(
                "/proc/cpuinfo",
                100,
                "c",
                "c",
                |_| ControlFlow::Continue(())
            ),
            Err(Error::TooLarge { limit: 100, .. })
        ));
        let head = root
            .with_head("/proc/cpuinfo", "c", "c", <[u8]>::len)
            .unwrap();
        assert_eq!(head, whole.len().min(STACK_BUF));
    }

    #[test]
    fn test_root_join_name_refuses_escapes_and_overflow() {
        let mut buf = [0u8; 32];
        assert_eq!(
            join_name(&mut buf, "/sys/block/", b"sda", "/removable"),
            Some("/sys/block/sda/removable")
        );
        let mut buf = [0u8; 32];
        for bad in [&b".."[..], b".", b"a/b", b"", b"a\0b"] {
            assert_eq!(join_name(&mut buf, "/sys/block/", bad, ""), None);
        }
        assert_eq!(
            join_name(
                &mut buf,
                "/sys/class/net/",
                b"a-very-long-interface-name",
                "/mtu"
            ),
            None
        );
    }

    #[test]
    fn test_root_link_targets_come_from_link_files_in_fixtures() {
        let root = fixtures::root("wsl2");
        assert_eq!(
            root.link_target("/etc/localtime").as_deref(),
            Some("/usr/share/zoneinfo/Europe/London")
        );
        assert_eq!(root.link_target("/etc/none"), None);
    }
}
