//! Environment and reports on Linux (0.6): temperatures, graphics adapters, and
//! the settings the readiness, benchmark, and security reports read, all from
//! `/sys` and `/proc`.

use std::path::Path;

use super::{Root, disk::MountEntry};
use crate::{
    Bytes, Error, Result,
    bench::BenchFacts,
    gpu::{self, Gpu, GpuKind, GpuVendor},
    parse::pci_ids,
    readiness::{DataDirTuning, Dirty, Fact, Governor, Tuning},
    security::{Encryption, Lockdown, Mac, Posture, Tpm, TpmVersion, VulnStatus, Vulnerability},
    sensors::{self, SensorKind, Temperature},
};

/// The most hwmon devices, thermal zones, DRM cards, or CPUs looked at (T7).
const MAX_DEVICES: usize = 1024;
/// The most sensors kept.
const MAX_SENSORS: usize = 1024;
/// The most files listed in one hwmon directory.
const MAX_FILES: usize = 4096;
/// The most CPU vulnerability files.
const MAX_VULNERABILITIES: usize = 128;
/// The largest settings file read.
const SETTING: usize = 4096;
/// The most of `pci.ids` read: the 2025 file is about 1.4 MB (T7).
const PCI_IDS_LIMIT: usize = 8 << 20;
const PCI_IDS: [&str; 3] = [
    "/usr/share/hwdata/pci.ids",
    "/usr/share/misc/pci.ids",
    "/usr/share/pci.ids",
];

/// A name the kernel lists: never a path.
fn valid(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.starts_with('.')
}

/// `prefix` followed by digits only.
fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// The word in brackets, as the kernel marks the selected one:
/// `always [madvise] never` gives `madvise`.
fn selected(text: &str) -> Option<&str> {
    let start = text.find('[')? + 1;
    let end = start + text[start..].find(']')?;
    Some(&text[start..end])
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// A setting file's text: `Skip` when the kernel has no such file (the feature
/// is not built in), `Unknown` when it cannot be read.
fn setting(root: &Root, path: &str) -> Fact<String> {
    match root.read(path, SETTING, "setting", "a /proc/sys or /sys file") {
        Ok(data) => Fact::Value(crate::parse::lossy(crate::parse::trim(&data), SETTING)),
        Err(Error::NotFound { .. }) => Fact::Skip,
        Err(Error::PermissionDenied { .. }) => Fact::Unknown(format!("permission denied: {path}")),
        Err(e) => Fact::Unknown(format!("{e}: {path}")),
    }
}

fn number(root: &Root, path: &str) -> Fact<u64> {
    match setting(root, path) {
        Fact::Value(text) => match text.parse() {
            Ok(n) => Fact::Value(n),
            Err(_) => Fact::Unknown(format!("not a number in {path}")),
        },
        Fact::Skip => Fact::Skip,
        Fact::Unknown(why) => Fact::Unknown(why),
    }
}

/// Two facts as one: `Skip` if the first is, the first `Unknown` otherwise.
fn both<A, B>(a: Fact<A>, b: Fact<B>) -> Fact<(A, B)> {
    match (a, b) {
        (Fact::Value(a), Fact::Value(b)) => Fact::Value((a, b)),
        (Fact::Skip, _) | (_, Fact::Skip) => Fact::Skip,
        (Fact::Unknown(why), _) | (_, Fact::Unknown(why)) => Fact::Unknown(why),
    }
}

/// The value of a fact, when it has one.
fn value<T>(f: Fact<T>) -> Option<T> {
    match f {
        Fact::Value(v) => Some(v),
        _ => None,
    }
}

fn thp(root: &Root) -> Fact<(String, String)> {
    let word =
        |file: &str| match setting(root, &format!("/sys/kernel/mm/transparent_hugepage/{file}")) {
            Fact::Value(text) => match selected(&text) {
                Some(w) => Fact::Value(w.to_owned()),
                None => Fact::Unknown(format!("no selection in transparent_hugepage/{file}")),
            },
            Fact::Skip => Fact::Skip,
            Fact::Unknown(why) => Fact::Unknown(why),
        };
    both(word("enabled"), word("defrag"))
}

/// The governors of the cpufreq policies, each once, in policy order.
fn governors(root: &Root) -> Option<Governor> {
    let base = "/sys/devices/system/cpu/cpufreq";
    let mut names: Vec<String> = Vec::new();
    for policy in root.dir_names(base, MAX_DEVICES) {
        if !numbered(&policy, "policy") {
            continue;
        }
        if let Some(g) = root.text(&format!("{base}/{policy}/scaling_governor"))
            && !g.is_empty()
            && !names.contains(&g)
        {
            names.push(g);
        }
    }
    if names.is_empty() {
        return None;
    }
    let performance = names.iter().all(|n| n == "performance");
    Some(Governor {
        name: names.join(","),
        performance,
    })
}

pub(crate) fn tuning(root: &Root) -> Tuning {
    let vm = |f: &str| number(root, &format!("/proc/sys/vm/{f}"));
    let dirty = match (
        vm("dirty_ratio"),
        vm("dirty_background_ratio"),
        vm("dirty_bytes"),
        vm("dirty_background_bytes"),
    ) {
        (Fact::Value(ratio), Fact::Value(background_ratio), bytes, background_bytes) => {
            Fact::Value(Dirty {
                ratio,
                background_ratio,
                bytes: value(bytes).unwrap_or(0),
                background_bytes: value(background_bytes).unwrap_or(0),
            })
        }
        (Fact::Unknown(why), ..) | (_, Fact::Unknown(why), ..) => Fact::Unknown(why),
        _ => Fact::Skip,
    };
    // Without CONFIG_NUMA_BALANCING the file is absent: balancing is off.
    let balancing = match number(root, "/proc/sys/kernel/numa_balancing") {
        Fact::Skip => Fact::Value(0),
        f => f,
    };
    let ports = match setting(root, "/proc/sys/net/ipv4/ip_local_port_range") {
        Fact::Value(text) => {
            let mut it = text.split_ascii_whitespace().map(str::parse::<u32>);
            match (it.next(), it.next()) {
                (Some(Ok(first)), Some(Ok(last))) => Fact::Value((first, last)),
                _ => Fact::Unknown("not a range in ip_local_port_range".to_owned()),
            }
        }
        Fact::Skip => Fact::Skip,
        Fact::Unknown(why) => Fact::Unknown(why),
    };
    let backlog = match number(root, "/proc/sys/net/core/somaxconn") {
        Fact::Value(n) => Fact::Value((
            n,
            value(number(root, "/proc/sys/net/ipv4/tcp_max_syn_backlog")),
        )),
        Fact::Skip => Fact::Skip,
        Fact::Unknown(why) => Fact::Unknown(why),
    };
    Tuning {
        thp: thp(root),
        swappiness: vm("swappiness"),
        overcommit: both(vm("overcommit_memory"), vm("overcommit_ratio")),
        max_map_count: vm("max_map_count"),
        dirty,
        numa: both(balancing, vm("zone_reclaim_mode")),
        handles_unlimited: false,
        files_system: number(root, "/proc/sys/fs/file-max"),
        files_process: number(root, "/proc/sys/fs/nr_open"),
        // From getrlimit, in crate::sys::linux.
        memlock: Fact::Skip,
        backlog,
        ports,
        governor: governors(root).map_or(Fact::Skip, Fact::Value),
        clock_source: setting(
            root,
            "/sys/devices/system/clocksource/clocksource0/current_clocksource",
        ),
    }
}

/// How a mount keeps access times, from its per-mount options.
fn atime(options: &str) -> &'static str {
    let has = |o: &str| options.split(',').any(|v| v == o);
    if has("noatime") {
        "noatime"
    } else if has("relatime") {
        "relatime"
    } else {
        "strictatime"
    }
}

