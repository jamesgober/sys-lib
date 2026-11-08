//! Login sessions (wtsapi32) and how this process was started.

use std::{
    ffi::c_void,
    mem::{size_of, transmute},
    ptr::{null_mut, read_unaligned},
    sync::OnceLock,
    time::SystemTime,
};

use super::{
    export, ffi, filetime_to_system, from_wide, net::string_at, non_negative, os_error,
    system_library, unsupported,
};
use crate::{
    Error, Result,
    host::Session,
    process::{LaunchFacts, Launcher},
    virt::Strength,
};

/// `WTS_CURRENT_SERVER_HANDLE`: this machine.
const THIS_SERVER: ffi::Handle = null_mut();

/// The most sessions read (threat T7).
const MAX_SESSIONS: usize = 4_096;

/// The three calls this module makes, resolved at run time (see `ffi`).
struct Wts {
    enumerate: ffi::WtsEnumerateSessionsW,
    query: ffi::WtsQuerySessionInformationW,
    free: ffi::WtsFreeMemory,
}

/// wtsapi32, loaded from System32 once and kept; `None` where Windows has none.
fn wts() -> Option<&'static Wts> {
    static WTS: OnceLock<Option<Wts>> = OnceLock::new();
    WTS.get_or_init(|| {
        let module = system_library("wtsapi32.dll")?;
        let enumerate = export(module, c"WTSEnumerateSessionsW")?;
        let query = export(module, c"WTSQuerySessionInformationW")?;
        let free = export(module, c"WTSFreeMemory")?;
        // SAFETY: each address is the export of that name, whose signature is
        // the type it becomes (wtsapi32.h), and the module is never unloaded.
        let enumerate = unsafe { transmute::<*mut c_void, ffi::WtsEnumerateSessionsW>(enumerate) };
        // SAFETY: as above.
        let query = unsafe { transmute::<*mut c_void, ffi::WtsQuerySessionInformationW>(query) };
        // SAFETY: as above.
        let free = unsafe { transmute::<*mut c_void, ffi::WtsFreeMemory>(free) };
        Some(Wts {
            enumerate,
            query,
            free,
        })
    })
    .as_ref()
}

