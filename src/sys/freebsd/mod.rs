//! FreeBSD: sysctl (by name, and by number where the name takes an argument),
//! `kenv` for the firmware strings the loader records, and a short list of
//! libc calls (`statfs`, `getifaddrs`, `getrlimit`, `getrusage`, utmpx).
//!
//! The readings are split by area as on the other backends: `mem`, `cpu`,
//! `disk`, `net`, `process` (this process), and `host` for the live readings
//! (0.4); `detect`, `power`, and `device` for detection and identity (0.5);
//! `report` for the environment readings (0.6). This file keeps the static
//! facts, the process guard's needs, and the helpers the areas share: sysctl
//! and kenv reads, errno mapping, and byte readers. Every item of the
//! interface is defined here or in an area module, never taken from the stub
//! modules, so a missing item is a build error.

mod cpu;
mod detect;
#[cfg(feature = "device-id")]
mod device;
mod disk;
mod ffi;
mod host;
mod mem;
mod net;
mod power;
mod process;
mod report;

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    io::Read,
    mem::MaybeUninit,
    time::Duration,
};
#[cfg(feature = "guard")]
use std::{fs::OpenOptions, path::Path};

#[cfg(feature = "device-id")]
pub(crate) use self::device::*;
pub(crate) use self::{
    cpu::*, detect::*, disk::*, host::*, mem::*, net::*, power::*, process::*, report::*,
};
use crate::{
    Error, Result,
    cpu::Vendor,
    os::Source,
    parse::os_release,
    process::User,
    sys::{CpuFacts, OsFacts, ReleaseFacts},
};
#[cfg(feature = "guard")]
use crate::{process::Limit, sys::FsKind};

// ---------------------------------------------------------------------------
// Helpers shared by the readings
// ---------------------------------------------------------------------------

/// `Unsupported` on this platform.
fn unsupported(what: &'static str) -> Error {
    Error::Unsupported {
        what,
        platform: crate::os::kind().name(),
    }
}

/// The error for a failed call: `NotFound` and `PermissionDenied` where
/// `errno` says so, else `Os` with the code.
fn os_error(what: &'static str, call: &'static str, code: i32) -> Error {
    match code {
        ffi::ENOENT => Error::NotFound { what, source: call },
        ffi::EPERM | ffi::EACCES => Error::PermissionDenied {
            what,
            hint: "run outside the jail or the MAC policy that refuses it, or treat the value as unknown",
        },
        _ => Error::Os { what, call, code },
    }
}

fn last_errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
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

/// A plain C struct a sysctl may fill: integers only, so every byte pattern
/// is a valid value.
///
/// # Safety
///
/// Every bit pattern of `size_of::<Self>()` bytes must be a valid `Self`.
unsafe trait Plain: Default {}

// SAFETY: `timeval` is two integers.
unsafe impl Plain for ffi::Timeval {}
// SAFETY: `clockinfo` is five integers.
unsafe impl Plain for ffi::Clockinfo {}
// SAFETY: `xswdev` is five integers (and padding).
unsafe impl Plain for ffi::Xswdev {}

