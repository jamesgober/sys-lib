//! The machine and its environment.
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("running on {}", sys::host::name()?);
//! # Ok::<(), sys_lib::Error>(())
//! ```

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

#[cfg(test)]
mod tests {
    #[test]
    fn test_host_name_is_not_empty() {
        let name = super::name().unwrap();
        assert!(!name.is_empty() && name.len() <= 255);
    }
}
