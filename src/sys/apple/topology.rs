//! CPU topology and caches: `hw.nperflevels` and the `hw.perflevelN` sysctls,
//! with the global `hw.*` sizes where a machine has one level and lacks them.
//!
//! macOS has no call mapping a logical CPU to a core or a cache, and no affinity
//! to use such a map with, so the numbers here are positions: efficiency cores
//! first, as the kernel orders them on Apple silicon, each core's threads
//! together. The counts and the sharing are the kernel's.

use std::ffi::CString;

use super::sysctl_u64;
use crate::{
    Bytes,
    cpu::{Cache, CacheKind, Core, CoreKind, NumaNode, Package, Topology},
    sys::TopologyFacts,
};

/// The most performance levels read: the kernel defines two.
const MAX_LEVELS: u64 = 8;
/// The most cores believed at one level.
const MAX_CORES: u32 = 4_096;
/// The most hardware threads believed per core.
const MAX_THREADS: u32 = 8;

/// One kind of core: the counts and the cache sizes of one performance level. A
/// size of 0 is a cache the machine does not report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Level {
    physical: u32,
    logical: u32,
    kind: CoreKind,
    l1i: u64,
    l1d: u64,
    l2: u64,
    /// 0 when unreported: one per core.
    cpus_per_l2: u32,
    l3: u64,
    /// 0 when unreported: one for the level.
    cpus_per_l3: u32,
}

/// `hw.perflevel<n>.<leaf>`.
fn leaf(level: u64, name: &str) -> Option<u64> {
    let name = CString::new(format!("hw.perflevel{level}.{name}")).ok()?;
    sysctl_u64(&name)
}

/// A count that fits, and is not zero.
fn count(value: Option<u64>) -> Option<u32> {
    u32::try_from(value?).ok().filter(|&n| n > 0)
}

/// Level `n` of `levels`. Level 0 is the highest performance (`kern_mib.c`).
/// With one level the global sysctls describe the same cores, so they fill in
/// what the level does not say; with more they describe only one of them.
fn read_level(n: u64, levels: u64) -> Option<Level> {
    let single = levels == 1;
    let either = |name: &str, global: &core::ffi::CStr| {
        leaf(n, name).or_else(|| sysctl_u64(global).filter(|_| single))
    };
    let physical = count(leaf(n, "physicalcpu_max").or_else(|| leaf(n, "physicalcpu")))
        .or_else(|| count(sysctl_u64(c"hw.physicalcpu_max").filter(|_| single)))?;
    let logical = count(leaf(n, "logicalcpu_max").or_else(|| leaf(n, "logicalcpu")))
        .or_else(|| count(sysctl_u64(c"hw.logicalcpu_max").filter(|_| single)))
        .unwrap_or(physical);
    let kind = match (single, n) {
        (true, _) => CoreKind::Uniform,
        (false, 0) => CoreKind::Performance,
        (false, _) => CoreKind::Efficiency,
    };
    Some(Level {
        physical,
        logical,
        kind,
        l1i: either("l1icachesize", c"hw.l1icachesize").unwrap_or(0),
        l1d: either("l1dcachesize", c"hw.l1dcachesize").unwrap_or(0),
        l2: either("l2cachesize", c"hw.l2cachesize").unwrap_or(0),
        cpus_per_l2: count(leaf(n, "cpusperl2")).unwrap_or(0),
        l3: either("l3cachesize", c"hw.l3cachesize").unwrap_or(0),
        cpus_per_l3: count(leaf(n, "cpusperl3")).unwrap_or(0),
    })
}

fn cache(level: u8, kind: CacheKind, size: u64, line: u32, shared_by: &[u32]) -> Cache {
    Cache {
        level,
        kind,
        size: Bytes::new(size),
        line,
        shared_by: shared_by.to_vec(),
    }
}

