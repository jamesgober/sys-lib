//! Build info: facts fixed when the application was compiled, answering
//! "exactly which build is running?" in logs, crash reports, and health checks.
//!
//! The application calls [`emit`] from its own `build.rs` (with sys-lib also in
//! `[build-dependencies]`), and [`build_info!`](crate::build_info) anywhere at
//! run time:
//!
//! ```no_run
//! // build.rs
//! fn main() {
//!     sys_lib::build::emit();
//! }
//! ```
//!
//! ```
//! let build = sys_lib::build_info!();
//! println!("{build}");               // site 0.3.0 (9db9e36, release, built 2026-10-21T18:02:11Z)
//! println!("{}", build.public());    // site 0.3.0: safe for a public health endpoint
//! ```
//!
//! This module is the only code in sys-lib that runs another program, and it
//! does so in the application's build script, never at run time (D-004). It
//! never fails the build: anything it cannot learn becomes `unknown`.

// The examples show a build script, whose `fn main` is the point.
#![allow(clippy::needless_doctest_main)]

use core::fmt::{self, Write as _};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::fmt::{json_str, kv};

/// Values longer than this, or with characters outside printable ASCII, become
/// `unknown` (threat T11: a branch name can contain anything).
const MAX_VALUE: usize = 128;
const MAX_FEATURES: usize = 512;
const UNKNOWN: &str = "unknown";

/// Emits the build facts for [`build_info!`](crate::build_info). Call it from
/// the application's `build.rs`. Never fails and never panics.
///
/// It records the git commit and whether the tree was dirty, the build time
/// (`SOURCE_DATE_EPOCH` when set, for reproducible builds), the profile,
/// target, rustc version, and enabled features. It does not record the build
/// machine's host name, user, or source path.
///
/// It asks Cargo to re-run the build script when the git `HEAD`, the current
/// ref, the index, `src/`, `build.rs`, or `Cargo.toml` change, or when
/// `SOURCE_DATE_EPOCH` does.
pub fn emit() {
    emitter().emit();
}

/// A configurable [`emit`].
///
/// ```no_run
/// // Only the commit, no rustc version or feature list.
/// sys_lib::build::emitter().rustc(false).features(false).emit();
/// ```
pub fn emitter() -> Emitter {
    Emitter {
        git: true,
        rustc: true,
        features: true,
    }
}

/// What [`emit`] captures. See [`emitter`].
#[derive(Clone, Copy, Debug)]
#[must_use]
pub struct Emitter {
    git: bool,
    rustc: bool,
    features: bool,
}

impl Emitter {
    /// Capture the git commit and dirty state (default true).
    pub fn git(mut self, on: bool) -> Self {
        self.git = on;
        self
    }

    /// Capture the rustc version (default true).
    pub fn rustc(mut self, on: bool) -> Self {
        self.rustc = on;
        self
    }

    /// Capture the enabled Cargo features (default true).
    pub fn features(mut self, on: bool) -> Self {
        self.features = on;
        self
    }

    /// Prints the `cargo:` instructions.
    pub fn emit(self) {
        for line in self.instructions() {
            println!("{line}");
        }
    }

