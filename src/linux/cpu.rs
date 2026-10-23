//! CPU facts on Linux: present CPUs, cores, the name and vendor, the
//! processor's bitness, and what this process may use (affinity, cpuset, and
//! the cgroup CPU quota, E-12 to E-15).

use super::{
    Root,
    cgroup::{self, Hierarchy},
};
use crate::{
    cpu::Vendor,
    parse::{
        cgroup as cg,
        cpu_list::{CpuSet, MAX_CPUS},
        cpu_name, cpuinfo, proc_status,
    },
};

/// What the file tree says about the CPUs; gaps are filled by the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// Present CPUs (E-15: not `possible`, which VMs inflate).
    pub(crate) logical: Option<usize>,
    /// Physical cores.
    pub(crate) physical: Option<usize>,
    /// The brand string as reported (x86).
    pub(crate) raw_name: Option<String>,
    /// A name built from ARM core parts.
    pub(crate) arm_name: Option<String>,
    pub(crate) vendor: Vendor,
    pub(crate) is_64bit: Option<bool>,
}

/// Reads the static CPU facts.
pub(crate) fn read(root: &Root) -> Facts {
    let info = root
        .read_prefix(
            "/proc/cpuinfo",
            super::FILE_LIMIT,
            "cpuinfo",
            "/proc/cpuinfo",
        )
        .map(|d| cpuinfo::parse(&d))
        .unwrap_or_default();
    let present = root
        .with(
            "/sys/devices/system/cpu/present",
            "cpus",
            "/sys/devices/system/cpu/present",
            CpuSet::parse,
        )
        .ok()
        .and_then(Result::ok)
        .filter(|s| s.count() > 0);
    let logical = present
        .as_ref()
        .map(CpuSet::count)
        .or((info.processors > 0).then_some(info.processors));
    let physical = present
        .as_ref()
        .and_then(|p| cores_from_topology(root, p))
        .or(info.cores);
    let vendor = match (&info.vendor_id, info.arm_parts.first()) {
        (Some(v), _) => x86_vendor(v),
        (None, Some(&(implementer, _))) => arm_vendor(implementer),
        (None, None) => Vendor::Unknown,
    };
    Facts {
        logical,
        physical: physical.map(|p| p.min(logical.unwrap_or(p)).max(1)),
        raw_name: info.model_name.clone(),
        arm_name: cpu_name::arm_name(&info.arm_parts),
        vendor,
        is_64bit: info.is_64bit,
    }
}

/// Thread-sibling groups over the present CPUs: one per core. Each group file
/// lists the whole core, so its other members are not read again (half the
/// opens on an SMT machine). `None` when a CPU lacks the files (E-18: topology
/// hidden in a container).
fn cores_from_topology(root: &Root, present: &CpuSet) -> Option<usize> {
    let mut covered = CpuSet::empty();
    let mut cores = 0;
    for cpu in present.iter() {
        if covered.contains(cpu) {
            continue;
        }
        let base = format!("/sys/devices/system/cpu/cpu{cpu}/topology");
        let group = ["core_cpus_list", "thread_siblings_list"]
            .iter()
            .find_map(|file| {
                let path = format!("{base}/{file}");
                root.with(
                    &path,
                    "cpu topology",
                    "/sys/devices/system/cpu",
                    CpuSet::parse,
                )
                .ok()?
                .ok()
            })?;
        covered.union(&group);
        // A group that does not list its own CPU still counts that CPU once.
        covered.insert(cpu);
        cores += 1;
    }
    (cores > 0).then_some(cores)
}

