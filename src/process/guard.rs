//! The process guard: make sure this is the only copy running, make every
//! file it creates private by default, and make sure it will not run out of
//! file descriptors under load. One call, secure defaults, and a refusal with
//! an actionable error when a requirement cannot be met.

use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
};

use super::Limit;
use crate::{Error, Result, parse::pid_file, sys};

/// The default umask: owner read and write, group read, others nothing.
const DEFAULT_UMASK: u32 = 0o027;
/// The default soft open-file limit the guard raises to.
const DEFAULT_OPEN_FILES_TARGET: u64 = 65_536;
/// The default minimum hard open-file limit; below it the guard refuses to start.
const DEFAULT_OPEN_FILES_MINIMUM: u64 = 1_024;

/// Starts the process guard with secure defaults: a single-instance lock in
/// path-lib's runtime directory for `name`, `umask(0o027)`, and the soft
/// open-file limit raised to 65 536 (at least 1 024 required). Call it before
/// any file is opened and before threads start, and keep the [`Guard`] for the
/// life of the process.
///
/// ```no_run
/// use sys_lib as sys;
///
/// let guard = sys::process::guard("site")?;      // before any file is opened
/// println!("lock {:?}", guard.lock_file());
/// // ... run; the lock is held while `guard` lives.
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// [`Error::AlreadyRunning`] when another copy holds the lock,
/// [`Error::RunDirUnavailable`] when the lock directory cannot be created or
/// trusted, [`Error::OpenFilesTooLow`] when the hard open-file limit is below
/// the minimum, and [`Error::InvalidInput`] for an invalid name.
pub fn guard(name: &str) -> Result<Guard> {
    Guard::builder(name).build()
}

/// The running guard. Dropping it closes the lock file, which releases the
/// lock; the lock file itself is never deleted (deleting it would let two
/// copies run, see the plan's guard section).
#[must_use = "dropping the guard releases the single-instance lock"]
pub struct Guard {
    // Held for its lock; read by nothing.
    _lock: Option<File>,
    report: GuardReport,
}

impl Guard {
    /// A builder for options other than the defaults.
    ///
    /// ```no_run
    /// use sys_lib::process::Guard;
    ///
    /// // A database locks beside its data, so two copies under different users
    /// // or run directories still exclude each other.
    /// let guard = Guard::builder("hive")
    ///     .lock_dir("/var/lib/hive")
    ///     .open_files_target(1_048_576)
    ///     .open_files_minimum(65_536)
    ///     .build()?;
    /// # Ok::<(), sys_lib::Error>(())
    /// ```
    pub fn builder(name: &str) -> GuardBuilder {
        GuardBuilder {
            name: name.to_owned(),
            lock_dir: None,
            lock: true,
            allow_network_lock: false,
            umask: Some(DEFAULT_UMASK),
            open_files_target: DEFAULT_OPEN_FILES_TARGET,
            open_files_minimum: DEFAULT_OPEN_FILES_MINIMUM,
        }
    }

    /// What the guard did.
    #[must_use]
    pub fn report(&self) -> &GuardReport {
        &self.report
    }

    /// The lock file, when a lock was taken.
    #[must_use]
    pub fn lock_file(&self) -> Option<&Path> {
        self.report.lock_file.as_deref()
    }
}

impl fmt::Debug for Guard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Guard")
            .field("report", &self.report)
            .finish()
    }
}

/// Options for [`Guard`].
#[derive(Clone, Debug)]
pub struct GuardBuilder {
    name: String,
    lock_dir: Option<PathBuf>,
    lock: bool,
    allow_network_lock: bool,
    umask: Option<u32>,
    open_files_target: u64,
    open_files_minimum: u64,
}

