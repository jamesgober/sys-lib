//! The host name on Linux, from `/proc/sys/kernel/hostname`.

use super::Root;
use crate::Result;

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

#[cfg(test)]
mod tests {
    use super::{super::fixtures, *};

    #[test]
    fn test_linux_host_name_from_fixtures() {
        for f in fixtures::ALL {
            assert_eq!(name(&fixtures::root(f)).unwrap(), "fixture-host", "{f}");
        }
    }
}