/// What a sysctl is named by: its name, or its number (a name that takes an
/// argument, such as a process id, is read by number).
#[derive(Clone, Copy)]
enum Key<'a> {
    Name(&'a CStr),
    Mib(&'a [c_int]),
}

/// One sysctl read into `buf`, or the size it needs with no buffer: the bytes,
/// or the `errno`.
fn sysctl_raw(key: Key<'_>, buf: Option<&mut [u8]>) -> core::result::Result<usize, i32> {
    let (ptr, mut len) = match buf {
        Some(b) => (b.as_mut_ptr().cast::<c_void>(), b.len()),
        None => (core::ptr::null_mut(), 0),
    };
    let rc = match key {
        // SAFETY: `name` is NUL-terminated; `ptr` is null (a size query) or
        // `len` writable bytes, of which the kernel writes at most `len`,
        // updating it.
        Key::Name(name) => unsafe {
            ffi::sysctlbyname(name.as_ptr(), ptr, &raw mut len, core::ptr::null(), 0)
        },
        Key::Mib(mib) => {
            let count = u32::try_from(mib.len()).map_err(|_| ffi::EINVAL)?;
            // SAFETY: `mib` is `count` readable integers; the buffer as above.
            unsafe { ffi::sysctl(mib.as_ptr(), count, ptr, &raw mut len, core::ptr::null(), 0) }
        }
    };
    if rc != 0 {
        return Err(last_errno());
    }
    Ok(len)
}

/// A sysctl that answers with exactly one `T`.
fn sysctl_value<T: Plain>(key: Key<'_>) -> core::result::Result<T, SysctlError> {
    let mut value = T::default();
    let mut len = core::mem::size_of::<T>();
    let ptr = (&raw mut value).cast::<c_void>();
    let rc = match key {
        // SAFETY: `name` is NUL-terminated; `value` is `len` writable bytes,
        // the call writes at most `len`, and `Plain` makes any bytes it writes
        // a valid `T`.
        Key::Name(name) => unsafe {
            ffi::sysctlbyname(name.as_ptr(), ptr, &raw mut len, core::ptr::null(), 0)
        },
        Key::Mib(mib) => {
            let count = u32::try_from(mib.len()).map_err(|_| SysctlError::Errno(ffi::EINVAL))?;
            // SAFETY: `mib` is `count` readable integers; `value` as above.
            unsafe { ffi::sysctl(mib.as_ptr(), count, ptr, &raw mut len, core::ptr::null(), 0) }
        }
    };
    if rc != 0 {
        return Err(SysctlError::Errno(last_errno()));
    }
    if len != core::mem::size_of::<T>() {
        return Err(SysctlError::Size(len));
    }
    Ok(value)
}

/// An integer sysctl of 4 or 8 bytes (`int`, `u_int`, `long`, `uint64_t`),
/// as its bytes and their count.
fn sysctl_word(key: Key<'_>) -> core::result::Result<([u8; 8], usize), SysctlError> {
    let mut buf = [0u8; 8];
    let len = sysctl_raw(key, Some(&mut buf)).map_err(SysctlError::Errno)?;
    match len {
        4 | 8 => Ok((buf, len)),
        n => Err(SysctlError::Size(n)),
    }
}

/// An unsigned integer sysctl of 4 or 8 bytes.
fn sysctl_int(name: &CStr) -> core::result::Result<u64, SysctlError> {
    let (buf, len) = sysctl_word(Key::Name(name))?;
    Ok(word_unsigned(buf, len))
}

/// A signed integer sysctl of 4 or 8 bytes (`-1` is a common "none").
fn sysctl_signed(name: &CStr) -> core::result::Result<i64, SysctlError> {
    let (buf, len) = sysctl_word(Key::Name(name))?;
    Ok(word_signed(buf, len))
}

fn word_unsigned(buf: [u8; 8], len: usize) -> u64 {
    if len == 4 {
        u64::from(u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]))
    } else {
        u64::from_ne_bytes(buf)
    }
}

fn word_signed(buf: [u8; 8], len: usize) -> i64 {
    if len == 4 {
        i64::from(i32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]))
    } else {
        i64::from_ne_bytes(buf)
    }
}

/// An unsigned integer sysctl, or `None`.
fn sysctl_u64(name: &CStr) -> Option<u64> {
    sysctl_int(name).ok()
}

/// A string sysctl, bounded to 1 KiB, or `None` when absent or empty.
fn sysctl_string(name: &CStr) -> Option<String> {
    let mut buf = [0u8; 1024];
    let len = sysctl_raw(Key::Name(name), Some(&mut buf)).ok()?;
    let bytes = &buf[..len.min(buf.len())];
    let bytes = bytes.split(|&b| b == 0).next().unwrap_or(bytes);
    let text = crate::parse::lossy(crate::parse::trim(bytes), 1024);
    (!text.is_empty()).then_some(text)
}

