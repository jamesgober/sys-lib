//! Threat T4: path or value injection through the environment. A child
//! process reads every 0.3 fact twice, once with a clean environment and once
//! with hostile values in every variable a system library might be tempted to
//! trust; the answers must not change. (Which files are opened is checked
//! against the reviewed list in the Linux backend's own tests.)

mod common;

const MARKER: &str = "FACTS=";

/// The hostile values. `PATH` and the variables process start-up itself needs
/// (`SystemRoot`, `HOME`) are left alone; T16 covers `PATH`.
const HOSTILE: [(&str, &str); 22] = [
    ("TZ", "../../../../etc/shadow"),
    ("TZDIR", "/etc"),
    ("LANG", "../../etc/passwd"),
    ("LC_ALL", "\u{1b}[31m;rm -rf /"),
    ("LC_CTYPE", "x\ny"),
    ("HOSTNAME", "evil-host"),
    ("COMPUTERNAME", "EVIL-HOST"),
    ("USER", "evil-user"),
    ("USERNAME", "evil-user"),
    ("LOGNAME", "evil-user"),
    ("HOST_PROC", "/nonexistent"),
    ("HOST_SYS", "/nonexistent"),
    ("HOST_ETC", "/nonexistent"),
    ("HOST_ROOT", "/nonexistent"),
    ("PROCFS_PATH", "/nonexistent"),
    ("SYSFS_PATH", "/nonexistent"),
    ("NUMBER_OF_PROCESSORS", "9999"),
    ("PROCESSOR_IDENTIFIER", "Evil CPU"),
    ("PROCESSOR_ARCHITECTURE", "EVIL"),
    ("OS", "EvilOS"),
    ("OSTYPE", "evil"),
    ("RUST_MIN_STACK", "not-a-number"),
];

fn facts() -> String {
    use sys_lib::{cpu, host, os, process};
    let user = process::user().ok().map(|u| (u.name, u.id, u.is_admin));
    format!(
        "{:?}",
        (
            (
                os::name(),
                os::distro(),
                os::distro_id(),
                os::kernel(),
                os::version()
            ),
            (
                cpu::logical(),
                cpu::physical(),
                cpu::available(),
                cpu::name(),
                cpu::vendor()
            ),
            (cpu::is_64bit(), cpu::arch().name()),
            host::name().ok(),
            (user, process::is_elevated().ok()),
        )
    )
}

/// Run as a child only: print the facts on one line.
#[test]
fn child_prints_the_facts() {
    if common::role().as_deref() == Some("facts") {
        println!("{MARKER}{}", facts());
    }
}

fn child_facts(hostile: bool) -> String {
    let mut cmd = common::child("child_prints_the_facts", "facts");
    if hostile {
        cmd.envs(HOSTILE);
    }
    let out = cmd.output().expect("run the child");
    assert!(out.status.success(), "child failed: {out:?}");
    let text = String::from_utf8_lossy(&out.stdout);
    common::after(&text, MARKER)
        .expect("the child printed its facts")
        .to_owned()
}

#[test]
fn test_attack_t4_hostile_environment_changes_no_answer() {
    if common::role().is_some() {
        return;
    }
    let clean = child_facts(false);
    let hostile = child_facts(true);
    assert_eq!(clean, hostile);
    for (_, value) in HOSTILE {
        assert!(
            !hostile.contains(value),
            "a hostile value leaked into {hostile}"
        );
    }
}
