//! The legacy release files a Linux system may have instead of os-release:
//! `/etc/lsb-release`, the Red Hat family's `/etc/*-release` sentences,
//! `/etc/SuSE-release`, `/etc/alpine-release`, `/etc/arch-release`,
//! `/etc/gentoo-release`, `/etc/slackware-version`, and `/etc/debian_version`.

use super::{os_release, os_release::valid_id};

/// What a legacy release file says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Legacy {
    /// The distribution's name: "CentOS Linux", "Alpine Linux".
    pub(crate) name: String,
    /// Its os-release-style id: `centos`, `alpine`.
    pub(crate) id: String,
    /// The full description: "CentOS Linux release 7.9.2009 (Core)".
    pub(crate) pretty_name: String,
    pub(crate) version: Option<String>,
    pub(crate) codename: Option<String>,
}

/// The kinds of legacy file, in the order they are tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Lsb,
    RedHatStyle,
    Suse,
    Alpine,
    Arch,
    Gentoo,
    Slackware,
    Debian,
}

/// The files, in order. The Red Hat family's specific files come before the
/// generic `redhat-release`, which CentOS, Rocky, and AlmaLinux also ship.
pub(crate) const FILES: [(&str, Kind); 13] = [
    ("/etc/lsb-release", Kind::Lsb),
    ("/etc/centos-release", Kind::RedHatStyle),
    ("/etc/rocky-release", Kind::RedHatStyle),
    ("/etc/almalinux-release", Kind::RedHatStyle),
    ("/etc/fedora-release", Kind::RedHatStyle),
    ("/etc/oracle-release", Kind::RedHatStyle),
    ("/etc/redhat-release", Kind::RedHatStyle),
    ("/etc/SuSE-release", Kind::Suse),
    ("/etc/alpine-release", Kind::Alpine),
    ("/etc/arch-release", Kind::Arch),
    ("/etc/gentoo-release", Kind::Gentoo),
    ("/etc/slackware-version", Kind::Slackware),
    ("/etc/debian_version", Kind::Debian),
];

const MAX_TEXT: usize = 256;

/// Parses one legacy file. `None` when it says nothing usable.
pub(crate) fn parse(kind: Kind, data: &[u8]) -> Option<Legacy> {
    match kind {
        Kind::Lsb => lsb(data),
        Kind::RedHatStyle => red_hat_style(first_line(data)?),
        Kind::Suse => suse(data),
        Kind::Alpine => Some(simple(
            "Alpine Linux",
            "alpine",
            version_token(first_line(data)?),
        )),
        Kind::Arch => Some(simple("Arch Linux", "arch", None)),
        Kind::Gentoo => {
            let text = first_line(data)?;
            let version = text
                .rsplit(' ')
                .next()
                .and_then(|v| version_token(v.to_owned()));
            Some(Legacy {
                pretty_name: text.clone(),
                ..simple("Gentoo", "gentoo", version)
            })
        }
        Kind::Slackware => {
            let text = first_line(data)?;
            let version = text
                .split(' ')
                .nth(1)
                .and_then(|v| version_token(v.to_owned()));
            Some(Legacy {
                pretty_name: text.clone(),
                ..simple("Slackware", "slackware", version)
            })
        }
        Kind::Debian => {
            let text = first_line(data)?;
            // "12.7", or "trixie/sid" on testing and unstable: no number.
            let version = version_token(text.clone());
            let mut legacy = simple("Debian GNU/Linux", "debian", version);
            if legacy.version.is_none() {
                legacy.codename = text.split('/').next().and_then(|c| valid_id(c.to_owned()));
            }
            Some(legacy)
        }
    }
}

