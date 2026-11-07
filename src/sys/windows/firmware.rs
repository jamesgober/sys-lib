//! SMBIOS: the raw table from `GetSystemFirmwareTable('RSMB')`, what it says
//! about the machine, and the device identifiers it holds.
//!
//! The raw table holds serial numbers and the system UUID, so it is never kept:
//! each read goes into a [`Table`], wiped on drop, and only the parsed values
//! that carry no identifier are cached.

use core::sync::atomic::{Ordering, compiler_fence};
use std::sync::OnceLock;

use super::{ffi, last_error};
use crate::{
    Error, Result,
    mem::MemoryModule,
    parse::smbios::{self, Facts, MemoryDevice},
};

/// `RawSMBIOSData`: the fields before `SMBIOSTableData`.
const HEADER: usize = 8;

/// The largest table accepted (threat T7).
const MAX_TABLE: usize = 1 << 20;

/// Zeroes `buf` with writes that survive optimisation.
pub(super) fn wipe<T: Copy + Default>(buf: &mut [T]) {
    for slot in buf.iter_mut() {
        // SAFETY: `slot` is a valid, exclusive reference to an initialised `T`.
        unsafe { core::ptr::write_volatile(slot, T::default()) };
    }
    compiler_fence(Ordering::SeqCst);
}

/// One read of the raw table, wiped on drop.
struct Table {
    buf: Vec<u8>,
    len: usize,
}

impl Table {
    /// Asks for the size and reads, at most 3 times. The failure is the error
    /// code.
    fn read() -> core::result::Result<Table, i32> {
        // SAFETY: a null buffer with size 0 asks for the size needed.
        let mut size =
            unsafe { ffi::GetSystemFirmwareTable(ffi::RSMB, 0, std::ptr::null_mut(), 0) } as usize;
        for _ in 0..3 {
            if size == 0 {
                return Err(last_error());
            }
            if size > MAX_TABLE {
                break;
            }
            let mut table = Table {
                buf: vec![0; size],
                len: 0,
            };
            // SAFETY: `buf` is `size` writable bytes.
            let needed = unsafe {
                ffi::GetSystemFirmwareTable(ffi::RSMB, 0, table.buf.as_mut_ptr(), size as u32)
            } as usize;
            if needed != 0 && needed <= size {
                table.len = needed;
                return Ok(table);
            }
            size = needed;
        }
        Err(ffi::ERROR_INSUFFICIENT_BUFFER as i32)
    }

    fn raw(&self) -> &[u8] {
        &self.buf[..self.len.min(self.buf.len())]
    }
}

impl Drop for Table {
    fn drop(&mut self) {
        wipe(&mut self.buf);
    }
}

/// The structures of a `RawSMBIOSData`: what follows the header, cut to the
/// length the header gives and to the bytes that are there.
fn structures_of(raw: &[u8]) -> &[u8] {
    let Some(length) = raw.get(4..HEADER) else {
        return &[];
    };
    let length = u32::from_le_bytes([length[0], length[1], length[2], length[3]]) as usize;
    let end = HEADER.saturating_add(length).min(raw.len());
    &raw[HEADER..end]
}

/// What the table says, kept for the life of the process. No identifier.
struct Parsed {
    facts: Facts,
    modules: Vec<MemoryDevice>,
}

/// The parsed table, read once; the failure is the error code.
fn parsed() -> core::result::Result<&'static Parsed, i32> {
    static PARSED: OnceLock<core::result::Result<Parsed, i32>> = OnceLock::new();
    PARSED
        .get_or_init(|| {
            let table = Table::read()?;
            let structures = structures_of(table.raw());
            Ok(Parsed {
                facts: smbios::facts(structures),
                modules: smbios::memory_devices(structures),
            })
        })
        .as_ref()
        .map_err(|&code| code)
}

/// The firmware's facts, when the table can be read.
pub(super) fn facts() -> Option<&'static Facts> {
    parsed().ok().map(|p| &p.facts)
}

