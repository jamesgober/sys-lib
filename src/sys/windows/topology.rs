//! Packages, cores, caches, and NUMA nodes, from the variable-size records of
//! `GetLogicalProcessorInformationEx`, parsed from bytes in safe Rust. Windows
//! names a logical CPU by its group and bit; [`Groups`] turns that into one
//! dense index, the same across every group.

use super::{ffi, ffi::lpi, last_error, memory_status};
use crate::{
    Bytes,
    cpu::{Cache, CacheKind, Core, CoreKind, NumaNode, Package, Topology},
    sys::TopologyFacts,
};

/// The largest record buffer accepted (threat T7).
const MAX_RECORDS: usize = 16 << 20;

/// A record buffer, 8-byte aligned as the structs require.
pub(super) struct Records {
    buf: Vec<u64>,
    len: usize,
}

impl Records {
    /// Asks for the size and reads, at most 3 times: a processor coming online
    /// can grow the answer between the calls.
    pub(super) fn read(relationship: i32) -> Option<Records> {
        let mut buf: Vec<u64> = Vec::new();
        let mut len = 0u32;
        for _ in 0..3 {
            let target = if buf.is_empty() {
                std::ptr::null_mut()
            } else {
                buf.as_mut_ptr().cast::<u8>()
            };
            // SAFETY: `target` is null with `len` 0, which asks for the size,
            // or `buf`, which holds at least `len` writable bytes.
            let ok = unsafe {
                ffi::GetLogicalProcessorInformationEx(relationship, target, &raw mut len)
            };
            if ok != 0 {
                let len = (len as usize).min(buf.len() * 8);
                return Some(Records { buf, len });
            }
            if last_error() as u32 != ffi::ERROR_INSUFFICIENT_BUFFER
                || len == 0
                || len as usize > MAX_RECORDS
            {
                return None;
            }
            buf = vec![0; (len as usize).div_ceil(8)];
        }
        None
    }

    pub(super) fn bytes(&self) -> &[u8] {
        // SAFETY: `buf` is `buf.len() * 8` initialised bytes; a u8 view of it
        // is valid.
        let all = unsafe {
            std::slice::from_raw_parts(self.buf.as_ptr().cast::<u8>(), self.buf.len() * 8)
        };
        &all[..self.len.min(all.len())]
    }
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_ne_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
}

fn usize_at(b: &[u8], at: usize) -> Option<usize> {
    let s = b.get(at..at.checked_add(size_of::<usize>())?)?;
    Some(usize::from_ne_bytes(s.try_into().ok()?))
}

/// Each record's kind and body; the walk stops at the first record that does
/// not fit.
fn records(bytes: &[u8]) -> impl Iterator<Item = (i32, &[u8])> {
    let mut at = 0usize;
    core::iter::from_fn(move || {
        let rest = bytes.get(at..)?;
        let kind = u32_at(rest, lpi::RELATIONSHIP)? as i32;
        let size = u32_at(rest, lpi::SIZE)? as usize;
        if size < lpi::BODY || size > rest.len() {
            return None;
        }
        at += size;
        Some((kind, &rest[lpi::BODY..size]))
    })
}

/// How many records of a kind a buffer holds.
pub(super) fn count_records(bytes: &[u8], relationship: i32) -> usize {
    records(bytes)
        .filter(|&(kind, _)| kind == relationship)
        .count()
}

/// Each processor group: the index of its first CPU and its active mask.
struct Groups(Vec<(u32, usize)>);

impl Groups {
    fn parse(bytes: &[u8]) -> Groups {
        let mut groups = Vec::new();
        let Some((_, body)) = records(bytes).find(|&(k, _)| k == lpi::RELATION_GROUP) else {
            return Groups(groups);
        };
        let active = u16_at(body, lpi::GROUP_ACTIVE_COUNT).unwrap_or(0);
        let mut base = 0u32;
        for i in 0..usize::from(active) {
            let entry = lpi::GROUP_INFO + i * lpi::GROUP_INFO_ENTRY;
            let Some(mask) = usize_at(body, entry + lpi::GROUP_INFO_MASK) else {
                break;
            };
            groups.push((base, mask));
            base += mask.count_ones();
        }
        Groups(groups)
    }

