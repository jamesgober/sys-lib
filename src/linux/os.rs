//! OS identity on Linux: os-release, then the legacy release files, then
//! "Linux" (D-022); the kernel release from `/proc/sys/kernel/osrelease`.

use super::Root;
use crate::{
    os::Source,
    parse::{os_release, release},
    sys::{OsFacts, ReleaseFacts},
};

/// Reads the OS identity from a Linux tree.
pub(crate) fn read(root: &Root) -> OsFacts {
    let kernel = root
        .text("/proc/sys/kernel/osrelease")
        .filter(|k| !k.is_empty())
        .unwrap_or_default();
    let release = from_os_release(root).or_else(|| from_legacy(root));
    match release {
        Some(r) => OsFacts {
            name: r
                .pretty_name
                .clone()
                .or_else(|| r.name.clone())
                .unwrap_or_else(|| "Linux".to_owned()),
            version: r.version_id.clone(),
            kernel,
            distro: r.name.clone().unwrap_or_else(|| "Linux".to_owned()),
            distro_id: r.id.clone().unwrap_or_else(|| "linux".to_owned()),
            name_source: r.source,
            release: Some(r),
        },
        None => OsFacts {
            name: "Linux".to_owned(),
            version: None,
            kernel,
            distro: "Linux".to_owned(),
            distro_id: "linux".to_owned(),
            name_source: Source::Fallback,
            release: None,
        },
    }
}

fn from_os_release(root: &Root) -> Option<ReleaseFacts> {
    for (path, source) in [
        ("/etc/os-release", Source::OsRelease),
        ("/usr/lib/os-release", Source::UsrLibOsRelease),
    ] {
        let Ok(data) = root.read(path, 64 << 10, "os-release", "/etc/os-release") else {
            continue;
        };
        let r = os_release::parse(&data);
        if r.id.is_none() && r.name.is_none() && r.pretty_name.is_none() {
            continue;
        }
        return Some(ReleaseFacts {
            id: r.id,
            id_like: r.id_like,
            name: r.name,
            pretty_name: r.pretty_name,
            version: r.version,
            version_id: r.version_id,
            codename: r.version_codename,
            variant_id: r.variant_id,
            build_id: r.build_id,
            source,
        });
    }
    None
}

fn from_legacy(root: &Root) -> Option<ReleaseFacts> {
    for (path, kind) in release::FILES {
        let Ok(data) = root.read(path, 16 << 10, "release file", "/etc/*-release") else {
            continue;
        };
        if let Some(l) = release::parse(kind, &data) {
            return Some(ReleaseFacts {
                id: Some(l.id),
                id_like: Vec::new(),
                name: Some(l.name),
                pretty_name: Some(l.pretty_name),
                version: None,
                version_id: l.version,
                codename: l.codename,
                variant_id: None,
                build_id: None,
                source: Source::LegacyRelease,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_linux_os_fixture_wsl2_is_ubuntu() {
        let os = read(&fixtures::root("wsl2"));
        assert_eq!(os.name, "Ubuntu 24.04.4 LTS");
        assert_eq!(os.distro, "Ubuntu");
        assert_eq!(os.distro_id, "ubuntu");
        assert_eq!(os.version.as_deref(), Some("24.04"));
        assert!(
            os.kernel.contains("microsoft-standard-WSL2"),
            "{}",
            os.kernel
        );
        let r = os.release.unwrap();
        assert_eq!(r.id_like, vec!["debian"]);
        assert_eq!(r.codename.as_deref(), Some("noble"));
    }

    #[test]
    fn test_linux_os_fixture_minimal_scratch_falls_back_to_linux() {
        // E-47: no os-release and no legacy file.
        let os = read(&fixtures::root("minimal-scratch"));
        assert_eq!(
            (os.name.as_str(), os.distro.as_str(), os.distro_id.as_str()),
            ("Linux", "Linux", "linux")
        );
        assert_eq!(os.name_source, Source::Fallback);
        assert!(os.release.is_none());
        assert!(!os.kernel.is_empty());
    }

    #[test]
    fn test_linux_os_every_fixture_has_an_answer() {
        for name in fixtures::ALL {
            let os = read(&fixtures::root(name));
            assert!(
                !os.name.is_empty() && !os.distro.is_empty() && !os.distro_id.is_empty(),
                "{name}"
            );
        }
    }
}
