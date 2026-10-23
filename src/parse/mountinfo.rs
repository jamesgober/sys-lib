//! `/proc/self/mountinfo`: one mount per line,
//! `id parent major:minor root mount-point options [optional...] - fstype source super-options`,
//! with space, tab, newline, and backslash escaped as octal (`\040`, E-37).

use std::borrow::Cow;

/// At most this many lines are read (threat T7: a node with 100 000 mounts).
pub(crate) const MAX_LINES: usize = 1 << 20;

/// One mount.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Mount<'a> {
    pub(crate) root: Cow<'a, [u8]>,
    pub(crate) mount_point: Cow<'a, [u8]>,
    pub(crate) fstype: &'a [u8],
    pub(crate) source: Cow<'a, [u8]>,
    pub(crate) super_options: &'a [u8],
}

/// The mounts, in file order; malformed lines are skipped.
pub(crate) fn mounts(data: &[u8]) -> impl Iterator<Item = Mount<'_>> {
    super::lines(data, MAX_LINES).filter_map(line)
}

fn line(line: &[u8]) -> Option<Mount<'_>> {
    let mut fields = line.split(|&b| b == b' ');
    let _id = fields.next()?;
    let _parent = fields.next()?;
    let _dev = fields.next()?;
    let root = fields.next()?;
    let mount_point = fields.next()?;
    let _options = fields.next()?;
    // Optional fields up to the lone "-" separator.
    loop {
        if fields.next()? == b"-" {
            break;
        }
    }
    let fstype = fields.next()?;
    let source = fields.next()?;
    let super_options = fields.next().unwrap_or(b"");
    if root.is_empty() || mount_point.is_empty() || fstype.is_empty() {
        return None;
    }
    Some(Mount {
        root: unescape(root),
        mount_point: unescape(mount_point),
        fstype,
        source: unescape(source),
        super_options,
    })
}

/// Decodes `\NNN` octal escapes; anything else is kept as written.
pub(crate) fn unescape(field: &[u8]) -> Cow<'_, [u8]> {
    if !field.contains(&b'\\') {
        return Cow::Borrowed(field);
    }
    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        let octal = field
            .get(i + 1..i + 4)
            .filter(|d| field[i] == b'\\' && d.iter().all(|b| (b'0'..=b'7').contains(b)));
        match octal {
            Some(d) => {
                let v = u16::from(d[0] - b'0') * 64
                    + u16::from(d[1] - b'0') * 8
                    + u16::from(d[2] - b'0');
                out.push(u8::try_from(v).unwrap_or(b'?'));
                i += 4;
            }
            None => {
                out.push(field[i]);
                i += 1;
            }
        }
    }
    Cow::Owned(out)
}

/// Whether a comma-separated option list contains `option`.
pub(crate) fn has_option(options: &[u8], option: &[u8]) -> bool {
    options.split(|&b| b == b',').any(|o| o == option)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &[u8] = b"22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw\n\
        35 22 0:29 / /sys/fs/cgroup rw,nosuid shared:9 - cgroup2 cgroup2 rw,nsdelegate\n\
        36 22 0:30 / /sys/fs/cgroup/cpu,cpuacct rw - cgroup cgroup rw,cpu,cpuacct\n\
        40 22 0:31 /docker/abc /mnt/my\\040disk rw - ext4 /dev/sdb1 rw\n\
        garbage line\n";

    #[test]
    fn test_mountinfo_fields_with_and_without_optional_fields() {
        let m: Vec<_> = mounts(DATA).collect();
        assert_eq!(m.len(), 4);
        assert_eq!(m[0].fstype, b"ext4");
        assert_eq!(&*m[1].mount_point, b"/sys/fs/cgroup");
        assert_eq!(m[1].fstype, b"cgroup2");
        // No optional fields: "-" follows the options directly.
        assert_eq!(m[2].fstype, b"cgroup");
        assert!(has_option(m[2].super_options, b"cpu"));
        assert!(!has_option(m[2].super_options, b"cpuset"));
        assert_eq!(&*m[3].root, b"/docker/abc");
        assert_eq!(&*m[3].mount_point, b"/mnt/my disk");
    }

    #[test]
    fn test_mountinfo_unescape_octal() {
        // E-37: space, tab, newline, backslash.
        assert_eq!(&*unescape(br"a\040b\011c\012d\134e"), b"a b\tc\nd\\e");
        assert_eq!(&*unescape(br"\777"), b"?");
        assert_eq!(&*unescape(br"trailing\04"), br"trailing\04");
        assert!(matches!(unescape(b"plain"), Cow::Borrowed(_)));
    }

    #[test]
    fn test_mountinfo_never_panics_on_odd_input() {
        for input in [
            &b""[..],
            b"-",
            b"1 2 3 4 5 6 -",
            b"1 2 3 4 5 6 - x",
            b"\\",
            b"a b c d e f g h",
        ] {
            let _ = mounts(input).count();
        }
    }
}
