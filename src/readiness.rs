//! The server readiness report: the operating-system settings that decide
//! whether a server performs and stays up under load, each with the reason and
//! the fix, the way enterprise databases warn at start-up.
//!
//! **Read-only:** sys-lib reports and never changes a setting; [`Check::fix`]
//! tells the operator what to run. Each check runs only where it applies (on
//! that OS, for that [`Profile`]); elsewhere it is left out, not reported as
//! passing. A value sys-lib cannot read is [`Status::Unknown`] with the reason
//! in [`Check::found`], never a guess. Every threshold is a documented
//! constant in this module.
//!
//! ```
//! use sys_lib::readiness::{self, Profile};
//!
//! let report = readiness::report_for(Profile::Database, std::env::temp_dir());
//! println!("{report}");
//! for c in report.warnings() {
//!     eprintln!("{}: {} (found {}, want {}): {}", c.id, c.title, c.found, c.recommended, c.fix);
//! }
//! if !report.passed() {
//!     // refuse to start, or log loudly
//! }
//! ```

use core::fmt::{self, Write as _};
use std::path::Path;

use crate::{
    Bytes,
    disk::{Disk, DiskKind},
};

/// What the server is for, which decides the checks and their thresholds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Profile {
    /// Any long-running service: limits, clock, space.
    General,
    /// A database: memory management, write-back, NUMA, storage.
    Database,
    /// A network server: limits, listen backlog, ports.
    WebServer,
    /// A machine about to run benchmarks: frequency, power, clock.
    Benchmark,
}

impl Profile {
    /// A short lowercase name: `"database"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Database => "database",
            Self::WebServer => "webserver",
            Self::Benchmark => "benchmark",
        }
    }
}

/// The outcome of one check.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Status {
    /// As recommended.
    Pass,
    /// Works, but costs performance or headroom.
    Warn,
    /// Likely to fail under load, or losing data.
    Fail,
    /// Could not be read; [`Check::found`] says why.
    Unknown,
}

impl Status {
    /// `"PASS"`, `"WARN"`, `"FAIL"`, `"UNKNOWN"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
            Self::Unknown => "UNKNOWN",
        }
    }
}

/// One setting, what it is, and what it should be.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Check {
    /// A stable id: `"linux.thp"`, `"limits.nofile"`, `"fs.atime"`.
    pub id: &'static str,
    /// What it is: `"Transparent huge pages"`.
    pub title: &'static str,
    /// The outcome.
    pub status: Status,
    /// The value found, or why it could not be read.
    pub found: String,
    /// The value this profile wants.
    pub recommended: &'static str,
    /// Why it matters, in one sentence.
    pub why: &'static str,
    /// The command or setting that changes it.
    pub fix: &'static str,
}

/// Every check that applies here, for one profile.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Report {
    /// The profile checked.
    pub profile: Profile,
    /// The checks, in a fixed order.
    pub checks: Vec<Check>,
}

impl Report {
    /// No check failed. Warnings and unknowns still pass.
    #[must_use]
    pub fn passed(&self) -> bool {
        !self.checks.iter().any(|c| c.status == Status::Fail)
    }

    /// The checks that warn.
    pub fn warnings(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| c.status == Status::Warn)
    }

    /// The checks that fail.
    pub fn failures(&self) -> impl Iterator<Item = &Check> {
        self.checks.iter().filter(|c| c.status == Status::Fail)
    }

    /// One `key=value` line per check: `check=linux.thp status=WARN found=always`.
    ///
    /// # Errors
    ///
    /// Whatever `out` returns.
    pub fn write_kv(&self, out: &mut impl fmt::Write) -> fmt::Result {
        for (i, c) in self.checks.iter().enumerate() {
            if i > 0 {
                out.write_char('\n')?;
            }
            crate::fmt::kv(out, "check", c.id)?;
            out.write_char(' ')?;
            crate::fmt::kv(out, "status", c.status.name())?;
            out.write_char(' ')?;
            crate::fmt::kv(out, "found", &c.found)?;
            out.write_char(' ')?;
            crate::fmt::kv(out, "recommended", c.recommended)?;
        }
        Ok(())
    }

    /// One JSON object: the profile, whether it passed, and every check with
    /// its reason and fix.
    ///
    /// # Errors
    ///
    /// Whatever `out` returns.
    pub fn write_json(&self, out: &mut impl fmt::Write) -> fmt::Result {
        use crate::fmt::json_str;
        out.write_str("{\"profile\":")?;
        json_str(out, self.profile.name())?;
        write!(out, ",\"passed\":{},\"checks\":[", self.passed())?;
        for (i, c) in self.checks.iter().enumerate() {
            if i > 0 {
                out.write_char(',')?;
            }
            out.write_str("{\"id\":")?;
            json_str(out, c.id)?;
            out.write_str(",\"title\":")?;
            json_str(out, c.title)?;
            out.write_str(",\"status\":")?;
            json_str(out, c.status.name())?;
            out.write_str(",\"found\":")?;
            json_str(out, &c.found)?;
            out.write_str(",\"recommended\":")?;
            json_str(out, c.recommended)?;
            out.write_str(",\"why\":")?;
            json_str(out, c.why)?;
            out.write_str(",\"fix\":")?;
            json_str(out, c.fix)?;
            out.write_char('}')?;
        }
        out.write_str("]}")
    }
}