    fn instructions(self) -> Vec<String> {
        let env = |k: &str| std::env::var(k).ok();
        let dir = env("CARGO_MANIFEST_DIR").map(PathBuf::from);
        let mut out = Vec::new();

        let (commit, dirty) = match (&dir, self.git) {
            (Some(dir), true) => (git_commit(dir), git_dirty(dir)),
            _ => (None, None),
        };
        let commit = commit.unwrap_or_else(|| UNKNOWN.to_owned());
        let short = if commit == UNKNOWN {
            UNKNOWN.to_owned()
        } else {
            commit.chars().take(7).collect()
        };
        let built_at = match source_date_epoch(env("SOURCE_DATE_EPOCH").as_deref()) {
            Ok(Some(secs)) => rfc3339(secs),
            Ok(None) => rfc3339(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs()),
            ),
            Err(()) => {
                out.push(
                    "cargo:warning=SOURCE_DATE_EPOCH is not a non-negative integer; ignored"
                        .to_owned(),
                );
                rfc3339(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs()),
                )
            }
        };
        let rustc = if self.rustc {
            rustc_version(env("RUSTC").as_deref().unwrap_or("rustc"))
        } else {
            None
        };
        let features = if self.features {
            features(std::env::vars().map(|(k, _)| k))
        } else {
            String::new()
        };

        let mut set = |key: &str, value: &str, max: usize| {
            out.push(format!(
                "cargo:rustc-env=SYS_LIB_BUILD_{key}={}",
                clean(value, max)
            ));
        };
        set("COMMIT", &commit, MAX_VALUE);
        set("COMMIT_SHORT", &short, MAX_VALUE);
        set(
            "DIRTY",
            dirty.map_or(UNKNOWN, |d| if d { "true" } else { "false" }),
            MAX_VALUE,
        );
        set("TIME", &built_at, MAX_VALUE);
        set(
            "PROFILE",
            env("PROFILE").as_deref().unwrap_or(UNKNOWN),
            MAX_VALUE,
        );
        set(
            "TARGET",
            env("TARGET").as_deref().unwrap_or(UNKNOWN),
            MAX_VALUE,
        );
        set("RUSTC", rustc.as_deref().unwrap_or(UNKNOWN), MAX_VALUE);
        set("FEATURES", &features, MAX_FEATURES);

        if let Some(dir) = &dir {
            let mut watch = if self.git { git_watch(dir) } else { Vec::new() };
            watch.extend(
                ["src", "build.rs", "Cargo.toml"]
                    .iter()
                    .map(|rel| dir.join(rel))
                    .filter(|p| p.exists()),
            );
            out.extend(watch.iter().filter_map(|p| rerun_line(p)));
        }
        out.push("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH".to_owned());
        out
    }
}

/// A value safe for `rustc-env`: printable ASCII, bounded; otherwise `unknown`.
fn clean(value: &str, max: usize) -> String {
    let ok = value.len() <= max && value.bytes().all(|b| (0x20..0x7f).contains(&b));
    if ok {
        value.to_owned()
    } else {
        UNKNOWN.to_owned()
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The full commit hash (40 or 64 hex characters), or `None`.
fn git_commit(dir: &Path) -> Option<String> {
    let commit = git(dir, &["rev-parse", "HEAD"])?;
    let hex = commit.bytes().all(|b| b.is_ascii_hexdigit());
    (hex && (commit.len() == 40 || commit.len() == 64)).then_some(commit)
}

/// Whether tracked files differ from the commit; untracked files (build
/// artefacts) do not count.
fn git_dirty(dir: &Path) -> Option<bool> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()?;
    out.status.success().then_some(!out.stdout.is_empty())
}

/// The git files whose change means a new commit or a staged change: `HEAD`,
/// the ref it names, `packed-refs`, and the index. Worktrees keep `HEAD` and
/// the index in their own git directory and refs in the common one (E-59).
fn git_watch(dir: &Path) -> Vec<PathBuf> {
    let Some(git_dir) = git(dir, &["rev-parse", "--path-format=absolute", "--git-dir"]) else {
        return Vec::new();
    };
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .unwrap_or_else(|| git_dir.clone());
    let (git_dir, common) = (PathBuf::from(git_dir), PathBuf::from(common));
    let mut paths = vec![
        git_dir.join("HEAD"),
        git_dir.join("index"),
        common.join("packed-refs"),
    ];
    if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD"))
        && let Some(r) = head_ref(&head)
    {
        paths.push(common.join(r));
    }
    paths.retain(|p| p.exists());
    paths
}

/// The ref a `HEAD` file names, when it is a well-formed branch ref: under
/// `refs/`, bounded, and free of everything `git check-ref-format` forbids
/// (control characters, spaces, `~^:?*[\`, `..`). A tampered `HEAD` cannot
/// name a path outside the git directory or smuggle a line into the output
/// (threat T11).
fn head_ref(head: &str) -> Option<&str> {
    let r = head
        .strip_suffix('\n')
        .unwrap_or(head)
        .strip_prefix("ref: ")?;
    let ok = r.starts_with("refs/")
        && r.len() <= 1024
        && !r.contains("..")
        && !r.ends_with('/')
        && r.bytes()
            .all(|b| b > 0x20 && b != 0x7f && !b"~^:?*[\\".contains(&b));
    ok.then_some(r)
}

/// A `rerun-if-changed` line, or `None` for a path that would break the line
/// protocol (a line break, or not UTF-8).
fn rerun_line(path: &Path) -> Option<String> {
    let text = path.to_str()?;
    (!text.contains(['\n', '\r'])).then(|| format!("cargo:rerun-if-changed={text}"))
}

/// `Ok(None)` when unset or empty, `Err` when set to anything but a
/// non-negative integer (E-61).
fn source_date_epoch(value: Option<&str>) -> Result<Option<u64>, ()> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) => v.parse().map(Some).map_err(|_| ()),
    }
}