pub(crate) fn memory_modules() -> Result<Vec<MemoryModule>> {
    match parsed() {
        Ok(p) => Ok(p
            .modules
            .iter()
            .cloned()
            .map(crate::mem::module_from)
            .collect()),
        Err(code) if code as u32 == ffi::ERROR_ACCESS_DENIED => Err(Error::PermissionDenied {
            what: "memory modules",
            hint: "run as a user that may read the firmware tables, or treat the value as unknown",
        }),
        Err(code) => Err(Error::Os {
            what: "memory modules",
            call: "GetSystemFirmwareTable",
            code,
        }),
    }
}

#[cfg(feature = "device-id")]
pub(crate) use anchors::device_anchor;

#[cfg(feature = "device-id")]
mod anchors {
    use super::{super::Key, Table, ffi, smbios, structures_of, wipe};
    use crate::device::{ANCHOR_BUF, Anchor, Reading, canon};

    /// The key holding `MachineGuid`.
    const CRYPTOGRAPHY: &str = r"SOFTWARE\Microsoft\Cryptography";

    /// A `MachineGuid` is 36 characters; anything longer is not one.
    const GUID_BUF: usize = 64;

    /// The SMBIOS version the UUID's byte order changed in (E-75).
    const UUID_LITTLE_ENDIAN: (u8, u8) = (2, 6);

    /// Reads an anchor into `out`, in canonical form; the raw text stays in
    /// wiped buffers. `CpuSignature` and `PermanentMac` are read by
    /// `crate::device` itself.
    pub(crate) fn device_anchor(anchor: Anchor, out: &mut [u8; ANCHOR_BUF]) -> Reading {
        match anchor {
            Anchor::InstallId => install_id(out),
            Anchor::PlatformUuid | Anchor::BoardSerial | Anchor::ProductSerial => {
                from_table(anchor, out)
            }
            Anchor::CpuSignature | Anchor::PermanentMac => Reading::Unsupported,
        }
    }

    /// `MachineGuid`, in the 64-bit view of the registry whatever this process
    /// is (E-74).
    fn install_id(out: &mut [u8; ANCHOR_BUF]) -> Reading {
        let Some(key) = Key::local_machine(CRYPTOGRAPHY) else {
            return Reading::Absent;
        };
        let mut wide = [0u16; GUID_BUF];
        let Some(units) = key.wide("MachineGuid", &mut wide) else {
            wipe(&mut wide);
            return Reading::Absent;
        };
        // A unit outside ASCII becomes a NUL, which the hex parser refuses.
        let mut text = [0u8; GUID_BUF];
        for (byte, &unit) in text.iter_mut().zip(&wide[..units]) {
            *byte = u8::try_from(unit).unwrap_or(0);
        }
        let reading = canon::install_id(&text[..units], out);
        wipe(&mut wide);
        wipe(&mut text);
        reading
    }

    /// Reads the table afresh for this anchor: the identifiers are never cached.
    fn from_table(anchor: Anchor, out: &mut [u8; ANCHOR_BUF]) -> Reading {
        let table = match Table::read() {
            Ok(table) => table,
            Err(code) if code as u32 == ffi::ERROR_ACCESS_DENIED => {
                return Reading::PermissionDenied;
            }
            Err(_) => return Reading::Absent,
        };
        let raw = table.raw();
        let found = smbios::anchors(structures_of(raw));
        let text = |serial: Option<&[u8]>, out: &mut [u8; ANCHOR_BUF]| match serial {
            Some(s) => canon::serial(s, out),
            None => Reading::Absent,
        };
        match anchor {
            Anchor::BoardSerial => text(found.board_serial, out),
            Anchor::ProductSerial => text(found.product_serial, out),
            _ => match found.uuid {
                Some(uuid) => {
                    let version = (
                        raw.get(1).copied().unwrap_or(0),
                        raw.get(2).copied().unwrap_or(0),
                    );
                    canon::uuid_smbios(stored_order(uuid, version), out)
                }
                None => Reading::Absent,
            },
        }
    }