/// The number for a sysctl name, into `out`; the count used.
fn name_to_mib(name: &CStr, out: &mut [c_int; 8]) -> core::result::Result<usize, i32> {
    let mut len = out.len();
    // SAFETY: `name` is NUL-terminated and `out` has room for `len` integers;
    // the call writes at most `len` and updates it.
    let rc = unsafe { ffi::sysctlnametomib(name.as_ptr(), out.as_mut_ptr(), &raw mut len) };
    if rc != 0 {
        return Err(last_errno());
    }
    Ok(len.min(out.len()))
}

/// Answers of this size fit the stack buffer, so the common readings
/// allocate nothing.
const STACK_SYSCTL: usize = 16 << 10;

/// Calls `f` with a sysctl of variable size: in a stack buffer when it fits,
/// else in a heap buffer bounded at `max` bytes (checked before allocating).
/// Entries appearing between the size query and the read (`ENOMEM`) mean
/// asking again, at most three times.
fn with_sysctl<R>(
    what: &'static str,
    source: &'static str,
    key: Key<'_>,
    max: usize,
    f: impl FnOnce(&[u8]) -> Result<R>,
) -> Result<R> {
    let mut stack = [0u8; STACK_SYSCTL];
    let mut heap: Vec<u8> = Vec::new();
    for _ in 0..3 {
        let need = sysctl_raw(key, None).map_err(|code| os_error(what, source, code))?;
        let buf: &mut [u8] = if need <= STACK_SYSCTL {
            &mut stack
        } else {
            let size = need.saturating_add(need / 8).saturating_add(4096);
            if size > max {
                return Err(Error::TooLarge {
                    what,
                    limit: max as u64,
                });
            }
            heap.resize(size, 0);
            &mut heap
        };
        match sysctl_raw(key, Some(buf)) {
            Ok(len) => return f(&buf[..len.min(buf.len())]),
            Err(ffi::ENOMEM) => {}
            Err(code) => return Err(os_error(what, source, code)),
        }
    }
    Err(Error::Os {
        what,
        call: source,
        code: ffi::ENOMEM,
    })
}

/// What `kenv` found for a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kenv {
    /// The value's length in the buffer, without the NUL.
    Value(usize),
    Absent,
    Denied,
}

/// The size of a `kenv` value buffer: the longest value and its NUL.
const KENV_BUF: usize = ffi::KENV_MVALLEN + 1;

/// One loader environment variable into `out`, NUL dropped.
fn kenv_get(name: &CStr, out: &mut [u8]) -> Kenv {
    let len = c_int::try_from(out.len().min(KENV_BUF)).unwrap_or(0);
    // SAFETY: `name` is NUL-terminated and `out` has at least `len` writable
    // bytes; the kernel writes at most `len`.
    let n = unsafe {
        ffi::kenv(
            ffi::KENV_GET,
            name.as_ptr(),
            out.as_mut_ptr().cast::<c_char>(),
            len,
        )
    };
    if n < 0 {
        return match last_errno() {
            ffi::EPERM | ffi::EACCES => Kenv::Denied,
            _ => Kenv::Absent,
        };
    }
    let got = usize::try_from(n).unwrap_or(0).min(out.len());
    let end = out[..got].iter().position(|&b| b == 0).unwrap_or(got);
    Kenv::Value(end)
}

/// A loader environment variable as text, without placeholders and control
/// characters (`parse::smbios::clean`).
fn kenv_text(name: &CStr) -> Option<String> {
    let mut buf = [0u8; KENV_BUF];
    match kenv_get(name, &mut buf) {
        Kenv::Value(n) => crate::parse::smbios::clean(&buf[..n]),
        Kenv::Absent | Kenv::Denied => None,
    }
}

/// `N` bytes of `data` at `at`, when they are all there.
fn bytes_at<const N: usize>(data: &[u8], at: usize) -> Option<[u8; N]> {
    data.get(at..at.checked_add(N)?)?.try_into().ok()
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    bytes_at(data, at).map(u16::from_ne_bytes)
}

fn i32_at(data: &[u8], at: usize) -> Option<i32> {
    bytes_at(data, at).map(i32::from_ne_bytes)
}

