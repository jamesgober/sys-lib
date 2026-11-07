//! The host on Linux: its name (`/proc/sys/kernel/hostname`) and the
//! system-wide open-file count (`/proc/sys/fs/file-nr`).

use super::Root;
use crate::{
    Error, Result,
    host::{OpenFiles, Session},
    parse::{procfs, tz, utmp},
};

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

/// The zone's name: a valid `TZ`, the `/etc/localtime` link, then
/// `/etc/timezone` (E-92, E-93). Nothing here opens a name as a path.
pub(crate) fn zone_name(root: &Root, env: Option<&str>) -> Option<String> {
    if let Some(name) = env.and_then(tz::from_env) {
        return Some(name.to_owned());
    }
    let link = root.link_target("/etc/localtime");
    if let Some(name) = link.as_deref().and_then(tz::from_link) {
        return Some(name.to_owned());
    }
    root.with("/etc/timezone", "time zone", "/etc/timezone", |d| {
        tz::from_file(d).map(str::to_owned)
    })
    .ok()
    .flatten()
}

/// The record layout of this target's utmp file.
const fn layout() -> utmp::Layout {
    if cfg!(target_arch = "x86_64") {
        utmp::COMPAT32
    } else {
        utmp::NATIVE64
    }
}

/// Who is logged in, from the utmp database. Systems that no longer keep one
/// have nobody to list.
pub(crate) fn sessions(root: &Root) -> Result<Vec<Session>> {
    let limit = utmp::MAX_RECORDS * 400;
    for path in ["/run/utmp", "/var/run/utmp"] {
        match root.read_prefix(path, limit, "sessions", path) {
            Ok(data) => {
                return Ok(utmp::parse(&data, layout())
                    .into_iter()
                    .map(|e| Session {
                        user: e.user,
                        terminal: e.terminal,
                        remote: e.remote,
                        since: e.since,
                    })
                    .collect());
            }
            Err(Error::NotFound { .. }) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Vec::new())
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
