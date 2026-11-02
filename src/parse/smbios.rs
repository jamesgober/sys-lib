//! SMBIOS tables (DSP0134): the firmware's description of the machine, read
//! from `/sys/firmware/dmi/tables/DMI` on Linux and `GetSystemFirmwareTable`
//! on Windows. Firmware is not trusted (threat T15): every structure is checked
//! against the end of the table, at most [`MAX_STRUCTURES`] are walked, and a
//! missing terminator ends the walk.
//!
//! Serial numbers are read by [`anchors`] only, for device identity: the types
//! the rest of the crate sees never carry one.

#![cfg_attr(not(any(windows, test, fuzzing)), allow(dead_code))]

/// At most this many structures are walked (T15).
pub(crate) const MAX_STRUCTURES: usize = 4096;
/// The longest string kept.
const MAX_TEXT: usize = 256;

/// One structure: its formatted area and the strings after it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Structure<'a> {
    pub(crate) kind: u8,
    formatted: &'a [u8],
    strings: &'a [u8],
}

impl<'a> Structure<'a> {
    fn byte(&self, offset: usize) -> Option<u8> {
        self.formatted.get(offset).copied()
    }

    fn word(&self, offset: usize) -> Option<u16> {
        let b = self.formatted.get(offset..offset + 2)?;
        Some(u16::from_le_bytes([b[0], b[1]]))
    }

    fn dword(&self, offset: usize) -> Option<u32> {
        let b = self.formatted.get(offset..offset + 4)?;
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The string a formatted field at `offset` points to (1-based; 0 is none).
    fn string(&self, offset: usize) -> Option<&'a [u8]> {
        let index = usize::from(self.byte(offset)?);
        if index == 0 {
            return None;
        }
        self.strings
            .split(|&b| b == 0)
            .nth(index - 1)
            .filter(|s| !s.is_empty())
    }

    /// The string at `offset`, cleaned; `None` for a placeholder.
    fn text(&self, offset: usize) -> Option<String> {
        clean(self.string(offset)?)
    }
}

/// The structures of a table, in order, up to the end-of-table structure.
pub(crate) fn structures(table: &[u8]) -> impl Iterator<Item = Structure<'_>> {
    let mut rest = table;
    let mut left = MAX_STRUCTURES;
    core::iter::from_fn(move || {
        if left == 0 || rest.len() < 4 {
            return None;
        }
        left -= 1;
        let (kind, len) = (rest[0], usize::from(rest[1]));
        if len < 4 || len > rest.len() {
            return None;
        }
        let (formatted, after) = rest.split_at(len);
        // The string area ends with two zero bytes.
        let end = after.windows(2).position(|w| w == [0, 0])?;
        rest = &after[end + 2..];
        if kind == 127 {
            left = 0;
        }
        Some(Structure {
            kind,
            formatted,
            strings: &after[..end],
        })
    })
}

/// Strings firmware ships instead of a value, compared without case (E-70).
const PLACEHOLDERS: &[&str] = &[
    "to be filled by o.e.m.",
    "to be filled by oem",
    "default string",
    "system serial number",
    "system product name",
    "system manufacturer",
    "system version",
    "base board serial number",
    "base board product name",
    "base board manufacturer",
    "chassis serial number",
    "chassis manufacturer",
    "not specified",
    "not applicable",
    "not available",
    "unknown",
    "none",
    "n/a",
    "na",
    "oem",
    "o.e.m.",
    "invalid",
    "0",
    "0123456789",
    "123456789",
    "1234567890",
    "x.x",
];

/// Whether a firmware string says nothing: empty, whitespace, a repeated
/// filler character, or one of the known placeholders (E-70).
pub(crate) fn is_placeholder(text: &[u8]) -> bool {
    let t = text.trim_ascii();
    t.is_empty()
        || PLACEHOLDERS.iter().any(|p| t.eq_ignore_ascii_case(p.as_bytes()))
        || t.iter().all(|&b| b == t[0]) && matches!(t[0], b'0' | b'x' | b'X' | b'*' | b'-' | b'.')
}

/// A firmware string as text: lossy, bounded, trimmed, `None` for a placeholder.
pub(crate) fn clean(text: &[u8]) -> Option<String> {
    if is_placeholder(text) {
        return None;
    }
    Some(super::lossy(text.trim_ascii(), MAX_TEXT))
}

