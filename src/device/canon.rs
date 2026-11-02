//! Canonical forms of the raw identifiers, so one machine gives one id however
//! a platform spells them: UUIDs as 16 bytes in RFC 9562 order, machine ids as
//! 16 bytes parsed from hex, serials trimmed and upper-cased, a MAC as 6 bytes.
//! Placeholders that manufacturers and hypervisors ship count as absent; there
//! is one table, in `parse::smbios`.

use super::{ANCHOR_BUF, Reading};
use crate::{cpu, net, parse::smbios, virt};

/// Writes `value` to the front of `out`.
fn put(value: &[u8], out: &mut [u8; ANCHOR_BUF]) -> Reading {
    let n = value.len().min(ANCHOR_BUF);
    out[..n].copy_from_slice(&value[..n]);
    Reading::Value(n)
}

fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 32 hex digits, with or without UUID dashes, as 16 bytes.
fn hex16(text: &[u8]) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    let mut digits = text.iter().copied().filter(|&b| b != b'-');
    for byte in &mut out {
        let hi = nibble(digits.next()?)?;
        let lo = nibble(digits.next()?)?;
        *byte = hi << 4 | lo;
    }
    digits.next().is_none().then_some(out)
}

/// An installation id as text: `/etc/machine-id`, the Windows `MachineGuid`.
/// Empty, `uninitialized`, and all zeros are placeholders (E-69).
pub(crate) fn install_id(text: &[u8], out: &mut [u8; ANCHOR_BUF]) -> Reading {
    let t = text.trim_ascii();
    if t.is_empty() || t == b"uninitialized" {
        return Reading::Placeholder;
    }
    match hex16(t) {
        Some(id) if id == [0; 16] => Reading::Placeholder,
        Some(id) => put(&id, out),
        None => Reading::Placeholder,
    }
}

/// A UUID as text: Linux's `product_uuid`, macOS's `IOPlatformUUID`.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) fn uuid_text(text: &[u8], out: &mut [u8; ANCHOR_BUF]) -> Reading {
    match hex16(text.trim_ascii()) {
        Some(id) if smbios::uuid_is_placeholder(&id) => Reading::Placeholder,
        Some(id) => put(&id, out),
        None => Reading::Placeholder,
    }
}

/// A UUID as a raw SMBIOS table stores it (E-75).
pub(crate) fn uuid_smbios(raw: [u8; 16], out: &mut [u8; ANCHOR_BUF]) -> Reading {
    if smbios::uuid_is_placeholder(&raw) {
        return Reading::Placeholder;
    }
    put(&smbios::uuid_rfc_order(raw), out)
}

/// A serial number: trimmed and upper-cased (E-70).
pub(crate) fn serial(text: &[u8], out: &mut [u8; ANCHOR_BUF]) -> Reading {
    if smbios::is_placeholder(text) {
        return Reading::Placeholder;
    }
    let t = text.trim_ascii();
    let n = t.len().min(ANCHOR_BUF);
    for (slot, b) in out.iter_mut().zip(&t[..n]) {
        *slot = b.to_ascii_uppercase();
    }
    Reading::Value(n)
}

/// The CPU's signature: vendor, the platform's signature word, and the brand,
/// so a dual-boot machine sees one value.
pub(crate) fn cpu_signature(out: &mut [u8; ANCHOR_BUF]) -> Reading {
    let brand = cpu::raw_name().map_or(cpu::name(), str::trim);
    let vendor = cpu::vendor().name().as_bytes();
    let word = crate::sys::cpu_signature().to_be_bytes();
    if brand.is_empty() || brand == "Unknown" {
        return Reading::Absent;
    }
    let mut n = 0;
    for part in [vendor, &[0], &word, brand.as_bytes()] {
        let take = part.len().min(ANCHOR_BUF - n);
        out[n..n + take].copy_from_slice(&part[..take]);
        n += take;
    }
    Reading::Value(n)
}

