//! Live comparison: sys-lib's answers on this machine against the OS's own
//! tools (PLAN.md 19.1). Tests may start programs; the library never does.
//! Outside CI a missing tool skips its comparison; on CI it fails, so a runner
//! image change cannot quietly turn a test off.

use std::process::Command;

use sys_lib::{cpu, host, os, process};

/// A tool's trimmed standard output, or `None` when it is missing or fails.
fn tool(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program)
        .args(args)
        // nproc honours these; the comparison is with the affinity mask itself.
        .env_remove("OMP_NUM_THREADS")
        .env_remove("OMP_THREAD_LIMIT")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The tool's output, required on CI.
fn need(program: &str, args: &[&str]) -> Option<String> {
    let value = tool(program, args);
    if value.is_none() {
        assert!(
            std::env::var_os("CI").is_none(),
            "{program} {args:?} is missing on CI"
        );
        eprintln!("skipped: {program} {args:?} is not available here");
    }
    value
}

#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", windows)),
    allow(dead_code)
)]
fn number(text: &str) -> usize {
    text.trim()
        .parse()
        .unwrap_or_else(|_| panic!("not a number: {text:?}"))
}

#[cfg(unix)]
#[test]
fn test_live_unix_kernel_host_and_user_match_the_tools() {
    if let Some(release) = need("uname", &["-r"]) {
        assert_eq!(os::kernel(), release);
    }
    if let Some(node) = need("uname", &["-n"]) {
        assert_eq!(host::name().unwrap(), node);
    }
    let user = process::user().unwrap();
    if let Some(name) = need("id", &["-un"]) {
        assert_eq!(user.name, name);
    }
    if let Some(uid) = need("id", &["-u"]) {
        assert_eq!(user.id, Some(uid.parse().unwrap()));
        assert_eq!(process::is_elevated().unwrap(), uid == "0");
        assert_eq!(user.is_admin, uid == "0");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn test_live_linux_cpu_counts_match_getconf_and_nproc() {
    if let Some(conf) = need("getconf", &["_NPROCESSORS_CONF"]) {
        assert_eq!(cpu::logical(), number(&conf));
    }
    if let Some(nproc) = need("nproc", &[]) {
        // nproc is the affinity mask; available() also applies the cgroup
        // cpuset and CPU quota, so it can only be lower. CI runners set no quota.
        assert!(cpu::available() <= number(&nproc));
        if std::env::var_os("CI").is_some() {
            assert_eq!(cpu::available(), number(&nproc));
        }
    }
    if let Some(machine) = need("uname", &["-m"]) {
        let wide = matches!(
            machine.as_str(),
            "x86_64" | "aarch64" | "ppc64le" | "ppc64" | "s390x" | "riscv64"
        );
        assert_eq!(cpu::is_64bit(), wide, "uname -m says {machine}");
    }
}

#[cfg(target_os = "linux")]
#[test]
fn test_live_linux_distro_matches_a_shell_reading_os_release() {
    // The shell is the reference parser os-release(5) was written for.
    let path = ["/etc/os-release", "/usr/lib/os-release"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
    let Some(path) = path else {
        assert_eq!(os::distro(), "Linux");
        return;
    };
    let script = format!(
        ". {path} && printf '%s\\n' \"$ID\" \"$NAME\" \"$PRETTY_NAME\" \"$VERSION_ID\" \"$ID_LIKE\""
    );
    let Some(out) = need("sh", &["-c", &script]) else {
        return;
    };
    let mut fields = out.split('\n').chain(std::iter::repeat(""));
    let mut next = || fields.next().unwrap().to_owned();
    let (id, name, pretty, version_id, id_like) = (next(), next(), next(), next(), next());
    assert_eq!(os::distro_id(), if id.is_empty() { "linux" } else { &id });
    assert_eq!(os::distro(), if name.is_empty() { "Linux" } else { &name });
    let expected_name = [&pretty, &name]
        .into_iter()
        .find(|s| !s.is_empty())
        .map_or("Linux", |s| s.as_str());
    assert_eq!(os::name(), expected_name);
    assert_eq!(
        os::version().map(|v| v.text),
        (!version_id.is_empty()).then_some(version_id.as_str())
    );
    for like in id_like.split_whitespace() {
        assert!(
            os::is_like(like),
            "is_like({like:?}) with ID_LIKE={id_like:?}"
        );
    }
    assert!(os::is_like(os::distro_id()));
}

#[cfg(target_os = "macos")]
#[test]
fn test_live_macos_matches_sysctl_and_sw_vers() {
    let sysctl = |name: &str| need("sysctl", &["-n", name]);
    if let Some(n) = sysctl("hw.logicalcpu_max") {
        assert_eq!(cpu::logical(), number(&n));
    }
    if let Some(n) = sysctl("hw.physicalcpu_max") {
        assert_eq!(cpu::physical(), number(&n));
    }
    if let Some(n) = sysctl("hw.activecpu") {
        assert_eq!(cpu::available(), number(&n));
    }
    if let Some(brand) = sysctl("machdep.cpu.brand_string") {
        assert_eq!(cpu::raw_name(), Some(brand.as_str()));
    }
    if let Some(v) = need("sw_vers", &["-productVersion"]) {
        assert_eq!(os::version().map(|v| v.text), Some(v.as_str()));
        assert!(
            os::name().starts_with(&format!("macOS {v}")),
            "{}",
            os::name()
        );
    }
    assert_eq!(os::distro(), "macOS");
    assert_eq!(os::distro_id(), "macos");
}

#[cfg(windows)]
#[test]
fn test_live_windows_matches_cim_and_dotnet() {
    let script = "$p = @(Get-CimInstance Win32_Processor); $o = Get-CimInstance Win32_OperatingSystem; \
        $id = [Security.Principal.WindowsIdentity]::GetCurrent(); \
        ($p | Measure-Object NumberOfLogicalProcessors -Sum).Sum; \
        ($p | Measure-Object NumberOfCores -Sum).Sum; \
        $p[0].Name.Trim(); $o.Version; [System.Net.Dns]::GetHostName(); [Environment]::UserName; \
        ([Security.Principal.WindowsPrincipal]$id).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)";
    let Some(out) = need(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    ) else {
        return;
    };
    let lines: Vec<&str> = out.lines().map(str::trim).collect();
    assert_eq!(lines.len(), 7, "{out}");
    assert_eq!(cpu::logical(), number(lines[0]));
    assert_eq!(cpu::physical(), number(lines[1]));
    assert!(cpu::available() <= cpu::logical());
    if std::env::var_os("CI").is_some() {
        assert_eq!(cpu::available(), cpu::logical());
    }
    assert_eq!(cpu::raw_name(), Some(lines[2]));

    // "10.0.26200": major.minor.build; sys-lib's version text adds the update revision.
    let v = os::version().expect("Windows always has a version");
    assert_eq!(format!("{}.{}.{}", v.major, v.minor, v.patch), lines[3]);
    assert!(os::name().starts_with("Windows "), "{}", os::name());
    assert_eq!(os::distro(), "Windows");

    assert!(host::name().unwrap().eq_ignore_ascii_case(lines[4]));
    let user = process::user().unwrap();
    assert!(
        user.name.eq_ignore_ascii_case(lines[5]),
        "{} vs {}",
        user.name,
        lines[5]
    );
    // IsInRole sees the filtered token under UAC, as TokenElevation does.
    assert_eq!(
        process::is_elevated().unwrap(),
        lines[6].eq_ignore_ascii_case("true")
    );
}

/// `a` and `b` agree to within 2% of the larger plus `slack` bytes: memory
/// and free space move between the two readings (PLAN.md 19.1).
#[allow(dead_code)]
fn close(a: u64, b: u64, slack: u64) -> bool {
    a.abs_diff(b) <= a.max(b) / 50 + slack
}

#[allow(dead_code)]
const MIB: u64 = 1 << 20;

#[cfg(target_os = "linux")]
#[test]
fn test_live_linux_memory_disk_and_uptime_match_the_tools() {
    use sys_lib::{disk, mem};
    if let Some(free) = need("free", &["-b"]) {
        let row = |label: &str| -> Vec<u64> {
            free.lines()
                .find(|l| l.starts_with(label))
                .map(|l| {
                    l.split_whitespace()
                        .skip(1)
                        .filter_map(|v| v.parse().ok())
                        .collect()
                })
                .unwrap_or_default()
        };
        let m = mem::snapshot().unwrap();
        let memrow = row("Mem:");
        assert_eq!(m.host_total.raw(), memrow[0], "MemTotal");
        assert!(
            close(m.host_free.raw(), memrow[5], 16 * MIB),
            "available {} vs {}",
            m.host_free,
            memrow[5]
        );
        assert_eq!(m.swap.total.raw(), row("Swap:")[0], "SwapTotal");
    }
    if let Some(df) = need("df", &["-B1", "--output=size,avail", "/"]) {
        let values: Vec<u64> = df
            .lines()
            .nth(1)
            .unwrap()
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect();
        let s = disk::space("/").unwrap();
        assert_eq!(s.total.raw(), values[0], "df size");
        assert!(close(s.available.raw(), values[1], 16 * MIB), "df avail");
    }
    if let Some(online) = need("getconf", &["_NPROCESSORS_ONLN"]) {
        assert_eq!(cpu::online(), number(&online));
    }
    let proc_uptime: f64 = std::fs::read_to_string("/proc/uptime")
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let up = host::uptime().unwrap().as_secs_f64();
    assert!(
        (up - proc_uptime).abs() < 2.0,
        "uptime {up} vs /proc/uptime {proc_uptime}"
    );
}

#[cfg(target_os = "macos")]
#[test]
fn test_live_macos_memory_disk_load_and_uptime_match_the_tools() {
    use sys_lib::{disk, mem};
    if let Some(n) = need("sysctl", &["-n", "hw.memsize"]) {
        assert_eq!(mem::host_total().unwrap().raw(), n.parse::<u64>().unwrap());
    }
    if let Some(df) = need("df", &["-k", "/"]) {
        let blocks: u64 = df
            .lines()
            .nth(1)
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse()
            .unwrap();
        let s = disk::space("/").unwrap();
        assert!(
            close(s.total.raw(), blocks * 1024, 16 * MIB),
            "{} vs {}",
            s.total,
            blocks * 1024
        );
    }
    if let Some(n) = need("sysctl", &["-n", "hw.activecpu"]) {
        assert_eq!(cpu::online(), number(&n));
    }
    if let Some(l) = need("sysctl", &["-n", "vm.loadavg"]) {
        let one: f32 = l
            .trim_matches(|c| c == '{' || c == '}' || c == ' ')
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        let load = cpu::load().unwrap().unwrap();
        assert!((load.one - one).abs() < 1.0, "{} vs {one}", load.one);
    }
    if let Some(b) = need("sysctl", &["-n", "kern.boottime"]) {
        // "{ sec = 1700000000, usec = 123456 } Tue Nov 14 ..."
        let sec: u64 = b
            .split("sec = ")
            .nth(1)
            .unwrap()
            .split(',')
            .next()
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let boot = host::boot_time()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(boot.abs_diff(sec) <= 2, "{boot} vs {sec}");
    }
}

#[cfg(windows)]
#[test]
fn test_live_windows_memory_disk_and_uptime_match_cim_and_dotnet() {
    use sys_lib::{disk, mem};
    let script = "$cs = Get-CimInstance Win32_ComputerSystem; $os = Get-CimInstance Win32_OperatingSystem; \
        $d = [System.IO.DriveInfo]::new($env:SystemDrive); \
        $cs.TotalPhysicalMemory; $os.FreePhysicalMemory; $d.TotalSize; $d.AvailableFreeSpace; \
        [long]((Get-Date) - $os.LastBootUpTime).TotalMilliseconds";
    let Some(out) = need(
        "powershell",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    ) else {
        return;
    };
    let v: Vec<u64> = out.lines().map(|l| l.trim().parse().unwrap()).collect();
    assert_eq!(v.len(), 5, "{out}");
    let m = mem::snapshot().unwrap();
    assert_eq!(m.host_total.raw(), v[0], "TotalPhysicalMemory");
    assert!(
        close(m.host_free.raw(), v[1] * 1024, 64 * MIB),
        "free {} vs {}",
        m.host_free,
        v[1] * 1024
    );
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned()) + "\\";
    let s = disk::space(&drive).unwrap();
    assert_eq!(s.total.raw(), v[2], "drive size");
    assert!(close(s.available.raw(), v[3], 64 * MIB), "drive available");
    // The boot time from CIM (Windows PowerShell 5.1 has no TickCount64).
    // Fast Startup and sleep can separate the two clocks on a desktop, so
    // the comparison is strict only on CI's virtual machines.
    let up = host::uptime().unwrap().as_millis() as u64;
    if std::env::var_os("CI").is_some() {
        assert!(
            up.abs_diff(v[4]) < 5_000,
            "uptime {up} vs LastBootUpTime {}",
            v[4]
        );
    }
}

#[test]
fn test_live_samplers_measure_real_activity() {
    use std::time::Duration;
    // This process burns CPU for 100 ms: its sampler sees most of one core.
    let mut cpu = process::CpuSampler::new().unwrap();
    let start = std::time::Instant::now();
    let mut x = 1u64;
    while start.elapsed() < Duration::from_millis(100) {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
    }
    std::hint::black_box(x);
    let u = cpu.sample().unwrap();
    assert!(
        u.of_one_core > 30.0,
        "a busy loop used {:.1}% of a core",
        u.of_one_core
    );
    // Writing and syncing a file shows up as this process's storage writes
    // where the OS counts them per process.
    let before = process::io().ok();
    let path = std::env::temp_dir().join(format!("sys-lib-live-io-{}", std::process::id()));
    std::fs::write(&path, vec![7u8; 4 << 20]).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .sync_all()
        .unwrap();
    let after = process::io().ok();
    let _ = std::fs::remove_file(&path);
    if let (Some(b), Some(a)) = (before, after) {
        assert!((a - b).written.raw() > 0 || !a.storage_only || cfg!(target_os = "macos"));
    }
}

#[test]
fn test_live_counts_are_consistent_everywhere() {
    assert!(cpu::logical() >= 1);
    assert!((1..=cpu::logical()).contains(&cpu::available()));
    assert!((1..=cpu::logical()).contains(&cpu::physical()));
    assert_ne!(cpu::is_64bit(), cpu::is_32bit());
    assert!(!os::name().is_empty() && !os::distro().is_empty() && !os::distro_id().is_empty());
    assert_eq!(process::id(), std::process::id());
}
