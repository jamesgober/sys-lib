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

#[test]
fn test_live_counts_are_consistent_everywhere() {
    assert!(cpu::logical() >= 1);
    assert!((1..=cpu::logical()).contains(&cpu::available()));
    assert!((1..=cpu::logical()).contains(&cpu::physical()));
    assert_ne!(cpu::is_64bit(), cpu::is_32bit());
    assert!(!os::name().is_empty() && !os::distro().is_empty() && !os::distro_id().is_empty());
    assert_eq!(process::id(), std::process::id());
}