fn u64_at(data: &[u8], at: usize) -> Option<u64> {
    bytes_at(data, at).map(u64::from_ne_bytes)
}

fn i64_at(data: &[u8], at: usize) -> Option<i64> {
    bytes_at(data, at).map(i64::from_ne_bytes)
}

/// The bytes of a fixed C character array up to its first NUL. `c_char` is
/// `i8` on amd64 and `u8` on arm64.
fn c_array_bytes(chars: &[c_char]) -> Vec<u8> {
    chars
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| u8::from_ne_bytes(c.to_ne_bytes()))
        .collect()
}

/// A `timeval` as a duration; a negative or out-of-range field counts as zero.
fn timeval_duration(tv: ffi::Timeval) -> Duration {
    let usec = u32::try_from(tv.usec).unwrap_or(0).min(999_999);
    Duration::new(u64::try_from(tv.sec).unwrap_or(0), usec * 1_000)
}

/// One resource limit.
fn rlimit(resource: c_int, what: &'static str) -> Result<ffi::Rlimit> {
    let mut lim = ffi::Rlimit::default();
    // SAFETY: `lim` is a writable `rlimit`.
    let rc = unsafe { ffi::getrlimit(resource, &raw mut lim) };
    if rc != 0 {
        return Err(os_error(what, "getrlimit", last_errno()));
    }
    Ok(lim)
}

/// A limit's value; `RLIM_INFINITY` (and anything negative) is no limit.
fn limit_value(v: ffi::Rlim) -> Option<u64> {
    if v == ffi::RLIM_INFINITY {
        return None;
    }
    u64::try_from(v).ok()
}

/// This process's id as a `pid_t`.
fn own_pid() -> ffi::Pid {
    std::process::id().cast_signed()
}

/// A small file read whole, bounded at `max` bytes; `None` when absent,
/// unreadable, or larger.
fn read_small(path: &str, max: u64) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut data = Vec::new();
    file.take(max + 1).read_to_end(&mut data).ok()?;
    (data.len() as u64 <= max).then_some(data)
}

// ---------------------------------------------------------------------------
// Static facts
// ---------------------------------------------------------------------------

/// The release number at the front of `kern.osrelease`: `14.1-RELEASE-p5` is
/// `14.1`.
fn release_number(osrelease: &str) -> Option<String> {
    let end = osrelease
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(osrelease.len());
    let number = osrelease[..end].trim_end_matches('.');
    (!number.is_empty() && number.starts_with(|c: char| c.is_ascii_digit()))
        .then(|| number.to_owned())
}

/// The OS facts from `kern.ostype`, `kern.osrelease`, and the os-release file
/// `rc.d/os-release` writes at boot (FreeBSD 13 and later), when there is one.
fn os_from(ostype: Option<String>, osrelease: Option<String>, release: Option<&[u8]>) -> OsFacts {
    let family = ostype.unwrap_or_else(|| "FreeBSD".to_owned());
    let kernel = osrelease.unwrap_or_default();
    let parsed = release.map(os_release::parse);
    let version = parsed
        .as_ref()
        .and_then(|r| r.version_id.clone())
        .or_else(|| release_number(&kernel));
    let pretty = parsed.as_ref().and_then(|r| r.pretty_name.clone());
    let (name, name_source) = match pretty {
        Some(p) => (p, Source::OsRelease),
        None if kernel.is_empty() => (family.clone(), Source::Fallback),
        None => (format!("{family} {kernel}"), Source::Sysctl),
    };
    let distro = parsed
        .as_ref()
        .and_then(|r| r.name.clone())
        .unwrap_or_else(|| family.clone());
    let distro_id = parsed
        .as_ref()
        .and_then(|r| r.id.clone())
        .unwrap_or_else(|| family.to_ascii_lowercase());
    OsFacts {
        name,
        version,
        kernel,
        distro,
        distro_id,
        release: parsed.map(|r| ReleaseFacts {
            id: r.id,
            id_like: r.id_like,
            name: r.name,
            pretty_name: r.pretty_name,
            version: r.version,
            version_id: r.version_id,
            codename: r.version_codename,
            variant_id: r.variant_id,
            build_id: r.build_id,
            source: Source::OsRelease,
        }),
        edition: None,
        codename: None,
        name_source,
    }
}

