//! Memory on Linux: `/proc/meminfo` for the host, the cgroup's files for the
//! limit and the usage counted against it (E-1 to E-11), `/proc/vmstat` for
//! paging, and the huge-page files.

use core::ops::ControlFlow;

use super::{
    Root,
    cgroup::{Cgroups, Controller},
};
use crate::{
    Bytes, Error, Result,
    mem::{
        EventScope, HugePages, LimitSource, MemRaw, MemWant, MemoryEvents, MemoryLimit,
        MemorySource, PagingRaw, TransparentHugePages,
    },
    parse::procfs::{self, Meminfo, space_value as space_kv_value},
};

/// cgroup v1 writes "no limit" as the largest page-aligned `i64`
/// (`9223372036854771712` on 4 KiB pages, E-3); anything this large is none.
const V1_UNLIMITED: u64 = 1 << 62;

/// The cgroup memory files of this process, as paths, built once so a reading
/// formats nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MemPaths {
    v2: bool,
    /// v2: `memory.max` of the cgroup and every ancestor.
    max: Vec<String>,
    /// v2: `memory.swap.max` of the cgroup and every ancestor.
    swap_max: Vec<String>,
    /// v2 `memory.high`, v1 `memory.soft_limit_in_bytes`.
    soft: String,
    /// v2 `memory.current`, v1 `memory.usage_in_bytes`.
    current: String,
    /// `memory.stat` (v1: also `hierarchical_memory_limit`).
    stat: String,
    /// v1 `memory.memsw.limit_in_bytes`.
    memsw: String,
    /// v2 `memory.events`, v1 `memory.oom_control`.
    events: String,
}

impl MemPaths {
    pub(crate) fn new(cgroups: &Cgroups) -> Option<MemPaths> {
        let Controller { dir, v2 } = cgroups.memory.as_ref()?;
        let leaf = &dir.leaf;
        let each = |file: &str| dir.ancestors().map(|d| format!("{d}/{file}")).collect();
        Some(if *v2 {
            MemPaths {
                v2: true,
                max: each("memory.max"),
                swap_max: each("memory.swap.max"),
                soft: format!("{leaf}/memory.high"),
                current: format!("{leaf}/memory.current"),
                stat: format!("{leaf}/memory.stat"),
                memsw: String::new(),
                events: format!("{leaf}/memory.events"),
            }
        } else {
            MemPaths {
                v2: false,
                max: Vec::new(),
                swap_max: Vec::new(),
                soft: format!("{leaf}/memory.soft_limit_in_bytes"),
                current: format!("{leaf}/memory.usage_in_bytes"),
                stat: format!("{leaf}/memory.stat"),
                memsw: format!("{leaf}/memory.memsw.limit_in_bytes"),
                events: format!("{leaf}/memory.oom_control"),
            }
        })
    }
}

fn meminfo(root: &Root) -> Result<Meminfo> {
    root.with("/proc/meminfo", "memory", "/proc/meminfo", procfs::meminfo)
}

/// A cgroup value in bytes; `None` for `max`, a missing file, or junk.
fn bytes_file(root: &Root, path: &str) -> Option<u64> {
    root.with(
        path,
        "cgroup memory",
        "cgroup memory file",
        procfs::cgroup_bytes,
    )
    .ok()
    .flatten()
    .flatten()
}

/// The smallest finite value over `paths` (the tightest ancestor, E-2).
fn tightest(root: &Root, paths: &[String]) -> Option<u64> {
    paths.iter().filter_map(|p| bytes_file(root, p)).min()
}

/// A `key N` value of a cgroup `memory.stat` or `memory.events` file.
fn stat_value(root: &Root, path: &str, key: &[u8]) -> Option<u64> {
    root.with(path, "cgroup memory", "cgroup memory file", |d| {
        space_kv_value(d, key)
    })
    .ok()
    .flatten()
}