/// Memory wtsapi32 allocated, freed on drop.
struct WtsMemory(*mut c_void, &'static Wts);

impl Drop for WtsMemory {
    fn drop(&mut self) {
        // SAFETY: the memory came from wtsapi32 and is freed once.
        unsafe { (self.1.free)(self.0) };
    }
}

/// One `WTSQuerySessionInformationW` answer, with its size in bytes.
fn query(wts: &'static Wts, session: u32, class: i32) -> Option<(WtsMemory, usize)> {
    let mut memory = null_mut();
    let mut bytes = 0u32;
    // SAFETY: `memory` and `bytes` are writable locals the call fills.
    let ok = unsafe { (wts.query)(THIS_SERVER, session, class, &raw mut memory, &raw mut bytes) };
    // Only memory wtsapi32 allocated gets an owner, which frees it.
    if ok != 0 && !memory.is_null() {
        Some((WtsMemory(memory, wts), bytes as usize))
    } else {
        None
    }
}

/// The session's user name; `None` when it cannot be read.
fn user_name(wts: &'static Wts, session: u32) -> Option<String> {
    let (memory, bytes) = query(wts, session, ffi::WTS_USER_NAME)?;
    // SAFETY: the answer is `bytes` readable bytes of UTF-16 in a heap block,
    // aligned for it, and lives until `memory` drops.
    let units = unsafe { std::slice::from_raw_parts(memory.0.cast::<u16>(), bytes / 2) };
    Some(from_wide(units))
}

/// Whether the session came in over the network: any protocol but the console.
fn is_remote(wts: &'static Wts, session: u32) -> Option<bool> {
    let (memory, bytes) = query(wts, session, ffi::WTS_CLIENT_PROTOCOL_TYPE)?;
    if bytes < size_of::<u16>() {
        return None;
    }
    // SAFETY: the size was just checked; the answer lives until `memory` drops.
    let protocol = unsafe { read_unaligned(memory.0.cast::<u16>()) };
    Some(protocol != ffi::WTS_PROTOCOL_TYPE_CONSOLE)
}

/// The session's logon time. Windows leaves it zero when there is none.
fn logon_time(wts: &'static Wts, session: u32) -> Option<SystemTime> {
    let (memory, bytes) = query(wts, session, ffi::WTS_SESSION_INFO)?;
    if bytes < size_of::<ffi::WtsInfoW>() {
        return None;
    }
    // SAFETY: the size was just checked; the answer lives until `memory` drops.
    let info = unsafe { read_unaligned(memory.0.cast::<ffi::WtsInfoW>()) };
    match info.logon_time {
        0 => None,
        t => filetime_to_system(non_negative(t)),
    }
}

/// Who is logged in: the sessions with a user. Session 0 and the listeners have
/// none. A session this process may not ask about (another user's, without the
/// right) is left out, as is one that ended since the list was read.
pub(crate) fn sessions() -> Result<Vec<Session>> {
    let wts = wts().ok_or_else(|| unsupported("sessions"))?;
    let mut list = null_mut();
    let mut count = 0u32;
    // SAFETY: reserved is 0 and version 1, as documented; `list` and `count`
    // are writable locals the call fills.
    let ok = unsafe { (wts.enumerate)(THIS_SERVER, 0, 1, &raw mut list, &raw mut count) };
    if ok == 0 {
        return Err(os_error("sessions", "WTSEnumerateSessionsW"));
    }
    if list.is_null() {
        return Ok(Vec::new());
    }
    let list = WtsMemory(list, wts);
    let count = count as usize;
    if count > MAX_SESSIONS {
        return Err(Error::TooLarge {
            what: "sessions",
            limit: MAX_SESSIONS as u64,
        });
    }
    // SAFETY: the call wrote `count` entries, which live until `list` drops.
    let entries =
        unsafe { std::slice::from_raw_parts(list.0.cast::<ffi::WtsSessionInfoW>(), count) };
    let mut out = Vec::with_capacity(count);
    for entry in entries {
        let id = entry.session_id;
        let Some(user) = user_name(wts, id).filter(|u| !u.is_empty()) else {
            continue;
        };
        // SAFETY: the station names live in the list, until `list` drops.
        let station = unsafe { string_at(entry.win_station_name) };
        out.push(Session {
            user,
            terminal: Some(station).filter(|s| !s.is_empty()),
            remote: is_remote(wts, id).unwrap_or(false),
            since: logon_time(wts, id),
        });
    }
    Ok(out)
}

/// Session 0 with no console: services run there, and nothing interactive does.
fn is_service(session: Option<u32>, console: bool) -> bool {
    session == Some(0) && !console
}

pub(crate) fn launch() -> LaunchFacts {
    let mut session = 0u32;
    // SAFETY: `session` is a writable local.
    let ok = unsafe { ffi::ProcessIdToSessionId(std::process::id(), &raw mut session) };
    let console = !ffi::GetConsoleWindow().is_null();
    LaunchFacts {
        service: is_service((ok != 0).then_some(session), console).then_some((
            Launcher::WindowsService,
            None,
            Strength::Strong,
        )),
        ssh: std::env::var_os("SSH_CONNECTION").is_some(),
        ..LaunchFacts::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_windows_service_needs_session_zero_and_no_console() {
        assert!(is_service(Some(0), false));
        assert!(!is_service(Some(0), true));
        assert!(!is_service(Some(1), false));
        assert!(!is_service(None, false));
    }

    #[test]
    fn test_session_windows_live_list_and_launch() {
        for s in sessions().unwrap() {
            assert!(!s.user.is_empty());
            if let Some(since) = s.since {
                assert!(since <= SystemTime::now());
            }
        }
        let facts = launch();
        assert!(!facts.is_pid1 && !facts.notify_socket && !facts.cron);
        if let Some((by, unit, _)) = facts.service {
            assert_eq!(by, Launcher::WindowsService);
            assert_eq!(unit, None);
        }
    }
}
