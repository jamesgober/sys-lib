//! Device anchors: the platform UUID and serial number of
//! `IOPlatformExpertDevice`. macOS has no installation id, and the product
//! serial is the same value as the board's, so it is not counted twice.
//!
//! Each value goes from its `CFString` through a buffer that is wiped when it
//! goes, into the caller's, and nowhere else (threat T1).

#[cfg(target_os = "macos")]
use super::iokit::{Cf, Io};
use crate::device::{ANCHOR_BUF, Anchor, Reading};
#[cfg(target_os = "macos")]
use crate::device::{Raw, canon};

/// One string property of the platform device, in canonical form.
#[cfg(target_os = "macos")]
fn platform_string(
    key: &str,
    out: &mut [u8; ANCHOR_BUF],
    canonical: fn(&[u8], &mut [u8; ANCHOR_BUF]) -> Reading,
) -> Reading {
    let Some(platform) = Io::service(c"IOPlatformExpertDevice") else {
        return Reading::Absent;
    };
    let Ok(key) = Cf::string("device identity", key) else {
        return Reading::Absent;
    };
    let Some(value) = platform.property(&key) else {
        return Reading::Absent;
    };
    let mut text = Raw::new();
    match value.get().text(&mut text.0) {
        Some(bytes) => canonical(bytes, out),
        None => Reading::Absent,
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn device_anchor(anchor: Anchor, out: &mut [u8; ANCHOR_BUF]) -> Reading {
    match anchor {
        Anchor::PlatformUuid => platform_string("IOPlatformUUID", out, canon::uuid_text),
        Anchor::BoardSerial => platform_string("IOPlatformSerialNumber", out, canon::serial),
        _ => Reading::Unsupported,
    }
}

/// iOS exposes no device identifier to apps.
#[cfg(not(target_os = "macos"))]
pub(crate) fn device_anchor(_anchor: Anchor, _out: &mut [u8; ANCHOR_BUF]) -> Reading {
    Reading::Unsupported
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn test_device_apple_anchors() {
        let mut out = [0u8; ANCHOR_BUF];
        assert_eq!(
            device_anchor(Anchor::InstallId, &mut out),
            Reading::Unsupported
        );
        assert_eq!(
            device_anchor(Anchor::ProductSerial, &mut out),
            Reading::Unsupported
        );
        // Every Mac has a platform UUID: 16 bytes in canonical form.
        assert_eq!(
            device_anchor(Anchor::PlatformUuid, &mut out),
            Reading::Value(16)
        );
        // A virtual Mac's serial may be a placeholder or absent.
        assert!(matches!(
            device_anchor(Anchor::BoardSerial, &mut [0u8; ANCHOR_BUF]),
            Reading::Value(_) | Reading::Placeholder | Reading::Absent
        ));
    }
}