/// The memory reading. `paths` is `None` outside a memory cgroup; `lxcfs`
/// marks a `/proc/meminfo` that already shows the container (E-10).
pub(crate) fn read(
    root: &Root,
    paths: Option<&MemPaths>,
    lxcfs: bool,
    want: MemWant,
) -> Result<MemRaw> {
    let m = meminfo(root)?;
    let malformed = || Error::Malformed {
        what: "memory",
        source: "/proc/meminfo",
        line: None,
    };
    let host_total = m.total.ok_or_else(malformed)?;
    let host_available = m.available().ok_or_else(malformed)?;
    let mut raw = MemRaw {
        host_total,
        host_available,
        host_unused: m.free,
        cached: m.cache(),
        swap: if want.swap {
            Some((m.swap_total.unwrap_or(0), m.swap_free.unwrap_or(0)))
        } else {
            None
        },
        source: Some(if lxcfs {
            MemorySource::Lxcfs
        } else {
            MemorySource::Host
        }),
        ..MemRaw::default()
    };
    if lxcfs || !want.limit {
        return Ok(raw);
    }
    let Some(p) = paths else { return Ok(raw) };
    if p.v2 {
        let Some(hard) = tightest(root, &p.max) else {
            return Ok(raw);
        };
        // E-11: inactive_file read at a different instant may exceed current.
        let used = want
            .usage
            .then(|| bytes_file(root, &p.current))
            .flatten()
            .map(|c| c.saturating_sub(stat_value(root, &p.stat, b"inactive_file").unwrap_or(0)));
        raw.limit = Some(MemoryLimit {
            hard: Bytes::new(hard),
            soft: bytes_file(root, &p.soft).map(Bytes::new),
            swap: tightest(root, &p.swap_max).map(Bytes::new),
            source: LimitSource::Cgroup2,
        });
        raw.limited_used = used;
    } else {
        let hard =
            stat_value(root, &p.stat, b"hierarchical_memory_limit").filter(|&v| v < V1_UNLIMITED);
        let Some(hard) = hard else { return Ok(raw) };
        let used = want
            .usage
            .then(|| bytes_file(root, &p.current))
            .flatten()
            .map(|c| {
                c.saturating_sub(stat_value(root, &p.stat, b"total_inactive_file").unwrap_or(0))
            });
        let soft = bytes_file(root, &p.soft).filter(|&v| v < V1_UNLIMITED);
        // memory.memsw is memory plus swap; the swap share is the difference.
        let swap = bytes_file(root, &p.memsw)
            .filter(|&v| v < V1_UNLIMITED)
            .map(|memsw| memsw.saturating_sub(hard));
        raw.limit = Some(MemoryLimit {
            hard: Bytes::new(hard),
            soft: soft.map(Bytes::new),
            swap: swap.map(Bytes::new),
            source: LimitSource::Cgroup1,
        });
        raw.limited_used = used;
    }
    Ok(raw)
}

/// Out-of-memory events: the cgroup's, else the host's from `/proc/vmstat`.
pub(crate) fn events(root: &Root, paths: Option<&MemPaths>) -> Result<Option<MemoryEvents>> {
    if let Some(p) = paths {
        if p.v2 {
            if let Ok(e) = root.with(&p.events, "memory events", "cgroup memory.events", |d| {
                MemoryEvents {
                    oom_kills: space_kv_value(d, b"oom_kill").unwrap_or(0),
                    oom: space_kv_value(d, b"oom").unwrap_or(0),
                    max: space_kv_value(d, b"max").unwrap_or(0),
                    high: space_kv_value(d, b"high").unwrap_or(0),
                    scope: EventScope::Cgroup,
                }
            }) {
                return Ok(Some(e));
            }
        } else if let Some(kills) = stat_value(root, &p.events, b"oom_kill") {
            return Ok(Some(MemoryEvents {
                oom_kills: kills,
                scope: EventScope::Cgroup,
                ..MemoryEvents::default()
            }));
        }
    }
    // Kernel 4.13 and later count kills in /proc/vmstat.
    let kills = vmstat(root, b"oom_kill")?;
    Ok(kills.map(|oom_kills| MemoryEvents {
        oom_kills,
        scope: EventScope::Host,
        ..MemoryEvents::default()
    }))
}

/// One `/proc/vmstat` counter, read line by line through the stack.
fn vmstat(root: &Root, key: &[u8]) -> Result<Option<u64>> {
    let mut value = None;
    root.for_each_line(
        "/proc/vmstat",
        super::FILE_LIMIT,
        "paging",
        "/proc/vmstat",
        |line| match space_kv_value(line, key) {
            Some(v) => {
                value = Some(v);
                ControlFlow::Break(())
            }
            None => ControlFlow::Continue(()),
        },
    )?;
    Ok(value)
}