/// A table: status, id, title, found, recommended, and under each check that
/// did not pass, its fix.
impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, c) in self.checks.iter().enumerate() {
            if i > 0 {
                f.write_char('\n')?;
            }
            write!(
                f,
                "{:<8}{:<20}{:<34}{:<24}{}",
                c.status.name(),
                c.id,
                c.title,
                c.found,
                c.recommended
            )?;
            if c.status != Status::Pass {
                write!(f, "\n        {}", c.fix)?;
            }
        }
        Ok(())
    }
}

/// The report for `profile`, read now. Never fails: a value that cannot be
/// read is a check with [`Status::Unknown`]. Costs 0.2 to 2 ms [est.].
#[must_use]
pub fn report(profile: Profile) -> Report {
    evaluate(profile, &gather(None))
}

/// [`report`], plus the checks of the directory the server keeps its data in:
/// its filesystem, mount options, free space, kind of storage, and I/O
/// scheduler.
#[must_use]
pub fn report_for(profile: Profile, data_dir: impl AsRef<Path>) -> Report {
    evaluate(profile, &gather(Some(data_dir.as_ref())))
}

// ---------------------------------------------------------------------------
// What a platform reports
// ---------------------------------------------------------------------------

/// One setting as a platform reports it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Fact<T> {
    /// The check does not apply on this platform: left out of the report.
    #[default]
    Skip,
    /// The value.
    Value(T),
    /// The check applies, and the value could not be read: why.
    Unknown(String),
}

impl<T> Fact<T> {
    /// A reading's result as a fact: an error becomes `Unknown` with its text.
    pub(crate) fn of(r: crate::Result<T>) -> Fact<T> {
        match r {
            Ok(v) => Fact::Value(v),
            Err(e) => Fact::Unknown(e.to_string()),
        }
    }

    fn map<U>(self, f: impl FnOnce(T) -> U) -> Fact<U> {
        match self {
            Fact::Skip => Fact::Skip,
            Fact::Value(v) => Fact::Value(f(v)),
            Fact::Unknown(why) => Fact::Unknown(why),
        }
    }
}

/// The Linux write-back settings; a `_bytes` value, when not 0, replaces its ratio.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Dirty {
    pub(crate) ratio: u64,
    pub(crate) background_ratio: u64,
    pub(crate) bytes: u64,
    pub(crate) background_bytes: u64,
}

/// The CPU frequency policy: the Linux governors in use, or the Windows power
/// plan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Governor {
    /// `"performance"`, `"powersave,performance"` (CPUs differ), `"High performance"`.
    pub(crate) name: String,
    /// Every CPU runs at its highest frequency policy.
    pub(crate) performance: bool,
}

/// The locked-memory allowance.
#[allow(dead_code)] // each platform constructs its own variant
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MemLock {
    /// `RLIMIT_MEMLOCK`: `None` is unlimited.
    Limit { soft: Option<u64> },
    /// Windows: the token holds `SeLockMemoryPrivilege`.
    Privilege(bool),
}

/// The system's settings, as a platform reports them; `Skip` where a check
/// does not apply there.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Tuning {
    /// `transparent_hugepage/enabled` and `defrag`, the selected words.
    pub(crate) thp: Fact<(String, String)>,
    pub(crate) swappiness: Fact<u64>,
    /// `overcommit_memory` and `overcommit_ratio`.
    pub(crate) overcommit: Fact<(u64, u64)>,
    pub(crate) max_map_count: Fact<u64>,
    pub(crate) dirty: Fact<Dirty>,
    /// `numa_balancing` and `zone_reclaim_mode`.
    pub(crate) numa: Fact<(u64, u64)>,
    /// Open handles have no per-process limit (Windows).
    pub(crate) handles_unlimited: bool,
    /// The system-wide open-file ceiling (`fs.file-max`, `kern.maxfiles`).
    pub(crate) files_system: Fact<u64>,
    /// The per-process ceiling (`fs.nr_open`, `kern.maxfilesperproc`).
    pub(crate) files_process: Fact<u64>,
    pub(crate) memlock: Fact<MemLock>,
    /// `somaxconn`, and the SYN backlog where the OS has one.
    pub(crate) backlog: Fact<(u64, Option<u64>)>,
    /// The ephemeral port range, inclusive.
    pub(crate) ports: Fact<(u32, u32)>,
    pub(crate) governor: Fact<Governor>,
    pub(crate) clock_source: Fact<String>,
}

/// The data directory's settings, as a platform reports them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct DataDirTuning {
    /// How access times are kept: `"noatime"`, `"relatime"`, `"strictatime"`.
    pub(crate) atime: Fact<String>,
    /// The disk's I/O scheduler (Linux).
    pub(crate) scheduler: Fact<String>,
}

/// Everything [`evaluate`] reads, gathered once, so the thresholds can be
/// tested on any machine.
#[derive(Clone, Debug, Default)]
pub(crate) struct Inputs {
    pub(crate) tuning: Tuning,
    pub(crate) open_files_soft: Fact<Option<u64>>,
    pub(crate) clock_synchronized: Fact<bool>,
    pub(crate) swap_total: Fact<Bytes>,
    pub(crate) numa_nodes: usize,
    pub(crate) container_limits: Option<String>,
    pub(crate) data_dir: Option<DataDir>,
    pub(crate) bench: Option<crate::bench::Environment>,
}

