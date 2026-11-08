//! Explicit roots for monitoring agents (Linux).
//!
//! A monitoring agent in a container that has the host's root filesystem
//! mounted (say at `/host`, with the host's `/proc` and `/sys` under it) reads
//! the host's values through an explicit root. gopsutil offers the same
//! through the `HOST_PROC` and `HOST_SYS` environment variables; sys-lib offers
//! it only as a constructor in code, so no deployment can redirect a normal
//! program's readings through its environment (threat T4). The readings are
//! the same code the rest of the crate uses on `/`.
//!
//! The host's mounts and network counters are read through its process 1
//! (`/proc/1/mountinfo`, `/proc/1/net/dev`), which is the host's view when the
//! agent shares the host's process namespace (`hostPID`); otherwise they are
//! the agent's own. CPU, memory, disk I/O, load, and uptime are system-wide.
//!
//! ```no_run
//! # #[cfg(target_os = "linux")]
//! # fn main() -> sys_lib::Result<()> {
//! use sys_lib::probe::Probe;
//!
//! let host = Probe::with_root("/host")?;
//! let m = host.memory()?;
//! println!("host memory {} free of {}", m.free, m.total);
//! let mut cpu = host.cpu_sampler()?;
//! std::thread::sleep(std::time::Duration::from_secs(1));
//! println!("host cpu {}", cpu.sample()?.used());
//! # Ok(())
//! # }
//! # #[cfg(not(target_os = "linux"))]
//! # fn main() {}
//! ```

use core::time::Duration;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    Error, Result, cpu,
    disk::{self, Disk, ListOptions},
    host::Processes,
    linux::{self as tree, Root},
    mem::{self, MemWant},
    net,
    origin::{Host, Origin},
};

/// The most processes counted (T7).
const MAX_PROCESSES: u64 = 4_000_000;

/// A host's `/proc` and `/sys`, read through an explicit root.
#[derive(Clone, Debug)]
pub struct Probe {
    host: Arc<Host>,
    base: PathBuf,
    mountinfo: &'static str,
}

impl Probe {
    /// A probe of the host whose root filesystem is mounted at `root`.
    ///
    /// # Errors
    ///
    /// `NotFound` when `root` has no readable `proc/stat` (the host's `/proc`
    /// is not mounted there).
    pub fn with_root(root: impl Into<PathBuf>) -> Result<Probe> {
        let base: PathBuf = root.into();
        let tree = Root::at(&base);
        if tree.text("/proc/stat").is_none() {
            return Err(Error::NotFound {
                what: "probe root",
                source: "<root>/proc/stat",
            });
        }
        let net_dev = if tree.exists("/proc/1/net/dev") {
            "/proc/1/net/dev"
        } else {
            "/proc/net/dev"
        };
        let mountinfo = if tree.exists("/proc/1/mountinfo") {
            "/proc/1/mountinfo"
        } else {
            "/proc/self/mountinfo"
        };
        Ok(Probe {
            host: Arc::new(Host {
                root: tree,
                net_dev,
            }),
            base,
            mountinfo,
        })
    }

    fn root(&self) -> &Root {
        &self.host.root
    }

    fn origin(&self) -> Origin {
        Origin::host(Arc::clone(&self.host))
    }

    /// The host's memory, without any cgroup limit.
    ///
    /// # Errors
    ///
    /// As [`mem::snapshot`].
    pub fn memory(&self) -> Result<mem::Memory> {
        tree::mem::read(self.root(), None, false, MemWant::ALL).map(mem::from_raw)
    }

    /// A CPU sampler over the host's counters.
    ///
    /// # Errors
    ///
    /// As [`cpu::Sampler::new`].
    pub fn cpu_sampler(&self) -> Result<cpu::Sampler> {
        cpu::Sampler::from(self.origin())
    }

    /// The host's load average.
    ///
    /// # Errors
    ///
    /// As [`cpu::load`].
    pub fn load(&self) -> Result<Option<cpu::Load>> {
        tree::stat::load(self.root())
    }

    /// The host's uptime.
    ///
    /// # Errors
    ///
    /// As [`host::uptime`](crate::host::uptime).
    pub fn uptime(&self) -> Result<Duration> {
        tree::stat::uptime(self.root())
    }

    /// The host's filesystems, as [`disk::list`] selects them, with mount
    /// points as the host sees them and space read through the root.
    ///
    /// # Errors
    ///
    /// As [`disk::list`].
    pub fn disks(&self) -> Result<Vec<Disk>> {
        let mounts = tree::disk::mounts_at(self.root(), self.mountinfo, &ListOptions::default())?;
        Ok(mounts
            .into_iter()
            .map(|m| {
                let inside = self.base.join(
                    m.mount_point
                        .strip_prefix("/")
                        .unwrap_or(Path::new(&m.mount_point)),
                );
                Disk {
                    space: crate::sys::disk_space(&inside).ok(),
                    mount_point: m.mount_point,
                    device: m.device,
                    label: None,
                    filesystem: m.filesystem,
                    kind: m.kind,
                    read_only: m.read_only,
                    removable: m.removable,
                }
            })
            .collect())
    }

    /// A disk I/O sampler over the host's counters.
    ///
    /// # Errors
    ///
    /// As [`disk::IoSampler::new`].
    pub fn disk_io(&self) -> Result<disk::IoSampler> {
        disk::IoSampler::from(self.origin())
    }

    /// A network sampler over the host's counters.
    ///
    /// # Errors
    ///
    /// As [`net::Sampler::new`].
    pub fn net(&self) -> Result<net::Sampler> {
        net::Sampler::from(self.origin())
    }

    /// The host's process and thread counts.
    ///
    /// # Errors
    ///
    /// `Io` when the host's `/proc` cannot be listed; `TooLarge` past four
    /// million processes.
    pub fn processes(&self) -> Result<Processes> {
        let total = self.root().count_entries("/proc", MAX_PROCESSES, |name| {
            name.bytes().all(|b| b.is_ascii_digit())
        })?;
        Ok(Processes {
            total,
            threads: tree::stat::threads(self.root()),
            running: tree::stat::procs_running(self.root()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::fixtures;

    fn fixture() -> PathBuf {
        PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wsl2"))
    }

    #[test]
    fn test_probe_rejects_a_root_without_proc() {
        let err = Probe::with_root(std::env::temp_dir().join("sys-lib-no-such-root")).unwrap_err();
        assert!(matches!(err, Error::NotFound { .. }), "{err}");
    }

    #[test]
    fn test_probe_reads_a_fixture_host() {
        let p = Probe::with_root(fixture()).unwrap();
        let m = p.memory().unwrap();
        let expect = tree::mem::read(&fixtures::root("wsl2"), None, false, MemWant::ALL)
            .map(mem::from_raw)
            .unwrap();
        assert_eq!(m.total, expect.total);
        assert!(p.load().unwrap().is_some());
        assert!(p.uptime().unwrap() > Duration::ZERO);
        let mut s = p.cpu_sampler().unwrap();
        // A fixture's counters never move.
        assert!(matches!(s.sample(), Err(Error::TooSoon { .. })));
        assert!(p.disk_io().is_ok());
        assert!(p.net().is_ok());
        assert!(p.processes().is_ok());
        let _ = p.disks();
    }
}
