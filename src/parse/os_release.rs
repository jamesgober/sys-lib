//! `os-release(5)`: newline-separated `KEY=value` assignments with shell-style
//! quoting. Values may be unquoted, or in single or double quotes; inside double
//! quotes (and unquoted) `\` escapes `"`, `\`, `` ` ``, and `$`. Comments start
//! with `#`; blank and invalid lines are skipped. Keys are `[A-Z0-9_]+`.

/// At most this many lines are read; os-release files have a few dozen.
pub(crate) const MAX_LINES: usize = 512;
/// At most this many bytes of one value are kept.
pub(crate) const MAX_VALUE: usize = 512;

/// The fields sys-lib uses, owned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct OsReleaseData {
    pub(crate) id: Option<String>,
    pub(crate) id_like: Vec<String>,
    pub(crate) name: Option<String>,
    pub(crate) pretty_name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) version_id: Option<String>,
    pub(crate) version_codename: Option<String>,
    pub(crate) variant_id: Option<String>,
    pub(crate) build_id: Option<String>,
}

/// Parses an os-release file.
pub(crate) fn parse(data: &[u8]) -> OsReleaseData {
    let mut out = OsReleaseData::default();
    for (key, value) in pairs(data) {
        match key {
            b"ID" => out.id = valid_id(value),
            b"ID_LIKE" => {
                out.id_like = value
                    .split(' ')
                    .filter_map(|v| valid_id(v.to_owned()))
                    .take(16)
                    .collect();
            }
            b"NAME" => out.name = non_empty(value),
            b"PRETTY_NAME" => out.pretty_name = non_empty(value),
            b"VERSION" => out.version = non_empty(value),
            b"VERSION_ID" => out.version_id = non_empty(value),
            b"VERSION_CODENAME" => out.version_codename = non_empty(value),
            b"VARIANT_ID" => out.variant_id = valid_id(value),
            b"BUILD_ID" => out.build_id = non_empty(value),
            _ => {}
        }
    }
    out
}

fn non_empty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

/// An id by the os-release rule: lowercase letters, digits, `.`, `_`, `-`.
pub(crate) fn valid_id(value: String) -> Option<String> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        });
    ok.then_some(value)
}

/// Every valid `KEY=value` assignment, in order (shared with `/etc/lsb-release`
/// and the other shell-style release files).
pub(crate) fn pairs(data: &[u8]) -> impl Iterator<Item = (&[u8], String)> {
    super::lines(data, MAX_LINES).filter_map(assignment)
}

