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

/// The process's directory in `hierarchy`, if the process is in one and it is mounted.
pub(crate) fn dir(root: &Root, hierarchy: Hierarchy) -> Option<Dir> {
    let memberships = root
        .read("/proc/self/cgroup", 64 << 10, "cgroup", "/proc/self/cgroup")
        .ok()?;
    let membership = cgroup::memberships(&memberships).find(|m| matches(m, hierarchy))?;
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
    mountinfo::mounts(&mounts).find_map(|m| {
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
}
