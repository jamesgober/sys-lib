//! macOS and iOS: sysctl, Mach, libproc, CoreFoundation, and (macOS only) IOKit.
//!
//! The live readings (0.4) are split by area: `mem`, `cpu`, `disk`, `net`,
//! `process` (this process), and `host`. This file keeps the static facts, the
//! process guard's needs, and the helpers the areas share: sysctl reads, errno
//! mapping, byte readers, and the Mach send-right wrapper.

mod cpu;
mod disk;
mod ffi;
mod host;
mod mem;
mod net;
mod process;

pub(crate) use self::{cpu::*, disk::*, host::*, mem::*, net::*, process::*};

use std::{
    ffi::{CStr, c_char, c_void},
    mem::MaybeUninit,
    time::Duration,
};
#[cfg(feature = "guard")]
use std::{fs::OpenOptions, path::Path};

use crate::{
    Error, Result,
    cpu::Vendor,
    os::Source,
    process::User,
    sys::{CpuFacts, OsFacts},
};
#[cfg(feature = "guard")]
use crate::{process::Limit, sys::FsKind};

// ---------------------------------------------------------------------------
// Helpers shared by the readings
// ---------------------------------------------------------------------------

/// `Unsupported` on this platform (iOS lacks libproc, IOKit, and `getfsstat`).
#[cfg(not(target_os = "macos"))]
fn unsupported(what: &'static str) -> Error {
    Error::Unsupported {
        what,
        platform: crate::os::kind().name(),
    }
}

/// The error for a failed BSD call: `NotFound` and `PermissionDenied` where
/// `errno` says so, else `Os` with the code.
fn os_error(what: &'static str, call: &'static str, code: i32) -> Error {
    match code {
        ffi::ENOENT => Error::NotFound { what, source: call },
        ffi::EPERM | ffi::EACCES => Error::PermissionDenied {
            what,
            hint: "run outside the sandbox that refuses it, or treat the value as unknown",
        },
        _ => Error::Os { what, call, code },
    }
}

/// The error for a failed Mach or IOKit call (`kern_return_t`).
fn kern_error(what: &'static str, call: &'static str, code: ffi::KernReturn) -> Error {
    Error::Os { what, call, code }
}

/// Why a sysctl gave no value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SysctlError {
    /// The call failed with this `errno`.
    Errno(i32),
    /// The call answered with this many bytes, not the size expected.
    Size(usize),
}

impl SysctlError {
    /// The public error, naming the sysctl.
    fn into_error(self, what: &'static str, name: &'static CStr) -> Error {
        let source = name.to_str().unwrap_or("sysctl");
        match self {
            SysctlError::Errno(code) => os_error(what, source, code),
            SysctlError::Size(_) => Error::Malformed {
                what,
                source,
                line: None,
            },
        }
    }
}

/// A plain C struct a sysctl may fill: integers and arrays of them only, so
/// every byte pattern is a valid value.
///
/// # Safety
///
/// Every bit pattern of `size_of::<Self>()` bytes must be a valid `Self`.
unsafe trait Plain: Default {}

// SAFETY: `timeval` is two integers (and tail padding).
unsafe impl Plain for ffi::Timeval {}
// SAFETY: `xsw_usage` is five integers.
unsafe impl Plain for ffi::XswUsage {}