/// "1.89.0" from "rustc 1.89.0 (29483883e 2025-08-04)".
fn rustc_version(rustc: &str) -> Option<String> {
    let out = Command::new(rustc).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    parse_rustc_version(&text)
}

fn parse_rustc_version(text: &str) -> Option<String> {
    let version = text.split_whitespace().nth(1)?;
    version
        .starts_with(|c: char| c.is_ascii_digit())
        .then_some(version.to_owned())
}

/// The enabled features, from the build script's `CARGO_FEATURE_*` variables:
/// lowercase, sorted, comma-separated. Cargo turns `-` into `_` in these names,
/// so `device-id` reads back as `device_id`.
fn features(vars: impl Iterator<Item = String>) -> String {
    let mut names: Vec<String> = vars
        .filter_map(|k| {
            k.strip_prefix("CARGO_FEATURE_")
                .map(str::to_ascii_lowercase)
        })
        .collect();
    names.sort();
    names.dedup();
    names.join(",")
}

/// Seconds since the Unix epoch as an RFC 3339 UTC timestamp, to the second.
pub(crate) fn rfc3339(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Facts fixed when the application was compiled. Get it with
/// [`build_info!`](crate::build_info); every field is a `&'static str` in the
/// binary, so it costs nothing at run time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct BuildInfo {
    /// The application's `CARGO_PKG_NAME`.
    pub name: &'static str,
    /// The application's `CARGO_PKG_VERSION`.
    pub version: &'static str,
    /// The full commit hash, or `"unknown"`.
    pub commit: &'static str,
    /// The first 7 characters of the commit, or `"unknown"`.
    pub commit_short: &'static str,
    /// Whether uncommitted changes to tracked files were built; `None` when unknown.
    pub dirty: Option<bool>,
    /// The build time, RFC 3339 UTC, or `"unknown"`.
    pub built_at: &'static str,
    /// `"release"`, `"debug"`.
    pub profile: &'static str,
    /// The target triple, or `"unknown"`.
    pub target: &'static str,
    /// The rustc version, or `"unknown"`.
    pub rustc: &'static str,
    /// The enabled features, comma-separated and sorted.
    pub features: &'static str,
}

