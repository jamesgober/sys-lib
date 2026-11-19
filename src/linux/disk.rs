//! Disks on Linux: the mounted filesystems from `/proc/self/mountinfo`
//! (E-36, E-37) and the whole-disk I/O counters from `/proc/diskstats`.

use core::{ops::ControlFlow, time::Duration};
use std::path::{Path, PathBuf};

use super::{Root, join_name};
use crate::{
    Error, Result,
    disk::{DiskKind, DiskRaw, ListOptions},
    parse::{lossy, mountinfo, procfs},
};

/// Filesystems that report on the kernel, not on storage: skipped unless
/// `include_pseudo`. `overlay` and `rootfs` count as pseudo except as the
/// root (a container's root filesystem; an initramfs system).
const PSEUDO: &[&[u8]] = &[
    b"proc",
    b"rootfs",
    b"sysfs",
    b"cgroup",
    b"cgroup2",
    b"tmpfs",
    b"devtmpfs",
    b"devpts",
    b"squashfs",
    b"overlay",
    b"autofs",
    b"debugfs",
    b"tracefs",
    b"securityfs",
    b"pstore",
    b"bpf",
    b"mqueue",
    b"hugetlbfs",
    b"configfs",
    b"fusectl",
    b"binfmt_misc",
    b"efivarfs",
    b"ramfs",
    b"rpc_pipefs",
    b"nsfs",
    b"selinuxfs",
    b"fuse.lxcfs",
    b"fuse.gvfsd-fuse",
    b"fuse.portal",
];

/// Network filesystems: skipped unless `include_network`, because `statvfs`
/// on a dead hard mount can hang (T8).
const NETWORK: &[&[u8]] = &[
    b"nfs",
    b"nfs4",
    b"cifs",
    b"smb3",
    b"smbfs",
    b"fuse.sshfs",
    b"9p",
    b"afs",
    b"ceph",
    b"glusterfs",
    b"fuse.glusterfs",
    b"lustre",
    b"gfs2",
    b"ocfs2",
    b"fuse.s3fs",
    b"davfs",
    b"fuse.rclone",
];

/// One mount the list keeps, before its space is read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MountEntry {
    pub(crate) mount_point: PathBuf,
    pub(crate) device: String,
    pub(crate) filesystem: String,
    pub(crate) kind: DiskKind,
    pub(crate) read_only: bool,
    pub(crate) removable: bool,
    pub(crate) network: bool,
    /// `major:minor`, for deduplication.
    dev: Vec<u8>,
    /// The per-mount options (`rw,noatime`).
    options: String,
}

impl MountEntry {
    /// The device number, `major:minor`.
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "android", test)),
        allow(dead_code)
    )]
    pub(crate) fn dev(&self) -> &str {
        core::str::from_utf8(&self.dev).unwrap_or("")
    }

    /// The per-mount options.
    pub(crate) fn options(&self) -> &str {
        &self.options
    }
}

fn is_ro(options: &[u8]) -> bool {
    options.split(|&b| b == b',').any(|o| o == b"ro")
}

/// Whether the block device behind `source` (`/dev/sdb1`, `/dev/nvme0n1p2`)
/// is removable, from its disk's `removable` file.
fn removable(root: &Root, source: &[u8]) -> bool {
    block_file(root, source, "/removable").is_some_and(|t| t == "1")
}

/// `file` under the source's `/sys/block/<disk>`, the source itself or, for a
/// partition, the disk it is on.
fn block_file(root: &Root, source: &[u8], file: &str) -> Option<String> {
    let name = source.strip_prefix(b"/dev/")?;
    let read = |n: &[u8]| {
        let mut buf = [0u8; 128];
        join_name(&mut buf, "/sys/block/", n, file).and_then(|p| root.text(p))
    };
    if let Some(text) = read(name) {
        return Some(text);
    }
    // A partition: strip the partition number (and the `p` of nvme0n1p2).
    let disk = name.trim_ascii_end();
    let end = disk
        .iter()
        .rposition(|b| !b.is_ascii_digit())
        .map_or(0, |i| i + 1);
    let mut parent = &disk[..end];
    if parent.ends_with(b"p") && parent.len() > 1 && parent[parent.len() - 2].is_ascii_digit() {
        parent = &parent[..parent.len() - 1];
    }
    if parent.is_empty() || parent == name {
        return None;
    }
    read(parent)
}

