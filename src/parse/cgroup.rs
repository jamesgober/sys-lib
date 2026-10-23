//! cgroup files: `/proc/self/cgroup`, `cpu.max`, and the v1 CFS quota files.

/// The longest cgroup path accepted (threats T4 and T6).
pub(crate) const MAX_PATH: usize = 4096;
/// The deepest cgroup path accepted (the 0.4 defence idea, applied from 0.3).
pub(crate) const MAX_DEPTH: usize = 64;

/// One line of `/proc/self/cgroup`: `hierarchy-id:controllers:path`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Membership<'a> {
    pub(crate) hierarchy: u32,
    pub(crate) controllers: &'a [u8],
    pub(crate) path: &'a [u8],
}

impl Membership<'_> {
    /// The unified (v2) hierarchy: id 0 and no controllers.
    pub(crate) fn is_unified(&self) -> bool {
        self.hierarchy == 0 && self.controllers.is_empty()
    }

    /// Whether this v1 hierarchy carries `controller` (`cpu`, `cpuset`, `memory`).
    pub(crate) fn has(&self, controller: &[u8]) -> bool {
        self.controllers
            .split(|&b| b == b',')
            .any(|c| c == controller)
    }
}

/// The memberships, skipping malformed lines and paths that fail [`valid_path`].
pub(crate) fn memberships(data: &[u8]) -> impl Iterator<Item = Membership<'_>> {
    super::lines(data, 256).filter_map(|line| {
        let mut parts = line.splitn(3, |&b| b == b':');
        let hierarchy = u32::try_from(super::parse_u64(parts.next()?)?).ok()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        valid_path(path).then_some(Membership {
            hierarchy,
            controllers,
            path,
        })
    })
}

/// A cgroup path sys-lib will join under a mount point: absolute, no `..`
/// component, no NUL, at most [`MAX_PATH`] bytes and [`MAX_DEPTH`] levels.
pub(crate) fn valid_path(path: &[u8]) -> bool {
    path.starts_with(b"/")
        && path.len() <= MAX_PATH
        && !path.contains(&0)
        && path.split(|&b| b == b'/').all(|c| c != b"..")
        && path.split(|&b| b == b'/').filter(|c| !c.is_empty()).count() <= MAX_DEPTH
}

/// A CPU quota as `(quota, period)` in microseconds; `None` when unlimited.
pub(crate) type Quota = Option<(u64, u64)>;

/// cgroup v2 `cpu.max`: `max 100000` or `150000 100000`.
pub(crate) fn cpu_max(data: &[u8]) -> Option<Quota> {
    let mut parts = super::trim(data).split(|&b| b == b' ');
    let quota = parts.next()?;
    let period = super::parse_u64(parts.next()?)?;
    if parts.next().is_some() || period == 0 {
        return None;
    }
    if quota == b"max" {
        return Some(None);
    }
    let quota = super::parse_u64(quota)?;
    Some(Some((quota, period)))
}

/// cgroup v1 `cpu.cfs_quota_us` and `cpu.cfs_period_us` (`-1` means unlimited).
pub(crate) fn cfs(quota: &[u8], period: &[u8]) -> Option<Quota> {
    let quota = super::parse_i64(super::trim(quota))?;
    let period = super::parse_u64(super::trim(period))?;
    if period == 0 {
        return None;
    }
    if quota < 0 {
        return Some(None);
    }
    Some(Some((quota.unsigned_abs(), period)))
}

/// The CPUs a quota allows, rounded up, at least 1.
pub(crate) fn quota_cpus((quota, period): (u64, u64)) -> usize {
    let cpus = quota.div_ceil(period).max(1);
    usize::try_from(cpus).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cgroup_memberships_v1_v2_and_hybrid() {
        let data =
            b"12:cpu,cpuacct:/docker/abc\n4:cpuset:/docker/abc\n0::/system.slice/site.service\n";
        let m: Vec<_> = memberships(data).collect();
        assert_eq!(m.len(), 3);
        assert!(m[0].has(b"cpu") && m[0].has(b"cpuacct") && !m[0].has(b"cpuset"));
        assert!(m[2].is_unified());
        assert_eq!(m[2].path, b"/system.slice/site.service");
        // A named v1 hierarchy is not the unified one.
        let named: Vec<_> = memberships(b"1:name=systemd:/\n").collect();
        assert!(!named[0].is_unified());
    }

    #[test]
    fn test_cgroup_paths_are_validated() {
        assert!(valid_path(b"/"));
        assert!(valid_path(b"/a/b.scope"));
        assert!(!valid_path(b"relative"));
        assert!(!valid_path(b"/a/../etc"));
        assert!(!valid_path(b"/a/\0b"));
        let deep = b"/x".repeat(65);
        assert!(!valid_path(&deep));
        assert!(valid_path(&b"/x".repeat(64)));
        assert_eq!(memberships(b"0::/../../etc\n").count(), 0);
    }

    #[test]
    fn test_cgroup_cpu_max_and_cfs() {
        // E-12: fractional, below one, unlimited.
        assert_eq!(cpu_max(b"max 100000\n"), Some(None));
        assert_eq!(cpu_max(b"150000 100000"), Some(Some((150_000, 100_000))));
        assert_eq!(cpu_max(b"50000 100000"), Some(Some((50_000, 100_000))));
        assert_eq!(cpu_max(b"1 0"), None);
        assert_eq!(cpu_max(b"x 100000"), None);
        assert_eq!(cpu_max(b"1 2 3"), None);
        assert_eq!(cfs(b"-1\n", b"100000\n"), Some(None));
        assert_eq!(cfs(b"200000", b"100000"), Some(Some((200_000, 100_000))));
        assert_eq!(cfs(b"1", b"0"), None);
        assert_eq!(quota_cpus((150_000, 100_000)), 2);
        assert_eq!(quota_cpus((50_000, 100_000)), 1);
        assert_eq!(quota_cpus((0, 100_000)), 1);
        assert_eq!(
            quota_cpus((u64::MAX, 1)),
            usize::try_from(u64::MAX).unwrap_or(usize::MAX)
        );
    }
}