/// The CPUs this process may use now: the affinity mask, intersected with the
/// cgroup cpuset, then limited by the cgroup CPU quota; at least 1, at most
/// `logical`.
pub(crate) fn available(root: &Root, logical: usize) -> usize {
    let mut allowed = root
        .with("/proc/self/status", "status", "/proc/self/status", |d| {
            proc_status::field(d, b"Cpus_allowed_list").map(CpuSet::parse)
        })
        .ok()
        .flatten()
        .and_then(Result::ok)
        .unwrap_or_else(|| CpuSet::first(logical));
    if let Some(cpuset) = cpuset(root) {
        allowed.intersect(&cpuset);
    }
    let mut count = allowed.count();
    if count == 0 {
        // A list that does not intersect what the kernel reports is not
        // believed; fall back to every CPU rather than to none.
        count = logical;
    }
    if let Some(quota) = quota(root) {
        count = count.min(cg::quota_cpus(quota));
    }
    count.clamp(1, logical.max(1)).min(MAX_CPUS)
}

/// The cgroup cpuset, if the process is in one that sets it.
fn cpuset(root: &Root) -> Option<CpuSet> {
    let (dir, file) = match cgroup::dir(root, Hierarchy::Unified) {
        Some(d) => (d, "cpuset.cpus.effective"),
        None => (
            cgroup::dir(root, Hierarchy::V1("cpuset"))?,
            "cpuset.effective_cpus",
        ),
    };
    let read = |name: &str| {
        root.with(
            &format!("{}/{name}", dir.leaf),
            "cpuset",
            "cgroup cpuset",
            CpuSet::parse,
        )
        .ok()
        .and_then(Result::ok)
        .filter(|s| s.count() > 0)
    };
    read(file).or_else(|| read("cpuset.cpus"))
}

/// The tightest CPU quota over the cgroup and its ancestors, as `(quota, period)`.
pub(crate) fn quota(root: &Root) -> Option<(u64, u64)> {
    let mut tightest: Option<(u64, u64)> = None;
    let mut consider = |q: (u64, u64)| {
        let tighter = match tightest {
            None => true,
            // q.0 / q.1 < t.0 / t.1, compared without division.
            Some(t) => u128::from(q.0) * u128::from(t.1) < u128::from(t.0) * u128::from(q.1),
        };
        if tighter {
            tightest = Some(q);
        }
    };
    if let Some(dir) = cgroup::dir(root, Hierarchy::Unified) {
        for d in dir.ancestors() {
            let q = root
                .with(
                    &format!("{d}/cpu.max"),
                    "cpu quota",
                    "cgroup cpu.max",
                    cg::cpu_max,
                )
                .ok()
                .flatten();
            if let Some(Some(q)) = q {
                consider(q);
            }
        }
    } else if let Some(dir) = cgroup::dir(root, Hierarchy::V1("cpu")) {
        for d in dir.ancestors() {
            let quota = root.text(&format!("{d}/cpu.cfs_quota_us"));
            let period = root.text(&format!("{d}/cpu.cfs_period_us"));
            if let (Some(q), Some(p)) = (quota, period)
                && let Some(Some(q)) = cg::cfs(q.as_bytes(), p.as_bytes())
            {
                consider(q);
            }
        }
    }
    tightest
}

fn x86_vendor(id: &str) -> Vendor {
    match id {
        "GenuineIntel" => Vendor::Intel,
        "AuthenticAMD" => Vendor::Amd,
        "HygonGenuine" => Vendor::Hygon,
        "CentaurHauls" | "  Shanghai  " => Vendor::Zhaoxin,
        _ => Vendor::Unknown,
    }
}

