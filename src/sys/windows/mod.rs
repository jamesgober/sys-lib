//! Windows: kernel32, advapi32, iphlpapi, powrprof, and ntdll, called directly.

mod cpu;
mod disk;
mod ffi;
mod host;
mod mem;
mod process;

// Removed before release: every item must be this platform's own.
pub(crate) use super::pending::*;

pub(crate) use cpu::{
    CPU_STATES, CPU_TICK, cpu, cpu_available, cpu_online, cpu_pressure, cpu_quota, cpu_times,
    cpu_times_per_core, current_frequency, frequency, load, processor_is_64bit,
};
pub(crate) use disk::{disk_io_counters, disk_io_totals, disk_pressure, disk_space, disks};
#[cfg(feature = "guard")]
pub(crate) use ffi::FILE_ATTRIBUTE_REPARSE_POINT;
pub(crate) use host::{boot_time, host_open_files, processes, uptime};
pub(crate) use mem::{huge_pages, mem_events, mem_pressure, memory, page_size, paging};
#[cfg(all(test, feature = "guard"))]
pub(crate) use process::thread_count;
pub(crate) use process::{
    cpu_time, limits, memory_detail, open_files, parent_id, peak_memory, process_io,
    process_memory, started_at, threads,
};
use std::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
    time::{Duration, SystemTime},
};
#[cfg(feature = "guard")]
use std::{fs::OpenOptions, path::Path};

use crate::{Error, Result, os::Source, process::User, sys::OsFacts};
#[cfg(feature = "guard")]
use crate::{process::Limit, sys::FsKind};

/// A UTF-16, NUL-terminated copy of `s`.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `buf` up to its first NUL, as a lossy string.
fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn last_error() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `Os` for a failed call, with the thread's last error.
fn os_error(what: &'static str, call: &'static str) -> Error {
    Error::Os {
        what,
        call,
        code: last_error(),
    }
}

/// `Unsupported` on this platform.
fn unsupported(what: &'static str) -> Error {
    Error::Unsupported {
        what,
        platform: crate::os::kind().name(),
    }
}

/// A count of 100 ns intervals (`FILETIME`, `LARGE_INTEGER` times) as a
/// `Duration`, without overflow.
fn hundred_ns(n: u64) -> Duration {
    Duration::new(n / 10_000_000, (n % 10_000_000) as u32 * 100)
}

/// A signed OS counter that is never negative in practice; a negative one
/// reads as zero.
fn non_negative(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// 100 ns intervals from 1601-01-01 to 1970-01-01.
const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;

/// A `FILETIME` value (100 ns since 1601) as a `SystemTime`.
fn filetime_to_system(ft: u64) -> Option<SystemTime> {
    if ft >= UNIX_EPOCH_FILETIME {
        SystemTime::UNIX_EPOCH.checked_add(hundred_ns(ft - UNIX_EPOCH_FILETIME))
    } else {
        SystemTime::UNIX_EPOCH.checked_sub(hundred_ns(UNIX_EPOCH_FILETIME - ft))
    }
}

/// An open registry key, closed on drop.
struct Key(ffi::Hkey);

impl Key {
    /// Opens `HKLM\<path>` for reading, in the 64-bit view (E-74).
    fn local_machine(path: &str) -> Option<Key> {
        let path = wide(path);
        let mut key: ffi::Hkey = std::ptr::null_mut();
        // SAFETY: `path` is NUL-terminated and `key` is a writable handle slot.
        let rc = unsafe {
            ffi::RegOpenKeyExW(
                ffi::HKEY_LOCAL_MACHINE,
                path.as_ptr(),
                0,
                ffi::KEY_QUERY_VALUE | ffi::KEY_WOW64_64KEY,
                &raw mut key,
            )
        };
        (rc == 0).then_some(Key(key))
    }

    /// A `REG_SZ` value, at most 1 KiB of UTF-16.
    fn string(&self, name: &str) -> Option<String> {
        let name = wide(name);
        let mut buf = [0u16; 512];
        let mut kind = 0u32;
        let mut len = (buf.len() * 2) as u32;
        // SAFETY: `name` is NUL-terminated; `buf` is `len` writable bytes.
        let rc = unsafe {
            ffi::RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null_mut(),
                &raw mut kind,
                buf.as_mut_ptr().cast::<u8>(),
                &raw mut len,
            )
        };
        if rc != 0 || kind != ffi::REG_SZ {
            return None;
        }
        let units = (len as usize / 2).min(buf.len());
        let text = from_wide(&buf[..units]).trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    /// A `REG_DWORD` value.
    fn dword(&self, name: &str) -> Option<u32> {
        let name = wide(name);
        let mut value = 0u32;
        let mut kind = 0u32;
        let mut len = 4u32;
        // SAFETY: `name` is NUL-terminated; `value` is 4 writable bytes.
        let rc = unsafe {
            ffi::RegQueryValueExW(
                self.0,
                name.as_ptr(),
                std::ptr::null_mut(),
                &raw mut kind,
                (&raw mut value).cast::<u8>(),
                &raw mut len,
            )
        };
        (rc == 0 && kind == ffi::REG_DWORD && len == 4).then_some(value)
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: the key was opened by RegOpenKeyExW and is closed once.
        unsafe { ffi::RegCloseKey(self.0) };
    }
}

/// A handle closed on drop.
struct Owned(ffi::Handle);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is valid and owned here; it is closed once.
        unsafe { ffi::CloseHandle(self.0) };
    }
}