    /// Before SMBIOS 2.6 the UUID is stored whole, in RFC 9562 order, so the
    /// swap `canon::uuid_smbios` makes for a little-endian one is undone here
    /// (E-75). Linux's `product_uuid` text makes the same distinction.
    fn stored_order(uuid: [u8; 16], version: (u8, u8)) -> [u8; 16] {
        if version < UUID_LITTLE_ENDIAN {
            smbios::uuid_rfc_order(uuid)
        } else {
            uuid
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_firmware_windows_uuid_order_by_smbios_version() {
            let stored = [
                0x33, 0x22, 0x11, 0x00, 0x55, 0x44, 0x77, 0x66, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ];
            let mut out = [0u8; ANCHOR_BUF];
            // SMBIOS 2.5: the bytes are the UUID already.
            assert_eq!(
                canon::uuid_smbios(stored_order(stored, (2, 5)), &mut out),
                Reading::Value(16)
            );
            assert_eq!(out[..16], stored);
            // SMBIOS 2.6 and 3.x: the first three fields are swapped.
            for version in [(2, 6), (3, 0)] {
                canon::uuid_smbios(stored_order(stored, version), &mut out);
                assert_eq!(out[..4], [0x00, 0x11, 0x22, 0x33], "{version:?}");
            }
        }

        #[test]
        fn test_firmware_windows_live_anchors_are_readings() {
            let mut out = [0u8; ANCHOR_BUF];
            assert_eq!(
                device_anchor(Anchor::CpuSignature, &mut out),
                Reading::Unsupported
            );
            assert_eq!(
                device_anchor(Anchor::PermanentMac, &mut out),
                Reading::Unsupported
            );
            // MachineGuid exists on every Windows install.
            assert_eq!(
                device_anchor(Anchor::InstallId, &mut out),
                Reading::Value(16)
            );
            for anchor in [
                Anchor::PlatformUuid,
                Anchor::BoardSerial,
                Anchor::ProductSerial,
            ] {
                match device_anchor(anchor, &mut out) {
                    Reading::Value(n) => assert!(n > 0 && n <= ANCHOR_BUF),
                    Reading::Absent | Reading::Placeholder | Reading::PermissionDenied => {}
                    Reading::Unsupported => panic!("{anchor:?}"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_firmware_windows_header_is_skipped_and_bounds_hold() {
        let mut raw = vec![0, 3, 4, 0];
        raw.extend_from_slice(&3u32.to_le_bytes());
        raw.extend_from_slice(&[7, 8, 9, 10]);
        assert_eq!(structures_of(&raw), &[7, 8, 9]);
        // A length past the end stops at the end.
        raw[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(structures_of(&raw), &[7, 8, 9, 10]);
        assert_eq!(structures_of(&[0; 7]), &[] as &[u8]);
        assert_eq!(structures_of(&[]), &[] as &[u8]);
    }

    #[test]
    fn test_firmware_windows_wipe_zeroes_every_element() {
        let mut bytes = [0xAAu8; 32];
        wipe(&mut bytes);
        assert_eq!(bytes, [0; 32]);
        let mut units = [0xBBBBu16; 8];
        wipe(&mut units);
        assert_eq!(units, [0; 8]);
    }

    #[test]
    fn test_firmware_windows_live_table_reads_and_parses() {
        let table = Table::read().unwrap();
        let structures = structures_of(table.raw());
        assert!(smbios::structures(structures).count() > 0);
        assert!(smbios::structures(structures).any(|s| s.kind == 127));
        let facts = facts().unwrap();
        assert!(facts.bios.is_some() && facts.system.is_some());
        // The table parsed twice gives one answer.
        assert_eq!(*facts, smbios::facts(structures));
        let modules = memory_modules().unwrap();
        assert!(modules.iter().all(|m| m.size.raw() > 0));
    }
}
