//! Linux and Android: files under `/proc` and `/sys` first (through
//! `crate::linux`), and a short list of libc calls where no file carries the
//! value.

mod ffi;
mod live;

pub(crate) use live::*;

use std::{
    ffi::{CStr, c_char},
    mem::MaybeUninit,
};
#[cfg(feature = "guard")]
use std::{fs::OpenOptions, path::Path};

use crate::{
    Error, Result,
    linux::{self as tree, Root},
    process::User,
    sys::{CpuFacts, OsFacts},
};
#[cfg(feature = "guard")]
use crate::{process::Limit, sys::FsKind};

const ROOT: Root = Root::system();

pub(crate) fn os() -> OsFacts {
    #[cfg_attr(not(target_os = "android"), allow(unused_mut))]
    let mut facts = tree::os::read(&ROOT);
    #[cfg(target_os = "android")]
    android(&mut facts);
    facts
}

/// Android has no os-release: the release comes from a system property.
#[cfg(target_os = "android")]
fn android(facts: &mut OsFacts) {
    let mut buf = [0 as c_char; ffi::PROP_VALUE_MAX];
    // SAFETY: the name is a NUL-terminated literal and `buf` has the
    // PROP_VALUE_MAX bytes the call may write, NUL included.
    let len = unsafe {
        ffi::__system_property_get(c"ro.build.version.release".as_ptr(), buf.as_mut_ptr())
    };
    let version = (len > 0)
        .then(|| {
            let bytes: Vec<u8> = buf
                .iter()
                .take_while(|&&c| c != 0)
                // `c_char` is `u8` on aarch64 Android and `i8` on x86_64.
                .map(|&c| u8::from_ne_bytes(c.to_ne_bytes()))
                .collect();
            crate::parse::lossy(&bytes, 64)
        })
        .filter(|v| !v.is_empty());
    facts.name = match &version {
        Some(v) => format!("Android {v}"),
        None => "Android".to_owned(),
    };
    facts.version = version;
    facts.distro = "Android".to_owned();
    facts.distro_id = "android".to_owned();
    facts.release = None;
    facts.name_source = crate::os::Source::SystemProperty;
}

pub(crate) fn cpu() -> CpuFacts {
    let t = tree::cpu::read(&ROOT);
    let logical = t
        .logical
        .or_else(|| {
            usize::try_from(ffi::sysconf(ffi::SC_NPROCESSORS_CONF))
                .ok()
                .filter(|&n| n > 0)
        })
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(std::num::NonZero::get)
        })
        .unwrap_or(1);
    CpuFacts {
        logical,
        physical: t.physical,
        packages: t.packages,
        raw_name: t.raw_name,
        name: t.arm_name,
        vendor: t.vendor,
    }
}

pub(crate) fn cpu_available(logical: usize) -> usize {
    live::available(logical)
}

/// A 64-bit build runs only on a 64-bit processor; 32-bit Linux has no
/// implementation yet (D-013).
pub(crate) fn processor_is_64bit() -> bool {
    true
}

pub(crate) fn host_name() -> Result<String> {
    tree::host::name(&ROOT)
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

/// The account name for `uid` through `getpwuid_r`, growing the buffer on
/// `ERANGE` up to 64 KiB; a uid without an entry gives its number.
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
        // SAFETY: `pw_name` points into `buf`, NUL-terminated, and `buf` is
        // alive for this statement.
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
    Some(ffi::umask(mask & 0o777))
}

#[cfg(feature = "guard")]
fn get_nofile() -> Result<ffi::Rlimit> {
    let mut lim = ffi::Rlimit::default();
    // SAFETY: `lim` is a writable `rlimit`.
    let rc = unsafe { ffi::getrlimit(ffi::RLIMIT_NOFILE, &raw mut lim) };
    if rc != 0 {
        return Err(last_os_error("open-file limit", "getrlimit"));
    }
    Ok(lim)
}

#[cfg(feature = "guard")]
pub(crate) fn open_files_limit() -> Result<Limit> {
    let lim = get_nofile()?;
    let value = |v: u64| (v != ffi::RLIM_INFINITY).then_some(v);
    Ok(Limit {
        soft: value(lim.cur),
        hard: value(lim.max),
    })
}

