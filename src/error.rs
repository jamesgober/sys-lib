//! The error type: every failure says what failed and what to do.

use core::fmt;
use std::{io, path::PathBuf, time::Duration};

/// Everything that can go wrong in sys-lib.
///
/// Every variant's message is one sentence saying what failed, followed by
/// what to do about it. No variant ever carries file content, registry content,
/// or identifier values: [`Error::Malformed`] names the source (a constant
/// path) and a line number only.
///
/// # Examples
///
/// ```
/// use sys_lib as sys;
///
/// match sys::host::name() {
///     Ok(name) => println!("host {name}"),
///     Err(e) if e.is_unavailable() => println!("host name unknown here: {e}"),
///     Err(e) => return Err(e),
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// This reading does not exist on this platform. Treat the value as unknown.
    Unsupported {
        /// The reading.
        what: &'static str,
        /// The platform that lacks it.
        platform: &'static str,
    },
    /// The OS refused (root-only files, a sandbox, SELinux, seccomp).
    PermissionDenied {
        /// The reading.
        what: &'static str,
        /// The permission that would allow it.
        hint: &'static str,
    },
    /// A file or key that should exist does not (a container with a reduced `/proc`).
    NotFound {
        /// The reading.
        what: &'static str,
        /// The file, key, or call that was missing.
        source: &'static str,
    },
    /// The OS returned data that does not parse.
    Malformed {
        /// The reading.
        what: &'static str,
        /// The file, key, or call the data came from.
        source: &'static str,
        /// The line, when the data is line-based.
        line: Option<u32>,
    },
    /// A file, a list, or a caller's buffer exceeded its budget.
    TooLarge {
        /// The reading.
        what: &'static str,
        /// The budget that was exceeded.
        limit: u64,
    },
    /// A sampler was asked again before any time passed on the OS clock.
    TooSoon {
        /// The time since the previous sample.
        elapsed: Duration,
        /// The shortest interval that gives a meaningful value.
        minimum: Duration,
    },
    /// A system call failed.
    Os {
        /// The reading.
        what: &'static str,
        /// The call that failed.
        call: &'static str,
        /// The OS error code (`errno`, a Win32 error, an `NTSTATUS`, a `kern_return_t`).
        code: i32,
    },
    /// File I/O failed outside the cases above.
    Io {
        /// The reading.
        what: &'static str,
        /// The underlying error.
        source: io::Error,
    },
    /// Invalid input from the caller.
    InvalidInput {
        /// The input.
        what: &'static str,
        /// The rule it broke.
        reason: &'static str,
    },
    /// The process guard found another copy holding the lock.
    AlreadyRunning {
        /// The other copy's process id, when its pid file is readable.
        pid: Option<u32>,
        /// The lock file.
        lock_file: PathBuf,
    },
    /// The process guard's lock directory cannot be created, written, or trusted.
    RunDirUnavailable {
        /// The directory.
        path: PathBuf,
        /// Why, as an I/O error (the kind and a message naming the fix).
        source: io::Error,
    },
    /// The hard open-file limit is below the process guard's minimum.
    OpenFilesTooLow {
        /// The hard limit the process has.
        have: u64,
        /// The minimum the guard was asked for.
        need: u64,
    },
}

/// The result type used throughout sys-lib.
pub type Result<T, E = Error> = core::result::Result<T, E>;

impl Error {
    /// True for the errors that mean "this fact is not available here", which a
    /// caller usually treats as unknown: [`Unsupported`](Error::Unsupported),
    /// [`PermissionDenied`](Error::PermissionDenied), and [`NotFound`](Error::NotFound).
    ///
    /// ```
    /// let e = sys_lib::Error::Unsupported { what: "load average", platform: "windows" };
    /// assert!(e.is_unavailable());
    /// ```
    #[must_use]
    pub fn is_unavailable(&self) -> bool {
        matches!(
            self,
            Self::Unsupported { .. } | Self::PermissionDenied { .. } | Self::NotFound { .. }
        )
    }