/// SSD or HDD from the disk's `rotational` flag (DSK-04).
fn medium(root: &Root, source: &[u8]) -> DiskKind {
    match block_file(root, source, "/queue/rotational").as_deref() {
        Some("0") => DiskKind::Ssd,
        Some("1") => DiskKind::Hdd,
        _ => DiskKind::Unknown,
    }
}

/// The mount holding `path` (absolute and resolved): the longest mount point
/// that contains it, the last such mount when several stack. The table is
/// streamed and only the best match kept, so a host with more mounts than a
/// `disk::list` budget still answers; only the bytes read are bounded.
pub(crate) fn holding(root: &Root, path: &Path) -> Result<Option<MountEntry>> {
    holding_at(root, "/proc/self/mountinfo", path)
}

/// The bytes [`holding`] reads at most: the default `disk::list` budget.
const HOLDING_BYTES: usize = 16 << 20;

fn holding_at(root: &Root, file: &str, path: &Path) -> Result<Option<MountEntry>> {
    let want = path.as_os_str().as_encoded_bytes();
    let all = ListOptions {
        include_network: true,
        include_pseudo: true,
        include_duplicates: true,
        ..ListOptions::default()
    };
    let mut best: Option<(usize, MountEntry)> = None;
    root.for_each_line(
        file,
        HOLDING_BYTES,
        "disk of a path",
        "/proc/self/mountinfo",
        |line| {
            if let Some(m) = mountinfo::parse_line(line) {
                let at = &*m.mount_point;
                let better = best.as_ref().is_none_or(|(len, _)| at.len() >= *len);
                if better
                    && contains(at, want)
                    && let Some(e) = entry(root, &m, &all)
                {
                    best = Some((at.len(), e));
                }
            }
            ControlFlow::Continue(())
        },
    )?;
    Ok(best.map(|(_, e)| e))
}

/// Whether the mount point `at` contains `path`, by whole components.
fn contains(at: &[u8], path: &[u8]) -> bool {
    at == b"/" || (path.starts_with(at) && matches!(path.get(at.len()), None | Some(b'/')))
}

/// The filesystems `options` selects, each once (by device number) unless
/// `include_duplicates`, at their shortest mount point.
pub(crate) fn mounts(root: &Root, options: &ListOptions) -> Result<Vec<MountEntry>> {
    mounts_at(root, "/proc/self/mountinfo", options)
}

/// [`mounts`] from another `mountinfo` (a host's process 1, for a probe).
pub(crate) fn mounts_at(root: &Root, path: &str, options: &ListOptions) -> Result<Vec<MountEntry>> {
    let mut out: Vec<MountEntry> = Vec::new();
    let mut seen = 0usize;
    let mut too_many = false;
    // T7: the budget bounds the bytes read as well as the entries kept.
    let byte_budget = options.max_mounts.saturating_mul(4096).max(1 << 16);
    root.for_each_line(
        path,
        byte_budget,
        "disk list",
        "/proc/self/mountinfo",
        |line| {
            seen += 1;
            if seen > options.max_mounts {
                too_many = true;
                return ControlFlow::Break(());
            }
            let Some(m) = mountinfo::parse_line(line) else {
                return ControlFlow::Continue(());
            };
            let Some(entry) = entry(root, &m, options) else {
                return ControlFlow::Continue(());
            };
            let network = entry.network;
            let duplicate = if options.include_duplicates || network {
                None
            } else {
                out.iter().position(|e| e.dev == entry.dev)
            };
            match duplicate {
                // E-37: bind mounts and subvolumes of one filesystem: keep the
                // shortest mount point.
                Some(i) => {
                    if entry.mount_point.as_os_str().len() < out[i].mount_point.as_os_str().len() {
                        out[i] = entry;
                    }
                }
                None => out.push(entry),
            }
            ControlFlow::Continue(())
        },
    )?;
    if too_many {
        return Err(Error::TooLarge {
            what: "disk list",
            limit: options.max_mounts as u64,
        });
    }
    Ok(out)
}

