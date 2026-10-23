//! The operating system: kind, name, version, kernel, and distribution.
//!
//! The kind and the `is_*` checks are compile-time constants. Everything else
//! is a static fact, read once on first use and cached for the life of the
//! process (about a nanosecond afterwards).
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("{} (kernel {})", sys::os::name(), sys::os::kernel());
//! println!("{} ({})", sys::os::distro(), sys::os::distro_id());   // "Ubuntu (ubuntu)", "Windows (windows)"
//! if sys::os::is_like("debian") {
//!     // apt-based: Debian, Ubuntu, Mint, Pop!_OS, ...
//! }
//! const ON_WINDOWS: bool = sys::os::is_windows();
//! ```

use core::fmt;
use std::sync::OnceLock;

use crate::parse;

/// The operating system family, fixed when the program was compiled.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Kind {
    /// Linux (not Android).
    Linux,
    /// Windows.
    Windows,
    /// macOS.
    MacOs,
    /// Android.
    Android,
    /// iOS and iPadOS.
    Ios,
    /// FreeBSD.
    FreeBsd,
    /// OpenBSD.
    OpenBsd,
    /// NetBSD.
    NetBsd,
    /// Anything else.
    Other,
}

impl Kind {
    /// The lowercase name: `"linux"`, `"windows"`, `"macos"`, ...
    ///
    /// ```
    /// assert_eq!(sys_lib::os::Kind::MacOs.name(), "macos");
    /// ```
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Windows => "windows",
            Self::MacOs => "macos",
            Self::Android => "android",
            Self::Ios => "ios",
            Self::FreeBsd => "freebsd",
            Self::OpenBsd => "openbsd",
            Self::NetBsd => "netbsd",
            Self::Other => "other",
        }
    }
}

/// The operating system family this program was compiled for.
///
/// ```
/// # use sys_lib as sys;
/// #[cfg(target_os = "linux")]
/// assert_eq!(sys::os::kind(), sys::os::Kind::Linux);
/// ```
#[must_use]
pub const fn kind() -> Kind {
    if cfg!(target_os = "android") {
        Kind::Android
    } else if cfg!(target_os = "linux") {
        Kind::Linux
    } else if cfg!(target_os = "windows") {
        Kind::Windows
    } else if cfg!(target_os = "macos") {
        Kind::MacOs
    } else if cfg!(target_os = "ios") {
        Kind::Ios
    } else if cfg!(target_os = "freebsd") {
        Kind::FreeBsd
    } else if cfg!(target_os = "openbsd") {
        Kind::OpenBsd
    } else if cfg!(target_os = "netbsd") {
        Kind::NetBsd
    } else {
        Kind::Other
    }
}

/// Windows.
#[must_use]
pub const fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

/// The Linux kernel, not Android.
#[must_use]
pub const fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

/// macOS.
#[must_use]
pub const fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

/// macOS; the same as [`is_macos`].
#[must_use]
pub const fn is_mac() -> bool {
    is_macos()
}

/// Android.
#[must_use]
pub const fn is_android() -> bool {
    cfg!(target_os = "android")
}

/// iOS and iPadOS.
#[must_use]
pub const fn is_ios() -> bool {
    cfg!(target_os = "ios")
}

/// A Unix: Linux, Android, macOS, iOS, the BSDs, ...
#[must_use]
pub const fn is_unix() -> bool {
    cfg!(unix)
}

/// FreeBSD, OpenBSD, NetBSD, or DragonFly BSD.
#[must_use]
pub const fn is_bsd() -> bool {
    cfg!(any(
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))
}

/// A version number, with the original text kept: `24.04` stays `24.04`.
///
/// Ordering compares the numbers, then the text.
///
/// ```
/// if let Some(v) = sys_lib::os::version() {
///     println!("{v} = {}.{}.{}", v.major, v.minor, v.patch);
/// }
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub struct Version {
    /// The first number.
    pub major: u32,
    /// The second number, 0 when absent.
    pub minor: u32,
    /// The third number, 0 when absent.
    pub patch: u32,
    /// A fourth number (the Windows update revision, for example).
    pub build: Option<u32>,
    /// The text the numbers came from.
    pub text: &'static str,
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.text)
    }
}

/// Where a fact came from.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Source {
    /// `/etc/os-release`.
    OsRelease,
    /// `/usr/lib/os-release`.
    UsrLibOsRelease,
    /// A legacy release file (`/etc/redhat-release`, `/etc/lsb-release`, ...).
    LegacyRelease,
    /// A `sysctl` value (macOS, iOS).
    Sysctl,
    /// The Windows registry or `RtlGetVersion`.
    Registry,
    /// An Android system property.
    SystemProperty,
    /// None of the above: the answer names the OS family only.
    Fallback,
}

