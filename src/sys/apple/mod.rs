//! macOS and iOS: sysctl, Mach, libproc, CoreFoundation, and (macOS only) IOKit.

mod ffi;

use std::{
    ffi::{CStr, c_char, c_void},
    mem::MaybeUninit,
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

/// An integer sysctl of 4 or 8 bytes.
fn sysctl_u64(name: &CStr) -> Option<u64> {
    let mut buf = [0u8; 8];
    let mut len = buf.len();
    // SAFETY: as in `sysctl_string`, with an 8-byte buffer.
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
    match len {
        4 => Some(u64::from(u32::from_ne_bytes([
            buf[0], buf[1], buf[2], buf[3],
        ]))),
        8 => Some(u64::from_ne_bytes(buf)),
        _ => None,
    }
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
    let mut lim = ffi::Rlimit::default();
    // SAFETY: `lim` is a writable `rlimit`.
    let rc = unsafe { ffi::getrlimit(ffi::RLIMIT_NOFILE, &raw mut lim) };
    if rc != 0 {
        return Err(Error::Os {
            what: "open-file limit",
            call: "getrlimit",
            code: last_errno(),
        });
    }
    Ok(lim)
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

#[cfg(feature = "guard")]
pub(crate) fn thread_count() -> Option<usize> {
    #[cfg(target_os = "macos")]
    {
        let mut info = MaybeUninit::<ffi::ProcTaskinfo>::zeroed();
        let size = core::mem::size_of::<ffi::ProcTaskinfo>() as i32;
        // SAFETY: `info` is `size` writable bytes for the PROC_PIDTASKINFO flavour.
        let n = unsafe {
            ffi::proc_pidinfo(
                std::process::id() as i32,
                ffi::PROC_PIDTASKINFO,
                0,
                info.as_mut_ptr().cast::<c_void>(),
                size,
            )
        };
        if n != size {
            return None;
        }
        // SAFETY: the call filled all `size` bytes.
        let info = unsafe { info.assume_init() };
        usize::try_from(info.threadnum).ok()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
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