/// `file` of the block device `dev` (`major:minor`), or of the disk holding it
/// when it is a partition.
fn block_file(root: &Root, dev: &str, file: &str) -> Option<String> {
    let valid_dev = !dev.is_empty() && dev.bytes().all(|b| b.is_ascii_digit() || b == b':');
    if !valid_dev {
        return None;
    }
    let base = format!("/sys/dev/block/{dev}");
    root.text(&format!("{base}/{file}"))
        .or_else(|| root.text(&format!("{base}/../{file}")))
}

/// The mount holding `path`.
fn mount_of(root: &Root, path: &Path) -> Option<MountEntry> {
    super::disk::holding(root, path).ok().flatten()
}

pub(crate) fn data_dir_tuning(root: &Root, path: &Path) -> DataDirTuning {
    let Some(mount) = mount_of(root, path) else {
        let why = "not found in /proc/self/mountinfo".to_owned();
        return DataDirTuning {
            atime: Fact::Unknown(why.clone()),
            scheduler: Fact::Unknown(why),
        };
    };
    let scheduler = match block_file(root, mount.dev(), "queue/scheduler") {
        Some(text) => match selected(&text) {
            Some(s) => Fact::Value(s.to_owned()),
            // A single scheduler is printed without brackets.
            None if !text.is_empty() && !text.contains(' ') => Fact::Value(text),
            None => Fact::Unknown("no selection in queue/scheduler".to_owned()),
        },
        None => Fact::Unknown("no block device found for the mount".to_owned()),
    };
    DataDirTuning {
        atime: Fact::Value(atime(mount.options()).to_owned()),
        scheduler,
    }
}

// ---------------------------------------------------------------------------
// Benchmark environment
// ---------------------------------------------------------------------------

/// Whether any CPU was throttled since boot, from the thermal counters.
fn throttled(root: &Root) -> Option<bool> {
    let base = "/sys/devices/system/cpu";
    let mut any_counter = false;
    for cpu in root.dir_names(base, MAX_DEVICES * 16) {
        if !numbered(&cpu, "cpu") {
            continue;
        }
        for counter in ["core_throttle_count", "package_throttle_count"] {
            let path = format!("{base}/{cpu}/thermal_throttle/{counter}");
            if let Some(n) = root.text(&path).and_then(|t| t.parse::<u64>().ok()) {
                any_counter = true;
                if n > 0 {
                    return Some(true);
                }
            }
        }
    }
    any_counter.then_some(false)
}