/// The fields of os-release (Linux), or their legacy-file equivalents.
///
/// ```
/// if let Some(r) = sys_lib::os::os_release() {
///     println!("{} {:?} {:?}", r.pretty_name, r.version_id, r.codename);
/// }
/// ```
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OsRelease {
    /// `ID`: `ubuntu`, `fedora`; `linux` when the file names none.
    pub id: &'static str,
    /// `ID_LIKE`: the families this one derives from, closest first.
    pub id_like: &'static [&'static str],
    /// `NAME`: "Ubuntu", "Fedora Linux".
    pub name: &'static str,
    /// `PRETTY_NAME`: "Ubuntu 24.04.1 LTS".
    pub pretty_name: &'static str,
    /// `VERSION`: "24.04.1 LTS (Noble Numbat)".
    pub version: Option<&'static str>,
    /// `VERSION_ID`: "24.04".
    pub version_id: Option<&'static str>,
    /// `VERSION_CODENAME`: "noble".
    pub codename: Option<&'static str>,
    /// `VARIANT_ID`: "server", "workstation".
    pub variant_id: Option<&'static str>,
    /// `BUILD_ID`, on rolling distributions.
    pub build_id: Option<&'static str>,
    /// Which file the fields came from.
    pub source: Source,
}

/// Everything this module knows, in one value.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct OsInfo {
    /// [`kind`].
    pub kind: Kind,
    /// [`name`].
    pub name: &'static str,
    /// [`version`].
    pub version: Option<&'static Version>,
    /// [`kernel`].
    pub kernel: &'static str,
    /// [`distro`].
    pub distro: &'static str,
    /// [`distro_id`].
    pub distro_id: &'static str,
    /// [`os_release`].
    pub os_release: Option<&'static OsRelease>,
    /// Where `name` came from.
    pub name_source: Source,
}

/// The operating system's name and version: "Ubuntu 24.04.1 LTS",
/// "macOS 15.1 Sequoia", "Windows 11 Pro". Never empty, never fails: a minimal
/// container without os-release gives "Linux" (E-47).
#[must_use]
pub fn name() -> &'static str {
    info().name
}

/// The version: `24.04` on Ubuntu, `15.1` on macOS, `10.0.26200.6899` on Windows.
/// `None` on rolling distributions, which have none.
#[must_use]
pub fn version() -> Option<&'static Version> {
    info().version
}

/// The kernel release: "6.8.0-45-generic", "24.1.0" (Darwin), "10.0.26200.6899".
#[must_use]
pub fn kernel() -> &'static str {
    info().kernel
}

/// The distribution's name on Linux ("Ubuntu", "Fedora Linux", "Arch Linux"),
/// and the operating system's elsewhere ("Windows", "macOS", "Android", "iOS").
/// Never empty; "Linux" when a Linux system says nothing about itself.
///
/// ```
/// let d = sys_lib::os::distro();
/// assert!(!d.is_empty());
/// #[cfg(windows)]
/// assert_eq!(d, "Windows");
/// ```
#[must_use]
pub fn distro() -> &'static str {
    info().distro
}

/// The distribution's lowercase id on Linux (`ubuntu`, `fedora`, `arch`), and
/// the operating system's elsewhere (`windows`, `macos`, `android`, `ios`).
#[must_use]
pub fn distro_id() -> &'static str {
    info().distro_id
}

/// Whether the distribution is `id` or derives from it (its `ID` or `ID_LIKE`):
/// `is_like("debian")` is true on Debian, Ubuntu, Mint, and Pop!_OS. Off Linux
/// it compares with [`distro_id`] (`is_like("windows")` on Windows).
///
/// ```
/// let apt = sys_lib::os::is_like("debian");
/// let dnf = sys_lib::os::is_like("fedora") || sys_lib::os::is_like("rhel");
/// # let _ = (apt, dnf);
/// ```
#[must_use]
pub fn is_like(id: &str) -> bool {
    let info = info();
    info.distro_id == id
        || info
            .os_release
            .is_some_and(|r| r.id == id || r.id_like.contains(&id))
}

/// Every os-release field (Linux). `None` elsewhere, and on a Linux system with
/// neither os-release nor a legacy release file.
#[must_use]
pub fn os_release() -> Option<&'static OsRelease> {
    info().os_release
}

