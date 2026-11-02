//! Topology on Linux: one small directory per CPU under
//! `/sys/devices/system/cpu`, cache instances under each CPU's `cache/`, and
//! NUMA nodes under `/sys/devices/system/node`. CPU numbers come from the
//! `present` list; everything else is a fixed path from a number, so nothing
//! read from a file becomes part of a path.

use std::collections::BTreeMap;

use super::Root;
use crate::{
    Bytes,
    cpu::{Cache, CacheKind, Core, CoreKind, NumaNode, Package, Topology},
    parse::{cpu_list::CpuSet, parse_u64},
    sys::TopologyFacts,
};

/// The most CPUs walked (T7).
const MAX_CPUS: usize = 16_384;
/// The most NUMA nodes and per-CPU cache levels looked at.
const MAX_NODES: u32 = 1024;
const MAX_CACHES: u32 = 16;

fn number(root: &Root, path: &str) -> Option<u64> {
    parse_u64(root.text(path)?.trim().as_bytes())
}

fn list(root: &Root, path: &str) -> Option<Vec<u32>> {
    let text = root.text(path)?;
    let set = CpuSet::parse(text.trim().as_bytes()).ok()?;
    Some(set.iter().map(|c| c as u32).take(MAX_CPUS).collect())
}

/// A size like `32K`, `1024K`, or `16M`.
pub(crate) fn cache_size(text: &str) -> Option<Bytes> {
    let t = text.trim();
    let (digits, shift) = match t.as_bytes().last()? {
        b'K' => (&t[..t.len() - 1], 10),
        b'M' => (&t[..t.len() - 1], 20),
        b'G' => (&t[..t.len() - 1], 30),
        _ => (t, 0),
    };
    Some(Bytes::new(parse_u64(digits.as_bytes())?.checked_shl(shift)?))
}

/// One CPU's identity.
struct Cpu {
    number: u32,
    package: u32,
    siblings: Vec<u32>,
    capacity: Option<u16>,
}

fn cpu(root: &Root, number: u32) -> Option<Cpu> {
    let dir = format!("/sys/devices/system/cpu/cpu{number}");
    let siblings = list(root, &format!("{dir}/topology/core_cpus_list"))
        .or_else(|| list(root, &format!("{dir}/topology/thread_siblings_list")))?;
    let package = self::number(root, &format!("{dir}/topology/physical_package_id"))
        .and_then(|p| u32::try_from(p).ok())
        .unwrap_or(0);
    let capacity = self::number(root, &format!("{dir}/cpu_capacity"))
        .and_then(|c| u16::try_from(c).ok())
        .filter(|&c| c <= 1024);
    Some(Cpu {
        number,
        package,
        siblings,
        capacity,
    })
}

/// Intel lists its hybrid core PMUs; ARM gives each core a capacity.
fn core_kind(
    root_kinds: &(Option<Vec<u32>>, Option<Vec<u32>>),
    capacities: (u16, u16),
    logical: &[u32],
    capacity: Option<u16>,
) -> CoreKind {
    let in_list = |l: &Option<Vec<u32>>| {
        l.as_ref()
            .is_some_and(|l| logical.iter().any(|c| l.contains(c)))
    };
    if root_kinds.0.is_some() && root_kinds.1.is_some() {
        if in_list(&root_kinds.1) {
            return CoreKind::Efficiency;
        }
        if in_list(&root_kinds.0) {
            return CoreKind::Performance;
        }
        return CoreKind::Unknown;
    }
    match capacity {
        Some(c) if capacities.0 != capacities.1 && c == capacities.1 => CoreKind::Performance,
        Some(_) if capacities.0 != capacities.1 => CoreKind::Efficiency,
        _ => CoreKind::Uniform,
    }
}