/// The CPU vendor from an ARM implementer code (MIDR bits 31:24).
pub(crate) fn arm_vendor(implementer: u32) -> Vendor {
    match implementer {
        0x41 => Vendor::Arm,
        0x42 => Vendor::Broadcom,
        0x43 => Vendor::Marvell,
        0x46 => Vendor::Fujitsu,
        0x48 => Vendor::HiSilicon,
        0x4e => Vendor::Nvidia,
        0x51 => Vendor::Qualcomm,
        0x53 => Vendor::Samsung,
        0x61 => Vendor::Apple,
        0x6d => Vendor::Microsoft,
        0xc0 => Vendor::Ampere,
        _ => Vendor::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_linux_cpu_fixture_wsl2_counts_match_the_tools() {
        let root = fixtures::root("wsl2");
        let f = read(&root);
        let nproc_all: usize = fixtures::tool("wsl2", "nproc --all")
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(f.logical, Some(nproc_all));
        let physical = f.physical.unwrap();
        assert!(physical >= 1 && physical <= nproc_all);
        assert!(f.raw_name.is_some());
        assert_ne!(f.vendor, Vendor::Unknown);
        assert_eq!(f.is_64bit, Some(true));
        let nproc: usize = fixtures::tool("wsl2", "nproc").unwrap().parse().unwrap();
        assert_eq!(available(&root, nproc_all), nproc);
        assert_eq!(quota(&root), None);
    }

    #[test]
    fn test_linux_cpu_systemd_scope_quota_is_one_and_a_half() {
        // E-12: a fractional quota, nested under ancestors with "max".
        let root = fixtures::root("wsl2-systemd-scope-limited");
        assert_eq!(quota(&root), Some((150_000, 100_000)));
        let logical = read(&root).logical.unwrap();
        assert_eq!(available(&root, logical), 2);
    }

    #[test]
    fn test_linux_cpu_docker_quota_and_cpuset() {
        // E-12: quota 1.5 CPUs and a cpuset of two CPUs; nproc (affinity) says 2.
        let root = fixtures::root("docker-cgroup2-limited");
        assert_eq!(quota(&root), Some((150_000, 100_000)));
        let logical = read(&root).logical.unwrap();
        assert_eq!(available(&root, logical), 2);
        assert_eq!(
            fixtures::tool("docker-cgroup2-limited", "nproc").as_deref(),
            Some("2")
        );
    }

    #[test]
    fn test_linux_cpu_docker_cgroup_v1_quota_and_cpuset() {
        // E-13, D-027: a real cgroup v1 host (Ubuntu 20.04, hybrid layout) with
        // 4 CPUs; the container has quota 1.5 CPUs and a cpuset of two.
        let name = "docker-cgroup1-limited";
        let (root, log) = fixtures::root(name).recording();
        assert_eq!(quota(&root), Some((150_000, 100_000)));
        let logical = read(&root).logical.unwrap();
        assert_eq!(logical, 4);
        assert_eq!(available(&root, logical), 2);
        let opened = log.lock().unwrap().clone();
        assert!(
            opened
                .iter()
                .any(|p| p == "/sys/fs/cgroup/cpu,cpuacct/cpu.cfs_quota_us"),
            "{opened:?}"
        );
        assert!(
            opened
                .iter()
                .any(|p| p == "/sys/fs/cgroup/cpuset/cpuset.effective_cpus"),
            "{opened:?}"
        );
        assert_eq!(fixtures::tool(name, "nproc").as_deref(), Some("2"));
        assert_eq!(
            fixtures::tool(name, "getconf _NPROCESSORS_CONF").as_deref(),
            Some("4")
        );
    }

    #[test]
    fn test_linux_cpu_available_never_zero_never_above_logical() {
        for name in fixtures::ALL {
            let root = fixtures::root(name);
            let logical = read(&root).logical.unwrap_or(1);
            let a = available(&root, logical);
            assert!(a >= 1 && a <= logical, "{name}: {a} of {logical}");
            assert_eq!(available(&root, 0), 1, "{name}");
        }
    }

    #[test]
    fn test_linux_cpu_vendors() {
        assert_eq!(x86_vendor("GenuineIntel"), Vendor::Intel);
        assert_eq!(x86_vendor("AuthenticAMD"), Vendor::Amd);
        assert_eq!(x86_vendor("  Shanghai  "), Vendor::Zhaoxin);
        assert_eq!(x86_vendor("Weird"), Vendor::Unknown);
        assert_eq!(arm_vendor(0x41), Vendor::Arm);
        assert_eq!(arm_vendor(0x61), Vendor::Apple);
        assert_eq!(arm_vendor(0xff), Vendor::Unknown);
    }
}