/// The topology from the levels, lowest performance first in the numbering.
/// `line` is the machine's one cache-line size, `packages` at least 1, and
/// `memory` the host's: one NUMA node holds everything.
fn build(levels: &[Level], packages: u32, line: u32, memory: Option<u64>) -> TopologyFacts {
    let mut caches = Vec::new();
    let mut cores: Vec<Core> = Vec::new();
    let mut next_cpu = 0u32;
    for level in levels.iter().rev() {
        let physical = level.physical.min(MAX_CORES);
        let threads = (level.logical / level.physical.max(1)).clamp(1, MAX_THREADS);
        let first = next_cpu;
        for _ in 0..physical {
            let logical: Vec<u32> = (next_cpu..next_cpu + threads).collect();
            next_cpu += threads;
            if level.l1i > 0 {
                caches.push(cache(1, CacheKind::Instruction, level.l1i, line, &logical));
            }
            if level.l1d > 0 {
                caches.push(cache(1, CacheKind::Data, level.l1d, line, &logical));
            }
            cores.push(Core {
                id: cores.len() as u32,
                package: 0,
                logical,
                kind: level.kind,
                capacity: None,
                numa_node: Some(0),
            });
        }
        let cpus: Vec<u32> = (first..next_cpu).collect();
        let per_l2 = if level.cpus_per_l2 > 0 {
            level.cpus_per_l2
        } else {
            threads
        };
        if level.l2 > 0 {
            for shared in cpus.chunks(per_l2 as usize) {
                caches.push(cache(2, CacheKind::Unified, level.l2, line, shared));
            }
        }
        if level.l3 > 0 {
            let per_l3 = match level.cpus_per_l3 {
                0 => cpus.len().max(1),
                n => n as usize,
            };
            for shared in cpus.chunks(per_l3) {
                caches.push(cache(3, CacheKind::Unified, level.l3, line, shared));
            }
        }
    }
    let per_package = cores.len().div_ceil(packages.max(1) as usize).max(1);
    let mut package_list = Vec::new();
    for (id, members) in cores.chunks_mut(per_package).enumerate() {
        let id = id as u32;
        for core in members.iter_mut() {
            core.package = id;
        }
        package_list.push(Package {
            id,
            cores: members.iter().map(|c| c.id).collect(),
        });
    }
    TopologyFacts {
        topology: Topology {
            packages: package_list,
            cores,
        },
        caches,
        numa: vec![NumaNode {
            id: 0,
            cpus: (0..next_cpu).collect(),
            memory: memory.map(Bytes::new),
        }],
    }
}

