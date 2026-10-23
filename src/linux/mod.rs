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
pub(crate) mod host;
pub(crate) mod os;

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

#[cfg(test)]
pub(crate) mod fixtures {
    //! Helpers for tests that run the Linux backend against captured machines.

    use super::Root;

    pub(crate) fn root(name: &str) -> Root {
        Root::at(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/").to_owned() + name)
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
        const EXACT: [&str; 10] = [
            "/etc/os-release",
            "/usr/lib/os-release",
            "/proc/cpuinfo",
            "/proc/self/cgroup",
            "/proc/self/mountinfo",
            "/proc/self/status",
            "/proc/sys/kernel/hostname",
            "/proc/sys/kernel/osrelease",
            "/sys/devices/system/cpu/present",
            "/sys/fs/cgroup",
        ];
        const CGROUP_FILES: [&str; 6] = [
            "cpu.max",
            "cpuset.cpus",
            "cpuset.cpus.effective",
            "cpuset.effective_cpus",
            "cpu.cfs_quota_us",
            "cpu.cfs_period_us",
        ];
        if path.split('/').any(|c| c == "..") {
            return false;
        }
        if EXACT.contains(&path) || crate::parse::release::FILES.iter().any(|(f, _)| *f == path) {
            return true;
        }
        if let Some(rest) = path.strip_prefix("/sys/devices/system/cpu/cpu") {
            let (n, file) = rest.split_once('/').unwrap_or((rest, ""));
            return !n.is_empty()
                && n.bytes().all(|b| b.is_ascii_digit())
                && matches!(
                    file,
                    "topology/core_cpus_list" | "topology/thread_siblings_list"
                );
        }
        path.starts_with("/sys/fs/cgroup/")
            && path
                .rsplit('/')
                .next()
                .is_some_and(|f| CGROUP_FILES.contains(&f))
    }

    #[test]
    fn test_linux_backend_opens_only_reviewed_files() {
        // T4: no path comes from the environment or from file content beyond the
        // cgroup path, and every file opened is on the reviewed list.
        for name in fixtures::ALL {
            let (root, log) = fixtures::root(name).recording();
            let _ = os::read(&root);
            let facts = cpu::read(&root);
            let _ = cpu::available(&root, facts.logical.unwrap_or(1));
            let _ = cpu::quota(&root);
            let _ = host::name(&root);
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
            .filter(|p| p.contains("/topology/"))
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
    fn test_root_link_targets_come_from_link_files_in_fixtures() {
        let root = fixtures::root("wsl2");
        assert_eq!(
            root.link_target("/etc/localtime").as_deref(),
            Some("/usr/share/zoneinfo/Europe/London")
        );
        assert_eq!(root.link_target("/etc/none"), None);
    }
}
