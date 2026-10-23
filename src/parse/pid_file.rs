//! The process guard's `<name>.pid` file: a decimal process id and a newline.

/// The largest pid file read; anything longer is not ours.
pub(crate) const MAX_BYTES: usize = 128;

/// The process id in a pid file, or `None` for anything else.
pub(crate) fn parse(data: &[u8]) -> Option<u32> {
    if data.len() > MAX_BYTES {
        return None;
    }
    let pid = u32::try_from(super::parse_u64(super::trim(data))?).ok()?;
    (pid != 0).then_some(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pid_file_strict() {
        assert_eq!(parse(b"48213\n"), Some(48213));
        assert_eq!(parse(b"  7 "), Some(7));
        assert_eq!(parse(b"0\n"), None);
        assert_eq!(parse(b"-1"), None);
        assert_eq!(parse(b"12 34"), None);
        assert_eq!(parse(b"4294967296"), None);
        assert_eq!(parse(b""), None);
        assert_eq!(parse(&[b'1'; 129]), None);
    }
}
