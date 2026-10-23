//! `/proc/cpuinfo`: one block of `key\t: value` lines per logical CPU.
//! x86 blocks carry `vendor_id`, `model name`, `physical id`, `core id`, and
//! `flags`; AArch64 blocks carry `CPU implementer`, `CPU part`, and
//! `CPU architecture`.

use std::collections::BTreeSet;

/// At most this many lines are read (16 384 CPUs times about 30 lines).
pub(crate) const MAX_LINES: usize = 1 << 19;
/// At most this many distinct ARM core kinds are kept.
const MAX_PARTS: usize = 16;

/// What `/proc/cpuinfo` says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CpuInfo {
    /// `model name` of the first CPU (x86, some ARM kernels).
    pub(crate) model_name: Option<String>,
    /// `vendor_id` (x86): `GenuineIntel`, `AuthenticAMD`.
    pub(crate) vendor_id: Option<String>,
    /// The number of `processor` blocks.
    pub(crate) processors: usize,
    /// Distinct `(implementer, part)` pairs (AArch64), in first-seen order.
    pub(crate) arm_parts: Vec<(u32, u32)>,
    /// Distinct `(physical id, core id)` pairs (x86), when every block has both.
    pub(crate) cores: Option<usize>,
    /// x86 `lm` (long mode) flag, or AArch64 `CPU architecture: 8` or later:
    /// the processor runs 64-bit code.
    pub(crate) is_64bit: Option<bool>,
}

/// Parses `/proc/cpuinfo`.
pub(crate) fn parse(data: &[u8]) -> CpuInfo {
    let mut info = CpuInfo::default();
    let mut core_pairs = BTreeSet::new();
    let mut pair_missing = false;
    let (mut physical, mut core) = (None::<u64>, None::<u64>);
    let mut implementer = None::<u32>;
    let mut in_block = false;

    for line in super::lines(data, MAX_LINES) {
        let line = super::trim(line);
        if line.is_empty() {
            if in_block {
                end_block(&mut physical, &mut core, &mut core_pairs, &mut pair_missing);
            }
            in_block = false;
            continue;
        }
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let key = super::trim(&line[..colon]);
        let value = super::trim(&line[colon + 1..]);
        match key {
            b"processor" => {
                if in_block {
                    end_block(&mut physical, &mut core, &mut core_pairs, &mut pair_missing);
                }
                in_block = true;
                info.processors = info.processors.saturating_add(1);
            }
            b"model name" if info.model_name.is_none() && !value.is_empty() => {
                info.model_name = Some(super::lossy(value, 256));
            }
            b"vendor_id" if info.vendor_id.is_none() && !value.is_empty() => {
                info.vendor_id = Some(super::lossy(value, 64));
            }
            b"physical id" => physical = super::parse_u64(value),
            b"core id" => core = super::parse_u64(value),
            b"flags" if info.is_64bit.is_none() => {
                info.is_64bit = Some(value.split(|&b| b == b' ').any(|f| f == b"lm"));
            }
            b"CPU architecture" if info.is_64bit.is_none() => {
                info.is_64bit = super::parse_u64(value).map(|v| v >= 8);
            }
            b"CPU implementer" => {
                implementer = super::parse_hex(value).and_then(|v| u32::try_from(v).ok())
            }
            b"CPU part" => {
                let part = super::parse_hex(value).and_then(|v| u32::try_from(v).ok());
                if let (Some(i), Some(p)) = (implementer, part)
                    && !info.arm_parts.contains(&(i, p))
                    && info.arm_parts.len() < MAX_PARTS
                {
                    info.arm_parts.push((i, p));
                }
            }
            _ => {}
        }
    }
    if in_block {
        end_block(&mut physical, &mut core, &mut core_pairs, &mut pair_missing);
    }
    if info.processors > 0 && !pair_missing && !core_pairs.is_empty() {
        info.cores = Some(core_pairs.len());
    }
    info
}

/// Records the `(physical id, core id)` pair of the block that just ended.
fn end_block(
    physical: &mut Option<u64>,
    core: &mut Option<u64>,
    pairs: &mut BTreeSet<(u64, u64)>,
    missing: &mut bool,
) {
    match (physical.take(), core.take()) {
        (Some(p), Some(c)) => {
            if pairs.len() < super::cpu_list::MAX_CPUS {
                pairs.insert((p, c));
            }
        }
        _ => *missing = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const X86: &[u8] = b"processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 9 7950X 16-Core Processor\n\
physical id\t: 0\ncore id\t\t: 0\nflags\t\t: fpu vme lm sse2\n\n\
processor\t: 1\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 9 7950X 16-Core Processor\n\
physical id\t: 0\ncore id\t\t: 0\nflags\t\t: fpu vme lm sse2\n\n\
processor\t: 2\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 9 7950X 16-Core Processor\n\
physical id\t: 0\ncore id\t\t: 1\nflags\t\t: fpu vme lm sse2\n";

    const ARM: &[u8] =
        b"processor\t: 0\nBogoMIPS\t: 108.00\nFeatures\t: fp asimd\nCPU implementer\t: 0x41\n\
CPU architecture: 8\nCPU variant\t: 0x4\nCPU part\t: 0xd0b\nCPU revision\t: 1\n\n\
processor\t: 1\nCPU implementer\t: 0x41\nCPU architecture: 8\nCPU part\t: 0xd05\n\n\
processor\t: 2\nCPU implementer\t: 0x41\nCPU architecture: 8\nCPU part\t: 0xd05\n\n\
Hardware\t: BCM2835\nSerial\t\t: 0000000000000000\n";

    #[test]
    fn test_cpuinfo_x86_fields_and_cores() {
        let i = parse(X86);
        assert_eq!(i.processors, 3);
        assert_eq!(i.vendor_id.as_deref(), Some("AuthenticAMD"));
        assert_eq!(
            i.model_name.as_deref(),
            Some("AMD Ryzen 9 7950X 16-Core Processor")
        );
        assert_eq!(i.cores, Some(2));
        assert_eq!(i.is_64bit, Some(true));
        assert!(i.arm_parts.is_empty());
    }

    #[test]
    fn test_cpuinfo_arm_parts_in_first_seen_order() {
        let i = parse(ARM);
        assert_eq!(i.processors, 3);
        assert_eq!(i.arm_parts, vec![(0x41, 0xd0b), (0x41, 0xd05)]);
        assert_eq!(i.cores, None);
        assert_eq!(i.is_64bit, Some(true));
        assert_eq!(i.model_name, None);
    }

    #[test]
    fn test_cpuinfo_missing_core_ids_give_no_core_count() {
        let i = parse(b"processor\t: 0\nphysical id\t: 0\ncore id\t: 0\n\nprocessor\t: 1\n");
        assert_eq!(i.processors, 2);
        assert_eq!(i.cores, None);
    }

    #[test]
    fn test_cpuinfo_never_panics_on_odd_input() {
        for input in [
            &b""[..],
            b":",
            b"processor",
            b"processor:\n:\n",
            b"\xff:\xff",
            b"CPU part: 0xd0b\n",
        ] {
            let _ = parse(input);
        }
    }
}