/// The paging counters in `/proc/vmstat` (pages, turned into bytes).
pub(crate) fn paging(root: &Root, page_size: u64) -> Result<PagingRaw> {
    let (mut swap_in, mut swap_out, mut major) = (None, None, None);
    root.for_each_line(
        "/proc/vmstat",
        super::FILE_LIMIT,
        "paging",
        "/proc/vmstat",
        |line| {
            if let Some(v) = space_kv_value(line, b"pswpin") {
                swap_in = Some(v);
            } else if let Some(v) = space_kv_value(line, b"pswpout") {
                swap_out = Some(v);
            } else if let Some(v) = space_kv_value(line, b"pgmajfault") {
                major = Some(v);
            }
            if swap_in.is_some() && swap_out.is_some() && major.is_some() {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        },
    )?;
    let malformed = || Error::Malformed {
        what: "paging",
        source: "/proc/vmstat",
        line: None,
    };
    Ok(PagingRaw {
        swap_in: swap_in.ok_or_else(malformed)?.saturating_mul(page_size),
        swap_out: swap_out.ok_or_else(malformed)?.saturating_mul(page_size),
        major_faults: major,
    })
}

/// The explicit huge-page pool and the transparent-huge-page mode.
pub(crate) fn huge_pages(root: &Root) -> Result<Option<HugePages>> {
    let m = meminfo(root)?;
    let Some(size) = m.huge_size else {
        return Ok(None);
    };
    let transparent = root
        .with(
            "/sys/kernel/mm/transparent_hugepage/enabled",
            "huge pages",
            "/sys/kernel/mm/transparent_hugepage/enabled",
            |d| match procfs::bracketed(d) {
                Some(b"always") => Some(TransparentHugePages::Always),
                Some(b"madvise") => Some(TransparentHugePages::Madvise),
                Some(b"never") => Some(TransparentHugePages::Never),
                _ => None,
            },
        )
        .ok()
        .flatten();
    Ok(Some(HugePages {
        page_size: Bytes::new(size),
        total: m.huge_total.unwrap_or(0),
        free: m.huge_free.unwrap_or(0),
        reserved: m.huge_rsvd.unwrap_or(0),
        transparent,
        large_page_minimum: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};
    use crate::mem::from_raw;

    fn snapshot(name: &str) -> crate::mem::Memory {
        let root = fixtures::root(name);
        let cg = fixtures::cgroups(name);
        let paths = MemPaths::new(&cg);
        from_raw(
            read(
                &root,
                paths.as_ref(),
                cg.lxcfs.meminfo,
                MemWant::ALL_FOR_TESTS,
            )
            .unwrap(),
        )
    }

    #[test]
    fn test_linux_mem_docker_cgroup_v2_limit_is_the_total() {
        let m = snapshot("docker-cgroup2-limited");
        assert_eq!(m.total, Bytes::from_mib(256));
        assert_eq!(m.source, MemorySource::Cgroup2);
        let limit = m.limit.unwrap();
        // Docker maps --memory-reservation to memory.low on cgroup v2, not to
        // the memory.high throttle point: no soft limit.
        assert_eq!((limit.hard, limit.soft), (Bytes::from_mib(256), None));
        assert!(m.used <= m.total && m.host_total > m.total);
        assert_eq!(m.used.saturating_add(m.free), m.total);
    }

    #[test]
    fn test_linux_mem_scope_limit_found_through_max_ancestors() {
        // E-2, E-5: a systemd scope with MemoryMax=256M under ancestors at max.
        let m = snapshot("wsl2-systemd-scope-limited");
        assert_eq!(m.total, Bytes::from_mib(256));
        assert_eq!(m.limit.unwrap().soft, Some(Bytes::from_mib(200)));
    }

    #[test]
    fn test_linux_mem_cgroup_v1_hierarchical_limit() {
        // E-4: the v1 memory controller.
        let m = snapshot("docker-cgroup1-limited");
        assert_eq!(m.total, Bytes::from_mib(256));
        assert_eq!(m.source, MemorySource::Cgroup1);
        assert!(m.used <= m.total);
    }

    #[test]
    fn test_linux_mem_without_a_limit_is_the_host() {
        // E-1: the WSL 2 init scope has memory.max = max.
        let m = snapshot("wsl2");
        assert_eq!(m.source, MemorySource::Host);
        assert_eq!(m.total, m.host_total);
        assert!(m.swap.total >= m.swap.used);
        let free_b: u64 = fixtures::tool("wsl2", "free -b")
            .unwrap()
            .lines()
            .find(|l| l.starts_with("Mem:"))
            .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
            .unwrap();
        assert_eq!(m.host_total.raw(), free_b);
    }

    #[test]
    fn test_linux_mem_lxcfs_is_not_limited_twice() {
        let root = fixtures::root("docker-cgroup2-limited");
        let paths = MemPaths::new(&fixtures::cgroups("docker-cgroup2-limited"));
        let raw = read(&root, paths.as_ref(), true, MemWant::ALL_FOR_TESTS).unwrap();
        assert_eq!((raw.limit, raw.source), (None, Some(MemorySource::Lxcfs)));
    }

    #[test]
    fn test_linux_mem_events_huge_pages_and_paging_from_fixtures() {
        let name = "wsl2-systemd-scope-limited";
        let root = fixtures::root(name);
        let paths = MemPaths::new(&fixtures::cgroups(name));
        let e = events(&root, paths.as_ref()).unwrap().unwrap();
        assert_eq!(e.scope, EventScope::Cgroup);
        let h = huge_pages(&root).unwrap().unwrap();
        assert_eq!(h.page_size, Bytes::from_mib(2));
        assert!(h.free <= h.total);
        let p = paging(&root, 4096).unwrap();
        assert!(p.major_faults.is_some());
        // No cgroup: the host's kill counter.
        let host = events(&root, None).unwrap().unwrap();
        assert_eq!(host.scope, EventScope::Host);
    }

    #[test]
    fn test_linux_mem_missing_meminfo_is_not_found() {
        let err = read(
            &fixtures::root("no-such-fixture"),
            None,
            false,
            MemWant::ALL_FOR_TESTS,
        )
        .unwrap_err();
        assert!(err.is_unavailable(), "{err}");
    }
}
