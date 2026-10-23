//! Threat T10: the umask is process-wide, so a thread creating a file while
//! the guard changes it can create the file with the old mask. The guard is
//! meant to run before threads start; when it finds others already running it
//! says so. One test in this file: the umask belongs to the whole process.

#![cfg(feature = "guard")]

use std::sync::mpsc;

use sys_lib::process::{Guard, GuardWarning};

#[test]
fn test_attack_t10_guard_after_threads_warns_and_sets_the_umask_once() {
    let (stop, wait) = mpsc::channel::<()>();
    let extra = std::thread::spawn(move || {
        let _ = wait.recv();
    });

    let guard = Guard::builder("t10")
        .lock(false)
        .open_files_target(0)
        .open_files_minimum(0)
        .build()
        .unwrap();
    let report = guard.report();
    let warned = report
        .warnings
        .iter()
        .any(|w| matches!(w, GuardWarning::ThreadsAlreadyRunning { threads } if *threads >= 2));
    // Every tier 1 platform can count threads; the warning is not optional there.
    if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
        assert!(warned, "no ThreadsAlreadyRunning warning: {report:?}");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(report.umask_after, Some(0o027));
        let path = std::env::temp_dir().join(format!("sys-lib-t10-{}", std::process::id()));
        std::fs::write(&path, b"x").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            mode, 0o640,
            "a file created after the guard has mode {mode:o}"
        );
    }
    #[cfg(not(unix))]
    assert_eq!(report.umask_after, None);

    stop.send(()).unwrap();
    extra.join().unwrap();
}