    /// An [`Io`](Error::Io) error, or the more specific variant its kind maps to.
    #[cfg_attr(
        not(any(target_os = "linux", target_os = "android", test)),
        allow(dead_code)
    )]
    pub(crate) fn io(what: &'static str, source: &'static str, err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Self::NotFound { what, source },
            io::ErrorKind::PermissionDenied => Self::PermissionDenied {
                what,
                hint: "run as a user that may read it, or treat the value as unknown",
            },
            _ => Self::Io { what, source: err },
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported { what, platform } => write!(
                f,
                "{what} is not available on {platform}; treat the value as unknown"
            ),
            Self::PermissionDenied { what, hint } => {
                write!(f, "reading {what} was refused by the OS; {hint}")
            }
            Self::NotFound { what, source } => write!(
                f,
                "{what} could not be read: {source} does not exist; treat the value as unknown \
                 (minimal containers and sandboxes often lack it)"
            ),
            Self::Malformed { what, source, line } => {
                write!(f, "{what} could not be read: {source} does not parse")?;
                if let Some(line) = line {
                    write!(f, " (line {line})")?;
                }
                write!(f, "; report it with the output of sys_lib::summary()")
            }
            Self::TooLarge { what, limit } => write!(
                f,
                "{what} exceeded its budget of {limit}; raise the budget with the Tier 2 options \
                 if the machine is genuinely that large, or pass a larger buffer"
            ),
            Self::TooSoon { elapsed, minimum } => write!(
                f,
                "sampled again after {elapsed:?}, before the OS clock moved; sample at least \
                 {minimum:?} apart"
            ),
            Self::Os { what, call, code } => {
                write!(
                    f,
                    "{what} could not be read: {call} failed with OS error {code}"
                )
            }
            Self::Io { what, source } => write!(f, "{what} could not be read: {source}"),
            Self::InvalidInput { what, reason } => write!(f, "invalid {what}: {reason}"),
            Self::AlreadyRunning { pid, lock_file } => {
                write!(f, "another copy is already running")?;
                if let Some(pid) = pid {
                    write!(f, " as process {pid}")?;
                }
                write!(
                    f,
                    " and holds {}; stop it, or give this copy another lock directory",
                    lock_file.display()
                )
            }
            Self::RunDirUnavailable { path, source } => write!(
                f,
                "the lock directory {} cannot be used: {source}",
                path.display()
            ),
            Self::OpenFilesTooLow { have, need } => write!(
                f,
                "the hard open-file limit is {have}, below the required {need}; raise it \
                 (LimitNOFILE= in the systemd unit, `ulimit -Hn`, or launchd's HardResourceLimits)"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::RunDirUnavailable { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;

    const fn assert_send_sync<T: Send + Sync + 'static>() {}
    const _: () = assert_send_sync::<Error>();

    fn every_variant() -> Vec<Error> {
        vec![
            Error::Unsupported {
                what: "load average",
                platform: "windows",
            },
            Error::PermissionDenied {
                what: "board serial",
                hint: "run as root",
            },
            Error::NotFound {
                what: "os-release",
                source: "/etc/os-release",
            },
            Error::Malformed {
                what: "cpu list",
                source: "/proc/self/status",
                line: Some(7),
            },
            Error::Malformed {
                what: "cpu list",
                source: "/proc/self/status",
                line: None,
            },
            Error::TooLarge {
                what: "mount list",
                limit: 4096,
            },
            Error::TooSoon {
                elapsed: Duration::from_millis(1),
                minimum: Duration::from_millis(10),
            },
            Error::Os {
                what: "host name",
                call: "GetComputerNameExW",
                code: 5,
            },
            Error::Io {
                what: "status",
                source: io::Error::other("disk on fire"),
            },
            Error::InvalidInput {
                what: "guard name",
                reason: "must not contain '/'",
            },
            Error::AlreadyRunning {
                pid: Some(4821),
                lock_file: PathBuf::from("/run/site/site.lock"),
            },
            Error::AlreadyRunning {
                pid: None,
                lock_file: PathBuf::from("/run/site/site.lock"),
            },
            Error::RunDirUnavailable {
                path: PathBuf::from("/tmp"),
                source: io::Error::other("writable by other users; chmod 700 it"),
            },
            Error::OpenFilesTooLow {
                have: 256,
                need: 1024,
            },
        ]
    }

    #[test]
    fn test_error_display_every_variant_says_what_to_do() {
        // NFR-15: every message names the failure and a next step.
        for e in every_variant() {
            let text = e.to_string();
            assert!(!text.is_empty());
            assert!(
                text.contains(';') || text.contains(':'),
                "no explanation in {text:?}"
            );
            assert!(!text.ends_with(' '), "trailing space in {text:?}");
        }
    }

    #[test]
    fn test_error_display_includes_the_specific_values() {
        let e = Error::AlreadyRunning {
            pid: Some(4821),
            lock_file: PathBuf::from("site.lock"),
        };
        assert!(e.to_string().contains("process 4821"));
        let e = Error::OpenFilesTooLow {
            have: 256,
            need: 1024,
        };
        assert!(e.to_string().contains("256") && e.to_string().contains("1024"));
        let e = Error::Malformed {
            what: "x",
            source: "/proc/stat",
            line: Some(3),
        };
        assert!(e.to_string().contains("line 3"));
    }

    #[test]
    fn test_error_is_unavailable_only_for_unknown_facts() {
        for e in every_variant() {
            let expected = matches!(
                e,
                Error::Unsupported { .. } | Error::PermissionDenied { .. } | Error::NotFound { .. }
            );
            assert_eq!(e.is_unavailable(), expected, "{e:?}");
        }
    }

    #[test]
    fn test_error_source_only_for_wrapped_io() {
        for e in every_variant() {
            let wraps = matches!(e, Error::Io { .. } | Error::RunDirUnavailable { .. });
            assert_eq!(e.source().is_some(), wraps, "{e:?}");
        }
    }

    #[test]
    fn test_error_io_maps_not_found_and_permission_denied() {
        let e = Error::io("x", "/proc/x", io::Error::from(io::ErrorKind::NotFound));
        assert!(matches!(
            e,
            Error::NotFound {
                source: "/proc/x",
                ..
            }
        ));
        let e = Error::io(
            "x",
            "/proc/x",
            io::Error::from(io::ErrorKind::PermissionDenied),
        );
        assert!(matches!(e, Error::PermissionDenied { .. }));
        let e = Error::io("x", "/proc/x", io::Error::from(io::ErrorKind::Interrupted));
        assert!(matches!(e, Error::Io { .. }));
    }
}
