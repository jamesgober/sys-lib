//! The platform boundary.
//!
//! Public functions never branch on `target_os`; they call into this module, and
//! each platform module provides the same `pub(crate)` functions. Unsafe code is
//! allowed only inside the platform modules declared here, and only to move bytes
//! from the operating system into a buffer: every parser that reads those bytes
//! is safe Rust.
//!
//! Each platform has an `ffi` module: the complete, reviewed list of the system
//! calls, structs, and constants sys-lib uses on that platform, declared by hand
//! (no `libc`, no `windows-sys`; `dev/PLAN.md` D-003). Every `#[repr(C)]` struct
//! carries a size and alignment assertion for each supported target, and its
//! unit tests compare size, alignment, and every field offset with the
//! definitions in `libc` or `windows-sys`, used as test oracles only (D-012).
//! CI compiles those tests for every target, cross targets included, so a layout
//! that differs from the platform headers does not build.

/// Asserts at compile time that a `#[repr(C)]` OS struct has the size and
/// alignment the platform headers give it.
#[allow(unused_macros)]
macro_rules! layout {
    ($t:ty, size = $size:expr, align = $align:expr) => {
        const _: () = assert!(
            ::core::mem::size_of::<$t>() == $size && ::core::mem::align_of::<$t>() == $align,
            concat!(
                "layout of ",
                stringify!($t),
                " differs from the platform headers"
            ),
        );
    };
}

/// Test oracle: asserts at compile time that one of our OS structs has the same
/// size, alignment, and field offsets as the oracle crate's definition.
/// `prefix` checks offsets only, for structs we deliberately declare shorter
/// than the newest SDK (the OS fills at most the size we pass).
#[cfg(test)]
#[allow(unused_macros)]
macro_rules! same_layout {
    ($ours:ty, $theirs:ty $(, $a:ident $(. $an:ident)* => $b:ident $(. $bn:ident)*)* $(,)?) => {
        const _: () = {
            assert!(::core::mem::size_of::<$ours>() == ::core::mem::size_of::<$theirs>(),
                concat!("size of ", stringify!($ours)));
            assert!(::core::mem::align_of::<$ours>() == ::core::mem::align_of::<$theirs>(),
                concat!("alignment of ", stringify!($ours)));
            $(assert!(
                ::core::mem::offset_of!($ours, $a $(. $an)*)
                    == ::core::mem::offset_of!($theirs, $b $(. $bn)*),
                concat!("offset of ", stringify!($ours), ".", stringify!($a $(. $an)*))
            );)*
        };
    };
    (prefix $ours:ty, $theirs:ty $(, $a:ident => $b:ident)* $(,)?) => {
        const _: () = {
            assert!(::core::mem::size_of::<$ours>() <= ::core::mem::size_of::<$theirs>(),
                concat!("size of ", stringify!($ours)));
            $(assert!(
                ::core::mem::offset_of!($ours, $a) == ::core::mem::offset_of!($theirs, $b),
                concat!("offset of ", stringify!($ours), ".", stringify!($a))
            );)*
        };
    };
}

// The readings, answered `Unsupported` until a platform defines its own (see
// the module's documentation). FreeBSD defines every item itself.
mod detection;
mod environment;
mod pending;

// CPUID: shared by the x86 backends.
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[allow(unsafe_code)]
mod x86;
#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
pub(crate) use x86::invariant_tsc as x86_invariant_tsc;

// Linux and Android on 64-bit targets, and 32-bit glibc Linux on x86 and Arm.
// Struct layouts are verified for LP64 (glibc, musl, bionic) and for those two
// 32-bit targets; other 32-bit Linux targets (musl, bionic) have no verified
// layout and answer as unsupported (D-003, D-063).
#[cfg(any(
    all(
        any(target_os = "linux", target_os = "android"),
        target_pointer_width = "64"
    ),
    all(
        target_os = "linux",
        target_env = "gnu",
        any(target_arch = "x86", target_arch = "arm")
    )
))]
#[allow(unsafe_code)]
mod linux;
#[cfg(any(
    all(
        any(target_os = "linux", target_os = "android"),
        target_pointer_width = "64"
    ),
    all(
        target_os = "linux",
        target_env = "gnu",
        any(target_arch = "x86", target_arch = "arm")
    )
))]
pub(crate) use linux::*;