/// Whether a MAC can identify hardware: not all zeros, broadcast, multicast, or
/// locally administered (E-77).
pub(crate) fn mac_is_usable(mac: &[u8; 6]) -> bool {
    *mac != [0; 6] && *mac != [0xFF; 6] && mac[0] & 0x03 == 0
}

/// The lowest permanent MAC of a physical interface. WSL 2's `eth0` MAC changes
/// on every boot, so the anchor is not used there (E-43).
pub(crate) fn permanent_mac(out: &mut [u8; ANCHOR_BUF]) -> Reading {
    if virt::is_wsl() {
        return Reading::Unsupported;
    }
    let Ok(list) = net::interfaces() else {
        return Reading::Absent;
    };
    let lowest = list
        .iter()
        .filter(|i| i.is_physical)
        .filter_map(|i| i.mac)
        .filter(|m| m.kind == net::MacKind::Permanent && mac_is_usable(&m.bytes))
        .map(|m| m.bytes)
        .min();
    match lowest {
        Some(mac) => put(&mac, out),
        None => Reading::Absent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf() -> [u8; ANCHOR_BUF] {
        [0; ANCHOR_BUF]
    }

    #[test]
    fn test_canon_install_ids() {
        let mut out = buf();
        assert_eq!(
            install_id(b"5ca1ab1e5ca1ab1e5ca1ab1e5ca1ab1e\n", &mut out),
            Reading::Value(16)
        );
        assert_eq!(out[..4], [0x5c, 0xa1, 0xab, 0x1e]);
        // MachineGuid: the same bytes in either case, with dashes.
        let mut other = buf();
        install_id(b"5CA1AB1E-5CA1-AB1E-5CA1-AB1E5CA1AB1E", &mut other);
        assert_eq!(out, other);
        for p in [
            &b""[..],
            b"  \n",
            b"uninitialized\n",
            b"00000000000000000000000000000000",
            b"short",
            b"5ca1ab1e5ca1ab1e5ca1ab1e5ca1ab1e00",
        ] {
            assert_eq!(install_id(p, &mut buf()), Reading::Placeholder, "{p:?}");
        }
    }

    #[test]
    fn test_canon_uuid_text_and_raw_table_agree() {
        // E-75, E-76: Linux's text and Windows' raw table give one value.
        let mut text = buf();
        uuid_text(b"33221100-5544-7766-8899-AABBCCDDEEFF\n", &mut text);
        let mut raw = buf();
        uuid_smbios(
            [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ],
            &mut raw,
        );
        assert_eq!(text, raw);
        for p in [
            &b"00000000-0000-0000-0000-000000000000"[..],
            b"FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF",
        ] {
            assert_eq!(uuid_text(p, &mut buf()), Reading::Placeholder);
        }
        assert_eq!(uuid_smbios([0; 16], &mut buf()), Reading::Placeholder);
    }

    #[test]
    fn test_canon_serials() {
        let mut a = buf();
        assert_eq!(serial(b"  c02xg1\n", &mut a), Reading::Value(6));
        assert_eq!(&a[..6], b"C02XG1");
        assert_eq!(serial(b"To be filled by O.E.M.", &mut buf()), Reading::Placeholder);
        assert_eq!(serial(b"", &mut buf()), Reading::Placeholder);
        // Longer than the buffer: bounded.
        assert_eq!(serial(&[b'a'; 1000], &mut buf()), Reading::Value(ANCHOR_BUF));
    }

    #[test]
    fn test_canon_mac_rules() {
        assert!(mac_is_usable(&[0x00, 0x1b, 0x21, 1, 2, 3]));
        assert!(!mac_is_usable(&[0; 6]));
        assert!(!mac_is_usable(&[0xFF; 6]));
        assert!(!mac_is_usable(&[0x01, 0, 0, 0, 0, 1]), "multicast");
        assert!(!mac_is_usable(&[0x02, 0, 0, 0, 0, 1]), "locally administered");
    }
}