/// Critical-error dialogs ("insert a disk in drive E:") are off for this
/// thread while a value lives, and the previous mode comes back on drop
/// (threat T8). Every volume call runs under one.
struct NoDialogs(Option<u32>);

impl NoDialogs {
    fn new() -> NoDialogs {
        let mut previous = 0u32;
        // SAFETY: `previous` is a writable local.
        let ok = unsafe { ffi::SetThreadErrorMode(ffi::SEM_FAILCRITICALERRORS, &raw mut previous) };
        NoDialogs((ok != 0).then_some(previous))
    }
}

impl Drop for NoDialogs {
    fn drop(&mut self) {
        if let Some(previous) = self.0 {
            // SAFETY: restores the mode saved by `new`; the old-mode output is
            // optional.
            unsafe { ffi::SetThreadErrorMode(previous, std::ptr::null_mut()) };
        }
    }
}

/// `GlobalMemoryStatusEx`.
fn memory_status() -> Result<ffi::MemoryStatusEx> {
    let mut status = ffi::MemoryStatusEx {
        length: size_of::<ffi::MemoryStatusEx>() as u32,
        ..ffi::MemoryStatusEx::default()
    };
    // SAFETY: `status` is a MEMORYSTATUSEX with its length set.
    let ok = unsafe { ffi::GlobalMemoryStatusEx(&raw mut status) };
    if ok == 0 {
        return Err(os_error("memory", "GlobalMemoryStatusEx"));
    }
    Ok(status)
}

/// `GetPerformanceInfo`: system-wide commit, cache, and object counts. It
/// counts handles across every process, so only the readings that need those
/// values call it.
fn performance_info(what: &'static str) -> Result<ffi::PerformanceInformation> {
    let mut info = ffi::PerformanceInformation::default();
    let size = size_of::<ffi::PerformanceInformation>() as u32;
    info.cb = size;
    // SAFETY: `info` is a PERFORMANCE_INFORMATION of the size passed.
    let ok = unsafe { ffi::K32GetPerformanceInfo(&raw mut info, size) };
    if ok == 0 {
        return Err(os_error(what, "GetPerformanceInfo"));
    }
    Ok(info)
}