impl BuildInfo {
    /// Used by [`build_info!`](crate::build_info); not part of the API.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn __from_env(
        name: &'static str,
        version: &'static str,
        debug: bool,
        commit: Option<&'static str>,
        commit_short: Option<&'static str>,
        dirty: Option<&'static str>,
        built_at: Option<&'static str>,
        profile: Option<&'static str>,
        target: Option<&'static str>,
        rustc: Option<&'static str>,
        features: Option<&'static str>,
    ) -> Self {
        const fn or_unknown(v: Option<&'static str>) -> &'static str {
            match v {
                Some(v) => v,
                None => UNKNOWN,
            }
        }
        let dirty = match dirty {
            Some(d) => match d.as_bytes() {
                b"true" => Some(true),
                b"false" => Some(false),
                _ => None,
            },
            None => None,
        };
        let profile = match profile {
            Some(p) => p,
            None if debug => "debug",
            None => "release",
        };
        let features = match features {
            Some(f) => f,
            None => "",
        };
        Self {
            name,
            version,
            commit: or_unknown(commit),
            commit_short: or_unknown(commit_short),
            dirty,
            built_at: or_unknown(built_at),
            profile,
            target: or_unknown(target),
            rustc: or_unknown(rustc),
            features,
        }
    }

    /// Only the name and version, for output that leaves your control (a public
    /// health endpoint): commit hashes and toolchain versions help an attacker
    /// fingerprint a deployment (threat T11).
    ///
    /// ```
    /// let public = sys_lib::build_info!().public().to_string();
    /// assert!(public.starts_with("sys-lib "));
    /// ```
    #[must_use]
    pub fn public(&self) -> PublicBuild<'_> {
        PublicBuild(self)
    }

    /// Writes `build.name=site build.version=0.3.0 ...`, without allocating.
    ///
    /// ```
    /// let mut line = String::new();
    /// sys_lib::build_info!().write_kv(&mut line)?;
    /// assert!(line.starts_with("build.name=sys-lib "));
    /// # Ok::<(), std::fmt::Error>(())
    /// ```
    pub fn write_kv(&self, out: &mut impl fmt::Write) -> fmt::Result {
        let dirty = match self.dirty {
            Some(true) => "true",
            Some(false) => "false",
            None => UNKNOWN,
        };
        let fields = [
            ("build.name", self.name),
            ("build.version", self.version),
            ("build.commit", self.commit),
            ("build.commit_short", self.commit_short),
            ("build.dirty", dirty),
            ("build.built_at", self.built_at),
            ("build.profile", self.profile),
            ("build.target", self.target),
            ("build.rustc", self.rustc),
            ("build.features", self.features),
        ];
        for (i, (k, v)) in fields.iter().enumerate() {
            if i > 0 {
                out.write_char(' ')?;
            }
            kv(out, k, v)?;
        }
        Ok(())
    }

    /// Writes one JSON object, without allocating; `dirty` is `null` when unknown.
    ///
    /// ```
    /// let mut json = String::new();
    /// sys_lib::build_info!().write_json(&mut json)?;
    /// assert!(json.starts_with(r#"{"name":"sys-lib","#));
    /// # Ok::<(), std::fmt::Error>(())
    /// ```
    pub fn write_json(&self, out: &mut impl fmt::Write) -> fmt::Result {
        out.write_char('{')?;
        let strings = [
            ("name", self.name),
            ("version", self.version),
            ("commit", self.commit),
            ("commit_short", self.commit_short),
        ];
        for (i, (k, v)) in strings.iter().enumerate() {
            if i > 0 {
                out.write_char(',')?;
            }
            json_str(out, k)?;
            out.write_char(':')?;
            json_str(out, v)?;
        }
        out.write_str(",\"dirty\":")?;
        out.write_str(match self.dirty {
            Some(true) => "true",
            Some(false) => "false",
            None => "null",
        })?;
        for (k, v) in [
            ("built_at", self.built_at),
            ("profile", self.profile),
            ("target", self.target),
            ("rustc", self.rustc),
            ("features", self.features),
        ] {
            out.write_char(',')?;
            json_str(out, k)?;
            out.write_char(':')?;
            json_str(out, v)?;
        }
        out.write_char('}')
    }
}

/// `site 0.3.0 (9db9e36, release, built 2026-10-21T18:02:11Z)`; unknown parts
/// are left out, and a dirty build shows `9db9e36-dirty`.
impl fmt::Display for BuildInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} (", self.name, self.version)?;
        if self.commit_short != UNKNOWN {
            write!(f, "{}", self.commit_short)?;
            if self.dirty == Some(true) {
                f.write_str("-dirty")?;
            }
            f.write_str(", ")?;
        }
        f.write_str(self.profile)?;
        if self.built_at != UNKNOWN {
            write!(f, ", built {}", self.built_at)?;
        }
        f.write_char(')')
    }
}

/// The name and version only: `site 0.3.0`. See [`BuildInfo::public`].
#[derive(Clone, Copy, Debug)]
pub struct PublicBuild<'a>(&'a BuildInfo);

impl fmt::Display for PublicBuild<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.0.name, self.0.version)
    }
}

