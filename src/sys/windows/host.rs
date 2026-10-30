//! The machine: uptime, boot time, and system-wide counts.

use std::time::{Duration, SystemTime};

use super::{ffi, performance_info};
use crate::{
    Error, Result,
    host::{OpenFiles, Processes},
};

/// Milliseconds since boot, sleep and hibernation included.
pub(crate) fn uptime() -> Result<Duration> {
    Ok(Duration::from_millis(ffi::GetTickCount64()))
}

/// The boot instant: now minus the uptime, computed once, so two calls agree
/// (the tick count and the clock are read a few milliseconds apart).
pub(crate) fn boot_time() -> Result<SystemTime> {
    static BOOT: std::sync::OnceLock<SystemTime> = std::sync::OnceLock::new();
    if let Some(b) = BOOT.get() {
        return Ok(*b);
    }
    let boot = SystemTime::now()
        .checked_sub(uptime()?)
        .ok_or(Error::Malformed {
            what: "boot time",
            source: "GetTickCount64",
            line: None,
        })?;
    Ok(*BOOT.get_or_init(|| boot))
}

pub(crate) fn processes() -> Result<Processes> {
    let info = performance_info("process count")?;
    Ok(Processes {
        total: u64::from(info.process_count),
        threads: Some(u64::from(info.thread_count)),
        running: None,
    })
}

/// Open handles across the machine; Windows has no system-wide maximum.
pub(crate) fn host_open_files() -> Result<OpenFiles> {
    let info = performance_info("open file count")?;
    Ok(OpenFiles {
        open: Some(u64::from(info.handle_count)),
        max: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_windows_live_counts_and_clocks() {
        let up = uptime().unwrap();
        assert!(up > Duration::ZERO);
        let boot = boot_time().unwrap();
        let since = SystemTime::now().duration_since(boot).unwrap();
        assert!(since + Duration::from_secs(1) >= up);
        let p = processes().unwrap();
        assert!(p.total >= 2 && p.threads.unwrap() >= p.total && p.running.is_none());
        let f = host_open_files().unwrap();
        assert!(f.open.unwrap() >= 1 && f.max.is_none());
    }
}
