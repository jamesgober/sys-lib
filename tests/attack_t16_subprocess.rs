//! Threat T16: subprocess hijacking. Libraries that fall back to running
//! `lsb_release`, `wmic`, or `sysctl` can be steered by a fake program earlier
//! on `PATH`. sys-lib starts no program at run time. This test plants fakes for
//! every tool such a fallback might use, each a link to this very binary that
//! records its own start in a marker file, then reads every fact with `PATH`
//! holding only the fakes. The marker must stay empty.
//!
//! A positive control runs one fake first, so a broken marker cannot pass.

use std::{
    ffi::OsStr,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

const ROLE: &str = "SYS_LIB_T16_ROLE";
const MARKER: &str = "SYS_LIB_T16_MARKER";

/// Programs a system library might be tempted to run, on any platform.
const FAKES: [&str; 26] = [
    "lsb_release",
    "uname",
    "hostname",
    "nproc",
    "getconf",
    "lscpu",
    "cat",
    "sh",
    "bash",
    "id",
    "whoami",
    "systemd-detect-virt",
    "dmidecode",
    "sw_vers",
    "sysctl",
    "system_profiler",
    "ioreg",
    "scutil",
    "wmic",
    "powershell",
    "pwsh",
    "reg",
    "systeminfo",
    "cmd",
    "git",
    "rustc",
];

fn main() -> ExitCode {
    // Started as one of the fakes: record it and stop.
    let argv0 = std::env::args_os().next().unwrap_or_default();
    let started_as = Path::new(&argv0)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or("");
    if FAKES.contains(&started_as) {
        if let Some(marker) = std::env::var_os(MARKER) {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(marker)
                .unwrap();
            writeln!(f, "{started_as}").unwrap();
        }
        return ExitCode::SUCCESS;
    }
    match std::env::var(ROLE).as_deref() {
        Ok("child") => {
            read_everything();
            println!("READ-ALL-DONE");
            ExitCode::SUCCESS
        }
        _ => {
            parent();
            println!("test test_attack_t16_no_program_is_started ... ok");
            ExitCode::SUCCESS
        }
    }
}

/// Every reading 0.3 offers, through the public API.
fn read_everything() {
    use sys_lib::{cpu, host, os, process};
    let _ = (
        os::info(),
        os::name(),
        os::version(),
        os::kernel(),
        os::distro(),
        os::distro_id(),
    );
    let _ = (os::is_like("debian"), os::os_release());
    let _ = (
        cpu::logical(),
        cpu::physical(),
        cpu::available(),
        cpu::name(),
        cpu::raw_name(),
    );
    let _ = (cpu::vendor(), cpu::is_64bit(), cpu::is_32bit(), cpu::arch());
    let _ = (
        host::name(),
        process::id(),
        process::user(),
        process::is_elevated(),
    );
    let _ = sys_lib::build_info!();
    #[cfg(feature = "instance")]
    let _ = sys_lib::Instance::start().to_string();
    #[cfg(feature = "guard")]
    {
        let dir = std::env::var_os("SYS_LIB_T16_LOCK_DIR").expect("lock dir");
        let _ = process::Guard::builder("t16")
            .lock_dir(dir)
            .keep_umask()
            .build();
    }
}

fn plant_fakes(dir: &Path) {
    let me = std::env::current_exe().unwrap();
    fs::create_dir_all(dir).unwrap();
    for name in FAKES {
        let fake = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let _ = fs::remove_file(&fake);
        // A hard link costs no space; the target directory is one filesystem.
        if fs::hard_link(&me, &fake).is_err() {
            fs::copy(&me, &fake).unwrap();
        }
    }
}

fn parent() {
    let work =
        PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("t16-{}", std::process::id()));
    let _ = fs::remove_dir_all(&work);
    let (fakes, marker, lock_dir) = (work.join("bin"), work.join("marker"), work.join("lock"));
    plant_fakes(&fakes);

    // Positive control: a program run by name from this PATH is a fake and is recorded.
    let status = Command::new("lsb_release")
        .arg("-a")
        .env("PATH", &fakes)
        .env(MARKER, &marker)
        .status()
        .expect("the planted lsb_release runs");
    assert!(status.success());
    assert_eq!(fs::read_to_string(&marker).unwrap().trim(), "lsb_release");
    fs::remove_file(&marker).unwrap();

    let out = Command::new(std::env::current_exe().unwrap())
        .env("PATH", &fakes)
        .env(ROLE, "child")
        .env(MARKER, &marker)
        .env("SYS_LIB_T16_LOCK_DIR", &lock_dir)
        .output()
        .unwrap();
    assert!(out.status.success(), "child failed: {out:?}");
    assert!(String::from_utf8_lossy(&out.stdout).contains("READ-ALL-DONE"));
    let started = fs::read_to_string(&marker).unwrap_or_default();
    assert!(started.is_empty(), "sys-lib started a program: {started}");
    let _ = fs::remove_dir_all(&work);
}