/// Everything above in one value.
#[must_use]
pub fn info() -> &'static OsInfo {
    static INFO: OnceLock<OsInfo> = OnceLock::new();
    INFO.get_or_init(|| from_facts(crate::sys::os()))
}

/// A static fact's text, kept for the life of the process. Called only from
/// `OnceLock` initialisers, so each string is stored once.
pub(crate) fn keep(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn keep_opt(s: Option<String>) -> Option<&'static str> {
    s.map(keep)
}

pub(crate) fn from_facts(f: crate::sys::OsFacts) -> OsInfo {
    let version = f.version.as_deref().and_then(|text| {
        let (major, minor, patch, build) = parse::version::parse(text)?;
        Some(&*Box::leak(Box::new(Version {
            major,
            minor,
            patch,
            build,
            text: keep(text.to_owned()),
        })))
    });
    let os_release = f.release.map(|r| {
        let id_like: Vec<&'static str> = r.id_like.into_iter().map(keep).collect();
        let name = r.name.unwrap_or_else(|| "Linux".to_owned());
        &*Box::leak(Box::new(OsRelease {
            id: keep(r.id.unwrap_or_else(|| "linux".to_owned())),
            id_like: Box::leak(id_like.into_boxed_slice()),
            pretty_name: keep(r.pretty_name.unwrap_or_else(|| name.clone())),
            name: keep(name),
            version: keep_opt(r.version),
            version_id: keep_opt(r.version_id),
            codename: keep_opt(r.codename),
            variant_id: keep_opt(r.variant_id),
            build_id: keep_opt(r.build_id),
            source: r.source,
        }))
    });
    OsInfo {
        kind: kind(),
        name: keep(f.name),
        version,
        kernel: keep(f.kernel),
        distro: keep(f.distro),
        distro_id: keep(f.distro_id),
        os_release,
        name_source: f.name_source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_os_constants_are_consistent() {
        assert_eq!(is_mac(), is_macos());
        assert!(!(is_linux() && is_android()));
        assert_eq!(kind() == Kind::Windows, is_windows());
        assert_eq!(kind() == Kind::Linux, is_linux());
        if is_linux() || is_macos() || is_android() || is_ios() {
            assert!(is_unix());
        }
    }

    #[test]
    fn test_os_live_answers_are_never_empty() {
        assert!(!name().is_empty());
        assert!(!distro().is_empty());
        assert!(!distro_id().is_empty());
        assert!(is_like(distro_id()));
        assert!(!is_like("not-a-distribution"));
        if is_windows() {
            assert_eq!((distro(), distro_id()), ("Windows", "windows"));
        }
        if is_macos() {
            assert_eq!((distro(), distro_id()), ("macOS", "macos"));
        }
    }

    #[test]
    fn test_os_info_is_cached() {
        assert!(std::ptr::eq(info(), info()));
        assert!(std::ptr::eq(name(), info().name));
    }

    #[test]
    fn test_os_from_facts_fills_defaults() {
        let info = from_facts(crate::sys::OsFacts {
            name: "X 1".into(),
            version: Some("8.10-beta".into()),
            kernel: "k".into(),
            distro: "X".into(),
            distro_id: "x".into(),
            release: Some(crate::sys::ReleaseFacts {
                id: None,
                id_like: vec!["rhel".into(), "fedora".into()],
                name: None,
                pretty_name: None,
                version: None,
                version_id: None,
                codename: None,
                variant_id: None,
                build_id: None,
                source: Source::OsRelease,
            }),
            name_source: Source::OsRelease,
        });
        let v = info.version.unwrap();
        assert_eq!(
            (v.major, v.minor, v.patch, v.build, v.text),
            (8, 10, 0, None, "8.10-beta")
        );
        assert_eq!(v.to_string(), "8.10-beta");
        let r = info.os_release.unwrap();
        assert_eq!((r.id, r.name, r.pretty_name), ("linux", "Linux", "Linux"));
        assert_eq!(r.id_like, &["rhel", "fedora"]);
    }

    #[test]
    fn test_os_concurrent_first_use_sees_one_value() {
        // 64 threads at first use all see the same cached value (concurrency plan).
        let ptrs: Vec<usize> = std::thread::scope(|s| {
            let handles: Vec<_> = (0..64)
                .map(|_| s.spawn(|| core::ptr::from_ref(info()) as usize))
                .collect();
            handles.into_iter().map(|h| h.join().unwrap_or(0)).collect()
        });
        assert!(ptrs.windows(2).all(|w| w[0] == w[1]));
    }
}