/// The data directory.
#[derive(Clone, Debug)]
pub(crate) struct DataDir {
    pub(crate) disk: Fact<Disk>,
    pub(crate) tuning: DataDirTuning,
}

fn gather(data_dir: Option<&Path>) -> Inputs {
    let tuning = crate::sys::tuning();
    let open_files_soft = Fact::of(crate::process::limits()).map(|l| l.open_files.soft);
    let clock_synchronized = match crate::host::clock_sync() {
        Ok(Some(c)) => Fact::Value(c.synchronized),
        Ok(None) => Fact::Unknown("the OS does not say".to_owned()),
        Err(e) => Fact::Unknown(e.to_string()),
    };
    let swap_total = Fact::of(crate::mem::swap()).map(|s| s.total);
    let mut limits = Vec::new();
    if let Ok(Some(l)) = crate::mem::limit() {
        limits.push(format!("memory {}", l.hard));
    }
    if let Ok(Some(q)) = crate::cpu::quota() {
        limits.push(format!("cpu {:.2}", q.cpus));
    }
    let container_limits = (!limits.is_empty()).then(|| limits.join(", "));
    let data_dir = data_dir.map(|path| {
        let disk = Fact::of(crate::disk::of(path));
        let tuning = match &disk {
            Fact::Value(d) => crate::sys::data_dir_tuning(path, d),
            _ => DataDirTuning::default(),
        };
        DataDir { disk, tuning }
    });
    Inputs {
        tuning,
        open_files_soft,
        clock_synchronized,
        swap_total,
        numa_nodes: crate::cpu::numa_nodes().len(),
        container_limits,
        data_dir,
        bench: None,
    }
}

// ---------------------------------------------------------------------------
// The thresholds
// ---------------------------------------------------------------------------

/// The least `vm.swappiness` a database wants.
pub const SWAPPINESS_MIN: u64 = 1;
/// The most `vm.swappiness` a database wants.
pub const SWAPPINESS_MAX: u64 = 10;
/// The least `vm.max_map_count` a database wants (memory-mapped storage).
pub const MAX_MAP_COUNT_MIN: u64 = 262_144;
/// The most `dirty_background_ratio` a database wants, in percent.
pub const DIRTY_BACKGROUND_MAX: u64 = 5;
/// The most `dirty_ratio` a database wants, in percent.
pub const DIRTY_MAX: u64 = 15;
/// The open-file soft limit a database or network server wants.
pub const OPEN_FILES_SERVER: u64 = 65_536;
/// The open-file soft limit any service wants.
pub const OPEN_FILES_GENERAL: u64 = 4_096;
/// Below this soft limit a database or network server fails under load.
pub const OPEN_FILES_FLOOR: u64 = 1_024;
/// The least listen backlog a database or network server wants.
pub const BACKLOG_MIN: u64 = 4_096;
/// The least ephemeral ports a database or network server wants.
pub const PORTS_MIN: u32 = 28_000;
/// The free space under which a data directory warns, in percent.
pub const FREE_WARN: f32 = 15.0;
/// The free space under which a data directory fails, in percent.
pub const FREE_FAIL: f32 = 5.0;

#[cfg(target_os = "macos")]
mod fixes {
    pub(super) const NOFILE: &str =
        "launchctl limit maxfiles 65536 200000, or SoftResourceLimits in the launchd plist";
    pub(super) const MEMLOCK: &str = "launchctl limit memlock unlimited unlimited";
    pub(super) const BACKLOG: &str = "sysctl -w kern.ipc.somaxconn=4096";
    pub(super) const PORTS: &str =
        "sysctl -w net.inet.ip.portrange.first=10000 net.inet.ip.portrange.last=65535";
    pub(super) const GOVERNOR: &str = "";
    pub(super) const CLOCK: &str = "enable Set time automatically in System Settings";
    pub(super) const SWAP: &str = "macOS manages swap itself";
}

#[cfg(windows)]
mod fixes {
    pub(super) const NOFILE: &str = "";
    pub(super) const MEMLOCK: &str =
        "grant Lock pages in memory (SeLockMemoryPrivilege) to the service account in secpol.msc";
    pub(super) const BACKLOG: &str = "";
    pub(super) const PORTS: &str = "netsh int ipv4 set dynamicport tcp start=10000 num=55535";
    pub(super) const GOVERNOR: &str = "powercfg /setactive SCHEME_MIN";
    pub(super) const CLOCK: &str = "w32tm /resync, and check the Windows Time service";
    pub(super) const SWAP: &str = "System Properties > Advanced > Performance > Virtual memory";
}

#[cfg(not(any(target_os = "macos", windows)))]
mod fixes {
    pub(super) const NOFILE: &str =
        "LimitNOFILE=65536 in the systemd unit, or nofile in /etc/security/limits.conf";
    pub(super) const MEMLOCK: &str =
        "LimitMEMLOCK=infinity in the systemd unit, or memlock in /etc/security/limits.conf";
    pub(super) const BACKLOG: &str =
        "sysctl -w net.core.somaxconn=4096 net.ipv4.tcp_max_syn_backlog=4096";
    pub(super) const PORTS: &str = "sysctl -w net.ipv4.ip_local_port_range=\"10000 65535\"";
    pub(super) const GOVERNOR: &str = "cpupower frequency-set -g performance";
    pub(super) const CLOCK: &str = "enable chrony or systemd-timesyncd: timedatectl set-ntp true";
    pub(super) const SWAP: &str = "add a small swap file or zram: swapon, or zram-generator";
}

