//! Finding this process's cgroup directories: the membership in
//! `/proc/self/cgroup`, joined under the matching mount from
//! `/proc/self/mountinfo` (never an assumed `/sys/fs/cgroup`, E-6).

use super::Root;
use crate::parse::{
    cgroup::{self, Membership},
    mountinfo,
};

/// A cgroup directory and the mount it lives under, as absolute paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dir {
    /// The mount point of the hierarchy.
    pub(crate) mount: String,
    /// The process's cgroup directory: `mount` plus the relative path.
    pub(crate) leaf: String,
}

impl Dir {
    /// The directories from the leaf up to the mount root, leaf first.
    pub(crate) fn ancestors(&self) -> impl Iterator<Item = &str> {
        let mut current = Some(self.leaf.as_str());
        let mount = self.mount.as_str();
        std::iter::from_fn(move || {
            let dir = current?;
            current = if dir.len() > mount.len() {
                dir.rfind('/').map(|i| &dir[..i.max(mount.len())])
            } else {
                None
            };
            Some(dir)
        })
        .take(cgroup::MAX_DEPTH + 1)
    }
}

/// Which hierarchy to look for.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Hierarchy {
    /// cgroup v2.
    Unified,
    /// The cgroup v1 hierarchy carrying this controller.
    V1(&'static str),
}

/// The two files every lookup needs, read once.
struct Tables {
    memberships: Vec<u8>,
    mounts: Vec<u8>,
}

impl Tables {
    fn read(root: &Root) -> Option<Tables> {
        let memberships = root
            .read("/proc/self/cgroup", 64 << 10, "cgroup", "/proc/self/cgroup")
            .ok()?;
        // A prefix is enough: the cgroup mounts come early, and a node with
        // 100 000 mounts must not cost a 15 MB read (T7).
        let mounts = root
            .read_prefix(
                "/proc/self/mountinfo",
                super::FILE_LIMIT,
                "mounts",
                "/proc/self/mountinfo",
            )
            .ok()?;
        Some(Tables {
            memberships,
            mounts,
        })
    }
}

/// A controller's directory and the cgroup version that carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Controller {
    pub(crate) dir: Dir,
    /// cgroup v2 (the unified hierarchy) rather than a v1 hierarchy.
    pub(crate) v2: bool,
}

/// Which `/proc` files LXCFS overlays with the container's own values: the
/// cgroup limits must then not be applied a second time (E-10).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Lxcfs {
    pub(crate) meminfo: bool,
    pub(crate) stat: bool,
}

/// This process's cgroups for every controller sys-lib reads, found once from
/// one read of `/proc/self/cgroup` and of `/proc/self/mountinfo`.
///
/// Each controller comes from the cgroup v1 hierarchy that carries it when
/// there is one (a v1 host, or a hybrid host whose v2 mount at
/// `/sys/fs/cgroup/unified` has no controllers, E-4), else from the unified
/// hierarchy. The system root's is resolved once per process (D-045).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Cgroups {
    pub(crate) memory: Option<Controller>,
    pub(crate) cpu: Option<Controller>,
    pub(crate) cpuset: Option<Controller>,
    /// The unified hierarchy's directory, for the pressure files (cgroup v2
    /// only: v1 keeps no per-cgroup pressure).
    pub(crate) unified: Option<Dir>,
    pub(crate) lxcfs: Lxcfs,
}

impl Cgroups {
    pub(crate) fn resolve(root: &Root) -> Cgroups {
        let Some(tables) = Tables::read(root) else {
            // E-7, E-8: no cgroup file or no mount table: host values.
            return Cgroups::default();
        };
        let unified = find(&tables, Hierarchy::Unified);
        let controller = |name: &'static str| {
            find(&tables, Hierarchy::V1(name))
                .map(|dir| Controller { dir, v2: false })
                .or_else(|| unified.clone().map(|dir| Controller { dir, v2: true }))
        };
        let mut lxcfs = Lxcfs::default();
        for m in mountinfo::mounts(&tables.mounts) {
            if m.fstype.starts_with(b"fuse")
                && (m.fstype.ends_with(b"lxcfs") || &*m.source == b"lxcfs")
            {
                match &*m.mount_point {
                    b"/proc/meminfo" => lxcfs.meminfo = true,
                    b"/proc/stat" => lxcfs.stat = true,
                    _ => {}
                }
            }
        }
        Cgroups {
            memory: controller("memory"),
            cpu: controller("cpu"),
            cpuset: controller("cpuset"),
            unified,
            lxcfs,
        }
    }
}