/// The CPUs kept from the scheduler, when there are any.
fn isolated(root: &Root) -> Option<String> {
    let read = |f: &str| {
        root.text(&format!("/sys/devices/system/cpu/{f}"))
            .filter(|t| !t.is_empty() && t != "(null)")
    };
    match (read("isolated"), read("nohz_full")) {
        (Some(i), Some(n)) if i == n => Some(i),
        (Some(i), Some(n)) => Some(format!("{i}, nohz_full {n}")),
        (Some(i), None) => Some(i),
        (None, Some(n)) => Some(format!("nohz_full {n}")),
        (None, None) => None,
    }
}

fn turbo(root: &Root) -> Option<bool> {
    let read = |p: &str| root.text(p);
    match read("/sys/devices/system/cpu/intel_pstate/no_turbo").as_deref() {
        Some("1") => return Some(false),
        Some("0") => return Some(true),
        _ => {}
    }
    match read("/sys/devices/system/cpu/cpufreq/boost").as_deref() {
        Some("1") => Some(true),
        Some("0") => Some(false),
        _ => None,
    }
}

fn aslr(root: &Root) -> Option<bool> {
    value(number(root, "/proc/sys/kernel/randomize_va_space")).map(|n| n != 0)
}

pub(crate) fn bench_facts(root: &Root) -> BenchFacts {
    let governor = governors(root);
    BenchFacts {
        power_saving: governor.as_ref().map(|g| !g.performance),
        governor: governor.map(|g| g.name),
        turbo: turbo(root),
        smt: match root.text("/sys/devices/system/cpu/smt/active").as_deref() {
            Some("1") => Some(true),
            Some("0") => Some(false),
            _ => None,
        },
        clock_source: root.text("/sys/devices/system/clocksource/clocksource0/current_clocksource"),
        throttled: throttled(root),
        isolated_cpus: isolated(root),
        aslr: aslr(root),
    }
}

// ---------------------------------------------------------------------------
// Security posture
// ---------------------------------------------------------------------------

const SECURE_BOOT: &str =
    "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

/// Secure Boot: the variable's fifth byte (after four attribute bytes).
/// Without `/sys/firmware/efi` the machine booted from BIOS, unless a container
/// hides it (`in_container`): then unknown.
fn secure_boot(root: &Root, in_container: bool) -> Option<bool> {
    if !root.exists("/sys/firmware/efi") {
        return (!in_container).then_some(false);
    }
    let mut buf = [0u8; 5];
    match root.read_into(SECURE_BOOT, &mut buf) {
        Ok(5) => Some(buf[4] == 1),
        Ok(_) => None,
        Err(std::io::ErrorKind::NotFound) if root.exists("/sys/firmware/efi/efivars") => {
            Some(false)
        }
        Err(_) => None,
    }
}

fn tpm(root: &Root) -> Option<Tpm> {
    let base = "/sys/class/tpm/tpm0";
    let version = match root.text(&format!("{base}/tpm_version_major")).as_deref() {
        Some("2") => TpmVersion::V2_0,
        Some("1") => TpmVersion::V1_2,
        _ => {
            // Kernels before 5.6: TPM 1.2 devices list their capabilities.
            let caps = root.text(&format!("{base}/caps"))?;
            if caps.contains("TCG version: 1.2") {
                TpmVersion::V1_2
            } else {
                return None;
            }
        }
    };
    Some(Tpm { version })
}

/// Whether the block device `dev`, or any device under it (LVM on LUKS), is a
/// dm-crypt mapping. Walks at most 16 devices, without recursion.
fn encrypted(root: &Root, dev: &str) -> Option<Encryption> {
    let valid_dev = !dev.is_empty() && dev.bytes().all(|b| b.is_ascii_digit() || b == b':');
    if !valid_dev || dev.starts_with("0:") {
        // 0:x is an anonymous device (btrfs subvolumes, overlay): no block
        // device to follow.
        return None;
    }
    let mut pending = vec![format!("/sys/dev/block/{dev}")];
    let mut seen = 0;
    while let Some(dir) = pending.pop() {
        seen += 1;
        if seen > 16 {
            return None;
        }
        if root
            .text(&format!("{dir}/dm/uuid"))
            .is_some_and(|u| u.starts_with("CRYPT-"))
        {
            return Some(Encryption::Enabled { kind: "LUKS" });
        }
        for slave in root.dir_names(&format!("{dir}/slaves"), 16) {
            if valid(&slave) {
                pending.push(format!("/sys/block/{slave}"));
            }
        }
    }
    root.exists(&format!("/sys/dev/block/{dev}"))
        .then_some(Encryption::Disabled)
}

fn mandatory_access_control(root: &Root) -> Option<Mac> {
    match root.text("/sys/fs/selinux/enforce").as_deref() {
        Some("1") => {
            return Some(Mac {
                kind: "SELinux",
                enforcing: true,
            });
        }
        Some("0") => {
            return Some(Mac {
                kind: "SELinux",
                enforcing: false,
            });
        }
        _ => {}
    }
    // AppArmor has no system-wide permissive mode: enabled means profiles
    // are enforced (each may still be in complain mode).
    (root
        .text("/sys/module/apparmor/parameters/enabled")
        .as_deref()
        == Some("Y"))
    .then_some(Mac {
        kind: "AppArmor",
        enforcing: true,
    })
}

