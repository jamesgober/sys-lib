//! The utmp database: fixed-size records of who is logged in. Read as a file, so
//! no libc iterator state is shared with the application, and so it is checked
//! like everything else the OS hands back.

use std::time::{Duration, SystemTime};

/// The most records read.
pub(crate) const MAX_RECORDS: usize = 4096;
/// `ut_type` of a login session.
const USER_PROCESS: i16 = 7;

/// glibc's record layout. x86-64 keeps 32-bit times for compatibility with
/// 32-bit programs (`__WORDSIZE_TIME64_COMPAT32`); other 64-bit targets do not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    size: usize,
    seconds_at: usize,
    seconds_len: usize,
}

/// `struct utmpx` on x86-64 Linux.
pub(crate) const COMPAT32: Layout = Layout {
    size: 384,
    seconds_at: 340,
    seconds_len: 4,
};
/// `struct utmpx` on other 64-bit Linux targets.
pub(crate) const NATIVE64: Layout = Layout {
    size: 400,
    seconds_at: 344,
    seconds_len: 8,
};

impl Layout {
    /// The record size.
    #[cfg(test)]
    pub(crate) const fn size(self) -> usize {
        self.size
    }
}

const LINE_AT: usize = 8;
const LINE_LEN: usize = 32;
const USER_AT: usize = 44;
const USER_LEN: usize = 32;
const HOST_AT: usize = 76;
const HOST_LEN: usize = 256;

/// A login session, owned. The remote host is read only to see that it is there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) user: String,
    pub(crate) terminal: Option<String>,
    pub(crate) remote: bool,
    pub(crate) since: Option<SystemTime>,
}

/// A NUL-padded field.
fn field(bytes: &[u8]) -> &[u8] {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    &bytes[..end]
}

fn entry(r: &[u8], layout: Layout) -> Option<Entry> {
    if i16::from_ne_bytes([r[0], r[1]]) != USER_PROCESS {
        return None;
    }
    let user = field(&r[USER_AT..USER_AT + USER_LEN]);
    if user.is_empty() {
        return None;
    }
    let line = field(&r[LINE_AT..LINE_AT + LINE_LEN]);
    let host = field(&r[HOST_AT..HOST_AT + HOST_LEN]);
    let raw = &r[layout.seconds_at..layout.seconds_at + layout.seconds_len];
    let seconds = match *raw {
        [a, b, c, d] => u64::from(u32::from_ne_bytes([a, b, c, d])),
        [a, b, c, d, e, f, g, h] => u64::from_ne_bytes([a, b, c, d, e, f, g, h]),
        _ => 0,
    };
    Some(Entry {
        user: super::lossy(user, USER_LEN),
        terminal: (!line.is_empty()).then(|| super::lossy(line, LINE_LEN)),
        remote: !host.is_empty(),
        since: (seconds != 0)
            .then(|| SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds)))
            .flatten(),
    })
}

/// The login sessions in a utmp file, at most [`MAX_RECORDS`] records read. A
/// trailing partial record is ignored.
pub(crate) fn parse(data: &[u8], layout: Layout) -> Vec<Entry> {
    data.chunks_exact(layout.size)
        .take(MAX_RECORDS)
        .filter_map(|r| entry(r, layout))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(layout: Layout, kind: i16, user: &str, line: &str, host: &str, secs: u32) -> Vec<u8> {
        let mut r = vec![0u8; layout.size];
        r[..2].copy_from_slice(&kind.to_ne_bytes());
        r[LINE_AT..LINE_AT + line.len()].copy_from_slice(line.as_bytes());
        r[USER_AT..USER_AT + user.len()].copy_from_slice(user.as_bytes());
        r[HOST_AT..HOST_AT + host.len()].copy_from_slice(host.as_bytes());
        if layout.seconds_len == 4 {
            r[layout.seconds_at..layout.seconds_at + 4].copy_from_slice(&secs.to_ne_bytes());
        } else {
            r[layout.seconds_at..layout.seconds_at + 8]
                .copy_from_slice(&u64::from(secs).to_ne_bytes());
        }
        r
    }

    #[test]
    fn test_utmp_both_layouts() {
        for layout in [COMPAT32, NATIVE64] {
            let mut data = record(layout, 2, "reboot", "~", "", 5);
            data.extend(record(
                layout,
                USER_PROCESS,
                "alice",
                "pts/0",
                "10.0.0.9",
                1000,
            ));
            data.extend(record(layout, USER_PROCESS, "bob", "tty1", "", 0));
            data.extend(record(layout, 8, "gone", "pts/1", "", 7));
            data.extend([1, 2, 3]);
            let e = parse(&data, layout);
            assert_eq!(e.len(), 2);
            assert_eq!(e[0].user, "alice");
            assert_eq!(e[0].terminal.as_deref(), Some("pts/0"));
            assert!(e[0].remote);
            assert_eq!(
                e[0].since,
                SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(1000))
            );
            assert!(!e[1].remote);
            assert_eq!(e[1].since, None);
            assert!(
                !format!("{e:?}").contains("10.0.0.9"),
                "the address stays out"
            );
        }
    }

    #[test]
    fn test_utmp_junk_and_budget() {
        assert!(parse(&[], COMPAT32).is_empty());
        assert!(parse(&[0xFF; 1000], NATIVE64).is_empty());
        let one = record(COMPAT32, USER_PROCESS, "u", "t", "", 1);
        assert_eq!(
            parse(&one.repeat(MAX_RECORDS + 5), COMPAT32).len(),
            MAX_RECORDS
        );
        // A user with no terminal, and bytes that are not UTF-8.
        let mut r = record(COMPAT32, USER_PROCESS, "x", "", "", 1);
        r[USER_AT] = 0xFF;
        assert_eq!(parse(&r, COMPAT32)[0].terminal, None);
    }
}