impl GuardBuilder {
    /// The directory for the lock and pid files. The default is path-lib's
    /// runtime directory for the name; a database should pass its data
    /// directory. Created owner-only (0700) when missing.
    #[must_use]
    pub fn lock_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.lock_dir = Some(dir.into());
        self
    }

    /// Whether to take the single-instance lock (default true). Pass false when
    /// a supervisor holds it, as tower does across hand-off restarts.
    #[must_use]
    pub fn lock(mut self, on: bool) -> Self {
        self.lock = on;
        self
    }

    /// Accept a lock directory on a network filesystem (`nfs`, `cifs`, WSL's
    /// `9p`), whose locks may not hold across machines (default false).
    #[must_use]
    pub fn allow_network_lock(mut self, on: bool) -> Self {
        self.allow_network_lock = on;
        self
    }

    /// The umask to set (default `0o027`). Unix only; ignored on Windows.
    #[must_use]
    pub fn umask(mut self, mask: u32) -> Self {
        self.umask = Some(mask & 0o777);
        self
    }

    /// Leave the umask as it is.
    #[must_use]
    pub fn keep_umask(mut self) -> Self {
        self.umask = None;
        self
    }

    /// The soft open-file limit to raise to (default 65 536), capped at the
    /// hard limit and, on macOS, at `kern.maxfilesperproc`.
    #[must_use]
    pub fn open_files_target(mut self, n: u64) -> Self {
        self.open_files_target = n;
        self
    }

    /// The hard open-file limit below which the guard refuses to start
    /// (default 1 024).
    #[must_use]
    pub fn open_files_minimum(mut self, n: u64) -> Self {
        self.open_files_minimum = n;
        self
    }

    /// Starts the guard. See [`guard`] for the errors.
    pub fn build(self) -> Result<Guard> {
        validate_name(&self.name)?;
        let mut warnings = Vec::new();

        // Plan the open-file limit first, so a refusal changes nothing (E-67).
        let files_before = sys::open_files_limit()?;
        let raise_to = plan_open_files(
            files_before,
            self.open_files_target,
            self.open_files_minimum,
            sys::open_files_ceiling(),
        )?;

        let (lock, lock_file, lock_filesystem) = if self.lock {
            let dir = match &self.lock_dir {
                Some(d) => {
                    create_owner_only(d)?;
                    d.clone()
                }
                None => runtime_dir(&self.name)?,
            };
            check_dir(&dir)?;
            let fs_kind = sys::filesystem(&dir);
            if let Some(kind) = fs_kind.filter(|k| k.network) {
                if !self.allow_network_lock {
                    return Err(unavailable(
                        &dir,
                        io::ErrorKind::Unsupported,
                        &format!(
                            "it is on a {} filesystem, where locks may not hold across machines; use a local \
                             directory, or allow_network_lock(true)",
                            kind.name
                        ),
                    ));
                }
                warnings.push(GuardWarning::NetworkLockAllowed {
                    filesystem: kind.name,
                });
            }
            let lock_path = dir.join(format!("{}.lock", self.name));
            let pid_path = dir.join(format!("{}.pid", self.name));
            let file = open_lock_file(&dir, &lock_path)?;
            match file.try_lock() {
                Ok(()) => {}
                Err(fs::TryLockError::WouldBlock) => {
                    return Err(Error::AlreadyRunning {
                        pid: read_pid(&pid_path),
                        lock_file: lock_path,
                    });
                }
                Err(fs::TryLockError::Error(e)) => {
                    return Err(Error::Io {
                        what: "the process guard's lock",
                        source: e,
                    });
                }
            }
            path_lib::write_atomic(&pid_path, format!("{}\n", std::process::id())).map_err(
                |e| {
                    unavailable(
                        &dir,
                        io::ErrorKind::Other,
                        &format!("the pid file cannot be written: {e}"),
                    )
                },
            )?;
            (Some(file), Some(lock_path), fs_kind.map(|k| k.name))
        } else {
            warnings.push(GuardWarning::LockDisabled);
            (None, None, None)
        };

        let files_after = match raise_to {
            Some(n) => {
                sys::set_open_files_soft(n)?;
                sys::open_files_limit()?
            }
            None => files_before,
        };

        let (umask_before, umask_after) = match self.umask {
            Some(mask) => {
                let before = sys::swap_umask(mask);
                (before, before.map(|_| mask))
            }
            None => {
                // Reading the umask means setting it; set it straight back.
                let current = sys::swap_umask(0o077);
                if let Some(c) = current {
                    sys::swap_umask(c);
                }
                (current, current)
            }
        };

        // The race is the umask's, so the warning is Unix-only. Windows has no
        // umask, and every Windows process starts with the loader's worker
        // threads, so a count there would warn on every start.
        #[cfg(unix)]
        if let Some(threads) = sys::thread_count().filter(|&n| n > 1) {
            warnings.push(GuardWarning::ThreadsAlreadyRunning { threads });
        }

        Ok(Guard {
            _lock: lock,
            report: GuardReport {
                lock_file,
                lock_filesystem,
                umask_before,
                umask_after,
                open_files_before: files_before,
                open_files_after: files_after,
                warnings,
            },
        })
    }
}