fn check(
    id: &'static str,
    title: &'static str,
    status: Status,
    found: impl Into<String>,
    recommended: &'static str,
    why: &'static str,
    fix: &'static str,
) -> Check {
    Check {
        id,
        title,
        status,
        found: found.into(),
        recommended,
        why,
        fix,
    }
}

/// Adds a check from a fact: left out when it does not apply, `Unknown` when
/// it could not be read, otherwise `judge` gives the status and the text found.
fn push<T>(
    out: &mut Vec<Check>,
    fact: &Fact<T>,
    meta: (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    ),
    judge: impl FnOnce(&T) -> (Status, String),
) {
    let (id, title, recommended, why, fix) = meta;
    match fact {
        Fact::Skip => {}
        Fact::Unknown(reason) => {
            out.push(check(
                id,
                title,
                Status::Unknown,
                reason.clone(),
                recommended,
                why,
                fix,
            ));
        }
        Fact::Value(v) => {
            let (status, found) = judge(v);
            out.push(check(id, title, status, found, recommended, why, fix));
        }
    }
}

const fn pass_if(ok: bool) -> Status {
    if ok { Status::Pass } else { Status::Warn }
}

fn has(profiles: &[Profile], profile: Profile) -> bool {
    profiles.contains(&profile)
}

#[allow(clippy::too_many_lines)] // one table, read top to bottom
pub(crate) fn evaluate(profile: Profile, inputs: &Inputs) -> Report {
    use Profile::{Benchmark, Database, WebServer};

    let t = &inputs.tuning;
    let mut out = Vec::new();
    let db = profile == Database;
    let server = matches!(profile, Database | WebServer);

    if db {
        push(
            &mut out,
            &t.thp,
            (
                "linux.thp",
                "Transparent huge pages",
                "madvise or never",
                "With always, the kernel stalls allocations to compact memory and bloats sparse heaps, which shows as latency spikes.",
                "echo madvise > /sys/kernel/mm/transparent_hugepage/enabled",
            ),
            |(enabled, defrag)| {
                let ok = enabled != "always" && defrag != "always";
                (pass_if(ok), format!("{enabled}, defrag {defrag}"))
            },
        );
        push(
            &mut out,
            &t.swappiness,
            (
                "linux.swappiness",
                "Swap tendency (vm.swappiness)",
                "1 to 10",
                "A high value swaps out the buffer pool to keep page cache, which a database already manages itself.",
                "sysctl -w vm.swappiness=1",
            ),
            |&v| {
                (
                    pass_if((SWAPPINESS_MIN..=SWAPPINESS_MAX).contains(&v)),
                    v.to_string(),
                )
            },
        );
        push(
            &mut out,
            &t.overcommit,
            (
                "linux.overcommit",
                "Memory overcommit",
                "0 or 2",
                "With 1 the kernel grants any allocation and the OOM killer ends the database when memory runs out.",
                "sysctl -w vm.overcommit_memory=0",
            ),
            |&(mode, ratio)| {
                let found = if mode == 2 {
                    format!("{mode}, ratio {ratio}")
                } else {
                    mode.to_string()
                };
                (
                    if mode == 1 {
                        Status::Fail
                    } else {
                        Status::Pass
                    },
                    found,
                )
            },
        );
        push(
            &mut out,
            &t.max_map_count,
            (
                "linux.max_map_count",
                "Memory map count",
                "at least 262144",
                "Memory-mapped storage engines need a map per file region and fail allocations past the limit.",
                "sysctl -w vm.max_map_count=262144",
            ),
            |&v| (pass_if(v >= MAX_MAP_COUNT_MIN), v.to_string()),
        );
        push(
            &mut out,
            &t.dirty,
            (
                "linux.dirty",
                "Dirty page write-back",
                "background at most 5%, foreground at most 15%",
                "Large dirty thresholds let gigabytes of writes pile up and then stall every writer while they flush.",
                "sysctl -w vm.dirty_background_ratio=5 vm.dirty_ratio=15",
            ),
            |d| {
                if d.bytes != 0 || d.background_bytes != 0 {
                    // Set in bytes: the operator chose a size on purpose.
                    return (
                        Status::Pass,
                        format!(
                            "background {} bytes, foreground {} bytes",
                            d.background_bytes, d.bytes
                        ),
                    );
                }
                (
                    pass_if(d.background_ratio <= DIRTY_BACKGROUND_MAX && d.ratio <= DIRTY_MAX),
                    format!(
                        "background {}%, foreground {}%",
                        d.background_ratio, d.ratio
                    ),
                )
            },
        );
        if inputs.numa_nodes > 1 {
            push(
                &mut out,
                &t.numa,
                (
                    "linux.numa",
                    "NUMA balancing and zone reclaim",
                    "both 0",
                    "Automatic balancing migrates the buffer pool between nodes, and zone reclaim drops cache instead of using remote memory.",
                    "sysctl -w kernel.numa_balancing=0 vm.zone_reclaim_mode=0",
                ),
                |&(balancing, reclaim)| {
                    (
                        pass_if(balancing == 0 && reclaim == 0),
                        format!("balancing {balancing}, zone reclaim {reclaim}"),
                    )
                },
            );
        }
    }

    // Open files: every profile.
    let (want, recommended) = if server {
        (OPEN_FILES_SERVER, "at least 65536")
    } else {
        (OPEN_FILES_GENERAL, "at least 4096")
    };
    const NOFILE_WHY: &str = "Every connection and every open table or segment file takes a descriptor; at the limit, accepts and opens fail.";
    if t.handles_unlimited {
        out.push(check(
            "limits.nofile",
            "Open-file limit",
            Status::Pass,
            "unlimited (handles)",
            recommended,
            NOFILE_WHY,
            fixes::NOFILE,
        ));
    } else {
        push(
            &mut out,
            &inputs.open_files_soft,
            (
                "limits.nofile",
                "Open-file limit",
                recommended,
                NOFILE_WHY,
                fixes::NOFILE,
            ),
            |&soft| match soft {
                None => (Status::Pass, "unlimited".to_owned()),
                Some(n) => {
                    let mut found = n.to_string();
                    if let Fact::Value(max) = t.files_system {
                        found.push_str(&format!(", system {max}"));
                    }
                    let status = if n >= want {
                        Status::Pass
                    } else if server && n < OPEN_FILES_FLOOR {
                        Status::Fail
                    } else {
                        Status::Warn
                    };
                    (status, found)
                }
            },
        );
    }

    if db {
        push(
            &mut out,
            &t.memlock,
            (
                "limits.memlock",
                "Locked-memory limit",
                "unlimited, when the buffer pool is locked",
                "A database that locks its buffer pool in memory, to keep it out of swap, can lock no more than this.",
                fixes::MEMLOCK,
            ),
            |m| match *m {
                MemLock::Limit { soft: None } => (Status::Pass, "unlimited".to_owned()),
                MemLock::Limit { soft: Some(n) } => (Status::Warn, Bytes::new(n).to_string()),
                MemLock::Privilege(true) => (Status::Pass, "granted".to_owned()),
                MemLock::Privilege(false) => (Status::Warn, "not granted".to_owned()),
            },
        );
    }

    if server {
        push(
            &mut out,
            &t.backlog,
            (
                "net.backlog",
                "Listen backlog",
                "at least 4096",
                "Connections beyond the backlog are dropped during bursts, and clients see timeouts.",
                fixes::BACKLOG,
            ),
            |&(somaxconn, syn)| {
                let least = syn.map_or(somaxconn, |s| s.min(somaxconn));
                let found = match syn {
                    Some(s) => format!("{somaxconn}, SYN {s}"),
                    None => somaxconn.to_string(),
                };
                (pass_if(least >= BACKLOG_MIN), found)
            },
        );
        push(
            &mut out,
            &t.ports,
            (
                "net.ports",
                "Ephemeral ports",
                "at least 28000 ports",
                "Each outgoing connection holds a port through TIME_WAIT; a narrow range runs out under load.",
                fixes::PORTS,
            ),
            |&(first, last)| {
                let count = last.saturating_sub(first).saturating_add(1);
                (
                    pass_if(count >= PORTS_MIN),
                    format!("{first} to {last} ({count})"),
                )
            },
        );
    }

    if has(&[Database, WebServer, Benchmark], profile) {
        push(
            &mut out,
            &t.governor,
            (
                "cpu.governor",
                "CPU frequency policy",
                "performance",
                "Power-saving policies add wake-up latency and make throughput depend on load history.",
                fixes::GOVERNOR,
            ),
            |g| (pass_if(g.performance), g.name.clone()),
        );
    }

    if has(&[Database, Benchmark], profile) {
        push(
            &mut out,
            &t.clock_source,
            (
                "clock.source",
                "Kernel clock source",
                "tsc",
                "Reading hpet or acpi_pm costs microseconds, which every timestamp and timeout pays.",
                "echo tsc > /sys/devices/system/clocksource/clocksource0/current_clocksource",
            ),
            |s| (pass_if(!crate::bench::slow_clock(s)), s.clone()),
        );
    }

    if profile != Benchmark {
        push(
            &mut out,
            &inputs.clock_synchronized,
            (
                "clock.sync",
                "Clock synchronized",
                "synchronized",
                "Leases, distributed locks, certificates, and log ordering assume clocks agree.",
                fixes::CLOCK,
            ),
            |&ok| {
                (
                    pass_if(ok),
                    if ok {
                        "synchronized"
                    } else {
                        "not synchronized"
                    }
                    .to_owned(),
                )
            },
        );
    }

    if db {
        push(
            &mut out,
            &inputs.swap_total,
            (
                "mem.swap",
                "Swap present",
                "some swap or zram, small",
                "Without any swap, a brief memory spike goes straight to the OOM killer instead of slowing down.",
                fixes::SWAP,
            ),
            |&total| {
                let status = if total.raw() == 0 {
                    Status::Warn
                } else {
                    Status::Pass
                };
                (status, total.to_string())
            },
        );
    }

    if let Some(dir) = &inputs.data_dir {
        data_dir_checks(&mut out, profile, dir);
    }

    if let Some(limits) = &inputs.container_limits {
        out.push(check(
            "container.limits",
            "Container limits",
            Status::Pass,
            limits.clone(),
            "reported",
            "Size thread pools and caches from these limits, not from the host.",
            "",
        ));
    }

    if profile == Benchmark {
        let env = inputs
            .bench
            .clone()
            .unwrap_or_else(crate::bench::environment);
        bench_checks(&mut out, &env);
    }

    Report {
        profile,
        checks: out,
    }
}

