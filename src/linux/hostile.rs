//! Attack tests T5 to T8 for the Linux backend, over a hostile machine tree
//! built at test time: malformed, overflowing, negative, binary, and oversized
//! values in every file the 0.4 readings open, budgets one over their limit,
//! and network mounts that must never be touched (PLAN.md 19.2, `hostile`).
//!
//! Every reading must return a value that keeps its invariants or an error;
//! none may panic or read past its budget.

use std::{
    fs,
    path::{Path, PathBuf},
};

use super::{FILE_LIMIT, Root, cgroup::Cgroups, cpu, disk, mem, net, process, stat};
use crate::{Error, cpu::Times, disk::ListOptions};

/// A temporary machine tree, removed when dropped.
struct Tree(PathBuf);

impl Tree {
    fn new(tag: &str) -> Tree {
        let dir =
            std::env::temp_dir().join(format!("sys-lib-hostile-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Tree(dir)
    }

    fn put(&self, abs: &str, content: &[u8]) {
        let path = self.0.join(abs.trim_start_matches('/'));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn root(&self) -> Root {
        Root::at(&self.0)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Junk every reading must survive: overflow, negatives, NUL, binary, empty.
const JUNK: &[&[u8]] = &[
    b"",
    b"\0\0\0\0",
    b"\xff\xfe\xfd binary \x00 junk",
    b"99999999999999999999999999999999",
    b"-1",
    b"MemTotal: -5 kB\nMemAvailable: 18446744073709551615 kB\nMemFree: nan kB\n",
    b"cpu  -1 -2 -3 -4\ncpu0 18446744073709551615 0 0 0\nbtime -1\n",
    b"some avg10=-1.0 avg60=nan avg300=1e9 total=-5\nfull avg10=999.99 avg60=0 avg300=0 total=0\n",
    b"max\nmax\n",
    b"18446744073709551615",
];

const FILES: &[&str] = &[
    "/proc/meminfo",
    "/proc/stat",
    "/proc/loadavg",
    "/proc/uptime",
    "/proc/vmstat",
    "/proc/diskstats",
    "/proc/net/dev",
    "/proc/net/snmp",
    "/proc/net/sockstat",
    "/proc/net/sockstat6",
    "/proc/pressure/cpu",
    "/proc/pressure/memory",
    "/proc/pressure/io",
    "/proc/self/status",
    "/proc/self/statm",
    "/proc/self/stat",
    "/proc/self/io",
    "/proc/sys/fs/file-nr",
    "/sys/devices/system/cpu/online",
    "/sys/kernel/mm/transparent_hugepage/enabled",
];

/// Runs every 0.4 reading over `root`, checking the invariants of whatever
/// succeeds.
fn read_everything(root: &Root) {
    let cg = Cgroups::resolve(root);
    let paths = mem::MemPaths::new(&cg);
    if let Ok(raw) = mem::read(
        root,
        paths.as_ref(),
        false,
        crate::mem::MemWant::ALL_FOR_TESTS,
    ) {
        let m = crate::mem::from_raw(raw);
        assert!(m.used <= m.total && m.used.saturating_add(m.free) == m.total);
        assert!(m.host_free <= m.host_total && m.swap.used <= m.swap.total);
        assert!(m.used_percent() <= crate::Percent::FULL);
    }
    let _ = mem::events(root, paths.as_ref());
    let _ = mem::paging(root, 4096);
    if let Ok(Some(h)) = mem::huge_pages(root) {
        let _ = h.free.min(h.total);
    }
    let _ = stat::cpu_times(root, 100);
    let mut times = vec![Times::default(); 8];
    let _ = stat::cpu_times_per_core(root, 100, &mut times);
    if let Ok(Some(l)) = stat::load(root) {
        assert!(l.one >= 0.0 && l.one.is_finite());
    }
    let _ = stat::boot_time(root);
    let _ = stat::uptime(root);
    for r in [
        stat::Resource::Cpu,
        stat::Resource::Memory,
        stat::Resource::Io,
    ] {
        if let Ok(Some(p)) = stat::pressure(root, &cg, r) {
            assert!(p.some.avg10 <= crate::Percent::FULL && p.some.avg10 >= crate::Percent::ZERO);
        }
    }
    let _ = cpu::online(root);
    let _ = cpu::frequency(root);
    let _ = cpu::current_frequency(root);
    let a = cpu::available(root, &cg, 8);
    assert!((1..=8).contains(&a));
    let _ = disk::mounts(root, &ListOptions::default());
    let _ = disk::io_totals(root);
    let _ = disk::io_counters(root);
    let _ = net::totals(root);
    let _ = net::counters(root, 4096);
    let _ = net::connections(root);
    let _ = process::memory(root, 4096);
    let _ = process::memory_detail(root);
    let _ = process::io(root);
    let _ = super::host::open_files(root);
}

#[test]
fn test_attack_t6_hostile_values_in_every_file_never_panic() {
    // T6: every file holds each kind of junk in turn.
    for (i, junk) in JUNK.iter().enumerate() {
        let t = Tree::new(&format!("junk{i}"));
        for f in FILES {
            t.put(f, junk);
        }
        read_everything(&t.root());
    }
}

#[test]
fn test_attack_t7_oversized_files_and_lists_are_too_large() {
    // T7: one over each budget: a /proc file past 1 MiB, more mounts than the
    // mount budget. Each is refused with TooLarge, never read whole.
    let t = Tree::new("t7");
    let big = vec![b'7'; FILE_LIMIT + 1];
    t.put("/proc/net/dev", &big);
    t.put("/proc/diskstats", &big);
    let root = t.root();
    assert!(matches!(net::totals(&root), Err(Error::TooLarge { .. })));
    assert!(matches!(
        disk::io_totals(&root),
        Err(Error::TooLarge { .. })
    ));
    let line = "1 0 8:1 / /m rw - ext4 /dev/sda1 rw\n";
    t.put("/proc/self/mountinfo", line.repeat(4097).as_bytes());
    let options = ListOptions::default();
    assert!(matches!(
        disk::mounts(&root, &options),
        Err(Error::TooLarge { limit: 4096, .. })
    ));
}

#[test]
fn test_attack_t5_values_read_at_different_instants_keep_invariants() {
    // T5: a cgroup whose usage exceeds its limit and whose inactive_file
    // exceeds its usage, as reads at different instants can give.
    let t = Tree::new("t5");
    t.put(
        "/proc/meminfo",
        b"MemTotal: 1048576 kB\nMemAvailable: 2097152 kB\nMemFree: 10 kB\n",
    );
    t.put("/proc/self/cgroup", b"0::/\n");
    t.put(
        "/proc/self/mountinfo",
        b"1 0 0:1 / /sys/fs/cgroup rw - cgroup2 cgroup2 rw\n",
    );
    t.put("/sys/fs/cgroup/memory.max", b"104857600\n");
    t.put("/sys/fs/cgroup/memory.current", b"209715200\n");
    t.put(
        "/sys/fs/cgroup/memory.stat",
        b"inactive_file 999999999999\n",
    );
    let root = t.root();
    let cg = Cgroups::resolve(&root);
    let paths = mem::MemPaths::new(&cg);
    let m = crate::mem::from_raw(
        mem::read(
            &root,
            paths.as_ref(),
            false,
            crate::mem::MemWant::ALL_FOR_TESTS,
        )
        .unwrap(),
    );
    assert_eq!(m.total.raw(), 100 << 20);
    assert!(m.used <= m.total, "{m:?}");
    assert_eq!(m.used.saturating_add(m.free), m.total);
    assert!(m.host_free <= m.host_total);
}

#[test]
fn test_attack_t8_network_mounts_are_never_listed_by_default() {
    // T8: a dead NFS or CIFS mount must not be touched by default: list()
    // calls statvfs only on what mounts() keeps.
    let t = Tree::new("t8");
    t.put(
        "/proc/self/mountinfo",
        b"1 0 8:1 / / rw - ext4 /dev/sda1 rw\n\
          2 1 0:50 / /mnt/dead rw - nfs4 10.255.255.1:/export rw\n\
          3 1 0:51 / /mnt/share rw - cifs //nas/share rw\n\
          4 1 0:52 / /mnt/ssh rw - fuse.sshfs user@host:/ rw\n",
    );
    let root = t.root();
    let kept = disk::mounts(&root, &ListOptions::default()).unwrap();
    assert_eq!(kept.len(), 1);
    assert!(kept.iter().all(|m| !m.network));
    let with = disk::mounts(
        &root,
        &ListOptions {
            include_network: true,
            ..ListOptions::default()
        },
    )
    .unwrap();
    assert_eq!(with.iter().filter(|m| m.network).count(), 3);
    assert!(t.path().exists());
}