fn lockdown(root: &Root) -> Option<Lockdown> {
    let text = root.text("/sys/kernel/security/lockdown")?;
    match selected(&text)? {
        "none" => Some(Lockdown::Off),
        "integrity" => Some(Lockdown::Integrity),
        "confidentiality" => Some(Lockdown::Confidentiality),
        _ => None,
    }
}

fn vulnerabilities(root: &Root) -> Vec<Vulnerability> {
    let base = "/sys/devices/system/cpu/vulnerabilities";
    let mut out = Vec::new();
    for name in root.dir_names(base, MAX_VULNERABILITIES) {
        if !valid(&name) {
            continue;
        }
        if let Some(detail) = root.text(&format!("{base}/{name}")) {
            out.push(Vulnerability {
                status: VulnStatus::of(&detail),
                name,
                detail,
            });
        }
    }
    out
}

pub(crate) fn posture(root: &Root, in_container: bool) -> Posture {
    let root_dev = mount_of(root, Path::new("/")).map(|m| m.dev().to_owned());
    Posture {
        secure_boot: secure_boot(root, in_container),
        tpm: tpm(root),
        disk_encryption: root_dev.and_then(|d| encrypted(root, &d)),
        mandatory_access_control: mandatory_access_control(root),
        aslr: aslr(root),
        kernel_lockdown: lockdown(root),
        code_integrity: None,
        // nftables and iptables state is readable by root only.
        firewall: None,
        cpu_vulnerabilities: vulnerabilities(root),
        elevated: None,
    }
}

// ---------------------------------------------------------------------------
// Temperatures
// ---------------------------------------------------------------------------

fn sensor_kind(name: &str) -> SensorKind {
    let n = name.to_ascii_lowercase();
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| n.starts_with(p));
    if starts(&[
        "coretemp",
        "k10temp",
        "k8temp",
        "zenpower",
        "x86_pkg_temp",
        "cpu",
        "soc",
        "apple_smc_cpu",
    ]) {
        SensorKind::Cpu
    } else if starts(&["nvme", "drivetemp", "sata", "hdd"]) {
        SensorKind::Storage
    } else if starts(&["amdgpu", "radeon", "nouveau", "i915", "xe", "gpu", "nvidia"]) {
        SensorKind::Gpu
    } else if starts(&["bat", "battery"]) {
        SensorKind::Battery
    } else if starts(&[
        "acpitz", "pch_", "thinkpad", "nct", "it87", "f71", "w83", "board", "ambient",
    ]) {
        SensorKind::Ambient
    } else {
        SensorKind::Other
    }
}

/// Millidegrees as degrees, when plausible.
fn millidegrees(text: Option<String>) -> Option<f32> {
    #[allow(clippy::cast_precision_loss)] // sensor values are far below 2^24
    let c = text?.parse::<i64>().ok()? as f32 / 1000.0;
    sensors::plausible(c).then_some(c)
}

