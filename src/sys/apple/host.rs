//! Host: uptime from the Mach continuous clock, the boot time, the machine's
//! process, thread, and open-file counts, the time zone, the clock's state, and
//! the login sessions.

use core::{
    ffi::{CStr, c_int},
    mem::MaybeUninit,
    time::Duration,
};
#[cfg(target_os = "macos")]
use std::sync::{Mutex, PoisonError};
use std::{path::Path, sync::OnceLock, time::SystemTime};

use super::{SendRight, ffi, kern_error};
use crate::{
    Error, Result,
    host::{ClockSync, OpenFiles, Processes, Session, TimeZone},
    parse::tz,
};

/// The Mach timebase (never changes), or `None` when it cannot be read.
fn timebase() -> Option<ffi::MachTimebaseInfo> {
    static TIMEBASE: OnceLock<Option<ffi::MachTimebaseInfo>> = OnceLock::new();
    *TIMEBASE.get_or_init(|| {
        let mut info = ffi::MachTimebaseInfo::default();
        // SAFETY: `info` is a writable `mach_timebase_info`.
        let kr = unsafe { ffi::mach_timebase_info(&raw mut info) };
        (kr == ffi::KERN_SUCCESS && info.numer > 0 && info.denom > 0).then_some(info)
    })
}

/// Mach time units to nanoseconds, in 128 bits so nothing overflows.
fn mach_to_duration(ticks: u64, timebase: ffi::MachTimebaseInfo) -> Duration {
    let nanos = u128::from(ticks) * u128::from(timebase.numer) / u128::from(timebase.denom);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
}

/// `mach_continuous_time`, which counts from boot and keeps counting while the
/// machine sleeps: monotonic, unlike the wall clock. When the clock cannot be
/// read, the wall clock minus the boot time.
pub(crate) fn uptime() -> Result<Duration> {
    if let Some(tb) = timebase() {
        let up = mach_to_duration(ffi::mach_continuous_time(), tb);
        if !up.is_zero() {
            return Ok(up);
        }
    }
    SystemTime::now()
        .duration_since(boot_time()?)
        .map_err(|_| Error::Malformed {
            what: "uptime",
            source: "kern.boottime",
            line: None,
        })
}

/// `kern.boottime`, which the kernel moves with the wall clock so that it stays
/// the boot instant.
pub(crate) fn boot_time() -> Result<SystemTime> {
    const WHAT: &str = "boot time";
    const NAME: &core::ffi::CStr = c"kern.boottime";
    let tv: ffi::Timeval = super::sysctl_value(NAME).map_err(|e| e.into_error(WHAT, NAME))?;
    let malformed = || Error::Malformed {
        what: WHAT,
        source: "kern.boottime",
        line: None,
    };
    if tv.sec <= 0 || !(0..1_000_000).contains(&tv.usec) {
        return Err(malformed());
    }
    SystemTime::UNIX_EPOCH
        .checked_add(super::timeval_duration(tv))
        .ok_or_else(malformed)
}

/// The default processor set's load information: the machine's task and
/// thread counts, as `top` shows them. (`kern.num_threads` is the thread
/// limit, not the count.)
fn set_load(what: &'static str) -> Result<ffi::ProcessorSetLoadInfo> {
    let host = SendRight::host(what)?;
    let mut set: ffi::MachPort = 0;
    // SAFETY: `set` is writable; a port returned is owned by `set` below.
    let kr = unsafe { ffi::processor_set_default(host.0, &raw mut set) };
    if kr != ffi::KERN_SUCCESS {
        return Err(kern_error(what, "processor_set_default", kr));
    }
    let set = SendRight(set);
    let mut info = ffi::ProcessorSetLoadInfo::default();
    let mut count = ffi::PROCESSOR_SET_LOAD_INFO_COUNT;
    // SAFETY: `info` is `count` `natural_t` units of writable memory; the
    // kernel writes at most that many.
    let kr = unsafe {
        ffi::processor_set_statistics(
            set.0,
            ffi::PROCESSOR_SET_LOAD_INFO,
            (&raw mut info).cast::<c_int>(),
            &raw mut count,
        )
    };
    if kr != ffi::KERN_SUCCESS {
        return Err(kern_error(what, "processor_set_statistics", kr));
    }
    Ok(info)
}

/// The processes by `proc_listallpids` (macOS; iOS: the processor set's task
/// count), and the threads from the processor set.
pub(crate) fn processes() -> Result<Processes> {
    const WHAT: &str = "process count";
    let threads = |load: &ffi::ProcessorSetLoadInfo| u64::try_from(load.thread_count).ok();
    #[cfg(target_os = "macos")]
    let (total, threads) = (
        pid_count(WHAT)?,
        set_load(WHAT).ok().as_ref().and_then(threads),
    );
    #[cfg(not(target_os = "macos"))]
    let (total, threads) = {
        let load = set_load(WHAT)?;
        let total = u64::try_from(load.task_count).map_err(|_| Error::Malformed {
            what: WHAT,
            source: "processor_set_statistics",
            line: None,
        })?;
        (total, threads(&load))
    };
    Ok(Processes {
        total,
        threads,
        running: None,
    })
}