/// SMBIOS's `MM/DD/YYYY` (`MM/DD/YY` on old firmware) as `YYYY-MM-DD`; any
/// other text stays as it is.
pub(crate) fn iso_date(text: &str) -> String {
    let parts: Vec<&str> = text.trim().split('/').collect();
    if let [m, d, y] = parts[..] {
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if digits(m) && digits(d) && digits(y) && m.len() <= 2 && d.len() <= 2 {
            let year = match y.len() {
                4 => y.to_owned(),
                // Two-digit years: SMBIOS 2.3 and earlier, so before 2000 unless small.
                2 if y < "70" => format!("20{y}"),
                2 => format!("19{y}"),
                _ => return text.to_owned(),
            };
            return format!("{year}-{m:0>2}-{d:0>2}");
        }
    }
    text.to_owned()
}

/// Type 0: the firmware.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Bios {
    pub(crate) vendor: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) date: Option<String>,
}

/// Type 1: the system. No serial, no UUID (see [`anchors`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct System {
    pub(crate) vendor: Option<String>,
    pub(crate) product: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) family: Option<String>,
}

/// Type 2: the board. No serial.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Board {
    pub(crate) vendor: Option<String>,
    pub(crate) product: Option<String>,
    pub(crate) version: Option<String>,
}

/// The facts a table holds about the machine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    pub(crate) bios: Option<Bios>,
    pub(crate) system: Option<System>,
    pub(crate) board: Option<Board>,
    /// Type 3: the chassis type, with the lock bit removed.
    pub(crate) chassis: Option<u8>,
    /// Type 3: the asset tag (clouds put their name here).
    pub(crate) asset_tag: Option<String>,
}

/// The first structure of each kind wins: firmware lists the main board first.
pub(crate) fn facts(table: &[u8]) -> Facts {
    let mut out = Facts::default();
    for s in structures(table) {
        match s.kind {
            0 if out.bios.is_none() => {
                out.bios = Some(Bios {
                    vendor: s.text(4),
                    version: s.text(5),
                    date: s.text(8).map(|d| iso_date(&d)),
                });
            }
            1 if out.system.is_none() => {
                out.system = Some(System {
                    vendor: s.text(4),
                    product: s.text(5),
                    version: s.text(6),
                    family: s.text(0x1A),
                });
            }
            2 if out.board.is_none() => {
                out.board = Some(Board {
                    vendor: s.text(4),
                    product: s.text(5),
                    version: s.text(6),
                });
            }
            3 if out.chassis.is_none() => {
                out.chassis = s.byte(5).map(|b| b & 0x7F);
                out.asset_tag = s.text(8);
            }
            _ => {}
        }
    }
    out
}

/// The identifiers a table holds, borrowed from it so the caller wipes one
/// buffer. Device identity only (threat T1).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Anchors<'a> {
    /// The system UUID as stored: the first three fields little-endian.
    pub(crate) uuid: Option<[u8; 16]>,
    pub(crate) board_serial: Option<&'a [u8]>,
    pub(crate) product_serial: Option<&'a [u8]>,
}

/// Reads the identifiers; placeholders are left in, for the caller to report.
pub(crate) fn anchors(table: &[u8]) -> Anchors<'_> {
    let mut out = Anchors::default();
    let (mut system, mut board) = (false, false);
    for s in structures(table) {
        match s.kind {
            1 if !system => {
                system = true;
                out.product_serial = s.string(7);
                out.uuid = s
                    .formatted
                    .get(8..24)
                    .and_then(|b| <[u8; 16]>::try_from(b).ok());
            }
            2 if !board => {
                board = true;
                out.board_serial = s.string(7);
            }
            _ => {}
        }
        if system && board {
            break;
        }
    }
    out
}

/// A SMBIOS UUID (first three fields little-endian since SMBIOS 2.6) in RFC 9562
/// byte order, the order Linux's `product_uuid` text gives (E-75).
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn uuid_rfc_order(raw: [u8; 16]) -> [u8; 16] {
    let mut u = raw;
    u[..4].reverse();
    u[4..6].reverse();
    u[6..8].reverse();
    u
}

/// Whether a UUID is the all-zero or all-`F` value firmware ships (E-70).
#[cfg_attr(not(feature = "device-id"), allow(dead_code))]
pub(crate) fn uuid_is_placeholder(uuid: &[u8; 16]) -> bool {
    uuid.iter().all(|&b| b == 0) || uuid.iter().all(|&b| b == 0xFF)
}

/// Type 17: one memory device, as read; the kind and form are raw SMBIOS codes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemoryDevice {
    pub(crate) bytes: u64,
    pub(crate) kind: u8,
    pub(crate) form: u8,
    pub(crate) speed_mts: Option<u32>,
    pub(crate) configured_speed_mts: Option<u32>,
    pub(crate) locator: Option<String>,
    pub(crate) manufacturer: Option<String>,
    pub(crate) part_number: Option<String>,
}

/// A speed field: 0 is unknown, `0xFFFF` means the extended field holds it.
fn speed(s: &Structure<'_>, short: usize, extended: usize) -> Option<u32> {
    match s.word(short)? {
        0 => None,
        0xFFFF => s.dword(extended).map(|v| v & 0x7FFF_FFFF).filter(|&v| v != 0),
        v => Some(u32::from(v)),
    }
}

