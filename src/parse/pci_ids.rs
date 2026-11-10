//! The `pci.ids` database (`/usr/share/hwdata/pci.ids`,
//! `/usr/share/misc/pci.ids`): a vendor and device name for a pair of ids,
//! found by reading the file once, line by line, without keeping it.
//!
//! ```text
//! # comment
//! 10de  NVIDIA Corporation
//! <TAB>2786  AD104 [GeForce RTX 4070]
//! <TAB><TAB>1043 8888  subsystem
//! C 00  Unclassified device          (the class list: the end of the devices)
//! ```

use core::ops::ControlFlow;

use super::{lossy, trim};

/// The longest name kept.
const NAME_MAX: usize = 256;

/// A lookup in progress: feed it lines until it breaks.
#[derive(Debug)]
pub(crate) struct Lookup {
    vendor: u16,
    device: u16,
    in_vendor: bool,
    /// The vendor's name, once its line was seen.
    pub(crate) vendor_name: Option<String>,
    /// The device's name, once its line was seen.
    pub(crate) device_name: Option<String>,
}

/// Four hex digits, exactly.
fn hex4(s: &[u8]) -> Option<u16> {
    if s.len() != 4 || !s.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u16::from_str_radix(core::str::from_utf8(s).ok()?, 16).ok()
}

/// `id  name` split at the first run of spaces after four hex digits.
fn entry(line: &[u8]) -> Option<(u16, String)> {
    let id = hex4(line.get(..4)?)?;
    let rest = line.get(4..)?;
    if !rest.starts_with(b" ") {
        return None;
    }
    let name = trim(rest);
    (!name.is_empty()).then(|| (id, lossy(name, NAME_MAX)))
}

impl Lookup {
    pub(crate) const fn new(vendor: u16, device: u16) -> Lookup {
        Lookup {
            vendor,
            device,
            in_vendor: false,
            vendor_name: None,
            device_name: None,
        }
    }

    /// One line, without its newline. `Break` when the answer is known.
    pub(crate) fn line(&mut self, line: &[u8]) -> ControlFlow<()> {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || line[0] == b'#' {
            return ControlFlow::Continue(());
        }
        if line.starts_with(b"\t\t") {
            return ControlFlow::Continue(());
        }
        if let Some(rest) = line.strip_prefix(b"\t") {
            if self.in_vendor
                && let Some((id, name)) = entry(rest)
                && id == self.device
            {
                self.device_name = Some(name);
                return ControlFlow::Break(());
            }
            return ControlFlow::Continue(());
        }
        // A vendor line, or the class list.
        if self.in_vendor || line.starts_with(b"C ") {
            // Past our vendor's devices, or past every vendor.
            return ControlFlow::Break(());
        }
        if let Some((id, name)) = entry(line)
            && id == self.vendor
        {
            self.vendor_name = Some(name);
            self.in_vendor = true;
        }
        ControlFlow::Continue(())
    }
}

/// The names for a pair of ids in a whole file (tests and fuzzing; the backend
/// streams the file through [`Lookup`]).
#[cfg(any(test, fuzzing))]
pub(crate) fn find(data: &[u8], vendor: u16, device: u16) -> (Option<String>, Option<String>) {
    let mut lookup = Lookup::new(vendor, device);
    for line in data
        .split(|&b| b == b'\n')
        .take(super::mountinfo::MAX_LINES)
    {
        if lookup.line(line).is_break() {
            break;
        }
    }
    (lookup.vendor_name, lookup.device_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DB: &[u8] = b"# pci.ids sample\n\
#\tcomment with a tab\n\
1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\
\t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]\n\
\t\t1002 0e3b  RX 7900 GRE\n\
10de  NVIDIA Corporation\n\
\t2786  AD104 [GeForce RTX 4070]\n\
\t2788  AD104\r\n\
8086  Intel Corporation\n\
C 03  Display controller\n\
\t00  VGA compatible controller\n";

    #[test]
    fn test_pci_ids_finds_vendor_and_device() {
        assert_eq!(
            find(DB, 0x10de, 0x2786),
            (
                Some("NVIDIA Corporation".into()),
                Some("AD104 [GeForce RTX 4070]".into())
            )
        );
        assert_eq!(find(DB, 0x10de, 0x2788).1.as_deref(), Some("AD104"));
        // Subsystem lines are not devices.
        assert_eq!(find(DB, 0x1002, 0x0e3b).1, None);
        assert_eq!(
            find(DB, 0x1002, 0x744c).1.as_deref(),
            Some("Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]")
        );
    }

    #[test]
    fn test_pci_ids_unknown_and_malformed() {
        assert_eq!(
            find(DB, 0x8086, 0x1234),
            (Some("Intel Corporation".into()), None)
        );
        assert_eq!(find(DB, 0xabcd, 0x0001), (None, None));
        // The class list is never read as vendors.
        assert_eq!(find(DB, 0x0003, 0x0000), (None, None));
        assert_eq!(find(b"", 1, 2), (None, None));
        assert_eq!(find(b"10de\n\tzzzz  x\n10dX  y\n", 0x10de, 0), (None, None));
        assert_eq!(find(b"10deNVIDIA\n", 0x10de, 0), (None, None));
        assert_eq!(find(b"\xff\xfe\t\t\t\n\t", 0, 0), (None, None));
    }
}
