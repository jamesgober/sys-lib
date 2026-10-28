//! The host on Linux: its name (`/proc/sys/kernel/hostname`) and the
//! system-wide open-file count (`/proc/sys/fs/file-nr`).

use super::Root;
use crate::{Error, Result, host::OpenFiles, parse::procfs};

/// The longest host name kept (E-95).
pub(crate) const MAX_NAME: usize = 255;

pub(crate) fn name(root: &Root) -> Result<String> {
    root.with(
        "/proc/sys/kernel/hostname",
        "host name",
        "/proc/sys/kernel/hostname",
        |d| crate::parse::lossy(crate::parse::trim(d), MAX_NAME),
    )
}

/// Open files and the maximum, from `/proc/sys/fs/file-nr`.
pub(crate) fn open_files(root: &Root) -> Result<OpenFiles> {
    let (open, max) = root
        .with(
            "/proc/sys/fs/file-nr",
            "open files",
            "/proc/sys/fs/file-nr",
            procfs::file_nr,
        )?
        .ok_or(Error::Malformed {
            what: "open files",
            source: "/proc/sys/fs/file-nr",
            line: None,
        })?;
    Ok(OpenFiles {
        open: Some(open),
        max: Some(max),
    })
}

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_linux_host_open_files_from_a_fixture() {
        let f = open_files(&fixtures::root("wsl2")).unwrap();
        assert!(f.open.unwrap() > 0 && f.open <= f.max);
        assert!(
            open_files(&fixtures::root("no-such-fixture"))
                .unwrap_err()
                .is_unavailable()
        );
    }

    #[test]
    fn test_linux_host_name_from_fixtures() {
        for f in fixtures::ALL {
            assert_eq!(name(&fixtures::root(f)).unwrap(), "fixture-host", "{f}");
        }
    }
}