/// The entry for one mountinfo line, or `None` when `options` skips it.
fn entry(root: &Root, m: &mountinfo::Mount<'_>, options: &ListOptions) -> Option<MountEntry> {
    let is_root = &*m.mount_point == b"/";
    let network = NETWORK.contains(&m.fstype);
    let pseudo =
        PSEUDO.contains(&m.fstype) && !(is_root && matches!(m.fstype, b"overlay" | b"rootfs"));
    if (network && !options.include_network) || (pseudo && !options.include_pseudo) {
        return None;
    }
    let removable = removable(root, &m.source);
    let kind = if network {
        DiskKind::Network
    } else if m.fstype == b"tmpfs" || m.fstype == b"ramfs" {
        DiskKind::Ram
    } else if m.fstype == b"overlay" || m.fstype.starts_with(b"fuse") {
        DiskKind::Virtual
    } else if m.source.starts_with(b"/dev/sr") || m.fstype == b"iso9660" || m.fstype == b"udf" {
        DiskKind::Optical
    } else if removable {
        DiskKind::Removable
    } else {
        medium(root, &m.source)
    };
    Some(MountEntry {
        mount_point: PathBuf::from(lossy(&m.mount_point, 4096)),
        device: lossy(&m.source, 4096),
        filesystem: lossy(m.fstype, 64),
        kind,
        read_only: is_ro(m.options),
        removable,
        network,
        dev: m.device.to_vec(),
        options: lossy(m.options, 4096),
    })
}

/// Whether a `/proc/diskstats` name is a whole physical disk: listed in
/// `/sys/block` (partitions are not), and not a loop, RAM, zram,
/// device-mapper, or RAID device (whose I/O is counted on its members).
fn whole_disk(root: &Root, name: &[u8]) -> bool {
    const VIRTUAL: &[&[u8]] = &[b"loop", b"ram", b"zram", b"dm-", b"md", b"nbd"];
    if VIRTUAL.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    let mut buf = [0u8; 128];
    join_name(&mut buf, "/sys/block/", name, "").is_some_and(|p| root.exists(p))
}

fn raw(l: &procfs::DiskLine<'_>) -> DiskRaw {
    // Sectors are 512 bytes whatever the device's block size.
    DiskRaw {
        read: l.sectors_read.saturating_mul(512),
        written: l.sectors_written.saturating_mul(512),
        reads: l.reads,
        writes: l.writes,
        busy: Some(Duration::from_millis(l.io_ms)),
        op_time: Some(Duration::from_millis(l.read_ms.saturating_add(l.write_ms))),
        disks: 1,
    }
}

/// Every physical disk's counters, summed, through the stack.
pub(crate) fn io_totals(root: &Root) -> Result<DiskRaw> {
    let mut total = DiskRaw {
        busy: Some(Duration::ZERO),
        op_time: Some(Duration::ZERO),
        ..DiskRaw::default()
    };
    root.for_each_line(
        "/proc/diskstats",
        super::FILE_LIMIT,
        "disk I/O",
        "/proc/diskstats",
        |line| {
            if let Some(l) = procfs::diskstats_line(line)
                && whole_disk(root, l.name)
            {
                total.add(&raw(&l));
            }
            ControlFlow::Continue(())
        },
    )?;
    Ok(total)
}