/// What the guard did.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct GuardReport {
    /// The lock file, when a lock was taken.
    pub lock_file: Option<PathBuf>,
    /// The lock directory's filesystem: "ext4", "apfs", "NTFS", ...
    pub lock_filesystem: Option<&'static str>,
    /// The umask before the guard (Unix; `None` on Windows).
    pub umask_before: Option<u32>,
    /// The umask after the guard (Unix; `None` on Windows).
    pub umask_after: Option<u32>,
    /// The open-file limit before the guard (`None` values: unlimited or not applicable).
    pub open_files_before: Limit,
    /// The open-file limit after the guard.
    pub open_files_after: Limit,
    /// Conditions worth logging that did not stop the guard.
    pub warnings: Vec<GuardWarning>,
}

/// A condition the guard reports without refusing to start.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GuardWarning {
    /// Other threads were running when the umask changed; files they created
    /// meanwhile may have the old permissions. Call the guard before starting
    /// threads. Unix only: Windows has no umask.
    ThreadsAlreadyRunning {
        /// The thread count, this one included.
        threads: usize,
    },
    /// The lock directory is on a filesystem whose locks may not hold across
    /// machines, accepted because `allow_network_lock(true)` was set.
    NetworkLockAllowed {
        /// The filesystem.
        filesystem: &'static str,
    },
    /// `lock(false)`: no single-instance lock was taken.
    LockDisabled,
}

/// 1 to 64 ASCII letters, digits, `-`, `_`, `.`, not starting with `.` (threat T4).
fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidInput {
            what: "process guard name",
            reason: "use 1 to 64 ASCII letters, digits, '-', '_', or '.', not starting with '.'",
        })
    }
}

/// The soft limit to raise to, if any. Refuses when the hard limit (or the
/// platform ceiling) is below the minimum (E-66, E-67).
fn plan_open_files(
    before: Limit,
    target: u64,
    minimum: u64,
    ceiling: Option<u64>,
) -> Result<Option<u64>> {
    if before.soft.is_none() && before.hard.is_none() {
        return Ok(None); // Not applicable (Windows) or unlimited.
    }
    let hard = before.hard.unwrap_or(u64::MAX);
    let usable = hard.min(ceiling.unwrap_or(u64::MAX));
    if usable < minimum {
        return Err(Error::OpenFilesTooLow {
            have: usable,
            need: minimum,
        });
    }
    let want = target.max(minimum).min(usable);
    Ok((before.soft.unwrap_or(u64::MAX) < want).then_some(want))
}

fn unavailable(dir: &Path, kind: io::ErrorKind, reason: &str) -> Error {
    Error::RunDirUnavailable {
        path: dir.to_path_buf(),
        source: io::Error::new(kind, reason.to_owned()),
    }
}

fn runtime_dir(name: &str) -> Result<PathBuf> {
    let app = path_lib::App::new(name)
        .map_err(|e| unavailable(Path::new(name), io::ErrorKind::InvalidInput, &e.to_string()))?;
    let dir = app.dir(path_lib::Kind::Runtime).to_path_buf();
    app.ensure(path_lib::Kind::Runtime).map_err(|e| {
        unavailable(
            &dir,
            io::ErrorKind::Other,
            &format!("it cannot be created: {e}"),
        )
    })?;
    Ok(dir)
}

fn create_owner_only(dir: &Path) -> Result<()> {
    if fs::symlink_metadata(dir).is_ok() {
        return Ok(());
    }
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(dir)
        .map_err(|e| unavailable(dir, e.kind(), &format!("it cannot be created: {e}")))
}