fn data_dir_checks(out: &mut Vec<Check>, profile: Profile, dir: &DataDir) {
    let db = profile == Profile::Database;
    const FS_WHY: &str = "Network filesystems break locking and fsync guarantees, and some local ones are slow under database write patterns.";
    let disk = match &dir.disk {
        Fact::Skip => return,
        Fact::Unknown(reason) => {
            out.push(check(
                "fs.type",
                "Data directory filesystem",
                Status::Unknown,
                reason.clone(),
                RECOMMENDED_FS,
                FS_WHY,
                "",
            ));
            return;
        }
        Fact::Value(d) => d,
    };
    let fs = disk.filesystem.to_ascii_lowercase();
    let network = disk.kind == DiskKind::Network;
    let status = if network {
        if db { Status::Fail } else { Status::Warn }
    } else if db {
        pass_if(RECOMMENDED_FS_NAMES.contains(&fs.as_str()))
    } else {
        Status::Pass
    };
    out.push(check(
        "fs.type",
        "Data directory filesystem",
        status,
        format!("{} on {}", disk.filesystem, disk.device),
        RECOMMENDED_FS,
        FS_WHY,
        "move the data directory to a local disk with a recommended filesystem",
    ));

    push(
        out,
        &dir.tuning.atime,
        (
            "fs.atime",
            "Data directory access times",
            "noatime or relatime",
            "Updating the access time on every read turns reads into writes.",
            "mount with noatime: add it to the options in /etc/fstab and remount",
        ),
        |a| (pass_if(a != "strictatime"), a.clone()),
    );

    if let Some(space) = disk.space {
        let free = space.available.percent_of(space.total).value();
        let status = if free < FREE_FAIL {
            Status::Fail
        } else if free < FREE_WARN {
            Status::Warn
        } else {
            Status::Pass
        };
        out.push(check(
            "disk.space",
            "Data directory free space",
            status,
            format!("{free:.0}% free ({} of {})", space.available, space.total),
            "at least 15% free",
            "A full disk stops writes and can corrupt files that are half written; copy-on-write filesystems need headroom.",
            "free space or grow the volume",
        ));
    }

    if db {
        let (status, found) = match disk.kind {
            DiskKind::Ssd => (Status::Pass, "ssd"),
            DiskKind::Hdd => (Status::Warn, "hdd"),
            DiskKind::Ram => (Status::Warn, "memory (lost on reboot)"),
            DiskKind::Network => (Status::Warn, "network"),
            DiskKind::Removable => (Status::Warn, "removable"),
            DiskKind::Optical => (Status::Warn, "optical"),
            DiskKind::Virtual => (Status::Pass, "virtual"),
            _ => (Status::Unknown, "unknown"),
        };
        out.push(check(
            "disk.kind",
            "Data directory storage",
            status,
            found,
            "ssd or nvme",
            "Random reads and fsync latency on rotating disks are a hundred times slower.",
            "move the data directory to solid-state storage",
        ));
        if disk.kind == DiskKind::Ssd {
            push(
                out,
                &dir.tuning.scheduler,
                (
                    "disk.scheduler",
                    "Data disk I/O scheduler",
                    "none or mq-deadline",
                    "Fair-queueing schedulers add latency that solid-state drives do not need.",
                    "echo none > /sys/block/<disk>/queue/scheduler, or a udev rule",
                ),
                |s| (pass_if(s == "none" || s == "mq-deadline"), s.clone()),
            );
        }
    }
}

