//! The machine and its environment.
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("running on {}", sys::host::name()?);
//! # Ok::<(), sys_lib::Error>(())
//! ```

use core::time::Duration;
use std::{sync::OnceLock, time::SystemTime};

use crate::{Result, os::keep, parse, sys::HardwareFacts};

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

/// The time zone: its name and the offset now.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct TimeZone {
    /// "Europe/London".
    pub iana: Option<String>,
    /// "GMT Standard Time".
    pub windows: Option<String>,
    /// East of UTC, now, including daylight saving.
    pub utc_offset_seconds: i32,
    /// "BST".
    pub abbreviation: Option<String>,
}

/// The time zone: the zone **name** and the current offset, not zone arithmetic
/// (that is a calendar crate's job). `TZ` is used only when it is a valid name;
/// it is never opened as a path (T4). When `/etc/localtime` is a copy rather
/// than a link, the name comes from `/etc/timezone` or is `None`: zone files are
/// not compared to guess (E-92).
///
/// The time zone is an identifying value: keep it out of output that leaves
/// the machine unless that is intended.
///
/// ```
/// let tz = sys_lib::host::timezone()?;
/// println!("zone {:?}, offset {} s", tz.iana, tz.utc_offset_seconds);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Os` when the offset cannot be read.
pub fn timezone() -> Result<TimeZone> {
    crate::sys::timezone()
}

/// The user's locale.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Locale {
    /// The BCP 47 tag: "en-US".
    pub tag: &'static str,
    /// As the system spells it: "en_US.UTF-8".
    pub raw: &'static str,
}

/// The locale, from the environment at the first call. `None` for `C`,
/// `POSIX`, and `C.UTF-8`, which express no preference (E-94).
#[must_use]
pub fn locale() -> Option<&'static Locale> {
    static FACT: OnceLock<Option<Locale>> = OnceLock::new();
    FACT.get_or_init(|| {
        let (tag, raw) = crate::sys::locale()?;
        Some(Locale {
            tag: keep(tag),
            raw: keep(raw),
        })
    })
    .as_ref()
}

/// `LC_ALL`, `LC_MESSAGES`, then `LANG`: the first that is set decides, as
/// POSIX has it. For the Unix backends.
#[allow(dead_code)] // unused on Windows and targets without a backend
pub(crate) fn posix_locale() -> Option<(String, String)> {
    let raw = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))?;
    let tag = parse::locale::tag(&raw)?;
    Some((tag, raw))
}

/// The hardware model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Model {
    /// The maker.
    pub vendor: Option<&'static str>,
    /// "ThinkPad X1 Carbon Gen 11", "Raspberry Pi 5", "Mac14,9".
    pub name: &'static str,
    /// The family.
    pub family: Option<&'static str>,
}

/// What kind of firmware the machine boots.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum FirmwareKind {
    /// UEFI.
    Uefi,
    /// Legacy BIOS.
    Bios,
    /// Not known.
    Unknown,
}

/// The machine's firmware. Inside a VM this describes the virtual firmware,
/// which is correct for the process.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Firmware {
    /// What kind it is.
    pub kind: FirmwareKind,
    /// "American Megatrends International, LLC.", "Apple Inc."
    pub vendor: Option<&'static str>,
    /// "1.40", "11881.41.5".
    pub version: Option<&'static str>,
    /// "2025-03-14".
    pub date: Option<&'static str>,
}

/// The main board. Serial numbers are never part of this answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct Board {
    /// The maker.
    pub vendor: Option<&'static str>,
    /// The product name.
    pub name: Option<&'static str>,
    /// The version.
    pub version: Option<&'static str>,
}

struct Hardware {
    model: Option<Model>,
    firmware: Option<Firmware>,
    board: Option<Board>,
}

fn hardware() -> &'static Hardware {
    static FACTS: OnceLock<Hardware> = OnceLock::new();
    FACTS.get_or_init(|| keep_hardware(crate::sys::hardware()))
}

fn keep_hardware(f: HardwareFacts) -> Hardware {
    let opt = |s: Option<String>| s.map(keep);
    let model = f.model_name.map(|name| Model {
        vendor: opt(f.model_vendor),
        name: keep(name),
        family: opt(f.model_family),
    });
    let known = f.firmware_kind.is_some()
        || f.firmware_vendor.is_some()
        || f.firmware_version.is_some()
        || f.firmware_date.is_some();
    let firmware = known.then(|| Firmware {
        kind: f.firmware_kind.unwrap_or(FirmwareKind::Unknown),
        vendor: opt(f.firmware_vendor),
        version: opt(f.firmware_version),
        date: opt(f.firmware_date),
    });
    let board = (f.board_vendor.is_some() || f.board_name.is_some() || f.board_version.is_some())
        .then(|| Board {
            vendor: opt(f.board_vendor),
            name: opt(f.board_name),
            version: opt(f.board_version),
        });
    Hardware {
        model,
        firmware,
        board,
    }
}

/// The hardware model, from DMI, SMBIOS, the device tree, or `hw.model` on
/// macOS ("Mac14,9"; no marketing-name table is shipped). Cached.
#[must_use]
pub fn model() -> Option<&'static Model> {
    hardware().model.as_ref()
}

/// The firmware. Placeholder strings are `None`. Cached.
#[must_use]
pub fn firmware() -> Option<&'static Firmware> {
    hardware().firmware.as_ref()
}

/// The main board. Placeholder strings are `None`. Cached.
#[must_use]
pub fn board() -> Option<&'static Board> {
    hardware().board.as_ref()
}

/// Whether the system clock is kept in sync.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub struct ClockSync {
    /// The kernel believes the clock is disciplined.
    pub synchronized: bool,
    /// The kernel's error bound.
    pub max_error: Option<Duration>,
    /// The kernel's estimate.
    pub estimated_error: Option<Duration>,
}

/// Whether the system clock is kept in sync (NTP, chrony, PTP), which leases,
/// distributed locks, and hybrid logical clocks depend on. `None` where the OS
/// has no unprivileged answer. Reading time itself stays in clock-lib.
///
/// ```
/// if let Some(c) = sys_lib::host::clock_sync()? {
///     println!("synchronized: {}", c.synchronized);
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Os` when the call fails.
pub fn clock_sync() -> Result<Option<ClockSync>> {
    crate::sys::clock_sync()
}

/// A login session.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Session {
    /// The user.
    pub user: String,
    /// The terminal.
    pub terminal: Option<String>,
    /// Whether it is remote. The remote address is not returned.
    pub remote: bool,
    /// When it started.
    pub since: Option<SystemTime>,
}

/// Who is logged in. User names are personal data: keep them out of output that
/// leaves the machine.
///
/// # Errors
///
/// `Unsupported` where the OS has no session list; `Os` when the call fails.
pub fn sessions() -> Result<Vec<Session>> {
    crate::sys::sessions()
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