#[cfg(feature = "guard")]
pub(crate) fn set_open_files_soft(n: u64) -> Result<()> {
    let mut lim = get_nofile()?;
    lim.cur = n.min(lim.max);
    // SAFETY: `lim` is a valid `rlimit`; raising the soft limit up to the hard
    // limit needs no privilege.
    let rc = unsafe { ffi::setrlimit(ffi::RLIMIT_NOFILE, &raw const lim) };
    if rc != 0 {
        return Err(last_os_error("open-file limit", "setrlimit"));
    }
    Ok(())
}

/// The kernel's per-process ceiling: `setrlimit` above `fs.nr_open` fails.
#[cfg(feature = "guard")]
pub(crate) fn open_files_ceiling() -> Option<u64> {
    ROOT.text("/proc/sys/fs/nr_open")
        .and_then(|t| t.parse().ok())
}

#[cfg(feature = "guard")]
pub(crate) fn thread_count() -> Option<usize> {
    let n = ROOT
        .with("/proc/self/status", "status", "/proc/self/status", |d| {
            crate::parse::proc_status::number(d, b"Threads")
        })
        .ok()??;
    usize::try_from(n).ok()
}

#[cfg(feature = "guard")]
pub(crate) fn lock_open_options(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600).custom_flags(ffi::O_NOFOLLOW);
}

/// The filesystem holding `path`, from its `statfs` magic number.
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
    // The magic is a 32-bit value stored in a signed word.
    Some(fs_kind(st.ftype as u32))
}

#[cfg(feature = "guard")]
fn fs_kind(magic: u32) -> FsKind {
    let (name, network) = match magic {
        0xEF53 => ("ext4", false),
        0x5846_5342 => ("xfs", false),
        0x9123_683E => ("btrfs", false),
        0x0102_1994 => ("tmpfs", false),
        0x794C_7630 => ("overlay", false),
        0x2FC1_2FC1 => ("zfs", false),
        0xF2F5_2010 => ("f2fs", false),
        0x2011_BAB0 => ("exfat", false),
        0x7366_746E => ("ntfs", false),
        0x4D44 => ("vfat", false),
        0x6165_6673 => ("bcachefs", false),
        ffi::NFS_SUPER_MAGIC => ("nfs", true),
        ffi::SMB_SUPER_MAGIC => ("smb", true),
        ffi::CIFS_SUPER_MAGIC => ("cifs", true),
        ffi::SMB2_SUPER_MAGIC => ("smb2", true),
        // FUSE locks depend on the filesystem behind it (sshfs, rclone, ...).
        ffi::FUSE_SUPER_MAGIC => ("fuse", true),
        // WSL 2's /mnt/c (E-44).
        ffi::V9FS_MAGIC => ("9p", true),
        ffi::CEPH_SUPER_MAGIC => ("ceph", true),
        ffi::AFS_SUPER_MAGIC => ("afs", true),
        ffi::CODA_SUPER_MAGIC => ("coda", true),
        _ => ("other", false),
    };
    FsKind { name, network }
}

#[cfg(feature = "guard")]
fn last_os_error(what: &'static str, call: &'static str) -> Error {
    Error::Os {
        what,
        call,
        code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sys_linux_user_name_of_root_and_of_an_unknown_uid() {
        assert_eq!(user_name(0).unwrap(), "root");
        // A uid nobody has: its number.
        assert_eq!(user_name(4_000_000_123).unwrap(), "4000000123");
    }

    #[cfg(feature = "guard")]
    #[test]
    fn test_sys_linux_filesystem_and_limits() {
        let fs = filesystem(Path::new("/proc")).unwrap();
        assert!(!fs.network);
        let lim = open_files_limit().unwrap();
        assert!(lim.soft.unwrap_or(u64::MAX) <= lim.hard.unwrap_or(u64::MAX));
        assert!(thread_count().unwrap() >= 1);
        assert!(fs_kind(0x0102_1997).network);
        assert_eq!(fs_kind(0xEF53).name, "ext4");
    }
}
