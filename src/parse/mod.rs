//! Parsers for the data the operating system hands back: safe Rust over
//! `&[u8]`, bounded, without recursion, and never panicking on any input.
//! Everything here is fuzzed and runs under Miri.
//!
//! The Linux parsers are compiled on every host so their tests, fuzz targets,
//! and Miri runs cover them everywhere; outside Linux nothing else calls them.

#![cfg_attr(
    not(all(
        any(target_os = "linux", target_os = "android"),
        target_pointer_width = "64"
    )),
    allow(dead_code)
)]

pub(crate) mod cgroup;
pub(crate) mod cpu_list;
pub(crate) mod cpu_name;
pub(crate) mod cpuinfo;
pub(crate) mod mountinfo;
pub(crate) mod os_release;
#[cfg_attr(not(feature = "guard"), allow(dead_code))] // the guard's pid file
pub(crate) mod pid_file;
pub(crate) mod proc_status;
pub(crate) mod procfs;
pub(crate) mod release;
pub(crate) mod version;

/// Splits `data` into lines, at most `max` of them, without allocating.
pub(crate) fn lines(data: &[u8], max: usize) -> impl Iterator<Item = &[u8]> {
    data.split(|&b| b == b'\n')
        .take(max)
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
}

/// Trims ASCII whitespace from both ends.
pub(crate) fn trim(data: &[u8]) -> &[u8] {
    data.trim_ascii()
}

/// A decimal `u64`, with no sign, no spaces, and no overflow.
pub(crate) fn parse_u64(data: &[u8]) -> Option<u64> {
    if data.is_empty() || data.len() > 20 {
        return None;
    }
    let mut value: u64 = 0;
    for &b in data {
        let digit = b.checked_sub(b'0').filter(|d| *d <= 9)?;
        value = value.checked_mul(10)?.checked_add(u64::from(digit))?;
    }
    Some(value)
}

/// A decimal `i64` with an optional leading `-`.
pub(crate) fn parse_i64(data: &[u8]) -> Option<i64> {
    match data.strip_prefix(b"-") {
        Some(rest) => {
            let v = parse_u64(rest)?;
            if v == 1 << 63 {
                Some(i64::MIN)
            } else {
                i64::try_from(v).ok().map(|v| -v)
            }
        }
        None => i64::try_from(parse_u64(data)?).ok(),
    }
}

/// A hexadecimal `u64`, with or without a `0x` prefix.
pub(crate) fn parse_hex(data: &[u8]) -> Option<u64> {
    let digits = data
        .strip_prefix(b"0x")
        .or_else(|| data.strip_prefix(b"0X"))
        .unwrap_or(data);
    if digits.is_empty() || digits.len() > 16 {
        return None;
    }
    let mut value: u64 = 0;
    for &b in digits {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => return None,
        };
        value = (value << 4) | u64::from(digit);
    }
    Some(value)
}

/// `data` as UTF-8, replacing invalid sequences, bounded to `max` bytes at a
/// character boundary.
pub(crate) fn lossy(data: &[u8], max: usize) -> String {
    let mut s = String::from_utf8_lossy(&data[..data.len().min(max)]).into_owned();
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_u64_rejects_signs_spaces_and_overflow() {
        assert_eq!(parse_u64(b"0"), Some(0));
        assert_eq!(parse_u64(b"18446744073709551615"), Some(u64::MAX));
        assert_eq!(parse_u64(b"18446744073709551616"), None);
        assert_eq!(parse_u64(b""), None);
        assert_eq!(parse_u64(b"+1"), None);
        assert_eq!(parse_u64(b" 1"), None);
        assert_eq!(parse_u64(b"1a"), None);
        assert_eq!(parse_u64(b"000000000000000000001"), None);
    }

    #[test]
    fn test_parse_i64_handles_the_extremes() {
        assert_eq!(parse_i64(b"-1"), Some(-1));
        assert_eq!(parse_i64(b"-9223372036854775808"), Some(i64::MIN));
        assert_eq!(parse_i64(b"9223372036854775807"), Some(i64::MAX));
        assert_eq!(parse_i64(b"9223372036854775808"), None);
        assert_eq!(parse_i64(b"-"), None);
    }

    #[test]
    fn test_parse_hex_accepts_prefix_and_both_cases() {
        assert_eq!(parse_hex(b"0x41"), Some(0x41));
        assert_eq!(parse_hex(b"d0B"), Some(0xd0b));
        assert_eq!(parse_hex(b"0x"), None);
        assert_eq!(parse_hex(b"ffffffffffffffff"), Some(u64::MAX));
        assert_eq!(parse_hex(b"1ffffffffffffffff"), None);
        assert_eq!(parse_hex(b"g"), None);
    }

    #[test]
    fn test_lossy_truncates_at_a_char_boundary() {
        assert_eq!(lossy("héllo".as_bytes(), 2), "h");
        assert_eq!(lossy(b"ab\xffcd", 64), "ab\u{fffd}cd");
        assert_eq!(lossy(b"abc", 64), "abc");
    }

    #[test]
    fn test_lines_bounded_and_strips_cr() {
        let v: Vec<_> = lines(b"a\r\nb\nc", 2).collect();
        assert_eq!(v, vec![&b"a"[..], &b"b"[..]]);
    }
}