/// The lock directory must be a real, owner-only directory (threat T9).
fn check_dir(dir: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(dir)
        .map_err(|e| unavailable(dir, e.kind(), &format!("it cannot be read: {e}")))?;
    if meta.file_type().is_symlink() {
        return Err(unavailable(
            dir,
            io::ErrorKind::InvalidInput,
            "it is a symbolic link; use the real directory",
        ));
    }
    if !meta.is_dir() {
        return Err(unavailable(
            dir,
            io::ErrorKind::InvalidInput,
            "it is not a directory",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.mode() & 0o022 != 0 {
            return Err(unavailable(
                dir,
                io::ErrorKind::PermissionDenied,
                "other users can write to it and could replace the lock file; chmod 700 it, or use another directory",
            ));
        }
        if sys::effective_uid().is_some_and(|uid| uid != meta.uid()) {
            return Err(unavailable(
                dir,
                io::ErrorKind::PermissionDenied,
                "it belongs to another user; use a directory this user owns",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & sys::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(unavailable(
                dir,
                io::ErrorKind::InvalidInput,
                "it is a junction or link; use the real directory",
            ));
        }
    }
    Ok(())
}

/// Opens the lock file without following a link at its path (threat T9).
fn open_lock_file(dir: &Path, path: &Path) -> Result<File> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(unavailable(
            dir,
            io::ErrorKind::InvalidInput,
            "the lock file is a symbolic link; remove it",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    sys::lock_open_options(&mut options);
    let file = options.open(path).map_err(|e| {
        unavailable(
            dir,
            e.kind(),
            &format!("the lock file cannot be opened: {e}"),
        )
    })?;
    let opened = file
        .metadata()
        .map_err(|e| unavailable(dir, e.kind(), &e.to_string()))?;
    let at_path =
        fs::symlink_metadata(path).map_err(|e| unavailable(dir, e.kind(), &e.to_string()))?;
    if !opened.is_file() || at_path.file_type().is_symlink() {
        return Err(unavailable(
            dir,
            io::ErrorKind::InvalidInput,
            "the lock file is not a regular file; remove it",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // The file opened must be the one at the path (no swap in between) and ours.
        if (opened.dev(), opened.ino()) != (at_path.dev(), at_path.ino()) {
            return Err(unavailable(
                dir,
                io::ErrorKind::InvalidInput,
                "the lock file changed while it was opened",
            ));
        }
        if sys::effective_uid().is_some_and(|uid| uid != opened.uid()) {
            return Err(unavailable(
                dir,
                io::ErrorKind::PermissionDenied,
                "the lock file belongs to another user; remove it",
            ));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if opened.file_attributes() & sys::FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(unavailable(
                dir,
                io::ErrorKind::InvalidInput,
                "the lock file is a link; remove it",
            ));
        }
    }
    Ok(file)
}

/// The other copy's pid from its pid file, read strictly (at most 128 bytes).
fn read_pid(path: &Path) -> Option<u32> {
    let mut data = Vec::with_capacity(pid_file::MAX_BYTES + 1);
    File::open(path)
        .ok()?
        .take(pid_file::MAX_BYTES as u64 + 1)
        .read_to_end(&mut data)
        .ok()?;
    pid_file::parse(&data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sys-lib-guard-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn test_guard_name_validation() {
        for good in ["site", "hive-db", "a.b_c", "x"] {
            assert!(validate_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            ".hidden",
            "a/b",
            "a\\b",
            "..",
            "sp ace",
            "naïve",
            &"x".repeat(65),
        ] {
            assert!(
                matches!(validate_name(bad), Err(Error::InvalidInput { .. })),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn test_guard_open_files_plan() {
        let limit = |soft, hard| Limit { soft, hard };
        // Raise to the target, capped at the hard limit.
        assert_eq!(
            plan_open_files(limit(Some(1024), Some(1_048_576)), 65_536, 1_024, None).unwrap(),
            Some(65_536)
        );
        assert_eq!(
            plan_open_files(limit(Some(1024), Some(4096)), 65_536, 1_024, None).unwrap(),
            Some(4096)
        );
        // Already high enough: unchanged.
        assert_eq!(
            plan_open_files(limit(Some(100_000), Some(1_048_576)), 65_536, 1_024, None).unwrap(),
            None
        );
        // E-66: an unlimited hard limit is capped at the platform ceiling.
        assert_eq!(
            plan_open_files(limit(Some(256), None), 65_536, 1_024, Some(24_576)).unwrap(),
            Some(24_576)
        );
        // E-67: a hard limit below the minimum refuses, naming both.
        let e = plan_open_files(limit(Some(256), Some(512)), 65_536, 1_024, None).unwrap_err();
        assert!(matches!(
            e,
            Error::OpenFilesTooLow {
                have: 512,
                need: 1_024
            }
        ));
        // Windows: not applicable.
        assert_eq!(
            plan_open_files(limit(None, None), 65_536, 1_024, None).unwrap(),
            None
        );
    }

    #[test]
    fn test_guard_locks_and_a_second_guard_is_refused() {
        let dir = temp_dir("second");
        let first = Guard::builder("guard-test")
            .lock_dir(&dir)
            .keep_umask()
            .open_files_target(0)
            .open_files_minimum(0)
            .build()
            .unwrap();
        let lock = first.lock_file().unwrap().to_path_buf();
        assert!(lock.ends_with("guard-test.lock"));
        assert_eq!(
            read_pid(&dir.join("guard-test.pid")),
            Some(std::process::id())
        );
        let second = Guard::builder("guard-test")
            .lock_dir(&dir)
            .keep_umask()
            .open_files_target(0)
            .open_files_minimum(0)
            .build();
        match second {
            Err(Error::AlreadyRunning { pid, lock_file }) => {
                assert_eq!(pid, Some(std::process::id()));
                assert_eq!(lock_file, lock);
            }
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }
        drop(first);
        // The lock is released; the lock file is never deleted.
        assert!(lock.exists());
        let again = Guard::builder("guard-test")
            .lock_dir(&dir)
            .keep_umask()
            .open_files_target(0)
            .open_files_minimum(0)
            .build();
        assert!(again.is_ok());
        drop(again);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_guard_lock_false_takes_no_lock() {
        let g = Guard::builder("nolock")
            .lock(false)
            .keep_umask()
            .open_files_target(0)
            .open_files_minimum(0)
            .build()
            .unwrap();
        assert_eq!(g.lock_file(), None);
        assert!(g.report().warnings.contains(&GuardWarning::LockDisabled));
    }

    #[test]
    fn test_guard_refuses_a_file_as_lock_dir() {
        let dir = temp_dir("file");
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::write(&dir, b"x").unwrap();
        let e = Guard::builder("g")
            .lock_dir(&dir)
            .keep_umask()
            .build()
            .unwrap_err();
        assert!(matches!(e, Error::RunDirUnavailable { .. }), "{e:?}");
        let _ = fs::remove_file(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_guard_lock_path_is_symlink_refuses() {
        // E-64, T9: a symlink planted at the lock path.
        let dir = temp_dir("symlink");
        create_owner_only(&dir).unwrap();
        let target = dir.join("elsewhere");
        fs::write(&target, b"").unwrap();
        std::os::unix::fs::symlink(&target, dir.join("g.lock")).unwrap();
        let e = Guard::builder("g")
            .lock_dir(&dir)
            .keep_umask()
            .build()
            .unwrap_err();
        assert!(e.to_string().contains("symbolic link"), "{e}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_guard_world_writable_dir_refuses() {
        // E-64, T9: a directory other users can write to.
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("writable");
        create_owner_only(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
        let e = Guard::builder("g")
            .lock_dir(&dir)
            .keep_umask()
            .build()
            .unwrap_err();
        assert!(e.to_string().contains("other users can write"), "{e}");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_guard_symlinked_dir_refuses() {
        let dir = temp_dir("dirlink");
        let real = temp_dir("dirlink-real");
        create_owner_only(&real).unwrap();
        std::os::unix::fs::symlink(&real, &dir).unwrap();
        let e = Guard::builder("g")
            .lock_dir(&dir)
            .keep_umask()
            .build()
            .unwrap_err();
        assert!(e.to_string().contains("symbolic link"), "{e}");
        let _ = fs::remove_file(&dir);
        let _ = fs::remove_dir_all(&real);
    }

    #[test]
    fn test_guard_stale_pid_file_is_replaced() {
        // E-63: a pid file left by a crashed run while the lock is free.
        let dir = temp_dir("stale");
        create_owner_only(&dir).unwrap();
        fs::write(dir.join("g.pid"), b"999999\n").unwrap();
        let g = Guard::builder("g")
            .lock_dir(&dir)
            .keep_umask()
            .open_files_target(0)
            .open_files_minimum(0)
            .build()
            .unwrap();
        assert_eq!(read_pid(&dir.join("g.pid")), Some(std::process::id()));
        drop(g);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_guard_report_with_threads_warns() {
        // E-65, T10: the test harness runs tests on threads, so on Unix a warning
        // is expected wherever the platform can count them; never on Windows.
        let g = Guard::builder("threads")
            .lock(false)
            .keep_umask()
            .open_files_target(0)
            .open_files_minimum(0)
            .build()
            .unwrap();
        let warned = g
            .report()
            .warnings
            .iter()
            .any(|w| matches!(w, GuardWarning::ThreadsAlreadyRunning { .. }));
        if cfg!(unix) {
            assert_eq!(warned, sys::thread_count().is_some_and(|n| n > 1));
        } else {
            assert!(!warned);
        }
        if cfg!(unix) {
            assert!(g.report().umask_before.is_some());
            assert_eq!(g.report().umask_before, g.report().umask_after);
        } else {
            assert_eq!(g.report().umask_before, None);
        }
    }
}
