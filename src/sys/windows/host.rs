//! The machine: uptime, boot time, system-wide counts, the time zone, and the
//! locale.

use std::{
    mem::MaybeUninit,
    time::{Duration, SystemTime},
};

use super::{ffi, from_wide, os_error, performance_info};
use crate::{
    Error, Result,
    host::{ClockSync, OpenFiles, Processes, TimeZone},
    parse,
};

/// `LOCALE_NAME_MAX_LENGTH` (winnls.h).
const LOCALE_NAME: usize = 85;

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

/// East of UTC, from `GetDynamicTimeZoneInformation`'s result: UTC is local
/// time plus the bias, and the bias of the period in force adds to it.
fn utc_offset_seconds(id: u32, bias: i32, standard: i32, daylight: i32) -> i32 {
    let period = match id {
        ffi::TIME_ZONE_ID_STANDARD => standard,
        ffi::TIME_ZONE_ID_DAYLIGHT => daylight,
        _ => 0,
    };
    bias.saturating_add(period).saturating_mul(-60)
}

pub(crate) fn timezone() -> Result<TimeZone> {
    let info = MaybeUninit::<ffi::DynamicTimeZoneInformation>::zeroed();
    // SAFETY: `info` is zeroed, so every field is valid before the call.
    let mut info = unsafe { info.assume_init() };
    // SAFETY: `info` is a writable DYNAMIC_TIME_ZONE_INFORMATION.
    let id = unsafe { ffi::GetDynamicTimeZoneInformation(&raw mut info) };
    if id == ffi::TIME_ZONE_ID_INVALID {
        return Err(os_error("time zone", "GetDynamicTimeZoneInformation"));
    }
    let key = from_wide(&info.time_zone_key_name);
    Ok(TimeZone {
        iana: parse::tz::iana_for_windows(&key).map(str::to_owned),
        utc_offset_seconds: utc_offset_seconds(
            id,
            info.bias,
            info.standard_bias,
            info.daylight_bias,
        ),
        windows: Some(key).filter(|k| !k.is_empty()),
        abbreviation: None,
    })
}

/// A locale name's BCP 47 tag: what precedes an alternate sort ("de-DE_phoneb").
fn locale_tag(name: &str) -> &str {
    name.split_once('_').map_or(name, |(tag, _)| tag)
}

/// `GetUserDefaultLocaleName`; `None` for the invariant locale, which has no
/// name.
pub(crate) fn locale() -> Option<(String, String)> {
    let mut buf = [0u16; LOCALE_NAME];
    // SAFETY: `buf` is writable for its length in units.
    let n = unsafe { ffi::GetUserDefaultLocaleName(buf.as_mut_ptr(), buf.len() as i32) };
    let n = usize::try_from(n).ok().filter(|&n| n > 0)?;
    let raw = from_wide(&buf[..n.min(buf.len())]);
    (!raw.is_empty()).then(|| (locale_tag(&raw).to_owned(), raw.clone()))
}

/// `Ok(None)`: Windows has no unprivileged answer. `GetSystemTimeAdjustmentPrecise`
/// says whether a slew is in force, not whether the clock is in sync: a healthy
/// desktop leaves it off, because the Windows Time service runs on a trigger and
/// is stopped between synchronizations. The service's last-sync state is
/// documented only through `w32tm`, which is a subprocess and fails while the
/// service is stopped.
pub(crate) fn clock_sync() -> Result<Option<ClockSync>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_windows_offset_from_bias() {
        // London in summer: bias 0, daylight bias -60: one hour east of UTC.
        assert_eq!(
            utc_offset_seconds(ffi::TIME_ZONE_ID_DAYLIGHT, 0, 0, -60),
            3_600
        );
        assert_eq!(utc_offset_seconds(ffi::TIME_ZONE_ID_STANDARD, 0, 0, -60), 0);
        // Chicago in winter and summer.
        assert_eq!(utc_offset_seconds(1, 360, 0, -60), -21_600);
        assert_eq!(utc_offset_seconds(2, 360, 0, -60), -18_000);
        // India: half hours, and no daylight period (id 0).
        assert_eq!(utc_offset_seconds(0, -330, 0, 0), 19_800);
        assert_eq!(utc_offset_seconds(0, i32::MAX, 0, 0), i32::MIN);
    }

    #[test]
    fn test_host_windows_locale_tag() {
        assert_eq!(locale_tag("en-US"), "en-US");
        assert_eq!(locale_tag("de-DE_phoneb"), "de-DE");
        assert_eq!(locale_tag("zh-Hans-CN"), "zh-Hans-CN");
    }

    #[test]
    fn test_host_windows_live_zone_locale_and_clock() {
        let tz = timezone().unwrap();
        assert!(tz.windows.is_some() && tz.abbreviation.is_none());
        assert!(tz.utc_offset_seconds.abs() <= 14 * 3_600);
        if let Some((tag, raw)) = locale() {
            assert!(!tag.is_empty() && raw.starts_with(&tag));
        }
        assert_eq!(clock_sync().unwrap(), None);
    }

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