fn caches(root: &Root, cpus: &[u32]) -> Vec<Cache> {
    // One entry per instance: an instance is level, kind, and its CPU list.
    let mut seen: BTreeMap<(u8, u8, Vec<u32>), Cache> = BTreeMap::new();
    for &cpu in cpus {
        for index in 0..MAX_CACHES {
            let dir = format!("/sys/devices/system/cpu/cpu{cpu}/cache/index{index}");
            let Some(level) = number(root, &format!("{dir}/level")) else {
                break;
            };
            let Ok(level) = u8::try_from(level) else {
                continue;
            };
            let kind = match root.text(&format!("{dir}/type")).as_deref().map(str::trim) {
                Some("Data") => CacheKind::Data,
                Some("Instruction") => CacheKind::Instruction,
                Some("Unified") => CacheKind::Unified,
                _ => continue,
            };
            let shared_by = list(root, &format!("{dir}/shared_cpu_list")).unwrap_or_default();
            let tag = kind as u8;
            if seen.contains_key(&(level, tag, shared_by.clone())) {
                continue;
            }
            let Some(size) = root
                .text(&format!("{dir}/size"))
                .and_then(|s| cache_size(&s))
            else {
                continue;
            };
            let line = number(root, &format!("{dir}/coherency_line_size"))
                .and_then(|l| u32::try_from(l).ok())
                .unwrap_or(0);
            seen.insert(
                (level, tag, shared_by.clone()),
                Cache {
                    level,
                    kind,
                    size,
                    line,
                    shared_by,
                },
            );
        }
    }
    seen.into_values().collect()
}

/// `Node 0 MemTotal:       65536 kB`, from a node's `meminfo`.
pub(crate) fn node_memory(data: &str) -> Option<Bytes> {
    let line = data.lines().find(|l| l.contains("MemTotal:"))?;
    let kib = line.split_ascii_whitespace().nth(3)?;
    Some(Bytes::from_kib(parse_u64(kib.as_bytes())?))
}

fn numa(root: &Root) -> Vec<NumaNode> {
    let mut out = Vec::new();
    for id in 0..MAX_NODES {
        let dir = format!("/sys/devices/system/node/node{id}");
        let Some(cpus) = list(root, &format!("{dir}/cpulist")) else {
            continue;
        };
        let memory = root
            .text(&format!("{dir}/meminfo"))
            .and_then(|d| node_memory(&d));
        out.push(NumaNode { id, cpus, memory });
    }
    out
}