pub(crate) fn os() -> OsFacts {
    let release = read_small("/etc/os-release", 64 << 10);
    os_from(
        sysctl_string(c"kern.ostype"),
        sysctl_string(c"kern.osrelease"),
        release.as_deref(),
    )
}

/// The vendor a CPUID vendor string names.
#[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
fn x86_vendor(id: &[u8; 12]) -> Vendor {
    match id {
        b"GenuineIntel" => Vendor::Intel,
        b"AuthenticAMD" => Vendor::Amd,
        b"HygonGenuine" => Vendor::Hygon,
        b"CentaurHauls" | b"  Shanghai  " => Vendor::Zhaoxin,
        _ => Vendor::Unknown,
    }
}

#[cfg(target_arch = "x86_64")]
fn vendor(_model: Option<&str>) -> Vendor {
    x86_vendor(&crate::sys::x86::vendor_id())
}

/// arm64 boards name the core's designer at the front of `hw.model`.
#[cfg(not(target_arch = "x86_64"))]
fn vendor(model: Option<&str>) -> Vendor {
    match model {
        Some(m) if m.starts_with("ARM ") => Vendor::Arm,
        _ => Vendor::Unknown,
    }
}

/// A count sysctl, when positive.
fn count(name: &CStr) -> Option<usize> {
    sysctl_u64(name)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|&n| n > 0)
}

pub(crate) fn cpu() -> CpuFacts {
    let logical = count(c"hw.ncpu")
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(std::num::NonZero::get)
        })
        .unwrap_or(1);
    let raw_name = sysctl_string(c"hw.model");
    CpuFacts {
        logical,
        physical: count(c"kern.smp.cores"),
        packages: None,
        vendor: vendor(raw_name.as_deref()),
        raw_name,
        name: None,
    }
}

/// The CPUs this process may run on: std reads them with `cpuset_getaffinity`.
pub(crate) fn cpu_available(logical: usize) -> usize {
    std::thread::available_parallelism().map_or(logical, std::num::NonZero::get)
}

/// Every supported FreeBSD target here is 64-bit.
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
    Ok(crate::parse::lossy(&c_array_bytes(&buf), 255))
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
    Ok(Limit {
        soft: limit_value(lim.cur),
        hard: limit_value(lim.max),
    })
}

