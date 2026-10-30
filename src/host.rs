//! The machine and its environment.
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("running on {}", sys::host::name()?);
//! # Ok::<(), sys_lib::Error>(())
//! ```

use core::time::Duration;
use std::time::SystemTime;

use crate::Result;

/// The host name. A live reading: DHCP and `hostnamectl` change it while
/// processes run. A name that is not UTF-8 is converted lossily, and one
/// longer than 255 bytes is truncated (E-95).
///
/// The host name is an identifying value: keep it out of output that leaves
/// the machine unless that is intended.
///
/// # Errors
///
/// `NotFound` or `PermissionDenied` when a sandbox hides it; `Os` when the
/// system call fails.
pub fn name() -> Result<String> {
    crate::sys::host_name()
}

/// The time since the machine booted, including time suspended: monotonic.
///
/// ```
/// println!("up {:?}", sys_lib::host::uptime()?);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Os` when the clock cannot be read.
pub fn uptime() -> Result<Duration> {
    crate::sys::uptime()
}

/// When the machine booted: stable when the wall clock steps.
///
/// # Errors
///
/// `NotFound` when the boot time is hidden; `Malformed`; `Os`.
pub fn boot_time() -> Result<SystemTime> {
    crate::sys::boot_time()
}

/// How many processes and threads the machine runs. Totals only: no process
/// is listed or inspected.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct Processes {
    /// Processes. Inside a container, the container's process namespace.
    pub total: u64,
    /// Threads, when reported.
    pub threads: Option<u64>,
    /// Runnable right now, when reported (Linux).
    pub running: Option<u64>,
}

/// How many processes and threads the machine runs.
///
/// ```
/// let p = sys_lib::host::processes()?;
/// println!("{} processes, {:?} threads", p.total, p.threads);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `TooLarge` past 4 million processes (Linux); `NotFound`; `Os`.
pub fn processes() -> Result<Processes> {
    crate::sys::processes()
}

/// How many files are open across the machine.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct OpenFiles {
    /// Open now (Windows: handles).
    pub open: Option<u64>,
    /// The system-wide maximum (Windows has none).
    pub max: Option<u64>,
}

/// How many files are open across the machine, and the maximum.
///
/// # Errors
///
/// `NotFound`; `Malformed`; `Os`.
pub fn open_files() -> Result<OpenFiles> {
    crate::sys::host_open_files()
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_host_counts_and_clocks_live() {
        let up = super::uptime().unwrap();
        assert!(up.as_secs() > 0);
        let boot = super::boot_time().unwrap();
        assert!(boot < std::time::SystemTime::now());
        let p = super::processes().unwrap();
        assert!(p.total >= 1);
        if let Some(t) = p.threads {
            assert!(t >= p.total);
        }
        let f = super::open_files().unwrap();
        if let (Some(open), Some(max)) = (f.open, f.max) {
            assert!(open <= max);
        }
    }

    #[test]
    fn test_host_name_is_not_empty() {
        let name = super::name().unwrap();
        assert!(!name.is_empty() && name.len() <= 255);
    }
}
