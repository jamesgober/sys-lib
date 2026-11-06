//! This process on Linux: `/proc/self/statm`, `status`, `stat`, and `io`. Only
//! `/proc/self` is read (threat T5: no other process's files, so no pid reuse).

use core::time::Duration;
use std::time::SystemTime;

use super::Root;
use crate::{
    Bytes, Error, Result,
    parse::{parse_u64, proc_status, procfs, trim},
    process::{LaunchFacts, Launcher, ProcessIo, ProcessMemory},
    virt::Strength,
};

fn malformed(source: &'static str) -> Error {
    Error::Malformed {
        what: "process",
        source,
        line: None,
    }
}

/// A `status` field such as `VmHWM:   2048 kB`, in bytes.
fn kib(data: &[u8], key: &[u8]) -> Option<u64> {
    let value = proc_status::field(data, key)?;
    let digits = trim(value.strip_suffix(b"kB").unwrap_or(value));
    parse_u64(digits)?.checked_mul(1024)
}

fn status<T>(root: &Root, f: impl FnOnce(&[u8]) -> T) -> Result<T> {
    root.with("/proc/self/status", "process", "/proc/self/status", f)
}

/// Resident memory: `/proc/self/statm` field 2 × the page size.
pub(crate) fn memory(root: &Root, page_size: u64) -> Result<Bytes> {
    let (_, resident) = root
        .with(
            "/proc/self/statm",
            "process memory",
            "/proc/self/statm",
            procfs::statm,
        )?
        .ok_or_else(|| malformed("/proc/self/statm"))?;
    Ok(Bytes::new(resident.saturating_mul(page_size)))
}

/// The peak resident size: `VmHWM`.
pub(crate) fn peak_memory(root: &Root) -> Result<Bytes> {
    status(root, |d| kib(d, b"VmHWM"))?
        .map(Bytes::new)
        .ok_or_else(|| malformed("/proc/self/status"))
}

/// The memory detail, from one read of `status`.
pub(crate) fn memory_detail(root: &Root) -> Result<ProcessMemory> {
    status(root, |d| {
        let shared = match (kib(d, b"RssFile"), kib(d, b"RssShmem")) {
            (None, None) => None,
            (a, b) => Some(Bytes::new(a.unwrap_or(0).saturating_add(b.unwrap_or(0)))),
        };
        Some(ProcessMemory {
            resident: Bytes::new(kib(d, b"VmRSS")?),
            peak_resident: Bytes::new(kib(d, b"VmHWM")?),
            virtual_size: Bytes::new(kib(d, b"VmSize")?),
            private: kib(d, b"RssAnon").map(Bytes::new),
            shared,
            swapped: kib(d, b"VmSwap").map(Bytes::new),
        })
    })?
    .ok_or_else(|| malformed("/proc/self/status"))
}

/// A numeric `status` field (`PPid`, `Threads`).
pub(crate) fn status_number(root: &Root, key: &[u8]) -> Result<Option<u64>> {
    status(root, |d| proc_status::number(d, key))
}

/// When this process started: `stat` field 22 (ticks since boot) after the
/// boot time.
pub(crate) fn started_at(root: &Root, ticks_per_sec: u64, boot: SystemTime) -> Result<SystemTime> {
    let ticks = root
        .with(
            "/proc/self/stat",
            "process start",
            "/proc/self/stat",
            procfs::stat_start_ticks,
        )?
        .ok_or_else(|| malformed("/proc/self/stat"))?;
    let hz = ticks_per_sec.max(1);
    let since_boot =
        Duration::from_secs(ticks / hz) + Duration::from_nanos((ticks % hz) * 1_000_000_000 / hz);
    boot.checked_add(since_boot)
        .ok_or_else(|| malformed("/proc/self/stat"))
}

/// Storage I/O: `/proc/self/io` (`read_bytes`, `write_bytes` count storage;
/// `syscr`, `syscw` count calls).
pub(crate) fn io(root: &Root) -> Result<ProcessIo> {
    root.with("/proc/self/io", "process I/O", "/proc/self/io", |d| {
        Some(ProcessIo {
            read: Bytes::new(proc_status::number(d, b"read_bytes")?),
            written: Bytes::new(proc_status::number(d, b"write_bytes")?),
            read_ops: proc_status::number(d, b"syscr"),
            write_ops: proc_status::number(d, b"syscw"),
            storage_only: true,
        })
    })?
    .ok_or_else(|| malformed("/proc/self/io"))
}

/// The systemd unit in a `/proc/self/cgroup` path: its last component, when
/// that is a `.service`.
pub(crate) fn service_unit(cgroup: &str) -> Option<&str> {
    cgroup
        .lines()
        .filter_map(|l| l.rsplit(':').next()?.rsplit('/').next())
        .find(|unit| unit.ends_with(".service") && unit.len() <= 256)
}

/// How this process was started. The cgroup is set by the service manager; the
/// environment alone is weak evidence.
pub(crate) fn launch(root: &Root, env: LaunchEnv, pid: u32) -> LaunchFacts {
    let cgroup = root.text("/proc/self/cgroup").unwrap_or_default();
    let unit = service_unit(&cgroup).map(str::to_owned);
    let cron = matches!(unit.as_deref(), Some("cron.service" | "crond.service"));
    let service = match (&unit, env.invocation) {
        _ if cron => None,
        (Some(_), _) => Some((Launcher::Systemd, unit.clone(), Strength::Strong)),
        (None, true) => Some((Launcher::Systemd, None, Strength::Weak)),
        (None, false) => None,
    };
    LaunchFacts {
        service,
        is_pid1: pid == 1,
        notify_socket: env.notify_socket,
        ssh: env.ssh,
        cron,
    }
}

/// The environment variables [`launch`] looks at. Weak evidence.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct LaunchEnv {
    pub(crate) invocation: bool,
    pub(crate) notify_socket: bool,
    pub(crate) ssh: bool,
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_linux_process_readings_from_a_fixture() {
        let root = fixtures::root("wsl2");
        let rss = memory(&root, 4096).unwrap();
        assert!(rss.raw() > 0);
        let peak = peak_memory(&root).unwrap();
        assert!(peak.raw() > 0);
        let d = memory_detail(&root).unwrap();
        assert!(d.virtual_size >= d.resident && d.peak_resident >= d.resident);
        assert!(d.private.is_some() && d.shared.is_some());
        assert!(status_number(&root, b"Threads").unwrap().unwrap() >= 1);
        assert!(status_number(&root, b"PPid").unwrap().is_some());
        let boot = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        assert!(started_at(&root, 100, boot).unwrap() >= boot);
        let io = io(&root).unwrap();
        assert!(io.storage_only && io.read_ops.is_some());
    }

    #[test]
    fn test_linux_process_kib_fields() {
        let s = b"VmHWM:\t  2048 kB\nVmRSS:\t1 kB\nThreads:\t3\nBad:\t-1 kB\n";
        assert_eq!(kib(s, b"VmHWM"), Some(2048 * 1024));
        assert_eq!(kib(s, b"VmRSS"), Some(1024));
        assert_eq!(kib(s, b"Threads"), Some(3 * 1024));
        assert_eq!(kib(s, b"Bad"), None);
        assert_eq!(kib(s, b"Missing"), None);
    }

    #[test]
    fn test_linux_process_missing_files_are_unavailable() {
        let root = fixtures::root("no-such-fixture");
        assert!(memory(&root, 4096).unwrap_err().is_unavailable());
        assert!(io(&root).unwrap_err().is_unavailable());
    }
}
