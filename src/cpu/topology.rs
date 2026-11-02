//! Topology, caches, and NUMA nodes: static facts, read on first use. The first
//! call reads up to a few hundred small files on a large Linux host, which is
//! why these are apart from `cpu::name()`. CPU hot-plug after the first call is
//! not reflected; `cpu::online()` is the live count.

use std::sync::OnceLock;

use crate::{Bytes, sys::TopologyFacts};

/// Packages (sockets) and cores, sorted by id.
///
/// ```
/// use sys_lib as sys;
///
/// let t = sys::cpu::topology();
/// let fast = t.cores.iter().filter(|c| c.kind == sys::cpu::CoreKind::Performance).count();
/// println!("{} cores, {fast} performance cores, hybrid: {}", t.cores.len(), sys::cpu::is_hybrid());
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Topology {
    /// The packages.
    pub packages: Vec<Package>,
    /// The cores.
    pub cores: Vec<Core>,
}

/// A processor package (socket).
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Package {
    /// Its id.
    pub id: u32,
    /// The ids of its cores.
    pub cores: Vec<u32>,
}

/// A physical core.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Core {
    /// Its id, unique across packages.
    pub id: u32,
    /// The package it is in.
    pub package: u32,
    /// Its logical CPU indexes.
    pub logical: Vec<u32>,
    /// Its kind.
    pub kind: CoreKind,
    /// Linux `cpu_capacity`, 0 to 1024.
    pub capacity: Option<u16>,
    /// The NUMA node it is in.
    pub numa_node: Option<u32>,
}

/// What kind of core this is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum CoreKind {
    /// A big core on a hybrid processor.
    Performance,
    /// A little core on a hybrid processor.
    Efficiency,
    /// Every core on this processor is alike.
    Uniform,
    /// The OS does not say.
    Unknown,
}

/// A CPU cache.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Cache {
    /// 1 for L1, and so on.
    pub level: u8,
    /// What it holds.
    pub kind: CacheKind,
    /// Its size.
    pub size: Bytes,
    /// Its line size in bytes.
    pub line: u32,
    /// The logical CPUs sharing this cache instance.
    pub shared_by: Vec<u32>,
}

/// What a cache holds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum CacheKind {
    /// Data.
    Data,
    /// Instructions.
    Instruction,
    /// Both.
    Unified,
}

/// A NUMA node.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct NumaNode {
    /// Its id.
    pub id: u32,
    /// The logical CPUs in it.
    pub cpus: Vec<u32>,
    /// The memory attached to it.
    pub memory: Option<Bytes>,
}

fn facts() -> &'static TopologyFacts {
    static FACTS: OnceLock<TopologyFacts> = OnceLock::new();
    FACTS.get_or_init(|| {
        let mut f = crate::sys::cpu_topology();
        f.topology.packages.sort_by_key(|p| p.id);
        f.topology.cores.sort_by_key(|c| (c.package, c.id));
        f.caches
            .sort_by_key(|c| (c.level, c.shared_by.first().copied()));
        f.numa.sort_by_key(|n| n.id);
        f
    })
}

/// Packages and cores. Empty where the OS hides them.
#[must_use]
pub fn topology() -> &'static Topology {
    &facts().topology
}

/// The caches, one entry per instance, sorted by level.
#[must_use]
pub fn caches() -> &'static [Cache] {
    &facts().caches
}

/// The cache line size in bytes, from the first level 1 data cache; 64 when
/// unknown, 128 on Apple silicon.
///
/// ```
/// assert!(sys_lib::cpu::cache_line().is_power_of_two());
/// ```
#[must_use]
pub fn cache_line() -> usize {
    let line = caches()
        .iter()
        .find(|c| c.level == 1 && c.kind != CacheKind::Instruction)
        .map(|c| c.line as usize)
        .filter(|l| l.is_power_of_two() && (16..=1024).contains(l));
    line.unwrap_or(if cfg!(all(target_vendor = "apple", target_arch = "aarch64")) {
        128
    } else {
        64
    })
}

/// The NUMA nodes, sorted by id. macOS reports one node with every CPU.
#[must_use]
pub fn numa_nodes() -> &'static [NumaNode] {
    &facts().numa
}

/// Whether the processor has more than one kind of core.
#[must_use]
pub fn is_hybrid() -> bool {
    let cores = &topology().cores;
    cores.iter().any(|c| c.kind == CoreKind::Performance)
        && cores.iter().any(|c| c.kind == CoreKind::Efficiency)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cpu_topology_live_is_consistent() {
        assert!(cache_line().is_power_of_two());
        let t = topology();
        assert!(std::ptr::eq(t, topology()));
        for c in &t.cores {
            assert!(!c.logical.is_empty());
            assert!(t.packages.iter().any(|p| p.id == c.package));
        }
        for pair in t.packages.windows(2) {
            assert!(pair[0].id < pair[1].id);
        }
        for pair in numa_nodes().windows(2) {
            assert!(pair[0].id < pair[1].id);
        }
    }
}
