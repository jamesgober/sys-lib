//! This process: its id, the user it runs as, whether it is elevated, and the
//! process guard every long-running service starts with.
//!
//! sys-lib does not list or inspect other processes.
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("pid {} as {}", sys::process::id(), sys::process::user()?.name);
//! if sys::process::is_elevated()? {
//!     println!("running as root or an elevated Administrator");
//! }
//! # Ok::<(), sys_lib::Error>(())
//! ```

#[cfg(feature = "guard")]
mod guard;
mod usage;

pub use usage::{
    CpuSampler, CpuTime, InfoOptions, Limits, ProcessInfo, ProcessIo, ProcessMemory, ProcessUsage,
    cpu_time, info, info_with, io, limits, memory, memory_detail, open_files, parent_id,
    peak_memory, started_at, threads,
};

#[cfg(feature = "guard")]
#[cfg_attr(docsrs, doc(cfg(feature = "guard")))]
pub use guard::{Guard, GuardBuilder, GuardReport, GuardWarning, guard};

use crate::Result;

/// This process's id.
#[must_use]
pub fn id() -> u32 {
    std::process::id()
}

/// A user account.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct User {
    /// The account name.
    pub name: String,
    /// The numeric user id (Unix); `None` on Windows, where accounts are SIDs.
    pub id: Option<u32>,
    /// Root, or an Administrator with an elevated token.
    pub is_admin: bool,
}

/// The effective user this process runs as now.
///
/// On Linux and macOS the name comes from the system's user database (NSS),
/// which can read `/etc/passwd` or, with directory-service users, wait on the
/// network: keep it off hot paths. A user id without an entry gives its number
/// as the name.
///
/// # Errors
///
/// `Os` when the user cannot be looked up.
pub fn user() -> Result<User> {
    crate::sys::user()
}

/// Whether this process runs as root, or with an elevated Administrator token.
///
/// This is a check, not a control: privilege drops belong to shield-sec, which
/// calls this afterwards to verify.
///
/// ```
/// if sys_lib::process::is_elevated()? {
///     // refuse to serve requests until privileges are dropped
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// `Os` when the token cannot be queried (Windows).
pub fn is_elevated() -> Result<bool> {
    crate::sys::is_elevated()
}

/// A resource limit: soft and hard values, `None` meaning unlimited or not
/// applicable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Limit {
    /// The limit in force.
    pub soft: Option<u64>,
    /// The ceiling the soft limit may be raised to without privileges.
    pub hard: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_id_matches_std() {
        assert_eq!(id(), std::process::id());
    }

    #[test]
    fn test_process_user_has_a_name() {
        let u = user().unwrap();
        assert!(!u.name.is_empty());
        assert_eq!(u.is_admin, is_elevated().unwrap());
        if cfg!(unix) {
            assert!(u.id.is_some());
        }
    }
}
