//! `/proc/stat`, `/proc/loadavg`, `/proc/uptime`, and the pressure-stall
//! files: CPU times, load, boot time, runnable tasks, and pressure.

use core::{ops::ControlFlow, time::Duration};
use std::time::{SystemTime, UNIX_EPOCH};

use super::{Root, cgroup::Cgroups};
use crate::{
    Error, Percent, Pressure, Result, Stall,
    cpu::{Load, Times},
    parse::procfs::{self, CpuTicks},
};

fn malformed(what: &'static str, source: &'static str) -> Error {
    Error::Malformed {
        what,
        source,
        line: None,
    }
}

/// Ticks to `Times`, at `ticks_per_sec` (`USER_HZ`, normally 100).
fn times(t: CpuTicks, ticks_per_sec: u64) -> Times {
    let d = |ticks: u64| {
        let hz = ticks_per_sec.max(1);
        Duration::from_secs(ticks / hz) + Duration::from_nanos((ticks % hz) * 1_000_000_000 / hz)
    };
    Times {
        user: d(t[0]),
        nice: d(t[1]),
        system: d(t[2]),
        idle: d(t[3]),
        iowait: d(t[4]),
        irq: d(t[5]),
        softirq: d(t[6]),
        steal: d(t[7]),
    }
}

/// The total CPU times: the first line of `/proc/stat`, from the file's
/// first 8 KiB only (E-27: hundreds of CPUs make the file long).
pub(crate) fn cpu_times(root: &Root, ticks_per_sec: u64) -> Result<Times> {
    let ticks = root.with_head("/proc/stat", "CPU times", "/proc/stat", |d| {
        crate::parse::lines(d, 1)
            .next()
            .and_then(procfs::stat_cpu_line)
    })?;
    match ticks {
        Some((None, t)) => Ok(times(t, ticks_per_sec)),
        _ => Err(malformed("CPU times", "/proc/stat")),
    }
}

/// The total and each CPU's times, indexed by CPU number (an offline CPU's
/// slot reads zero), through the stack. `TooLarge` when a CPU number is at or
/// past `out.len()`.
pub(crate) fn cpu_times_per_core(
    root: &Root,
    ticks_per_sec: u64,
    out: &mut [Times],
) -> Result<(Times, usize)> {
    let mut total = None;
    let mut count = 0usize;
    let mut too_large = false;
    for slot in out.iter_mut() {
        *slot = Times::default();
    }
    root.for_each_line(
        "/proc/stat",
        super::FILE_LIMIT,
        "CPU times",
        "/proc/stat",
        |line| {
            if !line.starts_with(b"cpu") {
                // The cpu lines come first; the rest of the file is not needed.
                return if total.is_some() {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                };
            }
            match procfs::stat_cpu_line(line) {
                Some((None, t)) => total = Some(times(t, ticks_per_sec)),
                Some((Some(id), t)) => match out.get_mut(id as usize) {
                    Some(slot) => {
                        *slot = times(t, ticks_per_sec);
                        count = count.max(id as usize + 1);
                    }
                    None => {
                        too_large = true;
                        return ControlFlow::Break(());
                    }
                },
                None => {}
            }
            ControlFlow::Continue(())
        },
    )?;
    if too_large {
        return Err(Error::TooLarge {
            what: "per-core CPU times",
            limit: out.len() as u64,
        });
    }
    let total = total.ok_or_else(|| malformed("CPU times", "/proc/stat"))?;
    Ok((total, count))
}

/// The load average.
pub(crate) fn load(root: &Root) -> Result<Option<Load>> {
    let parsed = root.with(
        "/proc/loadavg",
        "load average",
        "/proc/loadavg",
        procfs::loadavg,
    )?;
    let (one, five, fifteen, _, _) =
        parsed.ok_or_else(|| malformed("load average", "/proc/loadavg"))?;
    Ok(Some(Load::new(one, five, fifteen)))
}

/// Threads (`/proc/loadavg` field 4's total).
pub(crate) fn threads(root: &Root) -> Option<u64> {
    root.with("/proc/loadavg", "threads", "/proc/loadavg", procfs::loadavg)
        .ok()
        .flatten()
        .map(|(_, _, _, _, total)| total)
}

/// A `key N` value from `/proc/stat` (`btime`, `procs_running`), read
/// through the stack.
fn stat_value(root: &Root, key: &[u8]) -> Result<Option<u64>> {
    let mut value = None;
    root.for_each_line(
        "/proc/stat",
        super::FILE_LIMIT,
        "boot time",
        "/proc/stat",
        |line| match procfs::space_value(line, key) {
            Some(v) => {
                value = Some(v);
                ControlFlow::Break(())
            }
            None => ControlFlow::Continue(()),
        },
    )?;
    Ok(value)
}

/// The boot time: `btime` in `/proc/stat`.
pub(crate) fn boot_time(root: &Root) -> Result<SystemTime> {
    let secs = stat_value(root, b"btime")?.ok_or_else(|| malformed("boot time", "/proc/stat"))?;
    UNIX_EPOCH
        .checked_add(Duration::from_secs(secs))
        .ok_or_else(|| malformed("boot time", "/proc/stat"))
}

/// Tasks runnable now: `procs_running` in `/proc/stat`.
pub(crate) fn procs_running(root: &Root) -> Option<u64> {
    stat_value(root, b"procs_running").ok().flatten()
}

