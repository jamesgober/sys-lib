//! Windows: kernel32, advapi32, iphlpapi, powrprof, and ntdll, called directly.

mod ffi;

#[cfg(feature = "guard")]
pub(crate) use ffi::FILE_ATTRIBUTE_REPARSE_POINT;
use std::{
    ffi::c_void,
    mem::{MaybeUninit, size_of},
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

pub(crate) fn cpu() -> CpuFacts {
    let logical = usize::try_from(ffi::GetActiveProcessorCount(ffi::ALL_PROCESSOR_GROUPS))
        .ok()
        .filter(|&n| n > 0)
        .or_else(|| {
            std::thread::available_parallelism()
                .ok()
                .map(std::num::NonZero::get)
        })
        .unwrap_or(1);
    let key = Key::local_machine(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
    let raw_name = key.as_ref().and_then(|k| k.string("ProcessorNameString"));
    let vendor_id = key
        .as_ref()
        .and_then(|k| k.string("VendorIdentifier"))
        .unwrap_or_default();
    let vendor = if vendor_id == "GenuineIntel" {
        Vendor::Intel
    } else if vendor_id == "AuthenticAMD" {
        Vendor::Amd
    } else if vendor_id.contains("Qualcomm")
        || raw_name
            .as_deref()
            .is_some_and(|n| n.contains("Snapdragon"))
    {
        Vendor::Qualcomm
    } else if vendor_id.starts_with("ARM") {
        Vendor::Arm
    } else {
        Vendor::Unknown
    };
    CpuFacts {
        logical,
        physical: physical_cores(),
        raw_name,
        name: None,
        vendor,
    }
}

/// Counts the processor-core records of `GetLogicalProcessorInformationEx`
/// across every processor group.
fn physical_cores() -> Option<usize> {
    let mut len = 0u32;
    // SAFETY: a null buffer with length 0 asks for the size needed.
    unsafe {
        ffi::GetLogicalProcessorInformationEx(
            ffi::lpi::RELATION_PROCESSOR_CORE,
            std::ptr::null_mut(),
            &raw mut len,
        )
    };
    if len == 0 || len > 16 << 20 {
        return None;
    }
    // u64 elements keep the records 8-byte aligned, as the structs require.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` writable bytes.
    let ok = unsafe {
        ffi::GetLogicalProcessorInformationEx(
            ffi::lpi::RELATION_PROCESSOR_CORE,
            buf.as_mut_ptr().cast::<u8>(),
            &raw mut len,
        )
    };
    if ok == 0 {
        return None;
    }
    // SAFETY: `buf` is `buf.len() * 8` initialised bytes; a u8 view of it is valid.
    let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), buf.len() * 8) };
    let bytes = &bytes[..(len as usize).min(bytes.len())];
    let mut cores = 0usize;
    let mut at = 0usize;
    while at + 8 <= bytes.len() {
        let field = |o: usize| {
            u32::from_ne_bytes([
                bytes[at + o],
                bytes[at + o + 1],
                bytes[at + o + 2],
                bytes[at + o + 3],
            ])
        };
        let (relationship, size) = (
            field(ffi::lpi::RELATIONSHIP) as i32,
            field(ffi::lpi::SIZE) as usize,
        );
        if size == 0 || at + size > bytes.len() {
            break;
        }
        if relationship == ffi::lpi::RELATION_PROCESSOR_CORE {
            cores += 1;
        }
        at += size;
    }
    (cores > 0).then_some(cores)
}

pub(crate) fn cpu_available(logical: usize) -> usize {
    let process = ffi::GetCurrentProcess();
    let mut groups = [0u16; 64];
    let mut count = groups.len() as u16;
    // SAFETY: `groups` has `count` writable entries.
    let ok = unsafe { ffi::GetProcessGroupAffinity(process, &raw mut count, groups.as_mut_ptr()) };
    let mut cpus = if ok != 0 && count == 1 {
        let (mut mask, mut system) = (0usize, 0usize);
        // SAFETY: both masks are writable locals.
        let ok = unsafe { ffi::GetProcessAffinityMask(process, &raw mut mask, &raw mut system) };
        if ok != 0 {
            mask.count_ones() as usize
        } else {
            logical
        }
    } else if ok != 0 {
        // A process spanning several groups (Windows 11 and Server 2022 by
        // default, E-13) may use every processor of those groups.
        groups[..usize::from(count).min(groups.len())]
            .iter()
            .map(|&g| ffi::GetActiveProcessorCount(g) as usize)
            .sum()
    } else {
        logical
    };
    if let Some(cap) = job_cpu_cap(logical) {
        cpus = cpus.min(cap);
    }
    cpus.clamp(1, logical.max(1))
}