/// The most process ids listed; more is `TooLarge`.
#[cfg(target_os = "macos")]
const MAX_PIDS: usize = 4 << 20;

/// The number of processes. Asked with no buffer, `proc_listallpids` answers an
/// estimate with kernel slack, so the list itself is read and counted (into a
/// buffer a little larger than the estimate, read again if it fills).
#[cfg(target_os = "macos")]
fn pid_count(what: &'static str) -> Result<u64> {
    let mut pids: Vec<c_int> = Vec::new();
    for _ in 0..3 {
        // SAFETY: a null buffer asks for the estimate only.
        let estimate = unsafe { ffi::proc_listallpids(core::ptr::null_mut(), 0) };
        let estimate = usize::try_from(estimate)
            .ok()
            .filter(|&n| n > 0)
            .ok_or_else(|| super::os_error(what, "proc_listallpids", super::last_errno()))?;
        let room = estimate.saturating_add(64);
        if room > MAX_PIDS {
            return Err(Error::TooLarge {
                what,
                limit: MAX_PIDS as u64,
            });
        }
        pids.resize(room, 0);
        let bytes = c_int::try_from(room * core::mem::size_of::<c_int>()).unwrap_or(c_int::MAX);
        // SAFETY: `pids` is at least `bytes` writable bytes; the call writes at
        // most `bytes`.
        let n =
            unsafe { ffi::proc_listallpids(pids.as_mut_ptr().cast::<core::ffi::c_void>(), bytes) };
        let count = usize::try_from(n)
            .map_err(|_| super::os_error(what, "proc_listallpids", super::last_errno()))?;
        if count < room {
            return Ok(count as u64);
        }
    }
    // Still full after three tries: the table grows faster than it is read,
    // and a filled buffer is not the count (a fork storm, when it matters).
    Err(Error::TooLarge {
        what,
        limit: MAX_PIDS as u64,
    })
}

/// `kern.num_files` (open now) and `kern.maxfiles`.
pub(crate) fn host_open_files() -> Result<OpenFiles> {
    Ok(OpenFiles {
        open: super::sysctl_u64(c"kern.num_files"),
        max: super::sysctl_u64(c"kern.maxfiles"),
    })
}

/// The zone name: `TZ` when it is a valid name, else the zone the
/// `/etc/localtime` link points at. Neither is opened.
fn zone_name(env: Option<&str>, link: Option<&str>) -> Option<String> {
    env.and_then(tz::from_env)
        .or_else(|| link.and_then(tz::from_link))
        .map(str::to_owned)
}

/// The offset east of UTC now and the zone's abbreviation, from `localtime_r`.
fn local_offset(what: &'static str) -> Result<(i32, Option<String>)> {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| ffi::Time::try_from(d.as_secs()).unwrap_or(0));
    let mut tm = MaybeUninit::<ffi::Tm>::zeroed();
    // SAFETY: `now` is a readable `time_t` and `tm` a writable `struct tm`.
    let result = unsafe { ffi::localtime_r(&raw const now, tm.as_mut_ptr()) };
    if result.is_null() {
        return Err(Error::Os {
            what,
            call: "localtime_r",
            code: super::last_errno(),
        });
    }
    // SAFETY: zeroed is a valid `struct tm` (integers and a null pointer), and
    // the call succeeded.
    let tm = unsafe { tm.assume_init() };
    let offset = i32::try_from(tm.gmtoff).unwrap_or(0);
    if tm.zone.is_null() {
        return Ok((offset, None));
    }
    // SAFETY: a non-null `tm_zone` is a NUL-terminated string the library keeps
    // alive; it is copied here, before anything can change the zone.
    let zone = unsafe { CStr::from_ptr(tm.zone) };
    let zone = crate::parse::lossy(zone.to_bytes(), 32);
    Ok((offset, (!zone.is_empty()).then_some(zone)))
}

/// `TZ` and the `/etc/localtime` link for the name; the offset always.
pub(crate) fn timezone() -> Result<TimeZone> {
    let env = std::env::var("TZ").ok();
    let link = std::fs::read_link("/etc/localtime").ok();
    let (utc_offset_seconds, abbreviation) = local_offset("time zone")?;
    Ok(TimeZone {
        iana: zone_name(env.as_deref(), link.as_deref().and_then(Path::to_str)),
        windows: None,
        utc_offset_seconds,
        abbreviation,
    })
}