/// The limits of the job object this process is in, or `None` outside a job.
/// A nested job reports its own limits: the query answers for the immediate
/// job, not the effective minimum over the enclosing ones.
fn job_limits() -> Option<ffi::JobObjectExtendedLimitInformation> {
    let mut info = ffi::JobObjectExtendedLimitInformation::default();
    // SAFETY: a null job means the job of this process; `info` is the buffer
    // for the extended-limit class, of the size passed.
    let ok = unsafe {
        ffi::QueryInformationJobObject(
            std::ptr::null_mut(),
            ffi::JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
            (&raw mut info).cast::<c_void>(),
            size_of::<ffi::JobObjectExtendedLimitInformation>() as u32,
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(info)
}

pub(crate) fn os() -> OsFacts {
    let info = MaybeUninit::<ffi::OsVersionInfoExW>::zeroed();
    // SAFETY: `info` is zeroed, so every field is valid before the call.
    let mut info = unsafe { info.assume_init() };
    info.os_version_info_size = size_of::<ffi::OsVersionInfoExW>() as u32;
    // SAFETY: `info` is a valid OSVERSIONINFOEXW with its size set.
    let status = unsafe { ffi::RtlGetVersion(&raw mut info) };
    let (major, minor, build, product) = if status >= 0 {
        (
            info.major_version,
            info.minor_version,
            info.build_number,
            info.product_type,
        )
    } else {
        (0, 0, 0, 0)
    };
    let key = Key::local_machine(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let ubr = key.as_ref().and_then(|k| k.dword("UBR"));
    let product_name = key.as_ref().and_then(|k| k.string("ProductName"));
    let version = (major > 0).then(|| match ubr {
        Some(u) => format!("{major}.{minor}.{build}.{u}"),
        None => format!("{major}.{minor}.{build}"),
    });
    OsFacts {
        name: windows_name(major, minor, build, product, product_name.as_deref()),
        kernel: version.clone().unwrap_or_default(),
        version,
        distro: "Windows".to_owned(),
        distro_id: "windows".to_owned(),
        release: None,
        name_source: Source::Registry,
    }
}

/// "Windows 11 Pro", "Windows Server 2022 Datacenter". The registry's
/// `ProductName` still says "Windows 10" on Windows 11 (E-49), so the build
/// number decides the product; the edition comes from `ProductName`.
fn windows_name(
    major: u32,
    minor: u32,
    build: u32,
    product: u8,
    product_name: Option<&str>,
) -> String {
    let workstation = product == ffi::VER_NT_WORKSTATION || product == 0;
    match product_name {
        Some(n) if workstation && major == 10 && build >= 22_000 => {
            n.replacen("Windows 10", "Windows 11", 1)
        }
        Some(n) => n.to_owned(),
        None if !workstation => match build {
            14_393 => "Windows Server 2016".to_owned(),
            17_763 => "Windows Server 2019".to_owned(),
            20_348 => "Windows Server 2022".to_owned(),
            26_100 => "Windows Server 2025".to_owned(),
            _ => "Windows Server".to_owned(),
        },
        None => match (major, minor) {
            (10, _) if build >= 22_000 => "Windows 11".to_owned(),
            (10, _) => "Windows 10".to_owned(),
            (6, 3) => "Windows 8.1".to_owned(),
            (6, 2) => "Windows 8".to_owned(),
            (6, 1) => "Windows 7".to_owned(),
            _ => "Windows".to_owned(),
        },
    }
}

pub(crate) fn host_name() -> Result<String> {
    let mut len = 0u32;
    // SAFETY: a null buffer with size 0 asks for the size needed.
    unsafe {
        ffi::GetComputerNameExW(
            ffi::COMPUTER_NAME_DNS_HOSTNAME,
            std::ptr::null_mut(),
            &raw mut len,
        )
    };
    let mut buf = vec![0u16; (len as usize).clamp(1, 1024)];
    let mut size = buf.len() as u32;
    // SAFETY: `buf` has `size` writable UTF-16 units.
    let ok = unsafe {
        ffi::GetComputerNameExW(
            ffi::COMPUTER_NAME_DNS_HOSTNAME,
            buf.as_mut_ptr(),
            &raw mut size,
        )
    };
    if ok == 0 {
        return Err(os_error("host name", "GetComputerNameExW"));
    }
    let name = from_wide(&buf[..(size as usize).min(buf.len())]);
    Ok(crate::parse::lossy(name.as_bytes(), 255))
}

pub(crate) fn user() -> Result<User> {
    let mut buf = [0u16; 257];
    let mut size = buf.len() as u32;
    // SAFETY: `buf` has `size` writable UTF-16 units (UNLEN + 1).
    let ok = unsafe { ffi::GetUserNameW(buf.as_mut_ptr(), &raw mut size) };
    if ok == 0 {
        return Err(os_error("user name", "GetUserNameW"));
    }
    Ok(User {
        name: from_wide(&buf),
        id: None,
        is_admin: is_elevated()?,
    })
}

pub(crate) fn is_elevated() -> Result<bool> {
    let mut token: ffi::Handle = std::ptr::null_mut();
    // SAFETY: the pseudo-handle of this process is always valid; `token` is writable.
    let ok = unsafe {
        ffi::OpenProcessToken(ffi::GetCurrentProcess(), ffi::TOKEN_QUERY, &raw mut token)
    };
    if ok == 0 {
        return Err(os_error("elevation", "OpenProcessToken"));
    }
    let token = Owned(token);
    let mut elevation = ffi::TokenElevation {
        token_is_elevated: 0,
    };
    let mut returned = 0u32;
    // SAFETY: `elevation` is the TOKEN_ELEVATION buffer of the size passed.
    let ok = unsafe {
        ffi::GetTokenInformation(
            token.0,
            ffi::TOKEN_ELEVATION_CLASS,
            (&raw mut elevation).cast::<c_void>(),
            size_of::<ffi::TokenElevation>() as u32,
            &raw mut returned,
        )
    };
    if ok == 0 {
        return Err(os_error("elevation", "GetTokenInformation"));
    }
    Ok(elevation.token_is_elevated != 0)
}

// ---------------------------------------------------------------------------
// The process guard's needs
// ---------------------------------------------------------------------------

/// Windows has no umask.
#[cfg(feature = "guard")]
pub(crate) fn swap_umask(_mask: u32) -> Option<u32> {
    None
}

/// Not applicable: no per-process descriptor count limits sockets or files.
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

#[cfg(feature = "guard")]
pub(crate) fn lock_open_options(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    options.custom_flags(ffi::FILE_FLAG_OPEN_REPARSE_POINT);
}

#[cfg(feature = "guard")]
pub(crate) fn filesystem(path: &Path) -> Option<FsKind> {
    use std::os::windows::ffi::OsStrExt;
    let path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let _quiet = NoDialogs::new();
    let mut root = [0u16; 1024];
    // SAFETY: `path` is NUL-terminated; `root` has its length in units.
    let ok =
        unsafe { ffi::GetVolumePathNameW(path.as_ptr(), root.as_mut_ptr(), root.len() as u32) };
    if ok == 0 {
        return None;
    }
    // SAFETY: `root` is the NUL-terminated volume path just written.
    let network = unsafe { ffi::GetDriveTypeW(root.as_ptr()) } == ffi::DRIVE_REMOTE;
    let mut fs_name = [0u16; 64];
    // SAFETY: `root` is NUL-terminated; only the file-system-name buffer is
    // requested, with its length in units.
    let ok = unsafe {
        ffi::GetVolumeInformationW(
            root.as_ptr(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        )
    };
    let name = if ok == 0 {
        "other"
    } else {
        match from_wide(&fs_name).as_str() {
            "NTFS" => "NTFS",
            "ReFS" => "ReFS",
            "FAT32" => "FAT32",
            "FAT" => "FAT",
            "exFAT" => "exFAT",
            "CSVFS" => "CSVFS",
            _ => "other",
        }
    };
    Some(FsKind { name, network })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sys_windows_names_from_build_and_registry() {
        assert_eq!(
            windows_name(10, 0, 26_200, 1, Some("Windows 10 Pro")),
            "Windows 11 Pro"
        );
        assert_eq!(
            windows_name(10, 0, 19_045, 1, Some("Windows 10 Pro")),
            "Windows 10 Pro"
        );
        assert_eq!(
            windows_name(10, 0, 20_348, 3, Some("Windows Server 2022 Datacenter")),
            "Windows Server 2022 Datacenter"
        );
        assert_eq!(windows_name(10, 0, 17_763, 3, None), "Windows Server 2019");
        assert_eq!(windows_name(10, 0, 22_631, 1, None), "Windows 11");
        assert_eq!(windows_name(6, 1, 7601, 1, None), "Windows 7");
        assert_eq!(windows_name(0, 0, 0, 0, None), "Windows");
    }

    #[test]
    fn test_sys_windows_time_conversions_exact() {
        assert_eq!(hundred_ns(0), Duration::ZERO);
        assert_eq!(hundred_ns(15), Duration::from_nanos(1_500));
        assert_eq!(hundred_ns(10_000_000), Duration::from_secs(1));
        assert_eq!(
            hundred_ns(u64::MAX),
            Duration::new(u64::MAX / 10_000_000, 955_161_500)
        );
        assert_eq!(non_negative(-1), 0);
        assert_eq!(non_negative(7), 7);
        assert_eq!(
            filetime_to_system(UNIX_EPOCH_FILETIME),
            Some(SystemTime::UNIX_EPOCH)
        );
        assert_eq!(
            filetime_to_system(UNIX_EPOCH_FILETIME + 10_000_000),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
        );
        assert!(filetime_to_system(0).is_some_and(|t| t < SystemTime::UNIX_EPOCH));
    }

    #[test]
    fn test_sys_windows_live_identity() {
        assert!(os().version.is_some());
        assert!(!host_name().unwrap().is_empty());
        assert!(memory_status().unwrap().total_phys > 0);
    }
}
