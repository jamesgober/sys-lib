//! `resolv.conf(5)`: `nameserver`, `search`, and `domain` lines. Comments start
//! with `#` or `;`; a later `search` or `domain` replaces an earlier one, and
//! each `nameserver` adds a server.

use std::net::IpAddr;

use super::lines;

/// At most this many servers, or search domains, are kept before `TooLarge`.
pub(crate) const MAX_ENTRIES: usize = 64;
/// At most this many lines are read.
const MAX_LINES: usize = 4096;
/// The longest search domain: a DNS name is at most 253 bytes.
const MAX_DOMAIN: usize = 253;

/// What a `resolv.conf` says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Resolv {
    pub(crate) servers: Vec<IpAddr>,
    pub(crate) search: Vec<String>,
    /// More than [`MAX_ENTRIES`] servers or search domains were listed.
    pub(crate) too_large: bool,
}

/// A server address: an IPv6 scope (`fe80::1%eth0`) is dropped with its zone.
fn server(word: &[u8]) -> Option<IpAddr> {
    let text = core::str::from_utf8(word).ok()?;
    text.split('%').next()?.parse().ok()
}

/// A search domain: printable ASCII with no path or control characters.
fn domain(word: &[u8]) -> Option<String> {
    let ok = !word.is_empty()
        && word.len() <= MAX_DOMAIN
        && word
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    ok.then(|| String::from_utf8_lossy(word).into_owned())
}

/// Parses a `resolv.conf`.
pub(crate) fn parse(data: &[u8]) -> Resolv {
    let mut out = Resolv::default();
    for line in lines(data, MAX_LINES) {
        let mut words = line
            .split(|b| b.is_ascii_whitespace())
            .filter(|w| !w.is_empty());
        let Some(key) = words.next() else {
            continue;
        };
        match key {
            b"nameserver" => {
                if let Some(ip) = words.next().and_then(server) {
                    if out.servers.len() == MAX_ENTRIES {
                        out.too_large = true;
                    } else if !out.servers.contains(&ip) {
                        out.servers.push(ip);
                    }
                }
            }
            b"search" | b"domain" => {
                out.search.clear();
                for word in words {
                    if word.starts_with(b"#") || word.starts_with(b";") {
                        break;
                    }
                    if let Some(d) = domain(word) {
                        if out.search.len() == MAX_ENTRIES {
                            out.too_large = true;
                        } else if !out.search.contains(&d) {
                            out.search.push(d);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether `ip` is a local stub resolver: systemd-resolved's `127.0.0.53` or
/// `127.0.0.54`.
pub(crate) fn is_stub(ip: &IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v4) if matches!(v4.octets(), [127, 0, 0, 53 | 54]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolv_servers_and_search() {
        let r = parse(
            b"# generated\n; another comment\nnameserver 1.1.1.1\nnameserver 2606:4700::1111\n\
              nameserver fe80::1%eth0\nsearch corp.example  lab.example # trailing\n\
              options edns0\n",
        );
        assert_eq!(r.servers.len(), 3);
        assert_eq!(r.servers[0].to_string(), "1.1.1.1");
        assert_eq!(r.servers[2].to_string(), "fe80::1");
        assert_eq!(r.search, vec!["corp.example", "lab.example"]);
        assert!(!r.too_large);
    }

    #[test]
    fn test_resolv_last_search_wins_and_duplicates_go() {
        let r = parse(
            b"domain old.example\nsearch new.example\nnameserver 9.9.9.9\nnameserver 9.9.9.9\n",
        );
        assert_eq!(r.search, vec!["new.example"]);
        assert_eq!(r.servers.len(), 1);
    }

    #[test]
    fn test_resolv_junk_is_ignored() {
        let r =
            parse(b"nameserver\nnameserver not-an-ip\nsearch ../etc\x00 ok.example\n\xff\xfe\n");
        assert!(r.servers.is_empty());
        assert_eq!(r.search, vec!["ok.example"]);
    }

    #[test]
    fn test_resolv_budget() {
        let mut text = String::new();
        for i in 0..=MAX_ENTRIES {
            text.push_str(&format!("nameserver 10.0.{}.{}\n", i / 256, i % 256));
        }
        let r = parse(text.as_bytes());
        assert_eq!(r.servers.len(), MAX_ENTRIES);
        assert!(r.too_large);
    }

    #[test]
    fn test_resolv_stub() {
        assert!(is_stub(&"127.0.0.53".parse().unwrap()));
        assert!(!is_stub(&"127.0.0.1".parse().unwrap()));
        assert!(!is_stub(&"::1".parse().unwrap()));
    }
}