/// The environment's locale.
pub(crate) fn locale() -> Option<(String, String)> {
    crate::host::posix_locale().or_else(system_locale)
}

/// The locale chosen in System Settings: what an application started from the
/// Finder has, with no `LANG` set.
#[cfg(target_os = "macos")]
fn system_locale() -> Option<(String, String)> {
    // SAFETY: no arguments; the returned reference is owned by `Cf`.
    let locale = super::iokit::Cf::new(unsafe { ffi::CFLocaleCopyCurrent() })?;
    // SAFETY: `locale` is a live CFLocale; the identifier it returns is owned by
    // it, and `lent` ties the borrow to `locale`.
    let id = locale.lent(unsafe { ffi::CFLocaleGetIdentifier(locale.raw()) })?;
    let mut buf = [0u8; crate::parse::locale::MAX_LOCALE];
    let raw = core::str::from_utf8(id.text(&mut buf)?).ok()?;
    let tag = crate::parse::locale::tag(raw)?;
    Some((tag, raw.to_owned()))
}

#[cfg(not(target_os = "macos"))]
fn system_locale() -> Option<(String, String)> {
    None
}

/// What `ntp_adjtime` says: `state` is its return value. The kernel keeps the
/// error figures in microseconds; negative ones are not figures.
#[cfg(any(target_os = "macos", test))]
fn clock_from(state: c_int, status: c_int, maxerror: i64, esterror: i64) -> ClockSync {
    let micros = |n: i64| u64::try_from(n).ok().map(Duration::from_micros);
    ClockSync {
        synchronized: state != ffi::TIME_ERROR && status & ffi::STA_UNSYNC == 0,
        max_error: micros(maxerror),
        estimated_error: micros(esterror),
    }
}

/// `ntp_adjtime` with no mode bits set, which reads and needs no privilege. The
/// kernel starts with `STA_UNSYNC` set and `timed` may set the clock without
/// clearing it, so the flag clear proves synchronisation and the flag set
/// proves nothing: that is `None`, never a claim that the clock is adrift.
#[cfg(target_os = "macos")]
pub(crate) fn clock_sync() -> Result<Option<ClockSync>> {
    let mut timex = ffi::Timex::default();
    // SAFETY: `timex` is a writable `struct timex` with `modes` zero.
    let state = unsafe { ffi::ntp_adjtime(&raw mut timex) };
    if state < 0 {
        let code = super::last_errno();
        return Err(super::os_error(
            "clock synchronization",
            "ntp_adjtime",
            code,
        ));
    }
    let clock = clock_from(state, timex.status, timex.maxerror, timex.esterror);
    Ok(clock.synchronized.then_some(clock))
}

/// iOS has no public call.
#[cfg(not(target_os = "macos"))]
pub(crate) fn clock_sync() -> Result<Option<ClockSync>> {
    Ok(None)
}

/// The most utmpx records read; more is `TooLarge`.
#[cfg(target_os = "macos")]
const MAX_SESSIONS: usize = 4_096;

/// A login session from a utmpx record. The remote host is looked at only to
/// tell that there is one.
#[cfg(target_os = "macos")]
fn session_of(entry: &ffi::Utmpx) -> Option<Session> {
    if entry.utype != ffi::USER_PROCESS {
        return None;
    }
    let user = crate::parse::lossy(&super::c_array_bytes(&entry.user), 256);
    if user.is_empty() {
        return None;
    }
    let line = crate::parse::lossy(&super::c_array_bytes(&entry.line), 32);
    Some(Session {
        user,
        terminal: (!line.is_empty()).then_some(line),
        remote: entry.host[0] != 0,
        since: (entry.tv.sec > 0)
            .then(|| SystemTime::UNIX_EPOCH.checked_add(super::timeval_duration(entry.tv)))
            .flatten(),
    })
}

/// The utmpx database. Its iterator is one per process, so the walk holds a
/// lock; a caller using `getutxent` itself at the same moment is not covered.
#[cfg(target_os = "macos")]
pub(crate) fn sessions() -> Result<Vec<Session>> {
    static WALK: Mutex<()> = Mutex::new(());
    let _walk = WALK.lock().unwrap_or_else(PoisonError::into_inner);
    let mut out = Vec::new();
    // SAFETY: `WALK` is held, so nothing here moves the iterator meanwhile.
    unsafe { ffi::setutxent() };
    let mut within_budget = false;
    for _ in 0..MAX_SESSIONS {
        // SAFETY: as above.
        let entry = unsafe { ffi::getutxent() };
        // SAFETY: the result is null at the end, or a record the library keeps
        // until the next call, which comes after this borrow ends.
        let Some(entry) = (unsafe { entry.as_ref() }) else {
            within_budget = true;
            break;
        };
        out.extend(session_of(entry));
    }
    // SAFETY: as above.
    unsafe { ffi::endutxent() };
    if !within_budget {
        return Err(Error::TooLarge {
            what: "sessions",
            limit: MAX_SESSIONS as u64,
        });
    }
    Ok(out)
}