/// `/proc/uptime`, the fallback when `CLOCK_BOOTTIME` is unavailable.
pub(crate) fn uptime(root: &Root) -> Result<Duration> {
    root.with("/proc/uptime", "uptime", "/proc/uptime", procfs::uptime)?
        .ok_or_else(|| malformed("uptime", "/proc/uptime"))
}

/// The resource a pressure-stall file covers.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Resource {
    Cpu,
    Memory,
    Io,
}

impl Resource {
    fn host_path(self) -> &'static str {
        match self {
            Resource::Cpu => "/proc/pressure/cpu",
            Resource::Memory => "/proc/pressure/memory",
            Resource::Io => "/proc/pressure/io",
        }
    }

    fn cgroup_file(self) -> &'static str {
        match self {
            Resource::Cpu => "cpu.pressure",
            Resource::Memory => "memory.pressure",
            Resource::Io => "io.pressure",
        }
    }
}

fn stall(line: procfs::PsiLine) -> Stall {
    Stall::new(
        Percent::from_value(line.avg10),
        Percent::from_value(line.avg60),
        Percent::from_value(line.avg300),
    )
}

/// The pressure on `resource`: the cgroup's own file when the process is in a
/// cgroup v2 hierarchy that has one, else the host's, else `None` (no PSI,
/// kernel before 4.20 or disabled).
pub(crate) fn pressure(
    root: &Root,
    cgroups: &Cgroups,
    resource: Resource,
) -> Result<Option<Pressure>> {
    let parse = |d: &[u8]| procfs::psi(d);
    let cgroup = cgroups.unified.as_ref().and_then(|dir| {
        root.with(
            &format!("{}/{}", dir.leaf, resource.cgroup_file()),
            "pressure",
            "cgroup pressure file",
            parse,
        )
        .ok()
    });
    let lines = match cgroup {
        Some(l) => l,
        None => match root.with(resource.host_path(), "pressure", "/proc/pressure", parse) {
            Ok(l) => l,
            Err(e) if e.is_unavailable() => return Ok(None),
            Err(e) => return Err(e),
        },
    };
    match lines {
        (Some(some), full) => Ok(Some(Pressure::from_stalls(
            stall(some),
            full.map(stall),
            Some(some.total),
        ))),
        (None, _) => Err(malformed("pressure", "/proc/pressure")),
    }
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_linux_stat_cpu_times_total_and_per_core() {
        let root = fixtures::root("wsl2");
        let total = cpu_times(&root, 100).unwrap();
        assert!(total.user > Duration::ZERO && total.idle > Duration::ZERO);
        let mut out = vec![Times::default(); 64];
        let (t2, n) = cpu_times_per_core(&root, 100, &mut out).unwrap();
        assert_eq!(t2, total);
        assert_eq!(n, 32);
        let summed = out[..n].iter().fold(Duration::ZERO, |a, t| a + t.user);
        // The total line is the sum of the cpu lines, to within a tick each.
        let diff = summed.abs_diff(total.user);
        assert!(diff <= Duration::from_millis(10 * n as u64), "{diff:?}");
        let mut short = vec![Times::default(); 4];
        assert!(matches!(
            cpu_times_per_core(&root, 100, &mut short),
            Err(Error::TooLarge { limit: 4, .. })
        ));
    }

    #[test]
    fn test_linux_stat_ticks_convert_exactly() {
        let t = times([150, 0, 0, 1, 0, 0, 0, 0], 100);
        assert_eq!(t.user, Duration::from_millis(1500));
        assert_eq!(t.idle, Duration::from_millis(10));
        let t = times([1, 0, 0, 0, 0, 0, 0, 0], 250);
        assert_eq!(t.user, Duration::from_millis(4));
        let _ = times([u64::MAX; 8], 0);
    }

    #[test]
    fn test_linux_stat_load_boot_time_and_counts() {
        let root = fixtures::root("wsl2");
        let l = load(&root).unwrap().unwrap();
        assert!(l.one >= 0.0);
        assert!(threads(&root).unwrap() >= 1);
        let boot = boot_time(&root).unwrap();
        assert!(boot > UNIX_EPOCH + Duration::from_secs(1_600_000_000));
        assert!(procs_running(&root).unwrap() >= 1);
        assert!(uptime(&root).unwrap() > Duration::ZERO);
    }

    #[test]
    fn test_linux_stat_pressure_prefers_the_cgroup() {
        let name = "wsl2-systemd-scope-limited";
        let (root, log) = fixtures::root(name).recording();
        let cg = super::super::cgroup::Cgroups::resolve(&root);
        let p = pressure(&root, &cg, Resource::Cpu).unwrap().unwrap();
        assert!(p.some.avg10 <= Percent::FULL);
        let opened = log.lock().unwrap().clone();
        assert!(
            opened.iter().any(|f| f.ends_with(".scope/cpu.pressure")),
            "{opened:?}"
        );
        let m = pressure(&root, &cg, Resource::Memory).unwrap().unwrap();
        assert!(m.full.is_some());
        let io = pressure(&root, &cg, Resource::Io).unwrap().unwrap();
        assert!(io.total.is_some());
        // No pressure files at all: None, not an error.
        let none = pressure(
            &fixtures::root("no-such-fixture"),
            &Cgroups::default(),
            Resource::Cpu,
        )
        .unwrap();
        assert_eq!(none, None);
    }
}