/// The job object's hard CPU-rate cap, in CPUs (rounded up).
fn job_cpu_cap(logical: usize) -> Option<usize> {
    let mut info = ffi::JobObjectCpuRateControlInformation {
        control_flags: 0,
        value: 0,
    };
    // SAFETY: a null job means the job of this process; `info` is the buffer
    // for the CPU-rate class, of the size passed.
    let ok = unsafe {
        ffi::QueryInformationJobObject(
            std::ptr::null_mut(),
            ffi::JOB_OBJECT_CPU_RATE_CONTROL_INFORMATION,
            (&raw mut info).cast::<c_void>(),
            size_of::<ffi::JobObjectCpuRateControlInformation>() as u32,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || info.control_flags & ffi::JOB_OBJECT_CPU_RATE_CONTROL_ENABLE == 0 {
        return None;
    }
    // The rate is a share of every processor, in hundredths of a percent.
    let rate = if info.control_flags & ffi::JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP != 0 {
        info.value
    } else if info.control_flags & ffi::JOB_OBJECT_CPU_RATE_CONTROL_MIN_MAX_RATE != 0 {
        info.value >> 16
    } else {
        return None;
    };
    let cpus = (u64::from(rate) * logical as u64).div_ceil(10_000);
    usize::try_from(cpus.max(1)).ok()
}

pub(crate) fn processor_is_64bit() -> bool {
    if cfg!(target_pointer_width = "64") {
        return true;
    }
    let (mut process, mut native) = (0u16, 0u16);
    // SAFETY: both outputs are writable locals.
    let ok = unsafe {
        ffi::IsWow64Process2(ffi::GetCurrentProcess(), &raw mut process, &raw mut native)
    };
    ok != 0
        && matches!(
            native,
            ffi::IMAGE_FILE_MACHINE_AMD64 | ffi::IMAGE_FILE_MACHINE_ARM64
        )
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
        return Err(Error::Os {
            what: "host name",
            call: "GetComputerNameExW",
            code: last_error(),
        });
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
        return Err(Error::Os {
            what: "user name",
            call: "GetUserNameW",
            code: last_error(),
        });
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
        return Err(Error::Os {
            what: "elevation",
            call: "OpenProcessToken",
            code: last_error(),
        });
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
        return Err(Error::Os {
            what: "elevation",
            call: "GetTokenInformation",
            code: last_error(),
        });
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

// The guard does not count threads on Windows (no umask, so no race);
// `process::threads()` (PRC-07, 0.4) uses this.
#[allow(dead_code)]
pub(crate) fn thread_count() -> Option<usize> {
    // SAFETY: plain flags; the returned handle is checked before use.
    let snapshot = unsafe { ffi::CreateToolhelp32Snapshot(ffi::TH32CS_SNAPTHREAD, 0) };
    if snapshot == ffi::INVALID_HANDLE_VALUE {
        return None;
    }
    let snapshot = Owned(snapshot);
    let pid = std::process::id();
    let mut entry = ffi::ThreadEntry32 {
        size: size_of::<ffi::ThreadEntry32>() as u32,
        cnt_usage: 0,
        th32_thread_id: 0,
        th32_owner_process_id: 0,
        tp_base_pri: 0,
        tp_delta_pri: 0,
        flags: 0,
    };
    let mut count = 0usize;
    // SAFETY: `entry` is a THREADENTRY32 with its size set.
    let mut more = unsafe { ffi::Thread32First(snapshot.0, &raw mut entry) } != 0;
    while more {
        if entry.th32_owner_process_id == pid {
            count += 1;
        }
        // SAFETY: as above.
        more = unsafe { ffi::Thread32Next(snapshot.0, &raw mut entry) } != 0;
    }
    (count > 0).then_some(count)
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
    fn test_sys_windows_live_readings() {
        assert!(physical_cores().is_some_and(|c| c >= 1));
        let logical = cpu().logical;
        let a = cpu_available(logical);
        assert!(a >= 1 && a <= logical);
        assert!(os().version.is_some());
        assert!(!host_name().unwrap().is_empty());
        assert!(processor_is_64bit() || cfg!(target_pointer_width = "32"));
    }
}