/// iOS has no session list.
#[cfg(not(target_os = "macos"))]
pub(crate) fn sessions() -> Result<Vec<Session>> {
    Err(super::unsupported("sessions"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_apple_zone_name_order() {
        let link = Some("/var/db/timezone/zoneinfo/America/New_York");
        // E-93: a valid `TZ` wins; a hostile one is ignored.
        assert_eq!(
            zone_name(Some(":Europe/London"), link).as_deref(),
            Some("Europe/London")
        );
        assert_eq!(
            zone_name(Some("../../etc/shadow"), link).as_deref(),
            Some("America/New_York")
        );
        assert_eq!(zone_name(None, link).as_deref(), Some("America/New_York"));
        assert_eq!(
            zone_name(
                None,
                Some("/var/db/timezone/tz/2024a.1.0/zoneinfo/Asia/Tokyo")
            )
            .as_deref(),
            Some("Asia/Tokyo")
        );
        // E-92: a copy, not a link.
        assert_eq!(zone_name(None, None), None);
        assert_eq!(zone_name(None, Some("/etc/some-copy")), None);
    }

    #[test]
    fn test_host_apple_clock_state() {
        let good = clock_from(0, 0, 16_000, 250);
        assert!(good.synchronized);
        assert_eq!(good.max_error, Some(Duration::from_millis(16)));
        assert_eq!(good.estimated_error, Some(Duration::from_micros(250)));
        assert!(!clock_from(5, 0, 0, 0).synchronized, "TIME_ERROR");
        assert!(!clock_from(0, 0x0040, 0, 0).synchronized, "STA_UNSYNC");
        assert_eq!(clock_from(0, 0, -1, -1).max_error, None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_host_apple_session_records() {
        // SAFETY: a zeroed `utmpx` is valid (integers and arrays only).
        let blank: ffi::Utmpx = unsafe { MaybeUninit::zeroed().assume_init() };
        let put = |to: &mut [core::ffi::c_char], from: &[u8]| {
            for (dst, &src) in to.iter_mut().zip(from) {
                *dst = src.cast_signed();
            }
        };
        let mut e = blank;
        assert_eq!(session_of(&e), None, "not a user process");
        e.utype = ffi::USER_PROCESS;
        assert_eq!(session_of(&e), None, "no user");
        put(&mut e.user, b"maria");
        put(&mut e.line, b"ttys003");
        e.tv.sec = 1_700_000_000;
        let s = session_of(&e).unwrap();
        assert_eq!(s.user, "maria");
        assert_eq!(s.terminal.as_deref(), Some("ttys003"));
        assert!(!s.remote);
        let since = s.since.unwrap().duration_since(SystemTime::UNIX_EPOCH);
        assert_eq!(since.unwrap().as_secs(), 1_700_000_000);
        put(&mut e.host, b"203.0.113.9");
        let s = session_of(&e).unwrap();
        assert!(s.remote);
        assert!(!format!("{s:?}").contains("203.0.113.9"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_host_apple_live_detection_readings() {
        let tz = timezone().unwrap();
        assert!(tz.windows.is_none());
        assert!(tz.utc_offset_seconds.abs() <= 14 * 3600);
        let _ = clock_sync().unwrap();
        sessions().unwrap();
    }

    #[test]
    fn test_host_apple_mach_time_scaled_by_timebase() {
        let intel = ffi::MachTimebaseInfo { numer: 1, denom: 1 };
        assert_eq!(mach_to_duration(1_500, intel), Duration::from_nanos(1_500));
        // Apple silicon: 24 MHz ticks, 125/3 ns each.
        let arm = ffi::MachTimebaseInfo {
            numer: 125,
            denom: 3,
        };
        assert_eq!(mach_to_duration(24_000_000, arm), Duration::from_secs(1));
        assert_eq!(
            mach_to_duration(u64::MAX, arm),
            Duration::from_nanos(u64::MAX)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_host_apple_live_readings() {
        let up = uptime().unwrap();
        let boot = boot_time().unwrap();
        let since_boot = SystemTime::now().duration_since(boot).unwrap();
        // The two clocks agree to within a minute (the wall clock may step).
        assert!(
            up.abs_diff(since_boot) < Duration::from_secs(60),
            "{up:?} vs {since_boot:?}"
        );
        let p = processes().unwrap();
        assert!(p.total >= 2 && p.threads.is_some_and(|t| t >= p.total));
        let f = host_open_files().unwrap();
        assert!(f.open.is_some_and(|n| n > 0) && f.max.is_some_and(|n| n > 0));
    }
}
