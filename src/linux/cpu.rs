//! CPU facts on Linux: present CPUs, cores, the name and vendor, the
//! processor's bitness, and what this process may use (affinity, cpuset, and
//! the cgroup CPU quota, E-12 to E-15).

use super::{Root, cgroup::Cgroups};
use crate::{
    Mhz, Result,
    cpu::{CpuQuota, Frequency, Vendor},
    mem::LimitSource,
    parse::{
        cgroup as cg,
        cpu_list::{CpuSet, MAX_CPUS},
        cpu_name, cpuinfo, parse_u64, proc_status, trim,
    },
};

/// What the file tree says about the CPUs; gaps are filled by the caller.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Facts {
    /// Present CPUs (E-15: not `possible`, which VMs inflate).
    pub(crate) logical: Option<usize>,
    /// Physical cores.
    pub(crate) physical: Option<usize>,
    /// Processor packages (sockets).
    pub(crate) packages: Option<usize>,
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
    let topology = present.as_ref().and_then(|p| cores_from_topology(root, p));
    let physical = topology.map(|(cores, _)| cores).or(info.cores);
    let packages = topology.and_then(|(_, packages)| packages);
    let vendor = match (&info.vendor_id, info.arm_parts.first()) {
        (Some(v), _) => x86_vendor(v),
        (None, Some(&(implementer, _))) => arm_vendor(implementer),
        (None, None) => Vendor::Unknown,
    };
    Facts {
        logical,
        physical: physical.map(|p| p.min(logical.unwrap_or(p)).max(1)),
        packages,
        raw_name: info.model_name.clone(),
        arm_name: cpu_name::arm_name(&info.arm_parts),
        vendor,
        is_64bit: info.is_64bit,
    }
}

/// Thread-sibling groups over the present CPUs: one per core, and the
/// distinct packages those cores sit in. Each group file lists the whole core,
/// so its other members are not read again (half the opens on an SMT
/// machine). `None` when a CPU lacks the files (E-18: topology hidden in a
/// container); the package count is `None` when no `physical_package_id`
/// reads.
fn cores_from_topology(root: &Root, present: &CpuSet) -> Option<(usize, Option<usize>)> {
    let mut covered = CpuSet::empty();
    let mut cores = 0;
    let mut packages: Vec<u64> = Vec::new();
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
        let package = root
            .text(&format!("{base}/physical_package_id"))
            .and_then(|t| parse_u64(t.as_bytes()));
        if let Some(id) = package
            && !packages.contains(&id)
            && packages.len() < MAX_CPUS
        {
            packages.push(id);
        }
    }
    (cores > 0).then_some((cores, (!packages.is_empty()).then_some(packages.len())))
}

/// The CPUs this process may use now: the affinity mask, intersected with the
/// cgroup cpuset, then limited by the cgroup CPU quota; at least 1, at most
/// `logical`.
pub(crate) fn available(root: &Root, cgroups: &Cgroups, logical: usize) -> usize {
    let mut allowed = root
        .with("/proc/self/status", "status", "/proc/self/status", |d| {
            proc_status::field(d, b"Cpus_allowed_list").map(CpuSet::parse)
        })
        .ok()
        .flatten()
        .and_then(Result::ok)
        .unwrap_or_else(|| CpuSet::first(logical));
    if let Some(cpuset) = cpuset(root, cgroups) {
        allowed.intersect(&cpuset);
    }
    let mut count = allowed.count();
    if count == 0 {
        // A list that does not intersect what the kernel reports is not
        // believed; fall back to every CPU rather than to none.
        count = logical;
    }
    if let Some((quota, period, _)) = quota(root, cgroups) {
        count = count.min(cg::quota_cpus((quota, period)));
    }
    count.clamp(1, logical.max(1)).min(MAX_CPUS)
}