/// One `KEY=value` line, or `None` for comments, blanks, and invalid lines.
fn assignment(line: &[u8]) -> Option<(&[u8], String)> {
    let line = super::trim(line);
    if line.is_empty() || line.starts_with(b"#") {
        return None;
    }
    let eq = line.iter().position(|&b| b == b'=')?;
    let (key, rest) = (&line[..eq], &line[eq + 1..]);
    if key.is_empty()
        || !key
            .iter()
            .all(|&b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
    {
        return None;
    }
    Some((key, unquote(rest)?))
}

/// Shell-style unquoting of one value. `None` for an unterminated quote.
fn unquote(raw: &[u8]) -> Option<String> {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len().min(MAX_VALUE));
    let mut i = 0;
    while i < raw.len() {
        match raw[i] {
            b'\'' => {
                // Single quotes: everything literal up to the next quote.
                let end = raw[i + 1..].iter().position(|&b| b == b'\'')?;
                out.extend_from_slice(&raw[i + 1..i + 1 + end]);
                i += end + 2;
            }
            b'"' => {
                i += 1;
                loop {
                    match raw.get(i)? {
                        b'"' => {
                            i += 1;
                            break;
                        }
                        b'\\' if matches!(raw.get(i + 1), Some(b'"' | b'\\' | b'`' | b'$')) => {
                            out.push(raw[i + 1]);
                            i += 2;
                        }
                        &b => {
                            out.push(b);
                            i += 1;
                        }
                    }
                }
            }
            b'\\' if i + 1 < raw.len() => {
                out.push(raw[i + 1]);
                i += 2;
            }
            // An unquoted space ends the value; the rest of the line is not part of it.
            b' ' | b'\t' => break,
            b => {
                out.push(b);
                i += 1;
            }
        }
        if out.len() > MAX_VALUE {
            out.truncate(MAX_VALUE);
            break;
        }
    }
    Some(super::lossy(&out, MAX_VALUE))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UBUNTU: &[u8] = br#"PRETTY_NAME="Ubuntu 24.04.1 LTS"
NAME="Ubuntu"
VERSION_ID="24.04"
VERSION="24.04.1 LTS (Noble Numbat)"
VERSION_CODENAME=noble
ID=ubuntu
ID_LIKE=debian
HOME_URL="https://www.ubuntu.com/"
UBUNTU_CODENAME=noble
LOGO=ubuntu-logo
"#;

    #[test]
    fn test_os_release_ubuntu_fields() {
        let r = parse(UBUNTU);
        assert_eq!(r.pretty_name.as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(r.name.as_deref(), Some("Ubuntu"));
        assert_eq!(r.id.as_deref(), Some("ubuntu"));
        assert_eq!(r.id_like, vec!["debian"]);
        assert_eq!(r.version_id.as_deref(), Some("24.04"));
        assert_eq!(r.version_codename.as_deref(), Some("noble"));
    }

    #[test]
    fn test_os_release_quoting_and_escapes() {
        // E-48: single and double quotes, backslash escapes, comments, blank lines, junk.
        let r = parse(
            b"# comment\n\nNAME='Single quoted $X'\nPRETTY_NAME=\"Esc \\\"q\\\" \\\\ \\$HOME\"\n\
              ID=unquoted\\_ok\nbroken line\nVERSION_ID=\"unterminated\nlower=ignored\n",
        );
        assert_eq!(r.name.as_deref(), Some("Single quoted $X"));
        assert_eq!(r.pretty_name.as_deref(), Some("Esc \"q\" \\ $HOME"));
        assert_eq!(r.id.as_deref(), Some("unquoted_ok"));
        assert_eq!(r.version_id, None);
    }

    #[test]
    fn test_os_release_invalid_ids_are_dropped() {
        let r = parse(b"ID=Ubuntu\nID_LIKE=\"rhel centos FEDORA fedora\"\nVARIANT_ID=server\n");
        assert_eq!(r.id, None);
        assert_eq!(r.id_like, vec!["rhel", "centos", "fedora"]);
        assert_eq!(r.variant_id.as_deref(), Some("server"));
    }

    #[test]
    fn test_os_release_crlf_tabs_and_empty_values() {
        let r = parse(b"NAME=\"Fedora Linux\"\r\nPRETTY_NAME=\r\n\tID=fedora\r\n");
        assert_eq!(r.name.as_deref(), Some("Fedora Linux"));
        assert_eq!(r.pretty_name, None);
        assert_eq!(r.id.as_deref(), Some("fedora"));
    }

    #[test]
    fn test_os_release_never_panics_on_odd_input() {
        for input in [
            &b""[..],
            b"=",
            b"\"",
            b"'",
            b"A=\\",
            b"A=\"\\",
            b"A='",
            b"A=\"abc",
            b"\xff\xfe=\x00",
            b"ID=\x00",
            b"PRETTY_NAME=\"\xff\"",
        ] {
            let _ = parse(input);
        }
    }

    #[test]
    fn test_os_release_values_are_bounded() {
        let mut data = b"NAME=".to_vec();
        data.extend(std::iter::repeat_n(b'a', 10_000));
        assert_eq!(parse(&data).name.map(|n| n.len()), Some(MAX_VALUE));
    }
}