/// The application's build info, as a `&'static BuildInfo`. Expands in the
/// calling crate, so the name and version are the **application's**. An
/// application that never called [`build::emit`](crate::build::emit) still
/// compiles, with the commit, time, target, and rustc `unknown` (E-60).
///
/// ```
/// let build: &'static sys_lib::BuildInfo = sys_lib::build_info!();
/// assert_eq!(build.name, env!("CARGO_PKG_NAME"));
/// if build.dirty == Some(true) {
///     eprintln!("warning: built from uncommitted changes");
/// }
/// ```
#[macro_export]
macro_rules! build_info {
    () => {{
        static __SYS_LIB_BUILD_INFO: $crate::BuildInfo = $crate::BuildInfo::__from_env(
            ::core::env!("CARGO_PKG_NAME"),
            ::core::env!("CARGO_PKG_VERSION"),
            ::core::cfg!(debug_assertions),
            ::core::option_env!("SYS_LIB_BUILD_COMMIT"),
            ::core::option_env!("SYS_LIB_BUILD_COMMIT_SHORT"),
            ::core::option_env!("SYS_LIB_BUILD_DIRTY"),
            ::core::option_env!("SYS_LIB_BUILD_TIME"),
            ::core::option_env!("SYS_LIB_BUILD_PROFILE"),
            ::core::option_env!("SYS_LIB_BUILD_TARGET"),
            ::core::option_env!("SYS_LIB_BUILD_RUSTC"),
            ::core::option_env!("SYS_LIB_BUILD_FEATURES"),
        );
        &__SYS_LIB_BUILD_INFO
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_rfc3339_matches_clock_lib() {
        for secs in [
            0u64,
            1,
            59,
            86_399,
            86_400,
            951_782_400,
            1_700_000_000,
            1_761_069_731,
            4_102_444_800,
            253_402_300_799,
        ] {
            let ours = rfc3339(secs);
            let oracle = format!("{:.0}", clock_lib::Wall::from_unix_seconds(secs).unwrap());
            assert_eq!(ours, oracle, "{secs}");
        }
    }

    #[test]
    fn test_build_info_without_emit_compiles_with_unknowns() {
        // E-60: this crate's own build script never calls emit().
        let b = crate::build_info!();
        assert_eq!(b.name, "sys-lib");
        assert_eq!(b.version, crate::VERSION);
        assert_eq!(b.commit, "unknown");
        assert_eq!(b.dirty, None);
        assert_eq!(
            b.profile,
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
        );
        assert_eq!(
            b.to_string(),
            format!("sys-lib {} ({})", crate::VERSION, b.profile)
        );
        assert_eq!(
            b.public().to_string(),
            format!("sys-lib {}", crate::VERSION)
        );
        assert!(std::ptr::eq(b, crate::build_info!()) || b == crate::build_info!());
    }

    #[test]
    fn test_build_info_display_and_writers_with_every_field() {
        let b = BuildInfo::__from_env(
            "site",
            "0.3.0",
            false,
            Some("9db9e36aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            Some("9db9e36"),
            Some("true"),
            Some("2026-10-21T18:02:11Z"),
            Some("release"),
            Some("x86_64-unknown-linux-gnu"),
            Some("1.89.0"),
            Some("default,guard"),
        );
        assert_eq!(
            b.to_string(),
            "site 0.3.0 (9db9e36-dirty, release, built 2026-10-21T18:02:11Z)"
        );
        let mut kv_line = String::new();
        b.write_kv(&mut kv_line).unwrap();
        assert!(kv_line.contains("build.dirty=true build.built_at=2026-10-21T18:02:11Z"));
        assert!(kv_line.contains("build.features=default,guard"));
        let mut json = String::new();
        b.write_json(&mut json).unwrap();
        assert!(json.starts_with(r#"{"name":"site","version":"0.3.0","commit":"9db9e36"#));
        assert!(json.contains(r#""dirty":true,"built_at":"2026-10-21T18:02:11Z""#));
        assert!(json.ends_with(r#""features":"default,guard"}"#));
        // T11: the public view has the name and version only.
        assert_eq!(b.public().to_string(), "site 0.3.0");
    }

    #[test]
    fn test_build_hostile_values_become_unknown() {
        // T11: a branch or tag name can contain newlines, quotes, or 10 KB.
        assert_eq!(clean("main\ncargo:rustc-env=EVIL=1", MAX_VALUE), "unknown");
        assert_eq!(clean(&"x".repeat(10_000), MAX_VALUE), "unknown");
        assert_eq!(clean("caf\u{e9}", MAX_VALUE), "unknown");
        assert_eq!(clean("1.89.0", MAX_VALUE), "1.89.0");
        let b = BuildInfo::__from_env(
            "a",
            "1",
            true,
            None,
            None,
            Some("maybe"),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(b.dirty, None);
        assert_eq!(b.profile, "debug");
    }

    #[test]
    fn test_build_head_ref_rejects_tampered_heads() {
        // T11: HEAD is read as text; a tampered one must not name a path
        // outside the git directory or add a line to the output.
        assert_eq!(head_ref("ref: refs/heads/main\n"), Some("refs/heads/main"));
        assert_eq!(
            head_ref("ref: refs/heads/feature/0.3-boot-kit\n"),
            Some("refs/heads/feature/0.3-boot-kit")
        );
        assert_eq!(
            head_ref("ref: refs/heads/\"quoted\"\n"),
            Some("refs/heads/\"quoted\"")
        );
        assert_eq!(
            head_ref("ref: refs/heads/x\ncargo:rustc-link-arg=evil\n"),
            None
        );
        assert_eq!(head_ref("ref: refs/heads/x\r\n"), None);
        assert_eq!(head_ref("ref: refs/../../etc/passwd\n"), None);
        assert_eq!(head_ref("ref: /etc/passwd\n"), None);
        assert_eq!(head_ref("ref: refs/heads/a b\n"), None);
        assert_eq!(head_ref("ref: refs/heads/\n"), None);
        assert_eq!(
            head_ref(&format!("ref: refs/heads/{}\n", "x".repeat(10_000))),
            None
        );
        assert_eq!(head_ref("4f2c9a1e0b7d3c5a8e6f1b2d4c7a9e0f3b5d8c1a\n"), None);
    }

    #[test]
    fn test_build_rerun_line_refuses_line_breaks() {
        assert_eq!(
            rerun_line(Path::new("/src/app/.git/HEAD")).as_deref(),
            Some("cargo:rerun-if-changed=/src/app/.git/HEAD")
        );
        assert_eq!(
            rerun_line(Path::new("/src/a\ncargo:rustc-link-arg=evil")),
            None
        );
        assert_eq!(rerun_line(Path::new("/src/a\rb")), None);
    }

    #[test]
    fn test_build_source_date_epoch_rules() {
        // E-61: invalid or negative values are ignored with a warning.
        assert_eq!(source_date_epoch(None), Ok(None));
        assert_eq!(source_date_epoch(Some("")), Ok(None));
        assert_eq!(
            source_date_epoch(Some("1700000000")),
            Ok(Some(1_700_000_000))
        );
        assert_eq!(source_date_epoch(Some("-5")), Err(()));
        assert_eq!(source_date_epoch(Some("soon")), Err(()));
    }

    #[test]
    fn test_build_rustc_version_and_features_parsing() {
        assert_eq!(
            parse_rustc_version("rustc 1.89.0 (29483883e 2025-08-04)").as_deref(),
            Some("1.89.0")
        );
        assert_eq!(
            parse_rustc_version("rustc 1.92.0-nightly (abc 2025-10-01)").as_deref(),
            Some("1.92.0-nightly")
        );
        assert_eq!(parse_rustc_version("garbage"), None);
        let vars = [
            "CARGO_FEATURE_GUARD",
            "PATH",
            "CARGO_FEATURE_DEFAULT",
            "CARGO_FEATURE_DEVICE_ID",
        ]
        .map(String::from);
        assert_eq!(features(vars.into_iter()), "default,device_id,guard");
    }

    #[test]
    fn test_build_instructions_never_fail_without_cargo() {
        // Outside a build script every value is unknown, and nothing panics.
        let lines = emitter().git(false).rustc(false).instructions();
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("cargo:rustc-env=SYS_LIB_BUILD_COMMIT=unknown"))
        );
        assert!(lines.iter().all(|l| l.starts_with("cargo:")));
        assert!(lines.contains(&"cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH".to_owned()));
    }

    #[test]
    fn test_build_git_commit_of_this_repository() {
        // E-59: in a git checkout the commit is found; elsewhere it is None.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        if let Some(c) = git_commit(dir) {
            assert!(c.len() == 40 || c.len() == 64);
            assert!(git_dirty(dir).is_some());
            assert!(git_watch(dir).iter().any(|p| p.ends_with("HEAD")));
        }
        assert_eq!(git_commit(Path::new("/definitely/not/a/repository")), None);
    }
}