pub(crate) fn cpu_topology() -> TopologyFacts {
    let levels = sysctl_u64(c"hw.nperflevels")
        .unwrap_or(1)
        .clamp(1, MAX_LEVELS);
    let list: Vec<Level> = (0..levels).filter_map(|n| read_level(n, levels)).collect();
    let default_line = if cfg!(target_arch = "aarch64") {
        128
    } else {
        64
    };
    build(
        &list,
        count(sysctl_u64(c"hw.packages")).unwrap_or(1),
        count(sysctl_u64(c"hw.cachelinesize")).unwrap_or(default_line),
        sysctl_u64(c"hw.memsize"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;

    /// An M1: four performance and four efficiency cores, one L2 for each
    /// cluster, as `sysctl hw.perflevel0` and `hw.perflevel1` show.
    fn m1() -> [Level; 2] {
        [
            Level {
                physical: 4,
                logical: 4,
                kind: CoreKind::Performance,
                l1i: 192 * KIB,
                l1d: 128 * KIB,
                l2: 12 * MIB,
                cpus_per_l2: 4,
                l3: 0,
                cpus_per_l3: 0,
            },
            Level {
                physical: 4,
                logical: 4,
                kind: CoreKind::Efficiency,
                l1i: 128 * KIB,
                l1d: 64 * KIB,
                l2: 4 * MIB,
                cpus_per_l2: 4,
                l3: 0,
                cpus_per_l3: 0,
            },
        ]
    }

    /// A six-core Intel Mac with hyper-threading.
    fn intel() -> Level {
        Level {
            physical: 6,
            logical: 12,
            kind: CoreKind::Uniform,
            l1i: 32 * KIB,
            l1d: 32 * KIB,
            l2: 256 * KIB,
            cpus_per_l2: 0,
            l3: 12 * MIB,
            cpus_per_l3: 0,
        }
    }

    #[test]
    fn test_topology_apple_silicon_levels() {
        let f = build(&m1(), 1, 128, Some(16 << 30));
        let cores = &f.topology.cores;
        assert_eq!(cores.len(), 8);
        let kinds = |k| cores.iter().filter(|c| c.kind == k).count();
        assert_eq!(kinds(CoreKind::Performance), 4);
        assert_eq!(kinds(CoreKind::Efficiency), 4);
        // Efficiency cores take the low numbers.
        assert_eq!(cores[0].kind, CoreKind::Efficiency);
        assert_eq!(cores[0].logical, [0]);
        assert_eq!(cores[7].kind, CoreKind::Performance);
        assert_eq!(cores[7].logical, [7]);
        assert_eq!(f.topology.packages.len(), 1);
        assert_eq!(f.topology.packages[0].cores, (0..8).collect::<Vec<_>>());

        // 8 L1i, 8 L1d, and one L2 for each cluster.
        assert_eq!(f.caches.len(), 18);
        let l2: Vec<_> = f.caches.iter().filter(|c| c.level == 2).collect();
        assert_eq!(l2.len(), 2);
        assert_eq!(l2[0].size.raw(), 4 * MIB);
        assert_eq!(l2[0].shared_by, [0, 1, 2, 3]);
        assert_eq!(l2[1].size.raw(), 12 * MIB);
        assert_eq!(l2[1].shared_by, [4, 5, 6, 7]);
        assert!(f.caches.iter().all(|c| c.line == 128));

        assert_eq!(f.numa.len(), 1);
        assert_eq!(f.numa[0].cpus, (0..8).collect::<Vec<_>>());
        assert_eq!(f.numa[0].memory, Some(Bytes::new(16 << 30)));
    }

    #[test]
    fn test_topology_intel_threads_and_shared_l3() {
        let f = build(&[intel()], 1, 64, None);
        let cores = &f.topology.cores;
        assert_eq!(cores.len(), 6);
        assert!(cores.iter().all(|c| c.kind == CoreKind::Uniform));
        assert_eq!(cores[1].logical, [2, 3]);
        let l1d = f.caches.iter().find(|c| c.kind == CacheKind::Data);
        assert_eq!(l1d.unwrap().shared_by, [0, 1]);
        // Unreported sharing: L2 per core, L3 across the level.
        let l2: Vec<_> = f.caches.iter().filter(|c| c.level == 2).collect();
        assert_eq!(l2.len(), 6);
        assert_eq!(l2[5].shared_by, [10, 11]);
        let l3: Vec<_> = f.caches.iter().filter(|c| c.level == 3).collect();
        assert_eq!(l3.len(), 1);
        assert_eq!(l3[0].shared_by.len(), 12);
        assert_eq!(f.numa[0].memory, None);
    }

    #[test]
    fn test_topology_partial_cluster_and_packages() {
        // A cluster with cores disabled: the last L2 has fewer sharers.
        let mut six = m1()[0];
        six.physical = 6;
        six.logical = 6;
        let f = build(&[six], 2, 128, None);
        let l2: Vec<_> = f.caches.iter().filter(|c| c.level == 2).collect();
        assert_eq!(l2.len(), 2);
        assert_eq!(l2[1].shared_by, [4, 5]);
        assert_eq!(f.topology.packages.len(), 2);
        assert_eq!(f.topology.packages[1].cores, [3, 4, 5]);
        assert_eq!(f.topology.cores[5].package, 1);
    }

    #[test]
    fn test_topology_nothing_reported_stays_empty() {
        let f = build(&[], 1, 64, Some(1));
        assert!(f.topology.cores.is_empty() && f.caches.is_empty());
        assert!(f.topology.packages.is_empty());
        assert!(f.numa[0].cpus.is_empty());
        let mut bare = intel();
        (bare.l1i, bare.l1d, bare.l2, bare.l3) = (0, 0, 0, 0);
        assert!(build(&[bare], 1, 64, None).caches.is_empty());
    }

    #[test]
    fn test_topology_counts_are_bounded() {
        assert_eq!(count(Some(0)), None);
        assert_eq!(count(None), None);
        assert_eq!(count(Some(u64::MAX)), None);
        assert_eq!(count(Some(8)), Some(8));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_topology_apple_live_matches_the_counts() {
        let f = cpu_topology();
        let physical = sysctl_u64(c"hw.physicalcpu_max").unwrap();
        assert_eq!(f.topology.cores.len() as u64, physical);
        let logical: usize = f.topology.cores.iter().map(|c| c.logical.len()).sum();
        assert_eq!(logical as u64, sysctl_u64(c"hw.logicalcpu_max").unwrap());
        assert_eq!(f.numa[0].cpus.len(), logical);
    }
}