/// The cgroup cpuset, if the process is in one that sets it.
fn cpuset(root: &Root, cgroups: &Cgroups) -> Option<CpuSet> {
    let controller = cgroups.cpuset.as_ref()?;
    let dir = &controller.dir;
    let file = if controller.v2 {
        "cpuset.cpus.effective"
    } else {
        "cpuset.effective_cpus"
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

/// The tightest CPU quota over the cgroup and its ancestors, as `(quota,
/// period, from cgroup v2)`.
pub(crate) fn quota(root: &Root, cgroups: &Cgroups) -> Option<(u64, u64, bool)> {
    let controller = cgroups.cpu.as_ref()?;
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
    if controller.v2 {
        for d in controller.dir.ancestors() {
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
    } else {
        for d in controller.dir.ancestors() {
            let quota = root.text(&format!("{d}/cpu.cfs_quota_us"));
            let period = root.text(&format!("{d}/cpu.cfs_period_us"));
            if let (Some(q), Some(p)) = (quota, period)
                && let Some(Some(q)) = cg::cfs(q.as_bytes(), p.as_bytes())
            {
                consider(q);
            }
        }
    }
    tightest.map(|(q, p)| (q, p, controller.v2))
}

/// The CPU quota as the public type: CPUs' worth of time and the source.
pub(crate) fn cpu_quota(root: &Root, cgroups: &Cgroups) -> Option<CpuQuota> {
    let (quota, period, v2) = quota(root, cgroups)?;
    Some(CpuQuota {
        cpus: (quota as f64 / period as f64) as f32,
        source: if v2 {
            LimitSource::Cgroup2
        } else {
            LimitSource::Cgroup1
        },
    })
}

/// The CPUs online now (`/sys/devices/system/cpu/online`).
pub(crate) fn online(root: &Root) -> Option<usize> {
    root.with(
        "/sys/devices/system/cpu/online",
        "online CPUs",
        "/sys/devices/system/cpu/online",
        CpuSet::parse,
    )
    .ok()
    .and_then(Result::ok)
    .map(|s| s.count())
    .filter(|&n| n > 0)
}

/// A cpufreq file's value: kHz to MHz.
fn khz(root: &Root, path: &str) -> Option<Mhz> {
    let khz = root
        .with(
            path,
            "frequency",
            "/sys/devices/system/cpu/cpu*/cpufreq",
            |d| parse_u64(trim(d)),
        )
        .ok()??;
    u32::try_from(khz / 1000).ok().filter(|&m| m > 0).map(Mhz)
}

/// The base and maximum frequency of CPU 0.
pub(crate) fn frequency(root: &Root) -> Frequency {
    Frequency {
        base: khz(root, "/sys/devices/system/cpu/cpu0/cpufreq/base_frequency"),
        max: khz(
            root,
            "/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq",
        ),
    }
}

/// The mean of `scaling_cur_freq` over the online CPUs; `None` without cpufreq
/// (most virtual machines).
pub(crate) fn current_frequency(root: &Root) -> Result<Option<Mhz>> {
    let online = root
        .with(
            "/sys/devices/system/cpu/online",
            "online CPUs",
            "/sys/devices/system/cpu/online",
            CpuSet::parse,
        )
        .ok()
        .and_then(Result::ok)
        .unwrap_or_else(|| CpuSet::first(1));
    let (mut sum, mut n) = (0u64, 0u64);
    for cpu in online.iter() {
        if let Some(m) = khz(
            root,
            &format!("/sys/devices/system/cpu/cpu{cpu}/cpufreq/scaling_cur_freq"),
        ) {
            sum += u64::from(m.0);
            n += 1;
        }
    }
    Ok((n > 0).then(|| Mhz(u32::try_from(sum / n).unwrap_or(u32::MAX))))
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
        let cg = Cgroups::resolve(&root);
        assert_eq!(available(&root, &cg, nproc_all), nproc);
        assert_eq!(quota(&root, &cg), None);
    }

    #[test]
    fn test_linux_cpu_systemd_scope_quota_is_one_and_a_half() {
        // E-12: a fractional quota, nested under ancestors with "max".
        let root = fixtures::root("wsl2-systemd-scope-limited");
        let cg = Cgroups::resolve(&root);
        assert_eq!(quota(&root, &cg), Some((150_000, 100_000, true)));
        let logical = read(&root).logical.unwrap();
        assert_eq!(available(&root, &cg, logical), 2);
        let q = cpu_quota(&root, &cg).unwrap();
        assert_eq!((q.cpus, q.source), (1.5, LimitSource::Cgroup2));
    }

    #[test]
    fn test_linux_cpu_docker_quota_and_cpuset() {
        // E-12: quota 1.5 CPUs and a cpuset of two CPUs; nproc (affinity) says 2.
        let root = fixtures::root("docker-cgroup2-limited");
        let cg = Cgroups::resolve(&root);
        assert_eq!(quota(&root, &cg), Some((150_000, 100_000, true)));
        let logical = read(&root).logical.unwrap();
        assert_eq!(available(&root, &cg, logical), 2);
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
        let cg = Cgroups::resolve(&root);
        assert_eq!(quota(&root, &cg), Some((150_000, 100_000, false)));
        assert_eq!(cpu_quota(&root, &cg).unwrap().source, LimitSource::Cgroup1);
        let logical = read(&root).logical.unwrap();
        assert_eq!(logical, 4);
        assert_eq!(available(&root, &cg, logical), 2);
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
            let cg = Cgroups::resolve(&root);
            let logical = read(&root).logical.unwrap_or(1);
            let a = available(&root, &cg, logical);
            assert!(a >= 1 && a <= logical, "{name}: {a} of {logical}");
            assert_eq!(available(&root, &cg, 0), 1, "{name}");
            if let Some(o) = online(&root) {
                assert!(o >= 1 && o <= logical.max(o), "{name}");
            }
        }
    }

    #[test]
    fn test_linux_cpu_packages_and_frequency_from_fixtures() {
        let f = read(&fixtures::root("wsl2"));
        assert_eq!(f.packages, Some(1));
        let freq = frequency(&fixtures::root("wsl2"));
        // WSL 2 exposes no cpufreq: unknown, never a guess.
        assert_eq!(freq, Frequency::default());
        assert_eq!(current_frequency(&fixtures::root("wsl2")).unwrap(), None);
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