#[cfg(target_os = "macos")]
const RECOMMENDED_FS: &str = "local APFS";
#[cfg(windows)]
const RECOMMENDED_FS: &str = "local NTFS or ReFS";
#[cfg(not(any(target_os = "macos", windows)))]
const RECOMMENDED_FS: &str = "local ext4 or xfs";

/// The filesystems a database profile passes, lowercase.
const RECOMMENDED_FS_NAMES: &[&str] = &["ext4", "xfs", "apfs", "ntfs", "refs", "zfs", "ufs"];

fn bench_checks(out: &mut Vec<Check>, env: &crate::bench::Environment) {
    use crate::bench::Warning;
    let warned = |w| env.warnings.contains(&w);
    if let Some(turbo) = env.turbo {
        out.push(check(
            "cpu.turbo",
            "Turbo boost",
            pass_if(!turbo),
            if turbo { "enabled" } else { "disabled" },
            "disabled",
            "Turbo frequency depends on temperature and on what other cores do, so runs differ.",
            "echo 1 > /sys/devices/system/cpu/intel_pstate/no_turbo, or echo 0 > /sys/devices/system/cpu/cpufreq/boost",
        ));
    }
    if let Some(throttled) = env.throttled {
        out.push(check(
            "cpu.throttled",
            "Thermal throttling",
            pass_if(!throttled),
            if throttled { "throttled" } else { "not throttled" },
            "not throttled",
            "A throttled CPU runs slower than its rated frequency for reasons the benchmark does not control.",
            "improve cooling, or let the machine cool down",
        ));
    }
    if let Some(ac) = env.on_ac {
        out.push(check(
            "power.ac",
            "Mains power",
            pass_if(ac),
            if ac { "on mains" } else { "on battery" },
            "on mains",
            "On battery the OS lowers frequencies and parks cores.",
            "connect the charger",
        ));
    }
    out.push(check(
        "cpu.idle",
        "Machine idle",
        pass_if(!warned(Warning::Busy)),
        if warned(Warning::Busy) {
            "busy"
        } else {
            "idle"
        },
        "idle",
        "Other work competes for cores, caches, and memory bandwidth.",
        "stop other workloads before measuring",
    ));
    out.push(check(
        "virt.vm",
        "Bare metal",
        pass_if(!env.virtualized),
        if env.virtualized {
            "virtual machine"
        } else {
            "bare metal"
        },
        "bare metal",
        "Other guests share the hardware, and the hypervisor adds exits and steal time.",
        "measure on bare metal, or pin the VM to dedicated cores",
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::Space;

    fn tuned() -> Tuning {
        Tuning {
            thp: Fact::Value(("madvise".into(), "madvise".into())),
            swappiness: Fact::Value(1),
            overcommit: Fact::Value((0, 50)),
            max_map_count: Fact::Value(262_144),
            dirty: Fact::Value(Dirty {
                ratio: 10,
                background_ratio: 5,
                ..Dirty::default()
            }),
            numa: Fact::Value((0, 0)),
            handles_unlimited: false,
            files_system: Fact::Value(1 << 20),
            files_process: Fact::Value(1 << 20),
            memlock: Fact::Value(MemLock::Limit { soft: None }),
            backlog: Fact::Value((4096, Some(4096))),
            ports: Fact::Value((10_000, 65_535)),
            governor: Fact::Value(Governor {
                name: "performance".into(),
                performance: true,
            }),
            clock_source: Fact::Value("tsc".into()),
        }
    }

    fn inputs(tuning: Tuning) -> Inputs {
        Inputs {
            tuning,
            open_files_soft: Fact::Value(Some(1 << 20)),
            clock_synchronized: Fact::Value(true),
            swap_total: Fact::Value(Bytes::new(1 << 30)),
            numa_nodes: 2,
            container_limits: None,
            data_dir: None,
            bench: None,
        }
    }

    fn status(r: &Report, id: &str) -> Option<Status> {
        r.checks.iter().find(|c| c.id == id).map(|c| c.status)
    }

    #[test]
    fn test_readiness_tuned_database_passes_everything() {
        let r = evaluate(Profile::Database, &inputs(tuned()));
        assert!(r.passed());
        assert!(r.checks.iter().all(|c| c.status == Status::Pass), "{r}");
        assert_eq!(status(&r, "linux.numa"), Some(Status::Pass));
    }

    #[test]
    fn test_readiness_untuned_database_warns_and_fails() {
        let mut t = tuned();
        t.thp = Fact::Value(("always".into(), "madvise".into()));
        t.swappiness = Fact::Value(60);
        t.overcommit = Fact::Value((1, 50));
        t.max_map_count = Fact::Value(65_530);
        t.dirty = Fact::Value(Dirty {
            ratio: 20,
            background_ratio: 10,
            ..Dirty::default()
        });
        t.backlog = Fact::Value((4096, Some(512)));
        t.ports = Fact::Value((40_000, 60_999));
        t.clock_source = Fact::Value("hpet".into());
        t.memlock = Fact::Value(MemLock::Limit {
            soft: Some(8 << 20),
        });
        let mut i = inputs(t);
        i.open_files_soft = Fact::Value(Some(1024));
        i.swap_total = Fact::Value(Bytes::new(0));
        let r = evaluate(Profile::Database, &i);
        assert!(!r.passed());
        assert_eq!(status(&r, "linux.overcommit"), Some(Status::Fail));
        for id in [
            "linux.thp",
            "linux.swappiness",
            "linux.max_map_count",
            "linux.dirty",
            "net.backlog",
            "net.ports",
            "clock.source",
            "limits.memlock",
            "limits.nofile",
            "mem.swap",
        ] {
            assert_eq!(status(&r, id), Some(Status::Warn), "{id}");
        }
        assert_eq!(r.failures().count(), 1);
        // 32768..=60999 is 28232 ports: enough.
        let mut t = tuned();
        t.ports = Fact::Value((32_768, 60_999));
        assert_eq!(
            status(&evaluate(Profile::WebServer, &inputs(t)), "net.ports"),
            Some(Status::Pass)
        );
    }

    #[test]
    fn test_readiness_open_files_floor_fails_servers_only() {
        let mut i = inputs(tuned());
        i.open_files_soft = Fact::Value(Some(256));
        assert_eq!(
            status(&evaluate(Profile::WebServer, &i), "limits.nofile"),
            Some(Status::Fail)
        );
        assert_eq!(
            status(&evaluate(Profile::General, &i), "limits.nofile"),
            Some(Status::Warn)
        );
        let mut t = tuned();
        t.handles_unlimited = true;
        let mut i = inputs(t);
        i.open_files_soft = Fact::Skip;
        assert_eq!(
            status(&evaluate(Profile::Database, &i), "limits.nofile"),
            Some(Status::Pass)
        );
    }

    #[test]
    fn test_readiness_skip_is_left_out_and_unknown_reported() {
        let t = Tuning {
            swappiness: Fact::Unknown("permission denied".into()),
            ..Tuning::default()
        };
        let mut i = inputs(t);
        i.numa_nodes = 1;
        let r = evaluate(Profile::Database, &i);
        assert_eq!(status(&r, "linux.thp"), None);
        assert_eq!(status(&r, "linux.numa"), None);
        let c = r
            .checks
            .iter()
            .find(|c| c.id == "linux.swappiness")
            .unwrap();
        assert_eq!(c.status, Status::Unknown);
        assert_eq!(c.found, "permission denied");
        assert!(r.passed());
    }

    #[test]
    fn test_readiness_profiles_choose_their_checks() {
        let general = evaluate(Profile::General, &inputs(tuned()));
        assert_eq!(status(&general, "linux.thp"), None);
        assert_eq!(status(&general, "net.backlog"), None);
        assert_eq!(status(&general, "cpu.governor"), None);
        assert_eq!(status(&general, "clock.sync"), Some(Status::Pass));
        let web = evaluate(Profile::WebServer, &inputs(tuned()));
        assert_eq!(status(&web, "net.backlog"), Some(Status::Pass));
        assert_eq!(status(&web, "linux.swappiness"), None);
    }

    fn disk(fs: &str, kind: DiskKind, free: u64) -> Disk {
        Disk {
            mount_point: "/var/lib".into(),
            device: "/dev/nvme0n1p2".into(),
            label: None,
            filesystem: fs.into(),
            kind,
            space: Some(Space {
                total: Bytes::new(100),
                free: Bytes::new(free),
                available: Bytes::new(free),
            }),
            read_only: false,
            removable: false,
        }
    }

    #[test]
    fn test_readiness_data_directory() {
        let mut i = inputs(tuned());
        i.data_dir = Some(DataDir {
            disk: Fact::Value(disk("ext4", DiskKind::Ssd, 50)),
            tuning: DataDirTuning {
                atime: Fact::Value("relatime".into()),
                scheduler: Fact::Value("none".into()),
            },
        });
        let r = evaluate(Profile::Database, &i);
        for id in [
            "fs.type",
            "fs.atime",
            "disk.space",
            "disk.kind",
            "disk.scheduler",
        ] {
            assert_eq!(status(&r, id), Some(Status::Pass), "{id}");
        }
        i.data_dir = Some(DataDir {
            disk: Fact::Value(disk("nfs4", DiskKind::Network, 3)),
            tuning: DataDirTuning {
                atime: Fact::Value("strictatime".into()),
                scheduler: Fact::Skip,
            },
        });
        let r = evaluate(Profile::Database, &i);
        assert_eq!(status(&r, "fs.type"), Some(Status::Fail));
        assert_eq!(status(&r, "disk.space"), Some(Status::Fail));
        assert_eq!(status(&r, "fs.atime"), Some(Status::Warn));
        assert_eq!(status(&r, "disk.scheduler"), None);
        assert_eq!(
            status(&evaluate(Profile::General, &i), "fs.type"),
            Some(Status::Warn)
        );
        i.data_dir = Some(DataDir {
            disk: Fact::Value(disk("btrfs", DiskKind::Hdd, 10)),
            tuning: DataDirTuning::default(),
        });
        let r = evaluate(Profile::Database, &i);
        assert_eq!(status(&r, "fs.type"), Some(Status::Warn));
        assert_eq!(status(&r, "disk.space"), Some(Status::Warn));
        assert_eq!(status(&r, "disk.kind"), Some(Status::Warn));
    }

    #[test]
    fn test_readiness_benchmark_profile() {
        let mut i = inputs(tuned());
        i.bench = Some(crate::bench::environment());
        let r = evaluate(Profile::Benchmark, &i);
        assert!(status(&r, "cpu.idle").is_some() && status(&r, "virt.vm").is_some());
        assert_eq!(status(&r, "clock.sync"), None);
        assert_eq!(status(&r, "cpu.governor"), Some(Status::Pass));
    }

    #[test]
    fn test_readiness_outputs() {
        let mut t = tuned();
        t.swappiness = Fact::Value(60);
        let r = evaluate(Profile::Database, &inputs(t));
        let text = r.to_string();
        assert!(text.contains("WARN    linux.swappiness"));
        assert!(text.contains("\n        sysctl -w vm.swappiness=1"));
        let mut kv = String::new();
        r.write_kv(&mut kv).unwrap();
        assert!(kv.contains("check=linux.swappiness status=WARN found=60 recommended=\"1 to 10\""));
        let mut j = String::new();
        r.write_json(&mut j).unwrap();
        assert!(j.starts_with("{\"profile\":\"database\",\"passed\":true,\"checks\":[{\"id\":"));
        assert!(j.ends_with("}]}"));
    }

    #[test]
    fn test_readiness_live_never_fails() {
        for p in [
            Profile::General,
            Profile::Database,
            Profile::WebServer,
            Profile::Benchmark,
        ] {
            let r = report_for(p, std::env::temp_dir());
            assert_eq!(r.profile, p);
            assert!(r.checks.iter().any(|c| c.id == "limits.nofile"), "{r}");
            for c in &r.checks {
                assert!(!c.title.is_empty() && !c.why.is_empty(), "{c:?}");
            }
        }
    }
}
