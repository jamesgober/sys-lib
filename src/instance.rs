//! Instance info: facts fixed when this process **started**, answering
//! "which running copy is this?" when several run at once. Put the instance id
//! in every log line, audit record, and metric so events from two processes are
//! never mixed up.
//!
//! ```
//! use sys_lib::Instance;
//!
//! let me = Instance::start();
//! println!("{me}");                                 // 01J9ZQ4M8T6Y2K5V3W1X7R0B9C web-01 pid 48213 up 0s
//! eprintln!("instance={} event=boot", me.id());     // tag every log line
//! assert!(std::ptr::eq(me, Instance::start()));    // one instance per process
//! ```

use core::{fmt, str::FromStr};
use std::{
    sync::OnceLock,
    time::{Duration, SystemTime},
};

use clock_lib::{Monotonic, Wall};
use id_forge::ulid::Ulid;

use crate::{
    Error, Result,
    fmt::{json_str, kv},
};

static INSTANCE: OnceLock<Instance> = OnceLock::new();

/// The running copy of this program. Created once per process by
/// [`Instance::start`]; every caller sees the same value.
pub struct Instance {
    id: InstanceId,
    started_at: SystemTime,
    start: Monotonic,
    host: String,
    pid: u32,
    user: String,
}

/// The instance created by [`Instance::start`], or `None` before the first call.
///
/// ```
/// sys_lib::Instance::start();
/// assert!(sys_lib::instance::current().is_some());
/// ```
#[must_use]
pub fn current() -> Option<&'static Instance> {
    INSTANCE.get()
}

impl Instance {
    /// Creates the instance on the first call and returns it on every call.
    /// Never fails: an unreadable host or user name becomes `"unknown"`. Safe to
    /// call from many threads at once; exactly one instance is created (E-62).
    ///
    /// Cost: the first call creates a ULID and reads the host and user names
    /// (5 to 20 µs [est.]); later calls about 1 ns.
    pub fn start() -> &'static Instance {
        INSTANCE.get_or_init(|| Instance {
            id: InstanceId(*Ulid::new().as_bytes()),
            started_at: SystemTime::now(),
            start: Monotonic::now(),
            host: crate::host::name()
                .ok()
                .filter(|h| !h.is_empty())
                .unwrap_or_else(|| "unknown".to_owned()),
            pid: std::process::id(),
            user: crate::process::user()
                .map(|u| u.name)
                .ok()
                .filter(|u| !u.is_empty())
                .unwrap_or_else(|| "unknown".to_owned()),
        })
    }

    /// The instance id, a ULID: sortable by start time, unique without coordination.
    #[must_use]
    pub fn id(&self) -> InstanceId {
        self.id
    }

    /// When the process started (the wall clock at the first `start()`).
    #[must_use]
    pub fn started_at(&self) -> SystemTime {
        self.started_at
    }

    /// How long ago it started, by the monotonic clock: wall-clock steps do not affect it.
    #[must_use]
    pub fn uptime(&self) -> Duration {
        self.start.elapsed()
    }

    /// The host name at start, or `"unknown"`. An identifying value.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The process id at start.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The effective user at start, before any privilege drop, or `"unknown"`.
    /// The current user is [`process::user()`](crate::process::user).
    #[must_use]
    pub fn user_at_start(&self) -> &str {
        &self.user
    }

    /// False in a child created by `fork()` without `exec`, which inherits the
    /// parent's instance: a forked worker must not log under the parent's id.
    #[must_use]
    pub fn is_current_process(&self) -> bool {
        self.pid == std::process::id()
    }

    /// Writes `instance.id=... instance.started_at=... ...`, without allocating.
    ///
    /// ```
    /// let mut line = String::new();
    /// sys_lib::Instance::start().write_kv(&mut line)?;
    /// assert!(line.starts_with("instance.id="));
    /// # Ok::<(), std::fmt::Error>(())
    /// ```
    pub fn write_kv(&self, out: &mut impl fmt::Write) -> fmt::Result {
        let mut id = crate::fmt::StackBuf::<32>::new();
        fmt::Write::write_fmt(&mut id, format_args!("{}", self.id))?;
        let mut started = crate::fmt::StackBuf::<48>::new();
        fmt::Write::write_fmt(
            &mut started,
            format_args!("{:.0}", Wall::from(self.started_at)),
        )?;
        let mut pid = crate::fmt::StackBuf::<16>::new();
        fmt::Write::write_fmt(&mut pid, format_args!("{}", self.pid))?;
        kv(out, "instance.id", id.as_str())?;
        out.write_char(' ')?;
        kv(out, "instance.started_at", started.as_str())?;
        out.write_char(' ')?;
        kv(out, "instance.host", &self.host)?;
        out.write_char(' ')?;
        kv(out, "instance.pid", pid.as_str())?;
        out.write_char(' ')?;
        kv(out, "instance.user", &self.user)?;
        write!(out, " instance.uptime_secs={}", self.uptime().as_secs())
    }

    /// Writes one JSON object, without allocating.
    pub fn write_json(&self, out: &mut impl fmt::Write) -> fmt::Result {
        write!(
            out,
            "{{\"id\":\"{}\",\"started_at\":\"{:.0}\",\"host\":",
            self.id,
            Wall::from(self.started_at)
        )?;
        json_str(out, &self.host)?;
        write!(out, ",\"pid\":{},\"user\":", self.pid)?;
        json_str(out, &self.user)?;
        write!(out, ",\"uptime_secs\":{}}}", self.uptime().as_secs())
    }
}

