//! # sys_lib
//!
//! System facts for Rust services: operating system, CPU, memory, disks,
//! network, host, this process, virtualization, power, and device identity,
//! container-aware by default, plus the boot kit every long-running service
//! needs (build info, instance info, and a process guard).
//!
//! Every reading is a file read or a system call. sys-lib never starts a
//! thread or a timer, never runs another program, and has no third-party
//! runtime dependencies.
//!
//! ## Tier 1
//!
//! Add it with Cargo's rename, `sys = { package = "sys-lib", version = "0.2" }`,
//! and every file writes `sys::`; without the rename, `use sys_lib as sys;`
//! does the same (the examples use that form so they compile either way):
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("sys-lib {}", sys::VERSION);
//! ```
//!
//! The Tier 1 functions arrive from `0.3.0` on, one plain call each:
//!
//! ```text
//! sys::os::name()                // "Ubuntu 24.04.1 LTS", "macOS 15.1 Sequoia", "Windows 11 Pro"
//! sys::os::distro()              // "Ubuntu"; "Windows", "macOS" elsewhere
//! sys::cpu::name()               // "AMD Ryzen 9 7950X"
//! sys::cpu::physical()           // cores
//! sys::cpu::available()          // what this process may use (affinity, cpuset, quota)
//! sys::mem::total()?             // the container's limit inside a container
//! sys::mem::used_percent()?      // "32.8%"
//! sys::cpu::used()?              // blocks for 100 ms
//! sys::net::traffic()?           // download and upload; blocks for 1 s
//! sys::disk::space(path)?        // the filesystem holding `path`
//! sys::process::guard("site")?   // single-instance lock, umask, open-file limit
//! ```
//!
//! ## Status
//!
//! Design locked (`0.2.0`). The complete API is in `docs/API.md`, the design
//! in `dev/PLAN.md`, and the schedule in `dev/ROADMAP.md` (all in the
//! [repository](https://github.com/jamesgober/sys-lib)); the boot kit ships in
//! `0.3.0`.

#![doc(html_root_url = "https://docs.rs/sys-lib")]
#![cfg_attr(docsrs, feature(doc_cfg))]
// `deny`, not `forbid`: the per-platform `sys` modules lift it locally to call
// the OS where std has no API. Nowhere else.
#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]
#![deny(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::ptr_as_ptr,
    clippy::cast_ptr_alignment
)]
#![cfg_attr(
    not(test),
    deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
        clippy::dbg_macro
    )
)]

/// Crate version string, populated by Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod build;
pub mod cpu;
pub mod host;
#[cfg(feature = "instance")]
#[cfg_attr(docsrs, doc(cfg(feature = "instance")))]
pub mod instance;
pub mod os;
pub mod process;

mod error;
mod fmt;
#[cfg(fuzzing)]
#[doc(hidden)]
pub mod __fuzz {
    //! Parser entry points for `fuzz/`; not part of the API.
    pub use crate::fuzz::*;
}
#[cfg(fuzzing)]
mod fuzz;
#[cfg(any(target_os = "linux", target_os = "android", test))]
mod linux;
mod parse;
mod sys;
mod units;

pub use build::{BuildInfo, PublicBuild};
pub use error::{Error, Result};
#[cfg(feature = "instance")]
#[cfg_attr(docsrs, doc(cfg(feature = "instance")))]
pub use instance::{Instance, InstanceId};
pub use units::{Bytes, Percent, Units};

#[cfg(test)]
mod tests {
    use super::VERSION;

    #[test]
    fn test_version_matches_manifest() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        // Pre-release and build metadata (`1.0.0-rc.1`, `+build`) follow the core.
        let core = VERSION.split(['-', '+']).next().unwrap_or_default();
        assert_eq!(core.split('.').count(), 3, "expected MAJOR.MINOR.PATCH");
        assert!(
            core.split('.').all(|part| part.parse::<u64>().is_ok()),
            "every version component is a number"
        );
    }
}
