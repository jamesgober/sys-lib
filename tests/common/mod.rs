//! Helpers shared by the integration tests: re-running this test binary as a
//! child process, so a test can look at a fresh process or at two processes
//! at once. The library never starts a program; tests may (PLAN.md 19.1).

#![allow(dead_code)]

use std::process::{Command, Stdio};

/// A command that runs only the test `name` of this binary, with its output
/// shown, in a child process. The child sees `role` set, so the test knows
/// it is the child.
pub fn child(name: &str, role: &str) -> Command {
    let mut cmd = Command::new(std::env::current_exe().expect("the test binary's path"));
    cmd.args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env("SYS_LIB_TEST_ROLE", role)
        .stdin(Stdio::null());
    cmd
}

/// The role this process was started with, when it is a child.
pub fn role() -> Option<String> {
    std::env::var("SYS_LIB_TEST_ROLE").ok()
}

/// The text after `marker` on the first line holding it, in a child's output.
/// libtest writes `test <name> ... ` before the test's own output, so the
/// marker may share a line with it.
pub fn after<'a>(output: &'a str, marker: &str) -> Option<&'a str> {
    output
        .lines()
        .find_map(|l| l.find(marker).map(|i| &l[i + marker.len()..]))
}