/// `01J9ZQ4M8T6Y2K5V3W1X7R0B9C web-01 pid 48213 up 3h12m04s`.
impl fmt::Display for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} pid {} up ", self.id, self.host, self.pid)?;
        write_uptime(f, self.uptime())
    }
}

impl fmt::Debug for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Instance")
            .field("id", &self.id)
            .field("started_at", &self.started_at)
            .field("host", &self.host)
            .field("pid", &self.pid)
            .field("user", &self.user)
            .finish()
    }
}

/// `2d03h12m04s`, `3h12m04s`, `12m04s`, `4s`.
fn write_uptime(f: &mut impl fmt::Write, d: Duration) -> fmt::Result {
    let s = d.as_secs();
    let (days, h, m, sec) = (s / 86_400, s / 3600 % 24, s / 60 % 60, s % 60);
    if days > 0 {
        write!(f, "{days}d{h:02}h{m:02}m{sec:02}s")
    } else if h > 0 {
        write!(f, "{h}h{m:02}m{sec:02}s")
    } else if m > 0 {
        write!(f, "{m}m{sec:02}s")
    } else {
        write!(f, "{sec}s")
    }
}

/// An instance id: a 128-bit ULID, displayed as 26 Crockford base32 characters.
///
/// ```
/// use sys_lib::InstanceId;
///
/// let id = sys_lib::Instance::start().id();
/// let text = id.to_string();
/// assert_eq!(text.len(), 26);
/// assert_eq!(text.parse::<InstanceId>()?, id);
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct InstanceId([u8; 16]);

impl InstanceId {
    /// The 16 bytes, big-endian (the ULID's timestamp first).
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Parses the 26-character text form; anything else is `InvalidInput`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidInput`] unless `text` is exactly 26 valid characters.
    pub fn parse(text: &str) -> Result<InstanceId> {
        if text.len() != 26 {
            return Err(invalid_id());
        }
        Ulid::parse_str(text)
            .map(|u| InstanceId(*u.as_bytes()))
            .map_err(|_| invalid_id())
    }
}

fn invalid_id() -> Error {
    Error::InvalidInput {
        what: "instance id",
        reason: "use exactly 26 Crockford base32 characters",
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&Ulid::from_bytes(&self.0), f)
    }
}

impl fmt::Debug for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "InstanceId({self})")
    }
}

impl FromStr for InstanceId {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instance_start_once_from_64_threads() {
        // E-62.
        let ptrs: Vec<usize> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..64)
                .map(|_| s.spawn(|| core::ptr::from_ref(Instance::start()) as usize))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap_or(0)).collect()
        });
        assert!(ptrs.windows(2).all(|w| w[0] == w[1]));
        let me = Instance::start();
        assert!(me.is_current_process());
        assert_eq!(me.pid(), std::process::id());
        assert!(!me.host().is_empty() && !me.user_at_start().is_empty());
        assert!(current().is_some());
    }

    #[test]
    fn test_instance_id_round_trip_and_strict_parse() {
        let id = Instance::start().id();
        assert_eq!(InstanceId::parse(&id.to_string()).unwrap(), id);
        assert_eq!(format!("{id:?}"), format!("InstanceId({id})"));
        for bad in [
            "",
            "01J9ZQ4M8T6Y2K5V3W1X7R0B9",
            "01J9ZQ4M8T6Y2K5V3W1X7R0B9CX",
            "01J9ZQ4M8T6Y2K5V3W1X7R0B9!",
            "\u{e9}".repeat(13).as_str(),
        ] {
            assert!(
                matches!(InstanceId::parse(bad), Err(Error::InvalidInput { .. })),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn test_instance_writers() {
        let me = Instance::start();
        let mut line = String::new();
        me.write_kv(&mut line).unwrap();
        assert!(line.starts_with(&format!("instance.id={} instance.started_at=", me.id())));
        let mut json = String::new();
        me.write_json(&mut json).unwrap();
        assert!(json.starts_with(&format!("{{\"id\":\"{}\",\"started_at\":\"", me.id())));
        assert!(json.ends_with('}'));
        assert!(me.to_string().contains(&format!(" pid {} up ", me.pid())));
    }

    #[test]
    fn test_instance_uptime_format() {
        let fmt = |s| {
            let mut out = String::new();
            write_uptime(&mut out, Duration::from_secs(s)).unwrap();
            out
        };
        assert_eq!(fmt(4), "4s");
        assert_eq!(fmt(724), "12m04s");
        assert_eq!(fmt(11_524), "3h12m04s");
        assert_eq!(fmt(184_324), "2d03h12m04s");
    }
}