fn first_line(data: &[u8]) -> Option<String> {
    let line = super::lines(data, 1).next()?;
    // `str::trim`, not just ASCII: the parsers below trim with it too, so a
    // line of only a vertical tab is empty here rather than an empty name later.
    let text = super::lossy(line, MAX_TEXT).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn simple(name: &str, id: &str, version: Option<String>) -> Legacy {
    let pretty_name = match &version {
        Some(v) => format!("{name} {v}"),
        None => name.to_owned(),
    };
    Legacy {
        name: name.to_owned(),
        id: id.to_owned(),
        pretty_name,
        version,
        codename: None,
    }
}

/// A version token: starts with a digit, made of digits and dots.
fn version_token(text: String) -> Option<String> {
    let token: String = text
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let token = token.trim_end_matches('.').to_owned();
    (token.starts_with(|c: char| c.is_ascii_digit())).then_some(token)
}

fn lsb(data: &[u8]) -> Option<Legacy> {
    let (mut id, mut release, mut codename, mut description) = (None, None, None, None);
    for (key, value) in os_release::pairs(data) {
        match key {
            b"DISTRIB_ID" => id = Some(value),
            b"DISTRIB_RELEASE" => release = Some(value),
            b"DISTRIB_CODENAME" => codename = Some(value),
            b"DISTRIB_DESCRIPTION" => description = Some(value),
            _ => {}
        }
    }
    let name = id.filter(|n| !n.trim().is_empty())?;
    let lower = lsb_alias(valid_id(name.trim().replace(' ', "-"))?);
    let version = release.and_then(version_token);
    let pretty_name = description
        .filter(|d| !d.trim().is_empty())
        .unwrap_or_else(|| simple(&name, &lower, version.clone()).pretty_name);
    Some(Legacy {
        name,
        id: lower,
        pretty_name,
        version,
        codename: codename.and_then(valid_id),
    })
}

/// The os-release id for a `DISTRIB_ID` spelled differently from it, as
/// `lsb_release -i` reports on those systems; any other id is kept.
fn lsb_alias(id: String) -> String {
    let alias = match id.as_str() {
        "manjarolinux" => "manjaro",
        "vmware-photon-os" => "photon",
        "redhatenterpriseserver" | "redhatenterpriseworkstation" | "redhatenterprise" => "rhel",
        "amazonami" => "amzn",
        "oracleserver" => "ol",
        "scientificsl" | "scientific-linux" => "scientific",
        "opensuse-project" => "opensuse",
        _ => return id,
    };
    alias.to_owned()
}

/// "CentOS Linux release 7.9.2009 (Core)", "Fedora release 20 (Heisenbug)",
/// "Red Hat Enterprise Linux Server release 7.9 (Maipo)". Every real sentence
/// has a version after "release"; text without one is not a release sentence.
fn red_hat_style(text: String) -> Option<Legacy> {
    let (name, rest) = text.split_once(" release ")?;
    let name = name.trim().to_owned();
    let version = Some(version_token(rest.to_owned())?);
    let codename = rest
        .split_once('(')
        .and_then(|(_, c)| c.split_once(')'))
        .map(|(c, _)| c.trim().to_owned())
        .filter(|c| !c.is_empty());
    let id = red_hat_id(&name)?;
    Some(Legacy {
        name,
        id,
        pretty_name: text.clone(),
        version,
        codename,
    })
}

fn red_hat_id(name: &str) -> Option<String> {
    let id = if name.starts_with("Red Hat Enterprise Linux") {
        "rhel"
    } else if name.starts_with("CentOS") {
        "centos"
    } else if name.starts_with("Fedora") {
        "fedora"
    } else if name.starts_with("Rocky") {
        "rocky"
    } else if name.starts_with("AlmaLinux") {
        "almalinux"
    } else if name.starts_with("Oracle Linux") {
        "ol"
    } else if name.starts_with("Scientific Linux") {
        "scientific"
    } else {
        return valid_id(name.split(' ').next()?.to_ascii_lowercase());
    };
    Some(id.to_owned())
}

/// "SUSE Linux Enterprise Server 11 (x86_64)" then `VERSION = 11`, `PATCHLEVEL = 4`.
fn suse(data: &[u8]) -> Option<Legacy> {
    let first = first_line(data)?;
    let name = first.split(" (").next().unwrap_or(&first).trim().to_owned();
    if name.is_empty() {
        return None;
    }
    let (mut version, mut patch) = (None, None);
    for line in super::lines(data, 32).skip(1) {
        let text = super::lossy(line, MAX_TEXT);
        if let Some((k, v)) = text.split_once('=') {
            match k.trim() {
                "VERSION" => version = version_token(v.to_owned()),
                "PATCHLEVEL" => patch = version_token(v.to_owned()),
                _ => {}
            }
        }
    }
    let version = match (version, patch) {
        (Some(v), Some(p)) if p != "0" => Some(format!("{v}.{p}")),
        (v, _) => v,
    };
    let id = if name.contains("Enterprise") {
        "sles"
    } else {
        "opensuse"
    };
    Some(Legacy {
        name,
        id: id.to_owned(),
        pretty_name: first.clone(),
        version,
        codename: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_release_red_hat_family_sentences() {
        let l = parse(Kind::RedHatStyle, b"CentOS Linux release 7.9.2009 (Core)\n").unwrap();
        assert_eq!(
            (l.id.as_str(), l.version.as_deref(), l.codename.as_deref()),
            ("centos", Some("7.9.2009"), Some("Core"))
        );
        assert_eq!(l.name, "CentOS Linux");
        let l = parse(Kind::RedHatStyle, b"CentOS release 6.10 (Final)").unwrap();
        assert_eq!(
            (l.id.as_str(), l.version.as_deref()),
            ("centos", Some("6.10"))
        );
        let l = parse(
            Kind::RedHatStyle,
            b"Red Hat Enterprise Linux Server release 7.9 (Maipo)",
        )
        .unwrap();
        assert_eq!(
            (l.id.as_str(), l.name.as_str()),
            ("rhel", "Red Hat Enterprise Linux Server")
        );
        let l = parse(Kind::RedHatStyle, b"Fedora release 20 (Heisenbug)").unwrap();
        assert_eq!(
            (l.id.as_str(), l.version.as_deref()),
            ("fedora", Some("20"))
        );
        let l = parse(Kind::RedHatStyle, b"CentOS Stream release 9").unwrap();
        assert_eq!(
            (l.id.as_str(), l.version.as_deref(), l.codename),
            ("centos", Some("9"), None)
        );
        assert_eq!(parse(Kind::RedHatStyle, b"no release word here"), None);
        assert_eq!(parse(Kind::RedHatStyle, b"no version word here"), None);
        assert_eq!(parse(Kind::RedHatStyle, b""), None);
    }

    #[test]
    fn test_release_lsb() {
        let l = parse(
            Kind::Lsb,
            b"DISTRIB_ID=Ubuntu\nDISTRIB_RELEASE=12.04\nDISTRIB_CODENAME=precise\nDISTRIB_DESCRIPTION=\"Ubuntu 12.04.5 LTS\"\n",
        )
        .unwrap();
        assert_eq!(l.name, "Ubuntu");
        assert_eq!(l.id, "ubuntu");
        assert_eq!(l.pretty_name, "Ubuntu 12.04.5 LTS");
        assert_eq!(l.version.as_deref(), Some("12.04"));
        assert_eq!(l.codename.as_deref(), Some("precise"));
        assert_eq!(parse(Kind::Lsb, b"DISTRIB_RELEASE=1\n"), None);
        // DISTRIB_IDs spelled differently from the os-release id (corpus).
        let id = |data: &[u8]| parse(Kind::Lsb, data).map(|l| l.id);
        assert_eq!(
            id(b"DISTRIB_ID=\"ManjaroLinux\"\n").as_deref(),
            Some("manjaro")
        );
        assert_eq!(
            id(b"DISTRIB_ID=\"VMware Photon OS\"\n").as_deref(),
            Some("photon")
        );
        assert_eq!(
            id(b"DISTRIB_ID=RedHatEnterpriseServer\n").as_deref(),
            Some("rhel")
        );
        assert_eq!(id(b"DISTRIB_ID=\"Mariner\"\n").as_deref(), Some("mariner"));
    }

    #[test]
    fn test_release_suse_alpine_arch_gentoo_slackware() {
        let l = parse(
            Kind::Suse,
            b"SUSE Linux Enterprise Server 11 (x86_64)\nVERSION = 11\nPATCHLEVEL = 4\n",
        )
        .unwrap();
        assert_eq!(
            (l.id.as_str(), l.version.as_deref()),
            ("sles", Some("11.4"))
        );
        assert_eq!(l.name, "SUSE Linux Enterprise Server 11");
        let l = parse(Kind::Alpine, b"3.20.3\n").unwrap();
        assert_eq!(
            (l.name.as_str(), l.id.as_str(), l.version.as_deref()),
            ("Alpine Linux", "alpine", Some("3.20.3"))
        );
        assert_eq!(parse(Kind::Alpine, b"").map(|l| l.id), None);
        assert_eq!(parse(Kind::Arch, b"").unwrap().id, "arch");
        let l = parse(Kind::Gentoo, b"Gentoo Base System release 2.17\n").unwrap();
        assert_eq!(
            (l.id.as_str(), l.version.as_deref()),
            ("gentoo", Some("2.17"))
        );
        let l = parse(Kind::Slackware, b"Slackware 15.0\n").unwrap();
        assert_eq!(
            (l.id.as_str(), l.version.as_deref()),
            ("slackware", Some("15.0"))
        );
    }

    #[test]
    fn test_release_debian_version_number_or_codename() {
        let l = parse(Kind::Debian, b"12.7\n").unwrap();
        assert_eq!(
            (l.name.as_str(), l.version.as_deref()),
            ("Debian GNU/Linux", Some("12.7"))
        );
        let l = parse(Kind::Debian, b"trixie/sid\n").unwrap();
        assert_eq!(
            (l.version.as_deref(), l.codename.as_deref()),
            (None, Some("trixie"))
        );
    }

    #[test]
    fn test_release_never_panics_on_odd_input() {
        for kind in [
            Kind::Lsb,
            Kind::RedHatStyle,
            Kind::Suse,
            Kind::Alpine,
            Kind::Arch,
            Kind::Gentoo,
            Kind::Slackware,
            Kind::Debian,
        ] {
            for input in [
                &b""[..],
                b"\xff\xfe",
                b" release ",
                b"x release (",
                b"x release )(",
                b"\n\n\n",
                b"VERSION = \nPATCHLEVEL",
                // Found by fuzzing: an empty name after trimming.
                b"\x0b",
                b" (x86_64)\n",
            ] {
                if let Some(l) = parse(kind, input) {
                    assert!(!l.id.is_empty() && !l.name.is_empty(), "{kind:?} {input:?}");
                }
            }
        }
    }
}
