//! Host: uptime from `CLOCK_UPTIME`, the boot time, the machine's process,
//! thread, and open-file counts, the time zone, the clock's state, and the
//! login sessions.

use core::{ffi::CStr, mem::MaybeUninit, time::Duration};
use std::{
    path::Path,
    sync::{Mutex, PoisonError},
    time::SystemTime,
};

use super::{Key, ffi};
use crate::{
    Error, Result,
    host::{ClockSync, OpenFiles, Processes, Session, TimeZone},
    parse::tz,
};

/// `clock_gettime(CLOCK_UPTIME)`: time since boot, monotonic, as `uptime(1)`
/// reads it. When the clock cannot be read, the wall clock minus the boot time.
pub(crate) fn uptime() -> Result<Duration> {
    let mut ts = ffi::Timespec::default();
    // SAFETY: `ts` is a writable `timespec`.
    let rc = unsafe { ffi::clock_gettime(ffi::CLOCK_UPTIME, &raw mut ts) };
    if rc == 0
        && let (Ok(sec), Ok(nsec)) = (u64::try_from(ts.sec), u32::try_from(ts.nsec))
        && nsec < 1_000_000_000
    {
        return Ok(Duration::new(sec, nsec));
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
    const NAME: &CStr = c"kern.boottime";
    let tv: ffi::Timeval =
        super::sysctl_value(Key::Name(NAME)).map_err(|e| e.into_error(WHAT, NAME))?;
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

/// The most bytes of process records read: about 60 000 processes.
const MAX_PROCS: usize = 64 << 20;

/// The process and thread counts from a `kern.proc.proc` answer: one
/// `struct kinfo_proc` per process, each saying its own size.
fn count_procs(data: &[u8]) -> Option<(u64, u64)> {
    if !data.len().is_multiple_of(ffi::KINFO_PROC_SIZE) {
        return None;
    }
    let (mut procs, mut threads) = (0u64, 0u64);
    let (records, _) = data.as_chunks::<{ ffi::KINFO_PROC_SIZE }>();
    for record in records {
        let size = super::i32_at(record, ffi::KI_STRUCTSIZE)?;
        if usize::try_from(size).ok()? != ffi::KINFO_PROC_SIZE {
            return None;
        }
        let n = super::i32_at(record, ffi::KI_NUMTHREADS)?;
        procs += 1;
        threads = threads.saturating_add(u64::try_from(n).unwrap_or(0));
    }
    Some((procs, threads))
}

/// The processes and their threads from the process table (`kern.proc.proc`).
/// Inside a jail it holds the jail's processes; with
/// `security.bsd.see_other_uids` off, only this user's.
pub(crate) fn processes() -> Result<Processes> {
    const WHAT: &str = "process count";
    const SOURCE: &str = "kern.proc.proc";
    let mib = [ffi::CTL_KERN, ffi::KERN_PROC, ffi::KERN_PROC_PROC, 0];
    let (total, threads) = super::with_sysctl(WHAT, SOURCE, Key::Mib(&mib), MAX_PROCS, |data| {
        count_procs(data).ok_or(Error::Malformed {
            what: WHAT,
            source: SOURCE,
            line: None,
        })
    })?;
    Ok(Processes {
        total,
        threads: Some(threads),
        running: None,
    })
}

/// `kern.openfiles` (open now) and `kern.maxfiles`.
pub(crate) fn host_open_files() -> Result<OpenFiles> {
    Ok(OpenFiles {
        open: super::sysctl_u64(c"kern.openfiles"),
        max: super::sysctl_u64(c"kern.maxfiles"),
    })
}

/// The zone name: `TZ` when it is a valid name, else the zone the
/// `/etc/localtime` link points at, else the name `tzsetup(8)` records in
/// `/var/db/zoneinfo` (it copies the zone file to `/etc/localtime`, so there
/// is often no link). No zone file is opened.
fn zone_name(env: Option<&str>, link: Option<&str>, recorded: Option<&[u8]>) -> Option<String> {
    env.and_then(tz::from_env)
        .or_else(|| link.and_then(tz::from_link))
        .or_else(|| recorded.and_then(tz::from_file))
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

pub(crate) fn timezone() -> Result<TimeZone> {
    let env = std::env::var("TZ").ok();
    let link = std::fs::read_link("/etc/localtime").ok();
    let recorded = super::read_small("/var/db/zoneinfo", 4096);
    let (utc_offset_seconds, abbreviation) = local_offset("time zone")?;
    Ok(TimeZone {
        iana: zone_name(
            env.as_deref(),
            link.as_deref().and_then(Path::to_str),
            recorded.as_deref(),
        ),
        windows: None,
        utc_offset_seconds,
        abbreviation,
    })
}

/// The environment's locale; FreeBSD keeps no system-wide default elsewhere
/// (`login.conf` classes set it per user, into the environment).
pub(crate) fn locale() -> Option<(String, String)> {
    crate::host::posix_locale()
}

/// What `ntp_adjtime` says: `state` is its return value. The kernel keeps the
/// error figures in microseconds; negative ones are not figures.
fn clock_from(
    state: core::ffi::c_int,
    status: core::ffi::c_int,
    maxerror: i64,
    esterror: i64,
) -> ClockSync {
    let micros = |n: i64| u64::try_from(n).ok().map(Duration::from_micros);
    ClockSync {
        synchronized: state != ffi::TIME_ERROR && status & ffi::STA_UNSYNC == 0,
        max_error: micros(maxerror),
        estimated_error: micros(esterror),
    }
}

/// `ntp_adjtime` with no mode bits set, which reads and needs no privilege.
/// The kernel starts with `STA_UNSYNC` set and only an NTP daemon that
/// disciplines the clock (ntpd) clears it; a clock set by `ntpdate` or by
/// hand keeps it. So the flag clear proves synchronisation and the flag set
/// proves nothing: that is `None`, never a claim that the clock is adrift.
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

/// The most utmpx records read; more is `TooLarge`.
const MAX_SESSIONS: usize = 4_096;

/// A login session from a utmpx record. The remote host is looked at only to
/// tell that there is one.
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

/// The utmpx database (`/var/run/utx.active`). Its iterator is one per
/// process, so the walk holds a lock; a caller using `getutxent` itself at the
/// same moment is not covered.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_freebsd_zone_name_order() {
        let link = Some("/usr/share/zoneinfo/America/New_York");
        // E-93: a valid `TZ` wins; a hostile one is ignored.
        assert_eq!(
            zone_name(Some(":Europe/London"), link, None).as_deref(),
            Some("Europe/London")
        );
        assert_eq!(
            zone_name(Some("../../etc/shadow"), link, None).as_deref(),
            Some("America/New_York")
        );
        // tzsetup copied the file and recorded the name.
        assert_eq!(
            zone_name(None, None, Some(b"Asia/Tokyo\n")).as_deref(),
            Some("Asia/Tokyo")
        );
        assert_eq!(
            zone_name(None, link, Some(b"Asia/Tokyo\n")).as_deref(),
            Some("America/New_York")
        );
        assert_eq!(zone_name(None, None, Some(b"../etc/passwd")), None);
        assert_eq!(zone_name(None, None, None), None);
    }

    #[test]
    fn test_host_freebsd_clock_state() {
        let good = clock_from(0, 0, 16_000, 250);
        assert!(good.synchronized);
        assert_eq!(good.max_error, Some(Duration::from_millis(16)));
        assert_eq!(good.estimated_error, Some(Duration::from_micros(250)));
        assert!(!clock_from(5, 0, 0, 0).synchronized, "TIME_ERROR");
        assert!(!clock_from(0, 0x0040, 0, 0).synchronized, "STA_UNSYNC");
        assert_eq!(clock_from(0, 0, -1, -1).max_error, None);
    }

    #[test]
    fn test_host_freebsd_count_procs() {
        let mut record = vec![0u8; ffi::KINFO_PROC_SIZE];
        record[..4].copy_from_slice(&(ffi::KINFO_PROC_SIZE as i32).to_ne_bytes());
        record[ffi::KI_NUMTHREADS..ffi::KI_NUMTHREADS + 4].copy_from_slice(&4i32.to_ne_bytes());
        let mut two = record.clone();
        two.extend(&record);
        assert_eq!(count_procs(&two), Some((2, 8)));
        assert_eq!(count_procs(&[]), Some((0, 0)));
        assert_eq!(count_procs(&two[..1500]), None);
        two[ffi::KINFO_PROC_SIZE] = 0;
        assert_eq!(count_procs(&two), None, "a record of another size");
    }

    #[test]
    fn test_host_freebsd_session_records() {
        // SAFETY: a zeroed `utmpx` is valid (integers and arrays only).
        let blank: ffi::Utmpx = unsafe { MaybeUninit::zeroed().assume_init() };
        let put = |to: &mut [core::ffi::c_char], from: &[u8]| {
            for (dst, &src) in to.iter_mut().zip(from) {
                *dst = core::ffi::c_char::from_ne_bytes([src]);
            }
        };
        let mut e = blank;
        assert_eq!(session_of(&e), None, "not a user process");
        e.utype = ffi::USER_PROCESS;
        assert_eq!(session_of(&e), None, "no user");
        put(&mut e.user, b"maria");
        put(&mut e.line, b"pts/0");
        e.tv.sec = 1_700_000_000;
        let s = session_of(&e).unwrap();
        assert_eq!(s.user, "maria");
        assert_eq!(s.terminal.as_deref(), Some("pts/0"));
        assert!(!s.remote);
        put(&mut e.host, b"203.0.113.9");
        let s = session_of(&e).unwrap();
        assert!(s.remote);
        assert!(!format!("{s:?}").contains("203.0.113.9"));
    }

    #[test]
    fn test_host_freebsd_live_readings() {
        let up = uptime().unwrap();
        let boot = boot_time().unwrap();
        let since_boot = SystemTime::now().duration_since(boot).unwrap();
        // The two clocks agree to within a minute (the wall clock may step).
        assert!(
            up.abs_diff(since_boot) < Duration::from_secs(60),
            "{up:?} vs {since_boot:?}"
        );
        let p = processes().unwrap();
        assert!(p.total >= 1 && p.threads.is_some_and(|t| t >= p.total));
        let f = host_open_files().unwrap();
        assert!(f.open.is_some_and(|n| n > 0) && f.max.is_some_and(|n| n > 0));
        let tz = timezone().unwrap();
        assert!(tz.windows.is_none());
        assert!(tz.utc_offset_seconds.abs() <= 14 * 3600);
        let _ = clock_sync().unwrap();
        sessions().unwrap();
    }
}
