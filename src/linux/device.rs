//! The identifiers device identity reads on Linux: `/etc/machine-id` and the
//! root-only DMI files. Each is read into a wiped buffer, never through the
//! helpers that use an unwiped stack buffer (threat T1).

use std::io::ErrorKind;

use super::Root;
use crate::device::{ANCHOR_BUF, Anchor, Raw, Reading, canon};

fn failed(kind: ErrorKind) -> Reading {
    match kind {
        ErrorKind::PermissionDenied => Reading::PermissionDenied,
        _ => Reading::Absent,
    }
}

/// Reads `path` into `raw`, then canonicalises it into `out`.
fn read(
    root: &Root,
    path: &str,
    out: &mut [u8; ANCHOR_BUF],
    canonical: fn(&[u8], &mut [u8; ANCHOR_BUF]) -> Reading,
) -> Result<Reading, ErrorKind> {
    let mut raw = Raw::new();
    let n = root.read_into(path, &mut raw.0)?;
    Ok(canonical(&raw.0[..n], out))
}

/// A DMI identifier. Without `/sys/class/dmi` the machine has no such source
/// (WSL 2, many containers: E-41).
fn dmi(
    root: &Root,
    file: &str,
    out: &mut [u8; ANCHOR_BUF],
    canonical: fn(&[u8], &mut [u8; ANCHOR_BUF]) -> Reading,
) -> Reading {
    if !root.exists("/sys/class/dmi/id") {
        return Reading::Unsupported;
    }
    let mut path = String::with_capacity(40);
    path.push_str("/sys/class/dmi/id/");
    path.push_str(file);
    read(root, &path, out, canonical).unwrap_or_else(failed)
}

pub(crate) fn anchor(root: &Root, anchor: Anchor, out: &mut [u8; ANCHOR_BUF]) -> Reading {
    match anchor {
        Anchor::InstallId => read(root, "/etc/machine-id", out, canon::install_id)
            .or_else(|_| read(root, "/var/lib/dbus/machine-id", out, canon::install_id))
            .unwrap_or_else(failed),
        Anchor::PlatformUuid => dmi(root, "product_uuid", out, canon::uuid_text),
        Anchor::BoardSerial => dmi(root, "board_serial", out, canon::serial),
        Anchor::ProductSerial => dmi(root, "product_serial", out, canon::serial),
        _ => Reading::Unsupported,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::hostile::Tree;

    fn read_it(root: &Root, a: Anchor) -> Reading {
        anchor(root, a, &mut [0; ANCHOR_BUF])
    }

    #[test]
    fn test_device_machine_id_states() {
        // E-69.
        let t = Tree::new("mid");
        assert_eq!(read_it(&t.root(), Anchor::InstallId), Reading::Absent);
        for placeholder in [
            &b""[..],
            b"uninitialized\n",
            b"00000000000000000000000000000000\n",
        ] {
            t.put("/etc/machine-id", placeholder);
            assert_eq!(read_it(&t.root(), Anchor::InstallId), Reading::Placeholder);
        }
        t.put("/etc/machine-id", b"5ca1ab1e5ca1ab1e5ca1ab1e5ca1ab1e\n");
        assert_eq!(read_it(&t.root(), Anchor::InstallId), Reading::Value(16));
    }

    #[test]
    fn test_device_dbus_machine_id_is_the_fallback() {
        let t = Tree::new("dbus");
        t.put(
            "/var/lib/dbus/machine-id",
            b"5ca1ab1e5ca1ab1e5ca1ab1e5ca1ab1e\n",
        );
        assert_eq!(read_it(&t.root(), Anchor::InstallId), Reading::Value(16));
    }

    #[test]
    fn test_device_no_dmi_is_unsupported() {
        // E-41.
        let t = Tree::new("nodmi");
        assert_eq!(
            read_it(&t.root(), Anchor::PlatformUuid),
            Reading::Unsupported
        );
    }

    #[test]
    fn test_device_dmi_values_and_placeholders() {
        // E-70.
        let t = Tree::new("dmi");
        t.put(
            "/sys/class/dmi/id/product_uuid",
            b"00000000-0000-0000-0000-000000000000\n",
        );
        t.put(
            "/sys/class/dmi/id/board_serial",
            b"To be filled by O.E.M.\n",
        );
        t.put("/sys/class/dmi/id/product_serial", b"PF3XYZ\n");
        assert_eq!(
            read_it(&t.root(), Anchor::PlatformUuid),
            Reading::Placeholder
        );
        assert_eq!(
            read_it(&t.root(), Anchor::BoardSerial),
            Reading::Placeholder
        );
        assert_eq!(read_it(&t.root(), Anchor::ProductSerial), Reading::Value(6));
    }

    #[test]
    fn test_device_fixtures_hold_fake_identifiers_only() {
        // The fixtures' machine id is the fixed fake value capture.sh writes.
        let root = crate::linux::fixtures::root("wsl2");
        let mut out = [0; ANCHOR_BUF];
        assert_eq!(
            anchor(&root, Anchor::InstallId, &mut out),
            Reading::Value(16)
        );
        assert_eq!(out[..4], [0x5c, 0xa1, 0xab, 0x1e]);
    }
}
