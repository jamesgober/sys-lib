//! Platforms without an implementation yet (FreeBSD and 32-bit Linux arrive in
//! 0.6, D-031): answers std can give honestly, and `Unsupported` for the rest.

#[cfg(feature = "guard")]
use std::{fs::OpenOptions, path::Path};

use crate::{
    Error, Result,
    cpu::Vendor,
    os::{Source, kind},
    process::User,
    sys::{CpuFacts, OsFacts},
};
#[cfg(feature = "guard")]
use crate::{process::Limit, sys::FsKind};

fn unsupported(what: &'static str) -> Error {
    Error::Unsupported {
        what,
        platform: kind().name(),
    }
}

/// "FreeBSD", "OpenBSD": the family, honestly, with no version.
fn family() -> &'static str {
    match kind() {
        crate::os::Kind::FreeBsd => "FreeBSD",
        crate::os::Kind::OpenBsd => "OpenBSD",
        crate::os::Kind::NetBsd => "NetBSD",
        crate::os::Kind::Linux => "Linux",
        crate::os::Kind::Android => "Android",
        _ => std::env::consts::OS,
    }
}

pub(crate) fn os() -> OsFacts {
    OsFacts {
        name: family().to_owned(),
        version: None,
        kernel: String::new(),
        distro: family().to_owned(),
        distro_id: kind().name().to_owned(),
        release: None,
        name_source: Source::Fallback,
    }
}

pub(crate) fn cpu() -> CpuFacts {
    let logical = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    CpuFacts {
        logical,
        physical: None,
        raw_name: None,
        name: None,
        vendor: Vendor::Unknown,
    }
}

pub(crate) fn cpu_available(logical: usize) -> usize {
    std::thread::available_parallelism().map_or(logical, std::num::NonZero::get)
}

pub(crate) fn processor_is_64bit() -> bool {
    cfg!(target_pointer_width = "64")
}

pub(crate) fn host_name() -> Result<String> {
    Err(unsupported("host name"))
}

pub(crate) fn user() -> Result<User> {
    Err(unsupported("user"))
}

pub(crate) fn is_elevated() -> Result<bool> {
    Err(unsupported("elevation"))
}

#[cfg(all(feature = "guard", unix))]
pub(crate) fn effective_uid() -> Option<u32> {
    None
}

#[cfg(feature = "guard")]
pub(crate) fn swap_umask(_mask: u32) -> Option<u32> {
    None
}

#[cfg(feature = "guard")]
pub(crate) fn open_files_limit() -> Result<Limit> {
    Ok(Limit::default())
}

#[cfg(feature = "guard")]
pub(crate) fn set_open_files_soft(_n: u64) -> Result<()> {
    Ok(())
}

#[cfg(feature = "guard")]
pub(crate) fn open_files_ceiling() -> Option<u64> {
    None
}

#[cfg(all(feature = "guard", unix))]
pub(crate) fn thread_count() -> Option<usize> {
    None
}

#[cfg(feature = "guard")]
pub(crate) fn lock_open_options(_options: &mut OpenOptions) {}

#[cfg(feature = "guard")]
pub(crate) fn filesystem(_path: &Path) -> Option<FsKind> {
    None
}
