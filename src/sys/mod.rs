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

// Linux and Android on 64-bit targets. Struct layouts are verified for LP64
// (glibc, musl, bionic); a 32-bit Linux target has no verified layout yet and
// gets no implementation (D-003).
#[cfg(all(
    any(target_os = "linux", target_os = "android"),
    target_pointer_width = "64"
))]
#[allow(unsafe_code)]
mod linux;

// macOS and iOS: one module, with the macOS-only parts (IOKit, getfsstat) gated
// inside it (D-013).
#[cfg(all(
    any(target_os = "macos", target_os = "ios"),
    target_pointer_width = "64"
))]
#[allow(unsafe_code)]
mod apple;

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;