// macOS and iOS: one module, with the macOS-only parts (IOKit, getfsstat) gated
// inside it (D-013).
#[cfg(all(
    any(target_os = "macos", target_os = "ios"),
    target_pointer_width = "64"
))]
#[allow(unsafe_code)]
mod apple;
#[cfg(all(
    any(target_os = "macos", target_os = "ios"),
    target_pointer_width = "64"
))]
pub(crate) use apple::*;

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

// FreeBSD on 64-bit targets (amd64, arm64), FreeBSD 12 and later: the layouts
// are those of the 64-bit-inode ABI (D-090).
#[cfg(all(target_os = "freebsd", target_pointer_width = "64"))]
#[allow(unsafe_code)]
mod freebsd;
#[cfg(all(target_os = "freebsd", target_pointer_width = "64"))]
pub(crate) use freebsd::*;

// Everything else compiles and answers honestly: unknown, or what std knows.
#[cfg(not(any(
    all(
        any(target_os = "linux", target_os = "android"),
        target_pointer_width = "64"
    ),
    all(
        target_os = "linux",
        target_env = "gnu",
        any(target_arch = "x86", target_arch = "arm")
    ),
    all(
        any(target_os = "macos", target_os = "ios"),
        target_pointer_width = "64"
    ),
    all(target_os = "freebsd", target_pointer_width = "64"),
    windows
)))]
mod unsupported;
#[cfg(not(any(
    all(
        any(target_os = "linux", target_os = "android"),
        target_pointer_width = "64"
    ),
    all(
        target_os = "linux",
        target_env = "gnu",
        any(target_arch = "x86", target_arch = "arm")
    ),
    all(
        any(target_os = "macos", target_os = "ios"),
        target_pointer_width = "64"
    ),
    all(target_os = "freebsd", target_pointer_width = "64"),
    windows
)))]
pub(crate) use unsupported::*;

use crate::{cpu::Vendor, os::Source};

/// OS identity as a platform reports it, owned; `crate::os` caches it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct OsFacts {
    pub(crate) name: String,
    pub(crate) version: Option<String>,
    pub(crate) kernel: String,
    pub(crate) distro: String,
    pub(crate) distro_id: String,
    pub(crate) release: Option<ReleaseFacts>,
    /// "Professional", a Linux `VARIANT_ID`.
    pub(crate) edition: Option<String>,
    pub(crate) codename: Option<String>,
    pub(crate) name_source: Source,
}

/// The os-release fields (or their legacy-file equivalents), owned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ReleaseFacts {
    pub(crate) id: Option<String>,
    pub(crate) id_like: Vec<String>,
    pub(crate) name: Option<String>,
    pub(crate) pretty_name: Option<String>,
    pub(crate) version: Option<String>,
    pub(crate) version_id: Option<String>,
    pub(crate) codename: Option<String>,
    pub(crate) variant_id: Option<String>,
    pub(crate) build_id: Option<String>,
    pub(crate) source: Source,
}

/// Static CPU facts as a platform reports them; `crate::cpu` cleans and caches them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CpuFacts {
    /// Configured logical processors, at least 1.
    pub(crate) logical: usize,
    /// Physical cores, when the platform can tell.
    pub(crate) physical: Option<usize>,
    /// Processor packages (sockets), when the platform can tell.
    pub(crate) packages: Option<usize>,
    /// The brand string as reported, to be cleaned.
    pub(crate) raw_name: Option<String>,
    /// A name that needs no cleaning (ARM cores named from their part numbers).
    pub(crate) name: Option<String>,
    pub(crate) vendor: Vendor,
}

/// The CPU's layout as a platform reports it; `crate::cpu` sorts and caches it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TopologyFacts {
    pub(crate) topology: crate::cpu::Topology,
    pub(crate) caches: Vec<crate::cpu::Cache>,
    pub(crate) numa: Vec<crate::cpu::NumaNode>,
}

/// The machine's firmware facts as a platform reports them; `crate::host`
/// keeps them. No serial numbers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct HardwareFacts {
    pub(crate) model_vendor: Option<String>,
    pub(crate) model_name: Option<String>,
    pub(crate) model_family: Option<String>,
    pub(crate) firmware_kind: Option<crate::host::FirmwareKind>,
    pub(crate) firmware_vendor: Option<String>,
    pub(crate) firmware_version: Option<String>,
    pub(crate) firmware_date: Option<String>,
    pub(crate) board_vendor: Option<String>,
    pub(crate) board_name: Option<String>,
    pub(crate) board_version: Option<String>,
}

/// A filesystem's type name and whether its locks may not hold across machines.
#[cfg(feature = "guard")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FsKind {
    pub(crate) name: &'static str,
    pub(crate) network: bool,
}