/// Reads the whole layout. Everything is empty where `/sys` is hidden.
pub(crate) fn read(root: &Root) -> TopologyFacts {
    let present: Vec<u32> = list(root, "/sys/devices/system/cpu/present").unwrap_or_default();
    let cpus: Vec<Cpu> = present.iter().filter_map(|&n| cpu(root, n)).collect();
    let nodes = numa(root);
    let kinds = (
        list(root, "/sys/devices/cpu_core/cpus"),
        list(root, "/sys/devices/cpu_atom/cpus"),
    );
    let lowest = cpus.iter().filter_map(|c| c.capacity).min().unwrap_or(0);
    let highest = cpus.iter().filter_map(|c| c.capacity).max().unwrap_or(0);

    let mut cores: Vec<Core> = Vec::new();
    for c in &cpus {
        // The lowest CPU in the sibling list stands for the core.
        let id = c.siblings.iter().copied().min().unwrap_or(c.number);
        if id != c.number {
            continue;
        }
        cores.push(Core {
            id,
            package: c.package,
            kind: core_kind(&kinds, (lowest, highest), &c.siblings, c.capacity),
            capacity: c.capacity,
            numa_node: nodes
                .iter()
                .find(|n| n.cpus.contains(&c.number))
                .map(|n| n.id),
            logical: c.siblings.clone(),
        });
    }
    let mut packages: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for c in &cores {
        packages.entry(c.package).or_default().push(c.id);
    }
    let packages = packages
        .into_iter()
        .map(|(id, cores)| Package { id, cores })
        .collect();
    TopologyFacts {
        topology: Topology { packages, cores },
        caches: caches(root, &present),
        numa: nodes,
    }
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};
    use crate::linux::hostile::Tree;

    #[test]
    fn test_topology_wsl2_fixture() {
        let root = fixtures::root("wsl2");
        let f = read(&root);
        let logical: usize = f.topology.cores.iter().map(|c| c.logical.len()).sum();
        assert_eq!(logical, 32, "the fixture has 32 CPUs");
        assert_eq!(f.topology.packages.len(), 1);
        assert!(f.topology.cores.iter().all(|c| c.kind == CoreKind::Uniform));
        assert!(f.caches.iter().any(|c| c.level == 3 && c.kind == CacheKind::Unified));
        assert!(f.caches.iter().any(|c| c.level == 1 && c.line == 64));
    }

    #[test]
    fn test_topology_cache_sizes() {
        assert_eq!(cache_size("32K\n"), Some(Bytes::new(32 << 10)));
        assert_eq!(cache_size("16M"), Some(Bytes::new(16 << 20)));
        assert_eq!(cache_size("512"), Some(Bytes::new(512)));
        assert_eq!(cache_size(""), None);
        assert_eq!(cache_size("xK"), None);
    }

    #[test]
    fn test_topology_node_memory() {
        assert_eq!(
            node_memory("Node 0 MemTotal:       65536 kB\nNode 0 MemFree: 1 kB\n"),
            Some(Bytes::new(65536 << 10))
        );
        assert_eq!(node_memory("nothing"), None);
    }

    fn cpu_files(t: &Tree, n: u32, siblings: &str, capacity: &str) {
        let dir = format!("/sys/devices/system/cpu/cpu{n}");
        t.put(&format!("{dir}/topology/core_cpus_list"), siblings.as_bytes());
        t.put(&format!("{dir}/topology/physical_package_id"), b"0\n");
        t.put(&format!("{dir}/cpu_capacity"), capacity.as_bytes());
    }

    #[test]
    fn test_topology_arm_capacities_are_core_kinds() {
        let t = Tree::new("arm");
        t.put("/sys/devices/system/cpu/present", b"0-1\n");
        cpu_files(&t, 0, "0\n", "512\n");
        cpu_files(&t, 1, "1\n", "1024\n");
        let f = read(&t.root());
        assert_eq!(f.topology.cores[0].kind, CoreKind::Efficiency);
        assert_eq!(f.topology.cores[1].kind, CoreKind::Performance);
        assert_eq!(f.topology.cores[1].capacity, Some(1024));
    }

    #[test]
    fn test_topology_intel_hybrid_lists() {
        let t = Tree::new("intel");
        t.put("/sys/devices/system/cpu/present", b"0-2\n");
        cpu_files(&t, 0, "0-1\n", "1024\n");
        cpu_files(&t, 1, "0-1\n", "1024\n");
        cpu_files(&t, 2, "2\n", "1024\n");
        t.put("/sys/devices/cpu_core/cpus", b"0-1\n");
        t.put("/sys/devices/cpu_atom/cpus", b"2\n");
        let f = read(&t.root());
        assert_eq!(f.topology.cores.len(), 2);
        assert_eq!(f.topology.cores[0].kind, CoreKind::Performance);
        assert_eq!(f.topology.cores[1].kind, CoreKind::Efficiency);
        assert_eq!(f.topology.packages[0].cores, vec![0, 2]);
    }

    #[test]
    fn test_topology_numa_and_hidden_sys() {
        let t = Tree::new("numa");
        t.put("/sys/devices/system/cpu/present", b"0-1\n");
        cpu_files(&t, 0, "0\n", "x\n");
        cpu_files(&t, 1, "1\n", "x\n");
        t.put("/sys/devices/system/node/node0/cpulist", b"0\n");
        t.put("/sys/devices/system/node/node1/cpulist", b"1\n");
        t.put(
            "/sys/devices/system/node/node1/meminfo",
            b"Node 1 MemTotal: 1024 kB\n",
        );
        let f = read(&t.root());
        assert_eq!(f.numa.len(), 2);
        assert_eq!(f.numa[1].memory, Some(Bytes::new(1 << 20)));
        assert_eq!(f.topology.cores[1].numa_node, Some(1));
        let empty = Tree::new("empty");
        assert_eq!(read(&empty.root()), TopologyFacts::default());
    }
}
