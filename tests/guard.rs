//! The process guard across processes (threat T9, E-63): a second copy is
//! refused and told the holder's pid, and a holder killed without warning
//! (`SIGKILL`; `TerminateProcess` on Windows) leaves the lock free.

#![cfg(feature = "guard")]

mod common;

use std::{
    io::{BufRead, BufReader, Lines},
    path::{Path, PathBuf},
    process::{Child, ChildStdout, Stdio},
    time::{Duration, Instant},
};

use sys_lib::{Error, process::Guard};

const NAME: &str = "holder";

fn lock_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sys-lib-guard-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn guard_in(dir: &Path) -> sys_lib::Result<Guard> {
    Guard::builder(NAME)
        .lock_dir(dir)
        .keep_umask()
        .open_files_target(0)
        .open_files_minimum(0)
        .build()
}

/// A child process holding the lock, with its output still connected so it
/// never writes into a closed pipe.
struct Holder {
    child: Child,
    _out: Lines<BufReader<ChildStdout>>,
}

impl Holder {
    fn start(dir: &Path) -> Self {
        let mut child = common::child("child_holds_the_lock", "holder")
            .env("SYS_LIB_TEST_GUARD_DIR", dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("start the child");
        let mut out = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
        loop {
            match out.next() {
                Some(Ok(line)) if line.contains("LOCKED") => break,
                Some(Ok(line)) if line.contains("FAILED:") => panic!("child: {line}"),
                Some(_) => {}
                None => panic!("the child exited before taking the lock"),
            }
        }
        Self { child, _out: out }
    }

    fn kill(mut self) {
        self.child.kill().expect("kill the child");
        self.child.wait().expect("reap the child");
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Run as a child only: take the lock, say so, and hold it until killed (at
/// most a minute, so a failed parent never leaves it running).
#[test]
fn child_holds_the_lock() {
    if common::role().as_deref() != Some("holder") {
        return;
    }
    let dir = std::env::var("SYS_LIB_TEST_GUARD_DIR").expect("lock dir");
    match guard_in(Path::new(&dir)) {
        Ok(_guard) => {
            println!("LOCKED");
            std::thread::sleep(Duration::from_secs(60));
        }
        Err(e) => println!("FAILED: {e}"),
    }
}

#[test]
fn test_guard_second_process_is_refused_with_the_holder_pid() {
    if common::role().is_some() {
        return;
    }
    let dir = lock_dir("second");
    let holder = Holder::start(&dir);
    match guard_in(&dir) {
        Err(Error::AlreadyRunning { pid, lock_file }) => {
            assert_eq!(pid, Some(holder.child.id()));
            assert_eq!(lock_file, dir.join("holder.lock"));
        }
        other => panic!("expected AlreadyRunning, got {other:?}"),
    }
    holder.kill();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_guard_killed_holder_frees_the_lock() {
    if common::role().is_some() {
        return;
    }
    let dir = lock_dir("killed");
    Holder::start(&dir).kill();

    // On Unix the lock goes with the process: the first attempt succeeds.
    // Windows releases a dead process's byte-range locks after the process
    // object is signalled, "depending on available system resources"
    // (LockFileEx), so it gets a short bound instead.
    let bound = if cfg!(windows) {
        Duration::from_secs(5)
    } else {
        Duration::ZERO
    };
    let start = Instant::now();
    let guard = loop {
        match guard_in(&dir) {
            Ok(g) => break g,
            Err(Error::AlreadyRunning { .. }) if start.elapsed() < bound => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("the lock was not freed: {e}"),
        }
    };
    let lock_file = guard.lock_file().expect("a lock was taken").to_owned();
    let pid = std::fs::read_to_string(dir.join("holder.pid")).unwrap();
    assert_eq!(pid.trim(), std::process::id().to_string());

    // The lock file is never deleted, released or not.
    drop(guard);
    assert!(lock_file.exists());
    let _ = std::fs::remove_dir_all(&dir);
}
