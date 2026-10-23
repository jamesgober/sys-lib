//! # sys_lib
//!
//! System facts for Rust services, container-aware by default, plus the boot
//! kit every long-running service needs in its first milliseconds: build info,
//! instance info, and a process guard.
//!
//! Every reading is a file read or a system call. sys-lib never starts a
//! thread or a timer, never runs another program, and has no third-party
//! runtime dependencies.
//!
//! ## Tier 1
//!
//! Add it with Cargo's rename, `sys = { package = "sys-lib", version = "0.3" }`,
//! and every file writes `sys::`; without the rename, `use sys_lib as sys;`
//! does the same (the examples use that form so they compile either way).
//! One plain call per fact; static facts are read once and cached:
//!
//! ```
//! use sys_lib as sys;
//!
//! println!("{}", sys::os::name());         // "Ubuntu 24.04.1 LTS", "macOS 15.1 Sequoia", "Windows 11 Pro"
//! println!("{}", sys::os::distro());       // "Ubuntu"; "Windows", "macOS" elsewhere
//! println!("{}", sys::cpu::name());        // "AMD Ryzen 9 7950X"
//! println!("{} cores, {} threads", sys::cpu::physical(), sys::cpu::logical());
//! let workers = sys::cpu::available();     // what this process may use: affinity, cpuset, quota
//! # assert!(workers >= 1);
//! ```
//!
//! The boot kit:
//!
//! ```no_run
//! # #[cfg(all(feature = "instance", feature = "guard"))]
//! # fn main() -> Result<(), sys_lib::Error> {
//! use sys_lib as sys;
//!
//! let _guard = sys::process::guard("site")?;   // single-instance lock, umask, open-file limit
//! let build = sys::build_info!();              // with `sys::build::emit()` in build.rs
//! let instance = sys::Instance::start();
//! println!("{build}\n{instance}");
//! # Ok(())
//! # }
//! # #[cfg(not(all(feature = "instance", feature = "guard")))]
//! # fn main() {}
//! ```
//!
//! ## Status
//!
//! `0.3.0`, the boot kit: [`os`], [`cpu`], [`host`], [`process`], [`build`],
//! and instance info. Memory, disks, network, virtualization,
//! power, and device identity arrive through `0.6`; the complete approved API,
//! each item with the version it arrives in, is `docs/API.md` in the
//! [repository](https://github.com/jamesgober/sys-lib).

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