#[cfg(feature = "guard")]
pub(crate) fn set_open_files_soft(n: u64) -> Result<()> {
    let mut lim = get_nofile()?;
    let n = ffi::Rlim::try_from(n).unwrap_or(ffi::RLIM_INFINITY);
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

/// The kernel caps the descriptor table at `kern.maxfilesperproc`, whatever
/// the soft limit says.
#[cfg(feature = "guard")]
pub(crate) fn open_files_ceiling() -> Option<u64> {
    sysctl_u64(c"kern.maxfilesperproc")
}

/// The thread count for the guard's check.
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
    let mut buf = [0u8; disk::PATH_MAX];
    let path = disk::c_path(path, &mut buf).ok()?;
    let st = disk::statfs(path).ok()?;
    let name = match c_array_bytes(&st.fstypename).as_slice() {
        b"ufs" => "ufs",
        b"zfs" => "zfs",
        b"tmpfs" => "tmpfs",
        b"msdosfs" => "msdosfs",
        b"ext2fs" => "ext2fs",
        b"nullfs" => "nullfs",
        b"nfs" => "nfs",
        b"smbfs" => "smbfs",
        b"fusefs" => "fusefs",
        b"devfs" => "devfs",
        _ => "other",
    };
    Some(FsKind {
        name,
        network: st.flags & ffi::MNT_LOCAL == 0 || matches!(name, "nfs" | "smbfs"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sys_freebsd_release_number() {
        assert_eq!(release_number("14.1-RELEASE-p5").as_deref(), Some("14.1"));
        assert_eq!(release_number("15.0-CURRENT").as_deref(), Some("15.0"));
        assert_eq!(release_number("13.").as_deref(), Some("13"));
        assert_eq!(release_number("RELEASE"), None);
        assert_eq!(release_number(""), None);
        assert_eq!(release_number(".5"), None);
    }

    #[test]
    fn test_sys_freebsd_os_facts_from_sources() {
        let release = b"NAME=FreeBSD\nVERSION=\"14.1-RELEASE\"\nVERSION_ID=\"14.1\"\nID=freebsd\nPRETTY_NAME=\"FreeBSD 14.1-RELEASE\"\n";
        let f = os_from(
            Some("FreeBSD".into()),
            Some("14.1-RELEASE-p5".into()),
            Some(release),
        );
        assert_eq!(f.name, "FreeBSD 14.1-RELEASE");
        assert_eq!(f.version.as_deref(), Some("14.1"));
        assert_eq!(f.kernel, "14.1-RELEASE-p5");
        assert_eq!(
            (f.distro.as_str(), f.distro_id.as_str()),
            ("FreeBSD", "freebsd")
        );
        assert_eq!(f.name_source, Source::OsRelease);
        assert_eq!(f.release.unwrap().source, Source::OsRelease);
        // A jail or an older release without the file.
        let f = os_from(Some("FreeBSD".into()), Some("13.2-RELEASE".into()), None);
        assert_eq!(f.name, "FreeBSD 13.2-RELEASE");
        assert_eq!(f.version.as_deref(), Some("13.2"));
        assert_eq!(f.name_source, Source::Sysctl);
        assert!(f.release.is_none());
        let f = os_from(None, None, None);
        assert_eq!(
            (f.name.as_str(), f.distro_id.as_str()),
            ("FreeBSD", "freebsd")
        );
        assert_eq!((f.version, f.name_source), (None, Source::Fallback));
    }

    #[test]
    fn test_sys_freebsd_words() {
        let mut four = [0u8; 8];
        four[..4].copy_from_slice(&(-1i32).to_ne_bytes());
        assert_eq!(word_signed(four, 4), -1);
        assert_eq!(word_unsigned(four, 4), u64::from(u32::MAX));
        assert_eq!(word_unsigned(7u64.to_ne_bytes(), 8), 7);
        assert_eq!(limit_value(ffi::RLIM_INFINITY), None);
        assert_eq!(limit_value(-1), None);
        assert_eq!(limit_value(1024), Some(1024));
        assert_eq!(x86_vendor(b"GenuineIntel"), Vendor::Intel);
        assert_eq!(x86_vendor(b"AuthenticAMD"), Vendor::Amd);
        assert_eq!(x86_vendor(b"NotARealCpu!"), Vendor::Unknown);
    }

    #[cfg(target_os = "freebsd")]
    #[test]
    fn test_sys_freebsd_sysctls_answer() {
        assert!(sysctl_string(c"kern.osrelease").is_some());
        assert_eq!(sysctl_string(c"kern.ostype").as_deref(), Some("FreeBSD"));
        assert!(sysctl_u64(c"hw.ncpu").is_some_and(|n| n > 0));
        assert_eq!(sysctl_string(c"no.such.sysctl"), None);
        assert!(matches!(
            sysctl_int(c"no.such.sysctl"),
            Err(SysctlError::Errno(ffi::ENOENT))
        ));
        assert_eq!(user_name(0).unwrap(), "root");
        let mut mib = [0; 8];
        assert_eq!(name_to_mib(c"kern.ostype", &mut mib), Ok(2));
        assert_eq!(&mib[..2], &[ffi::CTL_KERN, 1]);
        let mut buf = [0u8; KENV_BUF];
        assert_eq!(kenv_get(c"no.such.kenv.variable", &mut buf), Kenv::Absent);
        let os = os();
        assert!(os.name.starts_with("FreeBSD"), "{os:?}");
        assert!(os.version.is_some());
        assert!(host_name().is_ok_and(|n| !n.is_empty()));
    }
}
