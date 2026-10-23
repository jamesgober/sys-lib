//! `/proc/<pid>/status`: `Key:\tvalue` lines.

/// The value of `key` (without the colon), trimmed.
pub(crate) fn field<'a>(data: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    super::lines(data, 256).find_map(|line| {
        let rest = line.strip_prefix(key)?.strip_prefix(b":")?;
        Some(super::trim(rest))
    })
}

/// A numeric field such as `Threads` or `PPid`.
pub(crate) fn number(data: &[u8], key: &[u8]) -> Option<u64> {
    super::parse_u64(field(data, key)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &[u8] =
        b"Name:\tsys-lib\nUmask:\t0022\nState:\tR (running)\nPPid:\t12\nThreads:\t3\n\
        Cpus_allowed:\tff\nCpus_allowed_list:\t0-7\nVmHWM:\t  2048 kB\n";

    #[test]
    fn test_proc_status_fields() {
        assert_eq!(field(STATUS, b"Cpus_allowed_list"), Some(&b"0-7"[..]));
        assert_eq!(number(STATUS, b"Threads"), Some(3));
        assert_eq!(number(STATUS, b"PPid"), Some(12));
        // `Cpus_allowed` must not match `Cpus_allowed_list` and vice versa.
        assert_eq!(field(STATUS, b"Cpus_allowed"), Some(&b"ff"[..]));
        assert_eq!(field(STATUS, b"Missing"), None);
        assert_eq!(number(STATUS, b"State"), None);
        assert_eq!(field(b"", b"Threads"), None);
    }
}