/// A sysctl that answers with exactly one `T`.
fn sysctl_value<T: Plain>(name: &CStr) -> core::result::Result<T, SysctlError> {
    let mut value = T::default();
    let mut len = core::mem::size_of::<T>();
    // SAFETY: `name` is NUL-terminated; `value` is `len` writable bytes, the
    // call writes at most `len`, and `Plain` makes any bytes it writes a valid
    // `T`.
    let rc = unsafe {
        ffi::sysctlbyname(
            name.as_ptr(),
            (&raw mut value).cast::<c_void>(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(SysctlError::Errno(last_errno()));
    }
    if len != core::mem::size_of::<T>() {
        return Err(SysctlError::Size(len));
    }
    Ok(value)
}

/// An integer sysctl of 4 or 8 bytes (`int`, `unsigned`, `quad`).
fn sysctl_int(name: &CStr) -> core::result::Result<u64, SysctlError> {
    let mut buf = [0u8; 8];
    let mut len = buf.len();
    // SAFETY: as in `sysctl_value`, with an 8-byte buffer.
    let rc = unsafe {
        ffi::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast::<c_void>(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(SysctlError::Errno(last_errno()));
    }
    match len {
        4 => Ok(u64::from(u32::from_ne_bytes([
            buf[0], buf[1], buf[2], buf[3],
        ]))),
        8 => Ok(u64::from_ne_bytes(buf)),
        n => Err(SysctlError::Size(n)),
    }
}

/// `N` bytes of `data` at `at`, when they are all there.
fn bytes_at<const N: usize>(data: &[u8], at: usize) -> Option<[u8; N]> {
    data.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    bytes_at(data, at).map(u16::from_ne_bytes)
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    bytes_at(data, at).map(u32::from_ne_bytes)
}

fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    bytes_at(data, at).map(i32::from_ne_bytes)
}

fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    bytes_at(data, at).map(u64::from_ne_bytes)
}

/// The bytes of a fixed C character array up to its first NUL.
#[cfg(target_os = "macos")]
fn c_array_bytes(chars: &[c_char]) -> Vec<u8> {
    chars
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c.cast_unsigned())
        .collect()
}

/// A `timeval` as a duration; a negative or out-of-range field counts as zero.
fn timeval_duration(tv: ffi::Timeval) -> Duration {
    let usec = u32::try_from(tv.usec).unwrap_or(0).min(999_999);
    Duration::new(u64::try_from(tv.sec).unwrap_or(0), usec * 1_000)
}

/// This process's id as a `pid_t`.
#[cfg(target_os = "macos")]
fn own_pid() -> i32 {
    std::process::id().cast_signed()
}

/// A send right to a Mach port this code holds one user reference to,
/// returned on drop. Ports are never cached: a cached name would not survive
/// `fork`.
struct SendRight(ffi::MachPort);

impl SendRight {
    /// The host's name port.
    fn host(what: &'static str) -> Result<SendRight> {
        match ffi::mach_host_self() {
            0 => Err(kern_error(what, "mach_host_self", 0)),
            port => Ok(SendRight(port)),
        }
    }
}

impl Drop for SendRight {
    fn drop(&mut self) {
        // SAFETY: this value owns one user reference to the right; returning
        // it once is correct.
        unsafe { ffi::mach_port_deallocate(ffi::mach_task_self_, self.0) };
    }
}

/// One resource limit.
fn rlimit(resource: core::ffi::c_int, what: &'static str) -> Result<ffi::Rlimit> {
    let mut lim = ffi::Rlimit::default();
    // SAFETY: `lim` is a writable `rlimit`.
    let rc = unsafe { ffi::getrlimit(resource, &raw mut lim) };
    if rc != 0 {
        return Err(os_error(what, "getrlimit", last_errno()));
    }
    Ok(lim)
}

// ---------------------------------------------------------------------------
// Static facts
// ---------------------------------------------------------------------------