/// The installed memory devices: empty slots are left out. The serial field
/// (offset 0x18) is never read.
pub(crate) fn memory_devices(table: &[u8]) -> Vec<MemoryDevice> {
    let mut out = Vec::new();
    for s in structures(table).filter(|s| s.kind == 17) {
        let bytes = match s.word(0x0C) {
            None | Some(0 | 0xFFFF) => continue,
            // The size is in the extended field, in MiB.
            Some(0x7FFF) => match s.dword(0x1C) {
                Some(mib) => u64::from(mib & 0x7FFF_FFFF) << 20,
                None => continue,
            },
            // Bit 15 set: KiB; clear: MiB.
            Some(v) if v & 0x8000 != 0 => u64::from(v & 0x7FFF) << 10,
            Some(v) => u64::from(v) << 20,
        };
        out.push(MemoryDevice {
            bytes,
            kind: s.byte(0x12).unwrap_or(0),
            form: s.byte(0x0E).unwrap_or(0),
            speed_mts: speed(&s, 0x15, 0x54),
            configured_speed_mts: speed(&s, 0x20, 0x58),
            locator: s.text(0x10),
            manufacturer: s.text(0x17),
            part_number: s.text(0x1A),
        });
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One structure: `body` after the four-byte header, then its strings.
    pub(crate) fn structure(kind: u8, body: &[u8], strings: &[&str]) -> Vec<u8> {
        let mut v = vec![kind, u8::try_from(body.len() + 4).unwrap(), 0, 0];
        v.extend_from_slice(body);
        for s in strings {
            v.extend_from_slice(s.as_bytes());
            v.push(0);
        }
        if strings.is_empty() {
            v.push(0);
        }
        v.push(0);
        v
    }

    fn type1(uuid: [u8; 16], serial: u8) -> Vec<u8> {
        let mut body = vec![1, 2, 3, serial];
        body.extend_from_slice(&uuid);
        body.extend_from_slice(&[6, 0, 0, 5]); // wake-up, SKU, family
        structure(
            1,
            &body,
            &["ACME", "Rocket", "1.2", "SERIAL-1", "Rockets"],
        )
    }

    pub(crate) fn sample() -> Vec<u8> {
        let mut t = Vec::new();
        t.extend(structure(
            0,
            &[1, 2, 0, 0, 3, 0],
            &["ACME BIOS", "1.40", "03/14/2025"],
        ));
        t.extend(type1([0x11; 16], 4));
        t.extend(structure(
            2,
            &[1, 2, 3, 4],
            &["ACME", "Board X", "To be filled by O.E.M.", "BOARD-9"],
        ));
        t.extend(structure(3, &[1, 0x89, 0, 0, 2], &["ACME", "Asset 7"]));
        t.extend(structure(127, &[], &[]));
        t
    }

    #[test]
    fn test_smbios_facts_from_a_table() {
        let f = facts(&sample());
        let bios = f.bios.unwrap();
        assert_eq!(bios.vendor.as_deref(), Some("ACME BIOS"));
        assert_eq!(bios.date.as_deref(), Some("2025-03-14"));
        let system = f.system.unwrap();
        assert_eq!(system.product.as_deref(), Some("Rocket"));
        assert_eq!(system.family.as_deref(), Some("Rockets"));
        let board = f.board.unwrap();
        assert_eq!(board.product.as_deref(), Some("Board X"));
        assert_eq!(board.version, None, "a placeholder is None");
        assert_eq!(f.chassis, Some(9), "the lock bit is removed");
        assert_eq!(f.asset_tag.as_deref(), Some("Asset 7"));
    }

    #[test]
    fn test_smbios_serials_only_through_anchors() {
        let t = sample();
        let text = format!("{:?}", facts(&t));
        assert!(!text.contains("SERIAL-1") && !text.contains("BOARD-9"));
        let a = anchors(&t);
        assert_eq!(a.product_serial, Some(&b"SERIAL-1"[..]));
        assert_eq!(a.board_serial, Some(&b"BOARD-9"[..]));
        assert_eq!(a.uuid, Some([0x11; 16]));
    }

    #[test]
    fn test_smbios_uuid_order_matches_the_linux_text() {
        // E-75: the bytes a Windows raw table stores for the UUID Linux prints as
        // 33221100-5544-7766-8899-aabbccddeeff.
        let raw = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        assert_eq!(
            uuid_rfc_order(raw),
            [
                0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff
            ]
        );
        assert!(uuid_is_placeholder(&[0; 16]) && uuid_is_placeholder(&[0xFF; 16]));
        assert!(!uuid_is_placeholder(&raw));
    }

    #[test]
    fn test_smbios_placeholders() {
        for p in [
            "",
            "   ",
            "To be filled by O.E.M.",
            "Default string",
            "System Serial Number",
            "Not Specified",
            "Not Applicable",
            "None",
            "0",
            "0123456789",
            "123456789",
            "Chassis Serial Number",
            "Base Board Serial Number",
            "000000",
            "XXXXXXXX",
        ] {
            assert!(is_placeholder(p.as_bytes()), "{p:?}");
        }
        for v in ["C02XG1", "Micro-Star International Co., Ltd.", "1.0", "a"] {
            assert!(!is_placeholder(v.as_bytes()), "{v:?}");
        }
    }

    #[test]
    fn test_smbios_dates() {
        assert_eq!(iso_date("03/14/2025"), "2025-03-14");
        assert_eq!(iso_date("3/4/2025"), "2025-03-04");
        assert_eq!(iso_date("12/31/99"), "1999-12-31");
        assert_eq!(iso_date("01/01/05"), "2005-01-01");
        assert_eq!(iso_date("2025-03-14"), "2025-03-14");
        assert_eq!(iso_date("March 2025"), "March 2025");
        assert_eq!(iso_date("1/2/345"), "1/2/345");
    }

    fn memory(size: u16, extended: u32, serial_string: u8) -> Vec<u8> {
        let mut body = [0u8; 0x5C - 4];
        let at = |o: usize| o - 4;
        body[at(0x0C)..at(0x0E)].copy_from_slice(&size.to_le_bytes());
        body[at(0x0E)] = 0x09; // DIMM
        body[at(0x10)] = 1;
        body[at(0x12)] = 0x22; // DDR5
        body[at(0x15)..at(0x17)].copy_from_slice(&0xFFFFu16.to_le_bytes());
        body[at(0x17)] = 2;
        body[at(0x18)] = serial_string;
        body[at(0x1A)] = 4;
        body[at(0x1C)..at(0x20)].copy_from_slice(&extended.to_le_bytes());
        body[at(0x20)..at(0x22)].copy_from_slice(&5600u16.to_le_bytes());
        body[at(0x54)..at(0x58)].copy_from_slice(&8000u32.to_le_bytes());
        structure(17, &body, &["DIMM_A1", "Kingston", "SERIAL-M", "KF556"])
    }

    #[test]
    fn test_smbios_memory_devices() {
        let mut t = memory(0x4000, 0, 3); // 16 GiB
        t.extend(memory(0, 0, 3)); // an empty slot
        t.extend(memory(0x7FFF, 64 << 10, 3)); // 64 GiB through the extended size
        t.extend(memory(0x8000 | 512, 0, 3)); // 512 KiB
        t.extend(structure(127, &[], &[]));
        let m = memory_devices(&t);
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].bytes, 16 << 30);
        assert_eq!(m[1].bytes, 64 << 30);
        assert_eq!(m[2].bytes, 512 << 10);
        assert_eq!((m[0].kind, m[0].form), (0x22, 0x09));
        assert_eq!(m[0].speed_mts, Some(8000));
        assert_eq!(m[0].configured_speed_mts, Some(5600));
        assert_eq!(m[0].locator.as_deref(), Some("DIMM_A1"));
        assert_eq!(m[0].part_number.as_deref(), Some("KF556"));
        assert!(!format!("{m:?}").contains("SERIAL-M"));
    }

    #[test]
    fn test_smbios_hostile_tables_stop_cleanly() {
        // T15: lengths past the end, too short, no terminator, huge counts.
        assert_eq!(structures(&[]).count(), 0);
        assert_eq!(structures(&[1, 200, 0, 0]).count(), 0);
        assert_eq!(structures(&[1, 3, 0, 0, 0, 0]).count(), 0);
        assert_eq!(structures(&[1, 4, 0, 0, b'a']).count(), 0);
        let endless = vec![9u8, 4, 0, 0, 0, 0].repeat(MAX_STRUCTURES + 10);
        assert_eq!(structures(&endless).count(), MAX_STRUCTURES);
        // Nothing past the end-of-table structure.
        let mut t = structure(127, &[], &[]);
        t.extend(type1([1; 16], 4));
        assert!(facts(&t).system.is_none());
        // A string index past the strings.
        let s = structure(0, &[9, 0, 0, 0], &["a"]);
        assert_eq!(facts(&s).bios.unwrap().vendor, None);
        // A truncated formatted area: fields past its end are absent.
        let short = structure(1, &[1], &["ACME"]);
        assert_eq!(anchors(&short).uuid, None);
        assert_eq!(memory_devices(&structure(17, &[0; 4], &[])), vec![]);
    }
}