    /// The logical CPU indexes of a group affinity: a bit's rank among the
    /// group's active bits, after every earlier group's CPUs.
    fn cpus(&self, group: u16, mask: usize, out: &mut Vec<u32>) {
        let Some(&(base, active)) = self.0.get(usize::from(group)) else {
            return;
        };
        for bit in (0..usize::BITS).filter(|&b| (mask & active) >> b & 1 == 1) {
            let below = active & ((1usize << bit) - 1);
            out.push(base + below.count_ones());
        }
    }

    /// The CPUs of the `count` group affinities at `masks_at`, sorted. Windows
    /// before the multi-group forms of the cache and NUMA records leaves the
    /// count 0 and one mask in place, so 0 reads as 1.
    fn list(&self, body: &[u8], count: usize, masks_at: usize) -> Vec<u32> {
        let mut out = Vec::new();
        for i in 0..count.max(1) {
            let entry = masks_at + i * lpi::AFFINITY_ENTRY;
            let (Some(mask), Some(group)) = (
                usize_at(body, entry + lpi::AFFINITY_MASK),
                u16_at(body, entry + lpi::AFFINITY_GROUP),
            ) else {
                break;
            };
            self.cpus(group, mask, &mut out);
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    fn of_processor(&self, body: &[u8]) -> Vec<u32> {
        let count = u16_at(body, lpi::PROCESSOR_GROUP_COUNT).unwrap_or(0);
        self.list(body, usize::from(count), lpi::PROCESSOR_GROUP_MASK)
    }
}

fn cache_of(groups: &Groups, body: &[u8]) -> Option<Cache> {
    let kind = match u32_at(body, lpi::CACHE_TYPE)? as i32 {
        lpi::CACHE_UNIFIED => CacheKind::Unified,
        lpi::CACHE_DATA => CacheKind::Data,
        // A trace cache holds decoded instructions.
        lpi::CACHE_INSTRUCTION | lpi::CACHE_TRACE => CacheKind::Instruction,
        _ => return None,
    };
    let count = u16_at(body, lpi::CACHE_GROUP_COUNT)?;
    let shared_by = groups.list(body, usize::from(count), lpi::CACHE_GROUP_MASK);
    (!shared_by.is_empty()).then_some(Cache {
        level: *body.get(lpi::CACHE_LEVEL)?,
        kind,
        size: Bytes::new(u64::from(u32_at(body, lpi::CACHE_SIZE)?)),
        line: u32::from(u16_at(body, lpi::CACHE_LINE_SIZE)?),
        shared_by,
    })
}

/// Adds a node's CPUs. A node spanning groups can come as several records.
fn add_numa(groups: &Groups, body: &[u8], nodes: &mut Vec<NumaNode>) {
    let (Some(id), Some(count)) = (
        u32_at(body, lpi::NUMA_NODE_NUMBER),
        u16_at(body, lpi::NUMA_GROUP_COUNT),
    ) else {
        return;
    };
    let cpus = groups.list(body, usize::from(count), lpi::NUMA_GROUP_MASK);
    if let Some(node) = nodes.iter_mut().find(|n| n.id == id) {
        node.cpus.extend(cpus);
        node.cpus.sort_unstable();
        node.cpus.dedup();
    } else if !cpus.is_empty() {
        nodes.push(NumaNode {
            id,
            cpus,
            memory: None,
        });
    }
}

/// Whether any core runs more than one logical processor (`LTP_PC_SMT`);
/// `None` when the buffer has no core records.
pub(super) fn smt_of(bytes: &[u8]) -> Option<bool> {
    let mut cores = records(bytes)
        .filter(|&(kind, _)| kind == lpi::RELATION_PROCESSOR_CORE)
        .peekable();
    cores.peek()?;
    Some(cores.any(|(_, body)| {
        body.get(lpi::PROCESSOR_FLAGS)
            .is_some_and(|flags| flags & lpi::LTP_PC_SMT != 0)
    }))
}

/// A higher efficiency class is the faster core. All alike is not hybrid,
/// whatever the class; the classes below the top one are the efficient cores.
fn core_kind(class: u8, lowest: u8, highest: u8) -> CoreKind {
    if lowest == highest {
        CoreKind::Uniform
    } else if class == highest {
        CoreKind::Performance
    } else {
        CoreKind::Efficiency
    }
}

/// Windows does not number packages or cores: both are numbered here, in the
/// order of their first logical CPU, which is stable for one boot.
fn topology_of(bytes: &[u8]) -> TopologyFacts {
    let groups = Groups::parse(bytes);
    let mut packages: Vec<Vec<u32>> = Vec::new();
    let mut cores: Vec<(Vec<u32>, u8)> = Vec::new();
    let mut caches = Vec::new();
    let mut numa: Vec<NumaNode> = Vec::new();
    for (kind, body) in records(bytes) {
        match kind {
            lpi::RELATION_PROCESSOR_PACKAGE => packages.push(groups.of_processor(body)),
            lpi::RELATION_PROCESSOR_CORE => {
                let class = body
                    .get(lpi::PROCESSOR_EFFICIENCY_CLASS)
                    .copied()
                    .unwrap_or(0);
                cores.push((groups.of_processor(body), class));
            }
            lpi::RELATION_CACHE => caches.extend(cache_of(&groups, body)),
            lpi::RELATION_NUMA_NODE | lpi::RELATION_NUMA_NODE_EX => {
                add_numa(&groups, body, &mut numa);
            }
            _ => {}
        }
    }
    packages.retain(|p| !p.is_empty());
    packages.sort();
    cores.retain(|(cpus, _)| !cpus.is_empty());
    cores.sort();
    let lowest = cores.iter().map(|&(_, class)| class).min().unwrap_or(0);
    let highest = cores.iter().map(|&(_, class)| class).max().unwrap_or(0);

    let mut topology = Topology {
        packages: (0u32..)
            .zip(&packages)
            .map(|(id, _)| Package {
                id,
                cores: Vec::new(),
            })
            .collect(),
        cores: Vec::with_capacity(cores.len()),
    };
    for (id, (logical, class)) in (0u32..).zip(cores) {
        // A core outside every package cannot be placed; the OS never says so.
        let Some(package) = packages
            .iter()
            .position(|p| p.binary_search(&logical[0]).is_ok())
        else {
            continue;
        };
        topology.packages[package].cores.push(id);
        topology.cores.push(Core {
            id,
            package: package as u32,
            kind: core_kind(class, lowest, highest),
            capacity: None,
            numa_node: numa
                .iter()
                .find(|n| n.cpus.binary_search(&logical[0]).is_ok())
                .map(|n| n.id),
            logical,
        });
    }
    TopologyFacts {
        topology,
        caches,
        numa,
    }
}

/// The layout. A node's memory is known only on a machine with one node, where
/// it is all of it: `GetNumaAvailableMemoryNodeEx` gives the memory free now,
/// not the memory attached, and no documented call gives that per node.
pub(crate) fn cpu_topology() -> TopologyFacts {
    let Some(records) = Records::read(lpi::RELATION_ALL) else {
        return TopologyFacts::default();
    };
    let mut facts = topology_of(records.bytes());
    if let [only] = &mut facts.numa[..] {
        only.memory = memory_status().ok().map(|m| Bytes::new(m.total_phys));
    }
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: i32, body: &[u8]) -> Vec<u8> {
        let size = (lpi::BODY + body.len()).next_multiple_of(8) as u32;
        let mut v = Vec::new();
        v.extend_from_slice(&kind.to_ne_bytes());
        v.extend_from_slice(&size.to_ne_bytes());
        v.extend_from_slice(body);
        v.resize(size as usize, 0);
        v
    }

    fn affinity(group: u16, mask: usize) -> Vec<u8> {
        let mut v = vec![0u8; lpi::AFFINITY_ENTRY];
        v[lpi::AFFINITY_MASK..lpi::AFFINITY_MASK + size_of::<usize>()]
            .copy_from_slice(&mask.to_ne_bytes());
        v[lpi::AFFINITY_GROUP..lpi::AFFINITY_GROUP + 2].copy_from_slice(&group.to_ne_bytes());
        v
    }

    fn groups(masks: &[usize]) -> Vec<u8> {
        let mut body = vec![0u8; lpi::GROUP_INFO];
        let count = masks.len() as u16;
        body[lpi::GROUP_ACTIVE_COUNT..lpi::GROUP_ACTIVE_COUNT + 2]
            .copy_from_slice(&count.to_ne_bytes());
        for mask in masks {
            let mut entry = vec![0u8; lpi::GROUP_INFO_ENTRY];
            entry[lpi::GROUP_INFO_MASK..lpi::GROUP_INFO_MASK + size_of::<usize>()]
                .copy_from_slice(&mask.to_ne_bytes());
            body.extend(entry);
        }
        record(lpi::RELATION_GROUP, &body)
    }

    fn processor(kind: i32, class: u8, masks: &[(u16, usize)]) -> Vec<u8> {
        let mut body = vec![0u8; lpi::PROCESSOR_GROUP_MASK];
        body[lpi::PROCESSOR_EFFICIENCY_CLASS] = class;
        let count = masks.len() as u16;
        body[lpi::PROCESSOR_GROUP_COUNT..lpi::PROCESSOR_GROUP_COUNT + 2]
            .copy_from_slice(&count.to_ne_bytes());
        for &(group, mask) in masks {
            body.extend(affinity(group, mask));
        }
        record(kind, &body)
    }

    fn cache(level: u8, kind: i32, count: u16, masks: &[(u16, usize)]) -> Vec<u8> {
        let mut body = vec![0u8; lpi::CACHE_GROUP_MASK];
        body[lpi::CACHE_LEVEL] = level;
        body[lpi::CACHE_LINE_SIZE..lpi::CACHE_LINE_SIZE + 2].copy_from_slice(&64u16.to_ne_bytes());
        body[lpi::CACHE_SIZE..lpi::CACHE_SIZE + 4].copy_from_slice(&32_768u32.to_ne_bytes());
        body[lpi::CACHE_TYPE..lpi::CACHE_TYPE + 4].copy_from_slice(&kind.to_ne_bytes());
        body[lpi::CACHE_GROUP_COUNT..lpi::CACHE_GROUP_COUNT + 2]
            .copy_from_slice(&count.to_ne_bytes());
        for &(group, mask) in masks {
            body.extend(affinity(group, mask));
        }
        record(lpi::RELATION_CACHE, &body)
    }

    fn numa(kind: i32, id: u32, count: u16, masks: &[(u16, usize)]) -> Vec<u8> {
        let mut body = vec![0u8; lpi::NUMA_GROUP_MASK];
        body[lpi::NUMA_NODE_NUMBER..lpi::NUMA_NODE_NUMBER + 4].copy_from_slice(&id.to_ne_bytes());
        body[lpi::NUMA_GROUP_COUNT..lpi::NUMA_GROUP_COUNT + 2]
            .copy_from_slice(&count.to_ne_bytes());
        for &(group, mask) in masks {
            body.extend(affinity(group, mask));
        }
        record(kind, &body)
    }

    #[test]
    fn test_topology_windows_hybrid_machine() {
        // Two P cores with SMT, two E cores, one package, one node.
        let mut b = groups(&[0b11_1111]);
        b.extend(processor(
            lpi::RELATION_PROCESSOR_PACKAGE,
            0,
            &[(0, 0b11_1111)],
        ));
        b.extend(processor(lpi::RELATION_PROCESSOR_CORE, 1, &[(0, 0b0011)]));
        b.extend(processor(lpi::RELATION_PROCESSOR_CORE, 1, &[(0, 0b1100)]));
        b.extend(processor(
            lpi::RELATION_PROCESSOR_CORE,
            0,
            &[(0, 0b01_0000)],
        ));
        b.extend(processor(
            lpi::RELATION_PROCESSOR_CORE,
            0,
            &[(0, 0b10_0000)],
        ));
        b.extend(cache(1, lpi::CACHE_DATA, 1, &[(0, 0b0011)]));
        b.extend(cache(1, lpi::CACHE_INSTRUCTION, 1, &[(0, 0b0011)]));
        b.extend(cache(3, lpi::CACHE_UNIFIED, 1, &[(0, 0b11_1111)]));
        b.extend(numa(lpi::RELATION_NUMA_NODE, 0, 1, &[(0, 0b11_1111)]));
        let f = topology_of(&b);
        assert_eq!(f.topology.packages.len(), 1);
        assert_eq!(f.topology.packages[0].cores, [0, 1, 2, 3]);
        let kinds: Vec<CoreKind> = f.topology.cores.iter().map(|c| c.kind).collect();
        assert_eq!(
            kinds,
            [
                CoreKind::Performance,
                CoreKind::Performance,
                CoreKind::Efficiency,
                CoreKind::Efficiency
            ]
        );
        assert_eq!(f.topology.cores[1].logical, [2, 3]);
        assert_eq!(f.topology.cores[3].logical, [5]);
        assert_eq!(f.topology.cores[0].numa_node, Some(0));
        assert_eq!(f.caches.len(), 3);
        assert_eq!(f.caches[0].kind, CacheKind::Data);
        assert_eq!(f.caches[1].kind, CacheKind::Instruction);
        assert_eq!(f.caches[2].kind, CacheKind::Unified);
        assert_eq!(f.caches[2].shared_by, [0, 1, 2, 3, 4, 5]);
        assert_eq!(
            (f.caches[0].line, f.caches[0].size),
            (64, Bytes::from_kib(32))
        );
        assert_eq!(f.numa[0].cpus.len(), 6);
    }

    #[test]
    fn test_topology_windows_uniform_when_every_class_matches() {
        for class in [0, 1, 7] {
            assert_eq!(core_kind(class, class, class), CoreKind::Uniform);
        }
        assert_eq!(core_kind(2, 0, 2), CoreKind::Performance);
        assert_eq!(core_kind(1, 0, 2), CoreKind::Efficiency);
        assert_eq!(core_kind(0, 0, 2), CoreKind::Efficiency);
    }

    #[test]
    fn test_topology_windows_groups_give_one_dense_index() {
        // Two groups, the first with a hole (bit 1 inactive): indexes stay dense
        // and the second group continues after the first.
        let g = groups(&[0b1101, 0b11]);
        let g = Groups::parse(&g);
        let mut out = Vec::new();
        g.cpus(0, 0b1101, &mut out);
        assert_eq!(out, [0, 1, 2]);
        out.clear();
        g.cpus(1, 0b10, &mut out);
        assert_eq!(out, [4]);
        // An inactive bit and an unknown group give nothing.
        out.clear();
        g.cpus(0, 0b10, &mut out);
        g.cpus(9, 0b1, &mut out);
        assert!(out.is_empty());
        // The top bit of a full mask.
        let g = groups(&[usize::MAX]);
        let g = Groups::parse(&g);
        let mut out = Vec::new();
        g.cpus(0, 1 << (usize::BITS - 1), &mut out);
        assert_eq!(out, [usize::BITS - 1]);
    }

    #[test]
    fn test_topology_windows_package_spanning_groups_and_numa_records_merge() {
        let mut b = groups(&[0b11, 0b11]);
        b.extend(processor(
            lpi::RELATION_PROCESSOR_PACKAGE,
            0,
            &[(0, 0b11), (1, 0b11)],
        ));
        b.extend(processor(lpi::RELATION_PROCESSOR_CORE, 0, &[(1, 0b11)]));
        b.extend(numa(lpi::RELATION_NUMA_NODE, 0, 1, &[(0, 0b11)]));
        b.extend(numa(
            lpi::RELATION_NUMA_NODE_EX,
            0,
            2,
            &[(0, 0b11), (1, 0b11)],
        ));
        let f = topology_of(&b);
        assert_eq!(f.topology.packages.len(), 1);
        assert_eq!(f.topology.cores[0].logical, [2, 3]);
        assert_eq!(f.topology.cores[0].package, 0);
        assert_eq!(f.numa.len(), 1);
        assert_eq!(f.numa[0].cpus, [0, 1, 2, 3]);
    }

    #[test]
    fn test_topology_windows_old_records_have_a_zero_group_count() {
        let mut b = groups(&[0b11]);
        b.extend(cache(2, lpi::CACHE_UNIFIED, 0, &[(0, 0b11)]));
        b.extend(numa(lpi::RELATION_NUMA_NODE, 3, 0, &[(0, 0b01)]));
        let f = topology_of(&b);
        assert_eq!(f.caches[0].shared_by, [0, 1]);
        assert_eq!(f.numa[0].id, 3);
        assert_eq!(f.numa[0].cpus, [0]);
    }

    #[test]
    fn test_topology_windows_hostile_buffers_stop_cleanly() {
        assert_eq!(topology_of(&[]), TopologyFacts::default());
        assert_eq!(topology_of(&[1, 2, 3]), TopologyFacts::default());
        // A size of zero, a size past the end, a size under the header.
        let mut zero = record(lpi::RELATION_PROCESSOR_CORE, &[]);
        zero[lpi::SIZE] = 0;
        assert_eq!(records(&zero).count(), 0);
        let mut long = record(lpi::RELATION_PROCESSOR_CORE, &[]);
        long[lpi::SIZE] = 0xFF;
        assert_eq!(records(&long).count(), 0);
        let mut short = record(lpi::RELATION_PROCESSOR_CORE, &[]);
        short[lpi::SIZE] = 4;
        assert_eq!(records(&short).count(), 0);
        // No group record: CPUs cannot be named, so nothing is claimed.
        let b = processor(lpi::RELATION_PROCESSOR_CORE, 0, &[(0, 0b11)]);
        assert!(topology_of(&b).topology.cores.is_empty());
        // A group count far past the record, and a body cut short.
        let mut b = groups(&[0b11]);
        let mut p = vec![0u8; lpi::PROCESSOR_GROUP_MASK];
        p[lpi::PROCESSOR_GROUP_COUNT..lpi::PROCESSOR_GROUP_COUNT + 2]
            .copy_from_slice(&u16::MAX.to_ne_bytes());
        b.extend(record(lpi::RELATION_PROCESSOR_CORE, &p));
        b.extend(record(lpi::RELATION_CACHE, &[1]));
        b.extend(record(lpi::RELATION_NUMA_NODE, &[1]));
        assert!(topology_of(&b).topology.cores.is_empty());
        // A core outside every package is left out.
        let mut b = groups(&[0b11]);
        b.extend(processor(lpi::RELATION_PROCESSOR_CORE, 0, &[(0, 0b01)]));
        assert!(topology_of(&b).topology.cores.is_empty());
        // An unknown cache type.
        let mut b = groups(&[0b11]);
        b.extend(cache(1, 9, 1, &[(0, 0b11)]));
        assert!(topology_of(&b).caches.is_empty());
    }

    #[test]
    fn test_topology_windows_smt_from_core_flags() {
        let mut one = processor(lpi::RELATION_PROCESSOR_CORE, 0, &[(0, 0b11)]);
        let plain = processor(lpi::RELATION_PROCESSOR_CORE, 0, &[(0, 0b100)]);
        assert_eq!(smt_of(&plain), Some(false));
        let mut both = plain.clone();
        both.extend(&one);
        assert_eq!(smt_of(&both), Some(false), "no flag set yet");
        one[lpi::BODY + lpi::PROCESSOR_FLAGS] = lpi::LTP_PC_SMT;
        let mut both = plain.clone();
        both.extend(&one);
        assert_eq!(smt_of(&both), Some(true));
        let package = processor(lpi::RELATION_PROCESSOR_PACKAGE, 0, &[(0, 0b111)]);
        assert_eq!(smt_of(&package), None, "no core records");
        assert_eq!(smt_of(&[]), None);
    }

    #[test]
    fn test_topology_windows_live_covers_every_logical_cpu() {
        let f = cpu_topology();
        let logical = crate::cpu::logical();
        let mut seen: Vec<u32> = f
            .topology
            .cores
            .iter()
            .flat_map(|c| c.logical.iter().copied())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), logical);
        assert_eq!(seen.last().copied(), Some(logical as u32 - 1));
        assert_eq!(f.topology.cores.len(), crate::cpu::physical());
        assert!(!f.caches.is_empty() && !f.numa.is_empty());
        assert!(f.topology.cores.iter().all(|c| c.numa_node.is_some()));
        assert!(f.caches.iter().all(|c| c.line.is_power_of_two()));
        if f.numa.len() == 1 {
            assert!(f.numa[0].memory.is_some());
        }
    }
}