/// A string sysctl, bounded to 1 KiB.
fn sysctl_string(name: &CStr) -> Option<String> {
    let mut buf = [0u8; 1024];
    let mut len = buf.len();
    // SAFETY: `name` is NUL-terminated; `buf` is `len` writable bytes and the
    // call writes at most `len`, updating it with the length used.
    let rc = unsafe {
        ffi::sysctlbyname(
            name.as_ptr(),
            buf.as_mut_ptr().cast::<c_void>(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    let bytes = &buf[..len.min(buf.len())];
    let bytes = bytes.split(|&b| b == 0).next().unwrap_or(bytes);
    let text = crate::parse::lossy(crate::parse::trim(bytes), 1024);
    (!text.is_empty()).then_some(text)
}

/// An integer sysctl of 4 or 8 bytes, or `None`.
fn sysctl_u64(name: &CStr) -> Option<u64> {
    sysctl_int(name).ok()
}

/// The marketing name of a macOS major version, for the ones released.
fn codename(major: u32) -> Option<&'static str> {
    Some(match major {
        11 => "Big Sur",
        12 => "Monterey",
        13 => "Ventura",
        14 => "Sonoma",
        15 => "Sequoia",
        26 => "Tahoe",
        _ => return None,
    })
}

pub(crate) fn os() -> OsFacts {
    let version = sysctl_string(c"kern.osproductversion");
    let kernel = sysctl_string(c"kern.osrelease").unwrap_or_default();
    let (family, id) = if cfg!(target_os = "ios") {
        ("iOS", "ios")
    } else {
        ("macOS", "macos")
    };
    let name = match &version {
        Some(v) => {
            let major = crate::parse::version::parse(v).map(|p| p.0);
            match major
                .and_then(codename)
                .filter(|_| cfg!(target_os = "macos"))
            {
                Some(c) => format!("{family} {v} {c}"),
                None => format!("{family} {v}"),
            }
        }
        None => family.to_owned(),
    };
    OsFacts {
        name,
        version,
        kernel,
        distro: family.to_owned(),
        distro_id: id.to_owned(),
        release: None,
        name_source: Source::Sysctl,
    }
}

pub(crate) fn cpu() -> CpuFacts {
    let count = |name: &CStr| {
        sysctl_u64(name)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n > 0)
    };
    let logical = count(c"hw.logicalcpu_max")
        .or_else(|| count(c"hw.ncpu"))
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(std::num::NonZero::get)
        })
        .unwrap_or(1);
    let vendor = match sysctl_string(c"machdep.cpu.vendor").as_deref() {
        Some("GenuineIntel") => Vendor::Intel,
        Some("AuthenticAMD") => Vendor::Amd,
        _ if cfg!(target_arch = "aarch64") => Vendor::Apple,
        _ => Vendor::Unknown,
    };
    CpuFacts {
        logical,
        physical: count(c"hw.physicalcpu_max"),
        packages: count(c"hw.packages"),
        raw_name: sysctl_string(c"machdep.cpu.brand_string"),
        name: None,
        vendor,
    }
}

pub(crate) fn cpu_available(logical: usize) -> usize {
    // macOS has no affinity API; `hw.activecpu` is what the scheduler uses.
    sysctl_u64(c"hw.activecpu")
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0)
        .unwrap_or(logical)
}

/// Every supported Apple platform is 64-bit.
pub(crate) fn processor_is_64bit() -> bool {
    true
}

pub(crate) fn host_name() -> Result<String> {
    let mut buf = [0 as c_char; 256];
    // SAFETY: `buf` is 256 writable bytes; the call writes at most that many.
    let rc = unsafe { ffi::gethostname(buf.as_mut_ptr(), buf.len()) };
    if rc != 0 {
        return Err(Error::Os {
            what: "host name",
            call: "gethostname",
            code: last_errno(),
        });
    }
    // The name is NUL-terminated unless it was truncated; stop at the buffer either way.
    let bytes: Vec<u8> = buf
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c.cast_unsigned())
        .collect();
    Ok(crate::parse::lossy(&bytes, 255))
}

pub(crate) fn user() -> Result<User> {
    let uid = ffi::geteuid();
    Ok(User {
        name: user_name(uid)?,
        id: Some(uid),
        is_admin: uid == 0,
    })
}

pub(crate) fn is_elevated() -> Result<bool> {
    Ok(ffi::geteuid() == 0)
}