/// The sensors of one hwmon device, from the directory that holds its files.
fn hwmon(root: &Root, dir: &str, name: &str, out: &mut Vec<Temperature>) {
    let kind = sensor_kind(name);
    let mut inputs: Vec<String> = root
        .dir_names(dir, MAX_FILES)
        .into_iter()
        .filter(|f| {
            f.strip_prefix("temp")
                .and_then(|r| r.strip_suffix("_input"))
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    // temp2 before temp10.
    inputs.sort_by_key(|f| f[4..f.len() - 6].parse::<u32>().unwrap_or(u32::MAX));
    let several = inputs.len() > 1;
    for input in inputs {
        if out.len() >= MAX_SENSORS {
            return;
        }
        let stem = &input[..input.len() - 6];
        let Some(celsius) = millidegrees(root.text(&format!("{dir}/{input}"))) else {
            continue;
        };
        let label = root
            .text(&format!("{dir}/{stem}_label"))
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| {
                if several {
                    format!("{name} {}", &stem[4..])
                } else {
                    name.to_owned()
                }
            });
        out.push(Temperature {
            label,
            kind,
            celsius,
            high: millidegrees(root.text(&format!("{dir}/{stem}_max"))),
            critical: millidegrees(root.text(&format!("{dir}/{stem}_crit"))),
        });
    }
}

fn thermal_zones(root: &Root, out: &mut Vec<Temperature>) {
    let base = "/sys/class/thermal";
    for zone in root.dir_names(base, MAX_DEVICES) {
        if !numbered(&zone, "thermal_zone") || out.len() >= MAX_SENSORS {
            continue;
        }
        let dir = format!("{base}/{zone}");
        let Some(celsius) = millidegrees(root.text(&format!("{dir}/temp"))) else {
            continue;
        };
        let label = root
            .text(&format!("{dir}/type"))
            .unwrap_or_else(|| zone.clone());
        let (mut high, mut critical) = (None, None);
        for trip in 0..32 {
            let Some(kind) = root.text(&format!("{dir}/trip_point_{trip}_type")) else {
                break;
            };
            let t = millidegrees(root.text(&format!("{dir}/trip_point_{trip}_temp")));
            match kind.as_str() {
                "critical" => critical = critical.or(t),
                "hot" => high = high.or(t),
                _ => {}
            }
        }
        out.push(Temperature {
            kind: sensor_kind(&label),
            label,
            celsius,
            high,
            critical,
        });
    }
}

pub(crate) fn temperatures(root: &Root) -> Result<Vec<Temperature>> {
    let base = "/sys/class/hwmon";
    let mut out = Vec::new();
    for dev in root.dir_names(base, MAX_DEVICES) {
        if !numbered(&dev, "hwmon") {
            continue;
        }
        let dir = format!("{base}/{dev}");
        // Older drivers keep their files under `device/`.
        let (dir, name) = match root.text(&format!("{dir}/name")) {
            Some(n) => (dir, n),
            None => match root.text(&format!("{dir}/device/name")) {
                Some(n) => (format!("{dir}/device"), n),
                None => continue,
            },
        };
        hwmon(root, &dir, &name, &mut out);
    }
    if out.is_empty() {
        thermal_zones(root, &mut out);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Graphics adapters
// ---------------------------------------------------------------------------

/// `0x10de` as a number.
fn pci_id(text: Option<String>) -> Option<u16> {
    let t = text?;
    u16::from_str_radix(t.strip_prefix("0x").unwrap_or(&t), 16).ok()
}

/// The vendor and kind of a GPU with no PCI ids, from its driver.
fn platform_gpu(driver: &str) -> Option<(GpuVendor, GpuKind)> {
    Some(match driver {
        "panfrost" | "panthor" | "lima" | "mali" => (GpuVendor::Arm, GpuKind::Integrated),
        "msm" | "msm_drm" => (GpuVendor::Qualcomm, GpuKind::Integrated),
        "asahi" => (GpuVendor::Apple, GpuKind::Integrated),
        "vc4" | "v3d" | "etnaviv" | "rockchip" | "mediatek" | "tegra" | "nouveau" => {
            (GpuVendor::Other, GpuKind::Integrated)
        }
        "virtio_gpu" | "virtio-gpu" => (GpuVendor::Other, GpuKind::Virtual),
        // Firmware framebuffers: a display, not a GPU.
        _ => return None,
    })
}

/// A readable name from `pci.ids`: the marketing name in brackets when there
/// is one, after the short vendor name.
fn display_name(vendor: GpuVendor, vendor_name: Option<&str>, device_name: &str) -> String {
    let device = match (device_name.find('['), device_name.rfind(']')) {
        (Some(a), Some(b)) if a < b => &device_name[a + 1..b],
        _ => device_name,
    };
    let vendor = match vendor {
        GpuVendor::Other => vendor_name.unwrap_or("Unknown"),
        v => v.name(),
    };
    format!("{vendor} {device}")
}

/// Looks up the names for each pair of ids in one pass per GPU.
fn pci_names(root: &Root, vendor: u16, device: u16) -> (Option<String>, Option<String>) {
    for path in PCI_IDS {
        let mut lookup = pci_ids::Lookup::new(vendor, device);
        let read = root.for_each_line(path, PCI_IDS_LIMIT, "pci.ids", "pci.ids", |line| {
            lookup.line(line)
        });
        if read.is_ok() {
            return (lookup.vendor_name, lookup.device_name);
        }
    }
    (None, None)
}

pub(crate) fn gpus(root: &Root) -> Result<Vec<Gpu>> {
    let base = "/sys/class/drm";
    let mut out = Vec::new();
    for card in root.dir_names(base, MAX_DEVICES) {
        if !numbered(&card, "card") {
            continue;
        }
        let dev = format!("{base}/{card}/device");
        let uevent = root.text(&format!("{dev}/uevent")).unwrap_or_default();
        let driver = uevent
            .lines()
            .find_map(|l| l.strip_prefix("DRIVER="))
            .map(str::to_owned);
        let vendor_id = pci_id(root.text(&format!("{dev}/vendor")));
        let device_id = pci_id(root.text(&format!("{dev}/device")));
        let memory = root
            .text(&format!("{dev}/mem_info_vram_total"))
            .and_then(|t| t.parse::<u64>().ok())
            .filter(|&b| b > 0)
            .map(Bytes::new);
        let (vendor, kind, name) = match (vendor_id, device_id) {
            (Some(v), Some(d)) => {
                let vendor = GpuVendor::from_pci(v);
                let kind = match v {
                    // QEMU's standard VGA, virtio, VirtualBox, Hyper-V, VMware.
                    0x1234 | 0x1af4 | 0x80ee | 0x1414 | 0x15ad => GpuKind::Virtual,
                    _ => gpu::kind_of(vendor),
                };
                let (vendor_name, device_name) = pci_names(root, v, d);
                let name = match device_name {
                    Some(dn) => display_name(vendor, vendor_name.as_deref(), &dn),
                    None => format!(
                        "{} GPU 0x{d:04x}",
                        match vendor {
                            GpuVendor::Other => vendor_name.as_deref().unwrap_or("Unknown"),
                            v => v.name(),
                        }
                    ),
                };
                (vendor, kind, name)
            }
            _ => {
                let Some((vendor, kind)) = driver.as_deref().and_then(platform_gpu) else {
                    continue;
                };
                let name = format!(
                    "{} GPU ({})",
                    vendor.name(),
                    driver.as_deref().unwrap_or("")
                );
                (vendor, kind, name)
            }
        };
        out.push(Gpu {
            name,
            vendor,
            kind,
            vendor_id,
            device_id,
            memory,
            driver,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::{fixtures, hostile::Tree};

    #[test]
    fn test_report_selected_and_atime() {
        assert_eq!(selected("always [madvise] never"), Some("madvise"));
        assert_eq!(selected("[none] mq-deadline kyber"), Some("none"));
        assert_eq!(selected("none"), None);
        assert_eq!(selected("[broken"), None);
        assert_eq!(atime("rw,noatime"), "noatime");
        assert_eq!(atime("rw,relatime,discard"), "relatime");
        assert_eq!(atime("rw"), "strictatime");
    }

    #[test]
    fn test_report_tuning_from_files() {
        let t = Tree::new("report-tuning");
        t.file(
            "/sys/kernel/mm/transparent_hugepage/enabled",
            "always [madvise] never\n",
        );
        t.file(
            "/sys/kernel/mm/transparent_hugepage/defrag",
            "always defer [madvise] never\n",
        );
        t.file("/proc/sys/vm/swappiness", "60\n");
        t.file("/proc/sys/vm/overcommit_memory", "0\n");
        t.file("/proc/sys/vm/overcommit_ratio", "50\n");
        t.file("/proc/sys/vm/max_map_count", "65530\n");
        t.file("/proc/sys/vm/dirty_ratio", "20\n");
        t.file("/proc/sys/vm/dirty_background_ratio", "10\n");
        t.file("/proc/sys/vm/dirty_bytes", "0\n");
        t.file("/proc/sys/vm/dirty_background_bytes", "0\n");
        t.file("/proc/sys/vm/zone_reclaim_mode", "0\n");
        t.file("/proc/sys/fs/file-max", "9223372036854775807\n");
        t.file("/proc/sys/net/core/somaxconn", "4096\n");
        t.file("/proc/sys/net/ipv4/tcp_max_syn_backlog", "512\n");
        t.file("/proc/sys/net/ipv4/ip_local_port_range", "32768\t60999\n");
        t.file(
            "/sys/devices/system/cpu/cpufreq/policy0/scaling_governor",
            "powersave\n",
        );
        t.file(
            "/sys/devices/system/cpu/cpufreq/policy1/scaling_governor",
            "performance\n",
        );
        t.file(
            "/sys/devices/system/cpu/cpufreq/policy2/scaling_governor",
            "powersave\n",
        );
        t.file(
            "/sys/devices/system/clocksource/clocksource0/current_clocksource",
            "tsc\n",
        );
        let tuning = tuning(&t.root());
        assert_eq!(
            tuning.thp,
            Fact::Value(("madvise".into(), "madvise".into()))
        );
        assert_eq!(tuning.swappiness, Fact::Value(60));
        assert_eq!(tuning.overcommit, Fact::Value((0, 50)));
        assert_eq!(tuning.max_map_count, Fact::Value(65530));
        assert_eq!(
            tuning.dirty,
            Fact::Value(Dirty {
                ratio: 20,
                background_ratio: 10,
                bytes: 0,
                background_bytes: 0
            })
        );
        // numa_balancing absent: off.
        assert_eq!(tuning.numa, Fact::Value((0, 0)));
        assert_eq!(tuning.files_system, Fact::Value(9_223_372_036_854_775_807));
        assert_eq!(tuning.files_process, Fact::Skip);
        assert_eq!(tuning.backlog, Fact::Value((4096, Some(512))));
        assert_eq!(tuning.ports, Fact::Value((32768, 60999)));
        assert_eq!(
            tuning.governor,
            Fact::Value(Governor {
                name: "powersave,performance".into(),
                performance: false
            })
        );
        assert_eq!(tuning.clock_source, Fact::Value("tsc".into()));
    }

    #[test]
    fn test_report_tuning_malformed_is_unknown() {
        let t = Tree::new("report-tuning-bad");
        t.file("/proc/sys/vm/swappiness", "sixty\n");
        t.file("/proc/sys/net/ipv4/ip_local_port_range", "32768\n");
        t.file(
            "/sys/kernel/mm/transparent_hugepage/enabled",
            "always madvise never\n",
        );
        t.file("/sys/kernel/mm/transparent_hugepage/defrag", "[never]\n");
        let tuning = tuning(&t.root());
        assert!(matches!(tuning.swappiness, Fact::Unknown(_)));
        assert!(matches!(tuning.ports, Fact::Unknown(_)));
        assert!(matches!(tuning.thp, Fact::Unknown(_)));
        assert_eq!(tuning.governor, Fact::Skip);
        assert_eq!(tuning.max_map_count, Fact::Skip);
    }

    #[test]
    fn test_report_bench_facts() {
        let t = Tree::new("report-bench");
        t.file(
            "/sys/devices/system/cpu/cpufreq/policy0/scaling_governor",
            "performance\n",
        );
        t.file("/sys/devices/system/cpu/intel_pstate/no_turbo", "0\n");
        t.file("/sys/devices/system/cpu/smt/active", "1\n");
        t.file(
            "/sys/devices/system/cpu/cpu0/thermal_throttle/core_throttle_count",
            "0\n",
        );
        t.file(
            "/sys/devices/system/cpu/cpu1/thermal_throttle/core_throttle_count",
            "3\n",
        );
        t.file("/sys/devices/system/cpu/isolated", "2-3\n");
        t.file("/sys/devices/system/cpu/nohz_full", "(null)\n");
        t.file("/proc/sys/kernel/randomize_va_space", "2\n");
        let b = bench_facts(&t.root());
        assert_eq!(b.governor.as_deref(), Some("performance"));
        assert_eq!(b.power_saving, Some(false));
        assert_eq!(b.turbo, Some(true));
        assert_eq!(b.smt, Some(true));
        assert_eq!(b.throttled, Some(true));
        assert_eq!(b.isolated_cpus.as_deref(), Some("2-3"));
        assert_eq!(b.aslr, Some(true));
        let empty = bench_facts(&Tree::new("report-bench-empty").root());
        assert_eq!(empty, BenchFacts::default());
    }

    // Device numbers (`253:1`) are not valid file names on Windows.
    #[cfg(unix)]
    #[test]
    fn test_report_posture() {
        let t = Tree::new("report-posture");
        let mut var = vec![0x06, 0, 0, 0, 1];
        t.bytes(SECURE_BOOT, &var);
        t.file("/sys/class/tpm/tpm0/tpm_version_major", "2\n");
        t.file("/sys/fs/selinux/enforce", "0\n");
        t.file("/proc/sys/kernel/randomize_va_space", "0\n");
        t.file(
            "/sys/kernel/security/lockdown",
            "none [integrity] confidentiality\n",
        );
        t.file(
            "/sys/devices/system/cpu/vulnerabilities/spectre_v2",
            "Mitigation: Enhanced / Automatic IBRS\n",
        );
        t.file(
            "/sys/devices/system/cpu/vulnerabilities/meltdown",
            "Not affected\n",
        );
        t.file(
            "/proc/self/mountinfo",
            "29 1 253:1 / / rw,relatime shared:1 - ext4 /dev/mapper/vg-root rw\n",
        );
        t.file("/sys/dev/block/253:1/dm/uuid", "LVM-abc\n");
        t.mkdir("/sys/dev/block/253:1/slaves/dm-0");
        t.file("/sys/block/dm-0/dm/uuid", "CRYPT-LUKS2-0123-root\n");
        let p = posture(&t.root(), false);
        assert_eq!(p.secure_boot, Some(true));
        assert_eq!(
            p.tpm,
            Some(Tpm {
                version: TpmVersion::V2_0
            })
        );
        assert_eq!(
            p.disk_encryption,
            Some(Encryption::Enabled { kind: "LUKS" })
        );
        assert_eq!(
            p.mandatory_access_control,
            Some(Mac {
                kind: "SELinux",
                enforcing: false
            })
        );
        assert_eq!(p.aslr, Some(false));
        assert_eq!(p.kernel_lockdown, Some(Lockdown::Integrity));
        assert_eq!(p.cpu_vulnerabilities.len(), 2);
        assert_eq!(p.cpu_vulnerabilities[0].name, "meltdown");
        assert_eq!(p.cpu_vulnerabilities[0].status, VulnStatus::NotAffected);
        assert_eq!(p.cpu_vulnerabilities[1].status, VulnStatus::Mitigated);
        var[4] = 0;
        t.bytes(SECURE_BOOT, &var);
        assert_eq!(posture(&t.root(), false).secure_boot, Some(false));
    }

    // Device numbers (`253:1`) are not valid file names on Windows.
    #[cfg(unix)]
    #[test]
    fn test_report_posture_bios_container_and_plain_disk() {
        let t = Tree::new("report-posture-bios");
        t.file(
            "/proc/self/mountinfo",
            "29 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw\n",
        );
        t.mkdir("/sys/dev/block/8:2");
        let p = posture(&t.root(), false);
        assert_eq!(p.secure_boot, Some(false));
        assert_eq!(p.disk_encryption, Some(Encryption::Disabled));
        assert_eq!(p.tpm, None);
        assert_eq!(posture(&t.root(), true).secure_boot, None);
        t.file("/sys/module/apparmor/parameters/enabled", "Y\n");
        assert_eq!(
            posture(&t.root(), false)
                .mandatory_access_control
                .map(|m| m.kind),
            Some("AppArmor")
        );
    }

    // Device numbers (`253:1`) are not valid file names on Windows.
    #[cfg(unix)]
    #[test]
    fn test_report_data_dir() {
        let t = Tree::new("report-datadir");
        t.file(
            "/proc/self/mountinfo",
            "29 1 259:2 / / rw,relatime shared:1 - ext4 /dev/nvme0n1p2 rw\n\
             30 29 259:3 / /var/lib rw,noatime shared:2 - xfs /dev/nvme0n1p3 rw\n",
        );
        t.file("/sys/dev/block/259:3/partition", "3\n");
        t.file(
            "/sys/dev/block/259:3/../queue/scheduler",
            "[none] mq-deadline\n",
        );
        let d = data_dir_tuning(&t.root(), Path::new("/var/lib/hive"));
        assert_eq!(d.atime, Fact::Value("noatime".into()));
        let d = data_dir_tuning(&t.root(), Path::new("/home"));
        assert_eq!(d.atime, Fact::Value("relatime".into()));
        assert!(matches!(d.scheduler, Fact::Unknown(_)));
    }

    #[test]
    fn test_report_temperatures_hwmon_and_zones() {
        let t = Tree::new("report-temps");
        t.file("/sys/class/hwmon/hwmon0/name", "coretemp\n");
        t.file("/sys/class/hwmon/hwmon0/temp1_input", "45000\n");
        t.file("/sys/class/hwmon/hwmon0/temp1_label", "Package id 0\n");
        t.file("/sys/class/hwmon/hwmon0/temp1_max", "80000\n");
        t.file("/sys/class/hwmon/hwmon0/temp1_crit", "100000\n");
        t.file("/sys/class/hwmon/hwmon0/temp10_input", "41000\n");
        t.file("/sys/class/hwmon/hwmon0/temp2_input", "43000\n");
        t.file("/sys/class/hwmon/hwmon1/device/name", "nvme\n");
        t.file("/sys/class/hwmon/hwmon1/device/temp1_input", "38850\n");
        // An absent sensor's placeholder.
        t.file("/sys/class/hwmon/hwmon2/name", "acpitz\n");
        t.file("/sys/class/hwmon/hwmon2/temp1_input", "-273150\n");
        t.file("/sys/class/thermal/thermal_zone0/type", "x86_pkg_temp\n");
        t.file("/sys/class/thermal/thermal_zone0/temp", "50000\n");
        let list = temperatures(&t.root()).unwrap();
        let labels: Vec<&str> = list.iter().map(|t| t.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Package id 0", "coretemp 2", "coretemp 10", "nvme"]
        );
        assert_eq!(list[0].kind, SensorKind::Cpu);
        assert_eq!(list[0].critical, Some(100.0));
        assert_eq!(list[3].kind, SensorKind::Storage);
        assert!((list[3].celsius - 38.85).abs() < 0.001);

        // No hwmon: the thermal zones.
        let z = Tree::new("report-zones");
        z.file("/sys/class/thermal/thermal_zone0/type", "acpitz\n");
        z.file("/sys/class/thermal/thermal_zone0/temp", "27800\n");
        z.file(
            "/sys/class/thermal/thermal_zone0/trip_point_0_type",
            "critical\n",
        );
        z.file(
            "/sys/class/thermal/thermal_zone0/trip_point_0_temp",
            "105000\n",
        );
        let list = temperatures(&z.root()).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, SensorKind::Ambient);
        assert_eq!(list[0].critical, Some(105.0));
    }

    #[test]
    fn test_report_gpus() {
        let t = Tree::new("report-gpus");
        t.file("/sys/class/drm/card0/device/vendor", "0x10de\n");
        t.file("/sys/class/drm/card0/device/device", "0x2786\n");
        t.file(
            "/sys/class/drm/card0/device/uevent",
            "DRIVER=nvidia\nPCI_SLOT_NAME=0000:01:00.0\n",
        );
        t.file("/sys/class/drm/card0-HDMI-A-1/status", "connected\n");
        t.file("/sys/class/drm/card1/device/vendor", "0x1002\n");
        t.file("/sys/class/drm/card1/device/device", "0x744c\n");
        t.file("/sys/class/drm/card1/device/uevent", "DRIVER=amdgpu\n");
        t.file(
            "/sys/class/drm/card1/device/mem_info_vram_total",
            "25753026560\n",
        );
        t.file("/sys/class/drm/card2/device/vendor", "0x1234\n");
        t.file("/sys/class/drm/card2/device/device", "0x1111\n");
        t.file("/sys/class/drm/card3/device/uevent", "DRIVER=simpledrm\n");
        t.file("/sys/class/drm/card4/device/uevent", "DRIVER=panfrost\n");
        t.file(
            "/usr/share/hwdata/pci.ids",
            "1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\
             \t744c  Navi 31 [Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M]\n\
             10de  NVIDIA Corporation\n\
             \t2786  AD104 [GeForce RTX 4070]\n",
        );
        let list = gpus(&t.root()).unwrap();
        assert_eq!(list.len(), 4, "{list:?}");
        assert_eq!(list[0].name, "NVIDIA GeForce RTX 4070");
        assert_eq!(list[0].kind, GpuKind::Discrete);
        assert_eq!(list[0].driver.as_deref(), Some("nvidia"));
        assert_eq!(
            list[1].name,
            "AMD Radeon RX 7900 XT/7900 XTX/7900 GRE/7900M"
        );
        assert_eq!(list[1].memory, Some(Bytes::new(25_753_026_560)));
        assert_eq!(list[2].name, "Other GPU 0x1111".replace("Other", "Unknown"));
        assert_eq!(list[2].kind, GpuKind::Virtual);
        assert_eq!(list[3].vendor, GpuVendor::Arm);
        assert_eq!(list[3].device_id, None);
    }

    #[test]
    fn test_report_fixtures_never_fail() {
        for name in fixtures::ALL {
            let root = fixtures::root(name);
            let _ = tuning(&root);
            let _ = bench_facts(&root);
            let _ = posture(&root, true);
            assert!(temperatures(&root).is_ok(), "{name}");
            assert!(gpus(&root).is_ok(), "{name}");
        }
    }
}