/// The process's directory in `hierarchy`, if the process is in one and it is mounted.
#[cfg(test)]
pub(crate) fn dir(root: &Root, hierarchy: Hierarchy) -> Option<Dir> {
    find(&Tables::read(root)?, hierarchy)
}

fn find(tables: &Tables, hierarchy: Hierarchy) -> Option<Dir> {
    let membership = cgroup::memberships(&tables.memberships).find(|m| matches(m, hierarchy))?;
    mountinfo::mounts(&tables.mounts).find_map(|m| {
        let wanted = match hierarchy {
            Hierarchy::Unified => m.fstype == b"cgroup2",
            Hierarchy::V1(c) => {
                m.fstype == b"cgroup" && mountinfo::has_option(m.super_options, c.as_bytes())
            }
        };
        if !wanted {
            return None;
        }
        // Inside a cgroup namespace the mount root is the process's own
        // cgroup ("/"); outside it is the hierarchy root. The membership
        // path must lie under it.
        let rel = membership
            .path
            .strip_prefix(&*m.root)
            .or_else(|| (&*m.root == b"/").then_some(membership.path))?;
        let rel = std::str::from_utf8(rel).ok()?.trim_end_matches('/');
        let mount = std::str::from_utf8(&m.mount_point)
            .ok()?
            .trim_end_matches('/')
            .to_owned();
        let rel = if rel.is_empty() || rel.starts_with('/') {
            rel.to_owned()
        } else {
            format!("/{rel}")
        };
        Some(Dir {
            leaf: format!("{mount}{rel}"),
            mount,
        })
    })
}

fn matches(m: &Membership<'_>, hierarchy: Hierarchy) -> bool {
    match hierarchy {
        Hierarchy::Unified => m.is_unified(),
        Hierarchy::V1(c) => m.has(c.as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_cgroup_dir_without_a_namespace_walks_to_the_mount() {
        // E-5: a systemd scope, no cgroup namespace.
        let d = dir(
            &fixtures::root("wsl2-systemd-scope-limited"),
            Hierarchy::Unified,
        )
        .unwrap();
        assert_eq!(d.mount, "/sys/fs/cgroup");
        assert!(
            d.leaf.starts_with("/sys/fs/cgroup/user.slice/") && d.leaf.ends_with(".scope"),
            "{}",
            d.leaf
        );
        let chain: Vec<_> = d.ancestors().collect();
        assert_eq!(chain.first().copied(), Some(d.leaf.as_str()));
        assert_eq!(chain.last().copied(), Some("/sys/fs/cgroup"));
        assert_eq!(chain.len(), 6);
    }

    #[test]
    fn test_cgroup_dir_inside_a_namespace_is_the_mount_root() {
        let d = dir(
            &fixtures::root("docker-cgroup2-limited"),
            Hierarchy::Unified,
        )
        .unwrap();
        assert_eq!(d.leaf, "/sys/fs/cgroup");
        assert_eq!(d.ancestors().collect::<Vec<_>>(), vec!["/sys/fs/cgroup"]);
    }

    #[test]
    fn test_cgroup_dir_missing_v1_hierarchy() {
        assert_eq!(dir(&fixtures::root("wsl2"), Hierarchy::V1("cpu")), None);
    }

    #[test]
    fn test_cgroups_v1_controllers_win_over_a_unified_mount() {
        // E-4: a cgroup v1 host; every controller from its own v1 hierarchy.
        let cg = fixtures::cgroups("docker-cgroup1-limited");
        let memory = cg.memory.unwrap();
        assert!(!memory.v2);
        assert_eq!(memory.dir.mount, "/sys/fs/cgroup/memory");
        let cpu = cg.cpu.unwrap();
        assert!(!cpu.v2);
        assert_eq!(cpu.dir.mount, "/sys/fs/cgroup/cpu,cpuacct");
        assert!(!cg.cpuset.unwrap().v2);
    }

    #[test]
    fn test_cgroups_v2_controllers_are_the_unified_hierarchy() {
        let cg = fixtures::cgroups("docker-cgroup2-limited");
        let memory = cg.memory.unwrap();
        assert!(memory.v2);
        assert_eq!(Some(&memory.dir), cg.unified.as_ref());
        assert_eq!(cg.lxcfs, Lxcfs::default());
        // E-7: no cgroup file at all.
        assert_eq!(fixtures::cgroups("no-such-fixture"), Cgroups::default());
    }
}