fn user_name(uid: u32) -> Result<String> {
    let mut size = 1024;
    loop {
        let mut buf = vec![0 as c_char; size];
        let mut pwd = MaybeUninit::<ffi::Passwd>::zeroed();
        let mut result: *mut ffi::Passwd = std::ptr::null_mut();
        // SAFETY: every pointer is valid for the call: `pwd` and `result` are
        // writable locals and `buf` is `size` writable bytes.
        let rc = unsafe {
            ffi::getpwuid_r(
                uid,
                pwd.as_mut_ptr(),
                buf.as_mut_ptr(),
                buf.len(),
                &raw mut result,
            )
        };
        if rc == ffi::ERANGE && size < 64 << 10 {
            size *= 2;
            continue;
        }
        if rc != 0 {
            return Err(Error::Os {
                what: "user name",
                call: "getpwuid_r",
                code: rc,
            });
        }
        if result.is_null() {
            return Ok(uid.to_string());
        }
        // SAFETY: on success with a non-null result, `result` points to `pwd`,
        // which the call initialised.
        let name_ptr = unsafe { (*result).name };
        if name_ptr.is_null() {
            return Ok(uid.to_string());
        }
        // SAFETY: `pw_name` points into `buf`, NUL-terminated, alive here.
        let name = unsafe { CStr::from_ptr(name_ptr) };
        let name = crate::parse::lossy(name.to_bytes(), 256);
        return Ok(if name.is_empty() {
            uid.to_string()
        } else {
            name
        });
    }
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The process guard's needs
// ---------------------------------------------------------------------------

#[cfg(feature = "guard")]
pub(crate) fn effective_uid() -> Option<u32> {
    Some(ffi::geteuid())
}

#[cfg(feature = "guard")]
pub(crate) fn swap_umask(mask: u32) -> Option<u32> {
    // `mode_t` is 16 bits here; the mask fits.
    Some(u32::from(ffi::umask((mask & 0o777) as ffi::Mode)))
}

#[cfg(feature = "guard")]
fn get_nofile() -> Result<ffi::Rlimit> {
    rlimit(ffi::RLIMIT_NOFILE, "open-file limit")
}

#[cfg(feature = "guard")]
pub(crate) fn open_files_limit() -> Result<Limit> {
    let lim = get_nofile()?;
    let value = |v: u64| (v < ffi::RLIM_INFINITY).then_some(v);
    Ok(Limit {
        soft: value(lim.cur),
        hard: value(lim.max),
    })
}

#[cfg(feature = "guard")]
pub(crate) fn set_open_files_soft(n: u64) -> Result<()> {
    let mut lim = get_nofile()?;
    lim.cur = n.min(lim.max);
    // SAFETY: `lim` is a valid `rlimit`; the soft value is at most the hard one.
    let rc = unsafe { ffi::setrlimit(ffi::RLIMIT_NOFILE, &raw const lim) };
    if rc != 0 {
        return Err(Error::Os {
            what: "open-file limit",
            call: "setrlimit",
            code: last_errno(),
        });
    }
    Ok(())
}

/// `setrlimit` above `kern.maxfilesperproc` fails with `EINVAL` (E-66).
#[cfg(feature = "guard")]
pub(crate) fn open_files_ceiling() -> Option<u64> {
    sysctl_u64(c"kern.maxfilesperproc")
}

/// The thread count for the guard's check: `threads`, or `None` where it is
/// unavailable (iOS).
#[cfg(feature = "guard")]
pub(crate) fn thread_count() -> Option<usize> {
    threads().ok()
}

#[cfg(feature = "guard")]
pub(crate) fn lock_open_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600).custom_flags(ffi::O_NOFOLLOW);
}

#[cfg(feature = "guard")]
pub(crate) fn filesystem(path: &Path) -> Option<FsKind> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut buf = MaybeUninit::<ffi::Statfs>::zeroed();
    // SAFETY: `c_path` is NUL-terminated and `buf` is a writable `statfs`.
    let rc = unsafe { ffi::statfs(c_path.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: `statfs` succeeded, so it initialised `buf`.
    let st = unsafe { buf.assume_init() };
    let name_bytes: Vec<u8> = st
        .fstypename
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c.cast_unsigned())
        .collect();
    let name = match name_bytes.as_slice() {
        b"apfs" => "apfs",
        b"hfs" => "hfs",
        b"msdos" => "msdos",
        b"exfat" => "exfat",
        b"nfs" => "nfs",
        b"smbfs" => "smbfs",
        b"afpfs" => "afpfs",
        b"webdav" => "webdav",
        b"macfuse" | b"osxfuse" => "macfuse",
        b"devfs" => "devfs",
        _ => "other",
    };
    Some(FsKind {
        name,
        network: st.flags & ffi::MNT_LOCAL == 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sys_apple_sysctls_answer() {
        assert!(sysctl_string(c"kern.osrelease").is_some());
        assert!(sysctl_u64(c"hw.logicalcpu_max").is_some_and(|n| n > 0));
        assert_eq!(sysctl_string(c"no.such.sysctl"), None);
        assert_eq!(codename(15), Some("Sequoia"));
        assert_eq!(codename(99), None);
        assert_eq!(user_name(0).unwrap(), "root");
    }
}
