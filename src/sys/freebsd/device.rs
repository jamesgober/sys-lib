//! Device anchors: `kern.hostuuid` as the installation id, and the SMBIOS
//! system UUID and serial numbers the loader copies into the kernel
//! environment (`smbios.system.uuid`, `smbios.planar.serial`,
//! `smbios.system.serial`), which `kenv(2)` reads without privilege.
//!
//! `kern.hostuuid` comes from `/etc/hostid`, which `rc.d/hostid` writes on
//! the first boot: the SMBIOS UUID when the firmware has a valid one, a
//! random UUID otherwise. It survives reboots and changes with a reinstall
//! or when an administrator replaces the file.
//!
//! Each value goes from the kernel into a buffer that is wiped when it goes,
//! into the caller's, and nowhere else (threat T1).

use super::{Kenv, Key, kenv_get};
use crate::device::{ANCHOR_BUF, Anchor, Raw, Reading, canon};

/// `kern.hostuuid`, in canonical form.
fn host_uuid(out: &mut [u8; ANCHOR_BUF]) -> Reading {
    let mut raw = Raw::new();
    match super::sysctl_raw(Key::Name(c"kern.hostuuid"), Some(&mut raw.0)) {
        Ok(len) => {
            let bytes = &raw.0[..len.min(ANCHOR_BUF)];
            let bytes = bytes.split(|&b| b == 0).next().unwrap_or(bytes);
            canon::install_id(bytes, out)
        }
        Err(super::ffi::EPERM | super::ffi::EACCES) => Reading::PermissionDenied,
        Err(_) => Reading::Absent,
    }
}

/// One loader environment variable, in canonical form.
fn kenv_anchor(
    name: &core::ffi::CStr,
    out: &mut [u8; ANCHOR_BUF],
    canonical: fn(&[u8], &mut [u8; ANCHOR_BUF]) -> Reading,
) -> Reading {
    let mut raw = Raw::new();
    match kenv_get(name, &mut raw.0) {
        Kenv::Value(n) => canonical(&raw.0[..n], out),
        Kenv::Absent => Reading::Absent,
        Kenv::Denied => Reading::PermissionDenied,
    }
}

pub(crate) fn device_anchor(anchor: Anchor, out: &mut [u8; ANCHOR_BUF]) -> Reading {
    match anchor {
        Anchor::InstallId => host_uuid(out),
        Anchor::PlatformUuid => kenv_anchor(c"smbios.system.uuid", out, canon::uuid_text),
        Anchor::BoardSerial => kenv_anchor(c"smbios.planar.serial", out, canon::serial),
        Anchor::ProductSerial => kenv_anchor(c"smbios.system.serial", out, canon::serial),
        _ => Reading::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_device_freebsd_anchors() {
        let mut out = [0u8; ANCHOR_BUF];
        assert_eq!(
            device_anchor(Anchor::CpuSignature, &mut out),
            Reading::Unsupported
        );
        // Every booted system has a host UUID, set or the all-zero placeholder.
        assert!(matches!(
            device_anchor(Anchor::InstallId, &mut out),
            Reading::Value(16) | Reading::Placeholder
        ));
        for anchor in [
            Anchor::PlatformUuid,
            Anchor::BoardSerial,
            Anchor::ProductSerial,
        ] {
            assert!(matches!(
                device_anchor(anchor, &mut [0u8; ANCHOR_BUF]),
                Reading::Value(_)
                    | Reading::Placeholder
                    | Reading::Absent
                    | Reading::PermissionDenied
            ));
        }
    }
}
