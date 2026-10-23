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

    /// The distribution corpus (D-022): release files from real images, with
    /// the answers a shell reads from os-release (`tests/fixtures/distros`,
    /// built by `capture_distros.sh`). Absent from the published package.
    fn corpus() -> Vec<(
        String,
        std::path::PathBuf,
        std::collections::BTreeMap<String, String>,
    )> {
        let base = std::path::Path::new(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/distros"
        ));
        let Ok(entries) = std::fs::read_dir(base) else {
            return Vec::new();
        };
        let mut out: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|e| e.path().is_dir())
            .map(|e| {
                let text = std::fs::read_to_string(e.path().join("EXPECTED.txt")).unwrap();
                let expected = text
                    .lines()
                    .filter_map(|l| l.split_once('='))
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect();
                (e.file_name().into_string().unwrap(), e.path(), expected)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn test_linux_os_distribution_corpus_matches_the_shell() {
        let corpus = corpus();
        if corpus.is_empty() {
            return; // the published package carries no fixtures (D-018)
        }
        assert!(
            corpus.len() >= 40,
            "the corpus has {} entries",
            corpus.len()
        );
        let mut wrong = Vec::new();
        for (slug, dir, want) in &corpus {
            let os = read(&Root::at(dir));
            let field = |k: &str| want.get(k).map(String::as_str).unwrap_or("");
            if want.contains_key("LEGACY") {
                // No os-release: the answers come from the legacy files.
                let (id, version, name) = match field("IMAGE") {
                    "centos:5" => ("centos", "5.11", "CentOS"),
                    "centos:6" => ("centos", "6.10", "CentOS"),
                    other => panic!("{slug}: no legacy expectation for {other}"),
                };
                let got = (
                    os.distro_id.as_str(),
                    os.version.as_deref(),
                    os.distro.as_str(),
                );
                if got != (id, Some(version), name) || os.name_source != Source::LegacyRelease {
                    wrong.push(format!("{slug}: legacy {got:?}"));
                }
                continue;
            }
            let or = |v: &str, d: &'static str| {
                if v.is_empty() {
                    d.to_owned()
                } else {
                    v.to_owned()
                }
            };
            let expect_name = or(
                if field("PRETTY_NAME").is_empty() {
                    field("NAME")
                } else {
                    field("PRETTY_NAME")
                },
                "Linux",
            );
            let checks = [
                // os-release(5) ids are lowercase; capitals shipped against
                // the rule (openEuler) are lowered.
                (
                    "ID",
                    os.distro_id.clone(),
                    or(&field("ID").to_ascii_lowercase(), "linux"),
                ),
                ("NAME", os.distro.clone(), or(field("NAME"), "Linux")),
                ("PRETTY_NAME", os.name.clone(), expect_name),
                (
                    "VERSION_ID",
                    os.version.clone().unwrap_or_default(),
                    field("VERSION_ID").to_owned(),
                ),
                (
                    "VERSION_CODENAME",
                    os.release
                        .as_ref()
                        .and_then(|r| r.codename.clone())
                        .unwrap_or_default(),
                    field("VERSION_CODENAME").to_owned(),
                ),
            ];
            for (key, got, expected) in checks {
                if got != expected {
                    wrong.push(format!(
                        "{slug}: {key} {got:?}, the shell reads {expected:?}"
                    ));
                }
            }
            let like = os
                .release
                .as_ref()
                .map(|r| r.id_like.clone())
                .unwrap_or_default();
            let shell_like: Vec<String> = field("ID_LIKE")
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            if like != shell_like {
                wrong.push(format!(
                    "{slug}: ID_LIKE {like:?}, the shell reads {shell_like:?}"
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "{} disagreements:\n{}",
            wrong.len(),
            wrong.join("\n")
        );
    }

    /// A copy of a corpus entry without its os-release files, so the legacy
    /// path is tested on real files too.
    fn without_os_release(slug: &str, dir: &std::path::Path) -> Option<std::path::PathBuf> {
        let legacy: Vec<_> = crate::parse::release::FILES
            .iter()
            .map(|(f, _)| f.trim_start_matches('/'))
            .filter(|f| dir.join(f).is_file())
            .collect();
        if legacy.is_empty() {
            return None;
        }
        let copy =
            std::env::temp_dir().join(format!("sys-lib-corpus-{}-{slug}", std::process::id()));
        for f in legacy {
            std::fs::create_dir_all(copy.join(f).parent().unwrap()).unwrap();
            std::fs::copy(dir.join(f), copy.join(f)).unwrap();
        }
        Some(copy)
    }

    #[test]
    fn test_linux_os_distribution_corpus_legacy_files_agree_with_os_release() {
        let mut wrong = Vec::new();
        let mut checked = 0;
        for (slug, dir, want) in corpus() {
            let Some(copy) = without_os_release(&slug, &dir) else {
                continue;
            };
            let os = read(&Root::at(&copy));
            let _ = std::fs::remove_dir_all(&copy);
            checked += 1;
            let id = want.get("ID").map_or("", String::as_str);
            // Where the legacy files name another family than os-release does,
            // the legacy answer is the best those files allow; these are the
            // known cases, each checked to give exactly this.
            let expected = match slug.as_str() {
                _ if want.contains_key("LEGACY") => continue,
                // Only /etc/debian_version besides os-release.
                "kalilinux-kali-rolling" | "parrotsec-core" => "debian",
                _ => id,
            };
            if os.distro_id != expected {
                wrong.push(format!(
                    "{slug}: legacy files say {:?}, os-release says {id:?}",
                    os.distro_id
                ));
            }
        }
        if checked > 0 {
            assert!(
                wrong.is_empty(),
                "{} disagreements:\n{}",
                wrong.len(),
                wrong.join("\n")
            );
        }
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