/// Each physical disk's counters.
pub(crate) fn io_counters(root: &Root) -> Result<Vec<(String, DiskRaw)>> {
    let mut out = Vec::new();
    root.for_each_line(
        "/proc/diskstats",
        super::FILE_LIMIT,
        "disk I/O",
        "/proc/diskstats",
        |line| {
            if let Some(l) = procfs::diskstats_line(line)
                && whole_disk(root, l.name)
                && out.len() < 4096
            {
                out.push((lossy(l.name, 64), raw(&l)));
            }
            ControlFlow::Continue(())
        },
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_linux_disk_of_answers_past_the_list_budget() {
        // A node with more mounts than `disk::list` accepts still answers for
        // one path, and matches whole path components.
        let dir = std::env::temp_dir().join(format!("sys-lib-mounts-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proc/self")).unwrap();
        let mut table = String::from("1 0 8:1 / / rw - ext4 /dev/sda1 rw\n");
        for i in 0..5000 {
            table.push_str(&format!("{} 1 0:{i} / /m/{i} rw - tmpfs tmpfs rw\n", i + 2));
        }
        table.push_str("9000 1 8:17 / /data rw - xfs /dev/sdb1 rw\n");
        std::fs::write(dir.join("proc/self/mountinfo"), table).unwrap();
        let root = Root::at(&dir);
        assert!(matches!(
            mounts(&root, &ListOptions::default()),
            Err(Error::TooLarge { .. })
        ));
        let m = holding(&root, Path::new("/data/db")).unwrap().unwrap();
        assert_eq!(
            (m.mount_point.as_path(), &*m.filesystem),
            (Path::new("/data"), "xfs")
        );
        let m = holding(&root, Path::new("/database")).unwrap().unwrap();
        assert_eq!(m.mount_point, Path::new("/"));
        let m = holding(&root, Path::new("/m/12/x")).unwrap().unwrap();
        assert_eq!(m.mount_point, Path::new("/m/12"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_linux_disk_of_long_mount_line_is_not_skipped() {
        // A mountinfo line longer than the 8 KiB line buffer.
        let dir = std::env::temp_dir().join(format!("sys-lib-longmount-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("proc/self")).unwrap();
        // Each space in a mount point is written as `\040`.
        let deep = format!("/{}", r"a\040b".repeat(2000));
        let table =
            format!("1 0 8:1 / / rw - ext4 /dev/sda1 rw\n2 1 0:50 / {deep} rw - tmpfs tmpfs rw\n");
        std::fs::write(dir.join("proc/self/mountinfo"), table).unwrap();
        let want = format!("/{}/f", "a b".repeat(2000));
        let m = holding(&Root::at(&dir), Path::new(&want)).unwrap().unwrap();
        assert_eq!(&*m.filesystem, "tmpfs");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_linux_disk_mounts_skip_pseudo_network_and_duplicates() {
        // WSL 2: ext4 root, 9p drvfs (network), many tmpfs and overlay helpers.
        let m = mounts(&fixtures::root("wsl2"), &ListOptions::default()).unwrap();
        assert!(
            m.iter()
                .any(|e| e.mount_point.as_os_str() == "/" && e.filesystem == "ext4"),
            "{m:?}"
        );
        assert!(
            m.iter().all(
                |e| !e.network && !matches!(e.filesystem.as_str(), "tmpfs" | "proc" | "rootfs")
            )
        );
        let mut devs: Vec<_> = m.iter().map(|e| e.dev.clone()).collect();
        devs.sort();
        let before = devs.len();
        devs.dedup();
        assert_eq!(before, devs.len(), "each filesystem once");
        let all = mounts(
            &fixtures::root("wsl2"),
            &ListOptions {
                include_network: true,
                include_pseudo: true,
                include_duplicates: true,
                ..ListOptions::default()
            },
        )
        .unwrap();
        assert!(all.len() > m.len());
        assert!(all.iter().any(|e| e.network && e.kind == DiskKind::Network));
    }

    #[test]
    fn test_linux_disk_container_overlay_root_is_kept() {
        let m = mounts(
            &fixtures::root("docker-cgroup2-limited"),
            &ListOptions::default(),
        )
        .unwrap();
        let root = m.iter().find(|e| e.mount_point.as_os_str() == "/").unwrap();
        assert_eq!(
            (root.filesystem.as_str(), root.kind),
            ("overlay", DiskKind::Virtual)
        );
    }

    #[test]
    fn test_linux_disk_mount_budget_is_too_large() {
        let small = ListOptions {
            max_mounts: 2,
            ..ListOptions::default()
        };
        assert!(matches!(
            mounts(&fixtures::root("wsl2"), &small),
            Err(Error::TooLarge { limit: 2, .. })
        ));
    }

    #[test]
    fn test_linux_disk_io_counts_whole_disks_only() {
        let root = fixtures::root("wsl2");
        let per = io_counters(&root).unwrap();
        let names: Vec<&str> = per.iter().map(|(n, _)| n.as_str()).collect();
        assert!(names.iter().all(|n| n.starts_with("sd")), "{names:?}");
        assert!(!names.is_empty());
        let total = io_totals(&root).unwrap();
        let summed = per.iter().fold(0u64, |a, (_, r)| a + r.read);
        assert_eq!(total.read, summed);
        assert!(total.busy.is_some() && total.op_time.is_some());
    }

    #[test]
    fn test_linux_disk_removable_parent_of_a_partition() {
        assert!(!removable(&fixtures::root("wsl2"), b"/dev/sda1"));
        assert!(!removable(&fixtures::root("wsl2"), b"none"));
        assert!(!removable(&fixtures::root("wsl2"), b"/dev/../etc"));
    }
}
