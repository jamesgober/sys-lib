//! Disks: free space, the mounted volumes, and the physical disks' counters.

use std::{
    ffi::{OsString, c_void},
    mem::{offset_of, size_of},
    os::windows::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use super::{NoDialogs, Owned, ffi, from_wide, hundred_ns, last_error, non_negative, unsupported};
use crate::{
    Error, Pressure, Result,
    disk::{Disk, DiskKind, DiskRaw, ListOptions, Space},
};

/// UTF-16 units a path may have and still be converted on the stack.
const STACK_PATH: usize = 512;

/// `\\?\Volume{GUID}\` is 49 units; the buffer leaves room.
const VOLUME_NAME: usize = 64;

/// The most UTF-16 units of mount paths one volume may report.
const MAX_MOUNT_PATHS: usize = 1 << 16;

/// Physical drive numbers probed past the last one that answered.
const MISSES: u32 = 8;

/// The highest physical drive number probed.
const MAX_DRIVES: u32 = 1024;

/// Calls `f` with `path` as NUL-terminated UTF-16, converted on the stack up
/// to 511 units and allocated only past that.
fn with_wide_path<T>(path: &Path, f: impl FnOnce(&[u16]) -> Result<T>) -> Result<T> {
    let nul = || Error::InvalidInput {
        what: "path",
        reason: "contains a NUL character",
    };
    let mut stack = [0u16; STACK_PATH];
    let mut len = 0usize;
    for unit in path.as_os_str().encode_wide() {
        if unit == 0 {
            return Err(nul());
        }
        if len + 1 == STACK_PATH {
            let heap: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            if heap[..heap.len() - 1].contains(&0) {
                return Err(nul());
            }
            return f(&heap);
        }
        stack[len] = unit;
        len += 1;
    }
    f(&stack[..=len])
}

/// `GetDiskFreeSpaceExW` takes a directory; for a file, the space is read at
/// the root of the volume holding it.
pub(crate) fn disk_space(path: &Path) -> Result<Space> {
    let _quiet = NoDialogs::new();
    with_wide_path(path, |p| match space_of(p) {
        Err(code) if code as u32 == ffi::ERROR_DIRECTORY => {
            let mut root = [0u16; STACK_PATH];
            // SAFETY: `p` is NUL-terminated; `root` is writable for its length
            // in units.
            let ok = unsafe {
                ffi::GetVolumePathNameW(p.as_ptr(), root.as_mut_ptr(), root.len() as u32)
            };
            if ok == 0 {
                return Err(space_error(code));
            }
            space_of(&root).map_err(space_error)
        }
        r => r.map_err(space_error),
    })
}

/// `GetDiskFreeSpaceExW` on a NUL-terminated path, or the error code.
fn space_of(path: &[u16]) -> core::result::Result<Space, i32> {
    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: `path` is NUL-terminated; the three outputs are writable locals.
    let ok = unsafe {
        ffi::GetDiskFreeSpaceExW(
            path.as_ptr(),
            &raw mut available,
            &raw mut total,
            &raw mut free,
        )
    };
    if ok == 0 {
        return Err(last_error());
    }
    Ok(Space::new(total, free, available))
}

fn space_error(code: i32) -> Error {
    match code as u32 {
        ffi::ERROR_FILE_NOT_FOUND
        | ffi::ERROR_PATH_NOT_FOUND
        | ffi::ERROR_INVALID_DRIVE
        | ffi::ERROR_NOT_READY => Error::NotFound {
            what: "disk space",
            source: "GetDiskFreeSpaceExW",
        },
        ffi::ERROR_ACCESS_DENIED => Error::PermissionDenied {
            what: "disk space",
            hint: "run as a user that may read the directory, or treat the value as unknown",
        },
        _ => Error::Os {
            what: "disk space",
            call: "GetDiskFreeSpaceExW",
            code,
        },
    }
}

/// A mount point to describe.
struct Root {
    /// The mount point, NUL-terminated, with its trailing backslash.
    path: Vec<u16>,
    /// `C:` for a drive letter, the volume's `\\?\Volume{GUID}\` name for a
    /// mounted folder.
    device: String,
    /// Found through the volume list (otherwise a drive letter no volume
    /// claimed: a network drive, a `subst` drive).
    volume: bool,
}

/// A volume search, closed on drop.
struct FindVolume(ffi::Handle);

impl Drop for FindVolume {
    fn drop(&mut self) {
        // SAFETY: the search was opened by FindFirstVolumeW and is closed once.
        unsafe { ffi::FindVolumeClose(self.0) };
    }
}

/// The volumes and their mount points: drive letters, then mounted folders
/// (`FindFirstVolumeW`, `GetVolumePathNamesForVolumeNameW`), then the drive
/// letters no volume claimed (`GetLogicalDriveStringsW`). Each volume is
/// listed once unless `include_duplicates`; a volume with no mount point is
/// not listed. Every call runs with critical-error dialogs off (threat T8).
pub(crate) fn disks(options: &ListOptions) -> Result<Vec<Disk>> {
    let _quiet = NoDialogs::new();
    let mut roots = Vec::new();
    let mut letters = 0u32;
    let volumes = volume_roots(options, &mut roots, &mut letters)?;
    let drives = drive_roots(options, &mut roots, letters)?;
    if !volumes && !drives {
        return Err(Error::Os {
            what: "disk list",
            call: "GetLogicalDriveStringsW",
            code: last_error(),
        });
    }
    let mut serials: Vec<u32> = Vec::new();
    let mut disks = Vec::with_capacity(roots.len());
    for root in &roots {
        let Some((disk, serial)) = describe(root, options) else {
            continue;
        };
        if let (false, Some(serial)) = (options.include_duplicates, serial) {
            // A drive letter no volume claimed but showing a listed volume's
            // serial number is a `subst` alias of it.
            if root.volume {
                serials.push(serial);
            } else if disk.kind != DiskKind::Network && serials.contains(&serial) {
                continue;
            }
        }
        disks.push(disk);
    }
    Ok(disks)
}

fn too_many(options: &ListOptions) -> Error {
    Error::TooLarge {
        what: "disk list",
        limit: options.max_mounts as u64,
    }
}

fn push(roots: &mut Vec<Root>, root: Root, options: &ListOptions) -> Result<()> {
    if roots.len() >= options.max_mounts {
        return Err(too_many(options));
    }
    roots.push(root);
    Ok(())
}

/// Adds every volume's mount points; false when the volumes cannot be listed.
fn volume_roots(options: &ListOptions, roots: &mut Vec<Root>, letters: &mut u32) -> Result<bool> {
    let mut name = [0u16; VOLUME_NAME];
    // SAFETY: `name` is writable for its length in units.
    let find = unsafe { ffi::FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32) };
    if find == ffi::INVALID_HANDLE_VALUE {
        return Ok(false);
    }
    let find = FindVolume(find);
    let mut paths = Vec::new();
    let mut volumes = 0usize;
    loop {
        volumes += 1;
        if volumes > options.max_mounts {
            return Err(too_many(options));
        }
        if mount_paths(&name, &mut paths) {
            add_volume(&name, &paths, options, roots, letters)?;
        }
        // SAFETY: `find` is the open search; `name` is writable for its length.
        let more = unsafe { ffi::FindNextVolumeW(find.0, name.as_mut_ptr(), name.len() as u32) };
        if more == 0 {
            return Ok(true);
        }
    }
}

/// Fills `buf` with a volume's mount points (a list of NUL-terminated paths
/// ending in an empty one); false when it has none or they cannot be read.
fn mount_paths(volume: &[u16], buf: &mut Vec<u16>) -> bool {
    buf.clear();
    buf.resize(512, 0);
    for _ in 0..3 {
        let mut needed = 0u32;
        // SAFETY: `volume` is NUL-terminated; `buf` is writable for its length
        // in units; `needed` is a writable local.
        let ok = unsafe {
            ffi::GetVolumePathNamesForVolumeNameW(
                volume.as_ptr(),
                buf.as_mut_ptr(),
                buf.len() as u32,
                &raw mut needed,
            )
        };
        if ok != 0 {
            return buf.first().is_some_and(|&c| c != 0);
        }
        let needed = needed as usize;
        if last_error() as u32 != ffi::ERROR_MORE_DATA
            || needed <= buf.len()
            || needed > MAX_MOUNT_PATHS
        {
            return false;
        }
        buf.resize(needed, 0);
    }
    false
}

/// The strings of a `REG_MULTI_SZ`-style list, up to the empty one.
fn multi_sz(buf: &[u16]) -> impl Iterator<Item = &[u16]> {
    buf.split(|&c| c == 0).take_while(|s| !s.is_empty())
}

/// The drive letter (0 for `A`) of a `X:\` root.
fn drive_letter(path: &[u16]) -> Option<u32> {
    match *path {
        [letter, colon, slash]
            if colon == u16::from(b':')
                && slash == u16::from(b'\\')
                && u8::try_from(letter).is_ok_and(|l| l.is_ascii_alphabetic()) =>
        {
            Some(u32::from(letter | 0x20) - u32::from(b'a'))
        }
        _ => None,
    }
}

fn letter_device(letter: u32) -> String {
    let c = char::from_u32(u32::from(b'A') + letter).unwrap_or('?');
    format!("{c}:")
}

fn nul_terminated(path: &[u16]) -> Vec<u16> {
    path.iter().copied().chain(std::iter::once(0)).collect()
}

/// Adds one volume's mount points, drive letters first: the first is the
/// volume's own entry, the others are duplicates.
fn add_volume(
    volume: &[u16],
    paths: &[u16],
    options: &ListOptions,
    roots: &mut Vec<Root>,
    letters: &mut u32,
) -> Result<()> {
    let mut list: Vec<&[u16]> = multi_sz(paths).collect();
    list.sort_by_key(|p| drive_letter(p).is_none());
    for letter in list.iter().filter_map(|p| drive_letter(p)) {
        *letters |= 1 << letter;
    }
    let take = if options.include_duplicates {
        list.len()
    } else {
        1
    };
    for path in list.into_iter().take(take) {
        let device = drive_letter(path).map_or_else(|| from_wide(volume), letter_device);
        let root = Root {
            path: nul_terminated(path),
            device,
            volume: true,
        };
        push(roots, root, options)?;
    }
    Ok(())
}

/// Adds the drive letters no volume claimed; false when they cannot be listed.
fn drive_roots(options: &ListOptions, roots: &mut Vec<Root>, letters: u32) -> Result<bool> {
    // 26 roots of 4 units ("C:\" and its NUL) and the final NUL.
    let mut buf = [0u16; 128];
    // SAFETY: `buf` is writable for its length in units.
    let n = unsafe { ffi::GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
    if n == 0 || n >= buf.len() {
        return Ok(false);
    }
    for path in multi_sz(&buf[..n]) {
        let Some(letter) = drive_letter(path) else {
            continue;
        };
        if letters & (1 << letter) != 0 {
            continue;
        }
        let root = Root {
            path: nul_terminated(path),
            device: letter_device(letter),
            volume: false,
        };
        push(roots, root, options)?;
    }
    Ok(true)
}

/// What `GetDriveTypeW` says the storage is. Telling solid-state from rotating
/// fixed disks arrives in 0.5.
fn drive_kind(drive: u32) -> DiskKind {
    match drive {
        ffi::DRIVE_REMOVABLE => DiskKind::Removable,
        ffi::DRIVE_REMOTE => DiskKind::Network,
        ffi::DRIVE_CDROM => DiskKind::Optical,
        ffi::DRIVE_RAMDISK => DiskKind::Ram,
        _ => DiskKind::Unknown,
    }
}

/// One mount point as a `Disk`, with its volume serial number when the volume
/// answered; `None` for a network drive left out, or a root that is gone.
/// Media not present (an empty card reader, a BitLocker-locked volume) is
/// listed with filesystem `unknown` and no space (E-38).
fn describe(root: &Root, options: &ListOptions) -> Option<(Disk, Option<u32>)> {
    // SAFETY: `root.path` is NUL-terminated.
    let drive = unsafe { ffi::GetDriveTypeW(root.path.as_ptr()) };
    if drive == ffi::DRIVE_NO_ROOT_DIR || (drive == ffi::DRIVE_REMOTE && !options.include_network) {
        return None;
    }
    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    let (mut serial, mut flags) = (0u32, 0u32);
    // SAFETY: `root.path` is NUL-terminated; both name buffers are writable for
    // their lengths in units; serial and flags are writable locals.
    let ok = unsafe {
        ffi::GetVolumeInformationW(
            root.path.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            &raw mut serial,
            std::ptr::null_mut(),
            &raw mut flags,
            fs.as_mut_ptr(),
            fs.len() as u32,
        )
    } != 0;
    let filesystem = Some(from_wide(&fs))
        .filter(|f| ok && !f.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    let disk = Disk {
        mount_point: PathBuf::from(OsString::from_wide(
            &root.path[..root.path.len().saturating_sub(1)],
        )),
        device: root.device.clone(),
        label: Some(from_wide(&label)).filter(|l| ok && !l.is_empty()),
        filesystem,
        kind: drive_kind(drive),
        space: if ok { space_of(&root.path).ok() } else { None },
        read_only: ok && flags & ffi::FILE_READ_ONLY_VOLUME != 0,
        removable: drive == ffi::DRIVE_REMOVABLE,
    };
    Some((disk, ok.then_some(serial)))
}

/// What opening and querying one physical drive gave.
enum Probe {
    Found(ffi::DiskPerformance),
    Missing,
    Denied,
    NoCounters,
}

/// `\\.\PhysicalDrive<n>`, NUL-terminated, built on the stack.
fn drive_path(n: u32, buf: &mut [u16; 32]) {
    let mut at = 0;
    for b in r"\\.\PhysicalDrive".bytes() {
        buf[at] = u16::from(b);
        at += 1;
    }
    let mut digits = [0u8; 10];
    let (mut len, mut rest) = (0, n);
    loop {
        digits[len] = b'0' + (rest % 10) as u8;
        len += 1;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    for &d in digits[..len].iter().rev() {
        buf[at] = u16::from(d);
        at += 1;
    }
    buf[at] = 0;
}

/// Opens physical drive `n` with no access rights (the counters query is
/// `FILE_ANY_ACCESS`, so this works unprivileged and disturbs no other user
/// of the disk) and reads its counters.
fn probe(n: u32) -> Probe {
    let mut path = [0u16; 32];
    drive_path(n, &mut path);
    // SAFETY: `path` is NUL-terminated; no security attributes or template.
    let handle = unsafe {
        ffi::CreateFileW(
            path.as_ptr(),
            0,
            ffi::FILE_SHARE_READ | ffi::FILE_SHARE_WRITE,
            std::ptr::null_mut(),
            ffi::OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if handle == ffi::INVALID_HANDLE_VALUE {
        return if last_error() as u32 == ffi::ERROR_ACCESS_DENIED {
            Probe::Denied
        } else {
            Probe::Missing
        };
    }
    let handle = Owned(handle);
    let mut perf = ffi::DiskPerformance::default();
    let mut returned = 0u32;
    // SAFETY: `handle` is open; no input; `perf` is a writable DISK_PERFORMANCE
    // of the size passed; the call is synchronous (no OVERLAPPED).
    let ok = unsafe {
        ffi::DeviceIoControl(
            handle.0,
            ffi::IOCTL_DISK_PERFORMANCE,
            std::ptr::null(),
            0,
            (&raw mut perf).cast::<c_void>(),
            size_of::<ffi::DiskPerformance>() as u32,
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 || (returned as usize) < offset_of!(ffi::DiskPerformance, storage_device_number) {
        return Probe::NoCounters;
    }
    Probe::Found(perf)
}

/// Calls `f` with each physical drive's counters, from drive 0 up to a run of
/// numbers that do not exist. `Unsupported` when drives exist but none gives
/// counters to this process.
fn each_drive(mut f: impl FnMut(u32, &ffi::DiskPerformance)) -> Result<()> {
    let (mut found, mut refused, mut misses) = (false, false, 0);
    for n in 0..MAX_DRIVES {
        match probe(n) {
            Probe::Found(perf) => {
                found = true;
                misses = 0;
                f(n, &perf);
            }
            Probe::Missing => {
                misses += 1;
                if misses == MISSES {
                    break;
                }
            }
            Probe::Denied | Probe::NoCounters => {
                refused = true;
                misses = 0;
            }
        }
    }
    if found {
        Ok(())
    } else if refused {
        Err(unsupported("disk I/O"))
    } else {
        Err(Error::NotFound {
            what: "disk I/O",
            source: r"\\.\PhysicalDrive0",
        })
    }
}

/// The clock reading busy time is measured from: the first query's clock
/// minus the uptime then (both 100 ns units), fixed for the process.
fn clock_origin(query_time: u64) -> u64 {
    static ORIGIN: OnceLock<u64> = OnceLock::new();
    *ORIGIN.get_or_init(|| query_time.saturating_sub(ffi::GetTickCount64().saturating_mul(10_000)))
}

/// Busy time: the time the counters cover minus idle time. `QueryTime` is the
/// system time of the query (wall clock, 100 ns since 1601), not a span, so it
/// is taken from `origin`: with a fixed origin the difference between two
/// readings is exact, and the total is about the busy time since boot.
fn busy_time(query_time: u64, idle_time: u64, origin: u64) -> Duration {
    hundred_ns(query_time.saturating_sub(origin).saturating_sub(idle_time))
}

fn disk_raw(p: &ffi::DiskPerformance) -> DiskRaw {
    let query_time = non_negative(p.query_time);
    DiskRaw {
        read: non_negative(p.bytes_read),
        written: non_negative(p.bytes_written),
        reads: u64::from(p.read_count),
        writes: u64::from(p.write_count),
        busy: Some(busy_time(
            query_time,
            non_negative(p.idle_time),
            clock_origin(query_time),
        )),
        op_time: Some(hundred_ns(
            non_negative(p.read_time).saturating_add(non_negative(p.write_time)),
        )),
        disks: 1,
    }
}

/// Every physical drive's counters added up: nothing allocated.
pub(crate) fn disk_io_totals() -> Result<DiskRaw> {
    let mut total = DiskRaw::default();
    each_drive(|_, p| total.add(&disk_raw(p)))?;
    Ok(total)
}

pub(crate) fn disk_io_counters() -> Result<Vec<(String, DiskRaw)>> {
    let mut out = Vec::new();
    each_drive(|n, p| out.push((format!("PhysicalDrive{n}"), disk_raw(p))))?;
    Ok(out)
}

/// Windows has no I/O stall measure.
pub(crate) fn disk_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn test_disk_windows_drive_letters_and_lists() {
        assert_eq!(drive_letter(&units(r"C:\")), Some(2));
        assert_eq!(drive_letter(&units(r"z:\")), Some(25));
        assert_eq!(drive_letter(&units(r"C:\mnt\")), None);
        assert_eq!(drive_letter(&units("C:")), None);
        assert_eq!(drive_letter(&units(r"1:\")), None);
        assert_eq!(letter_device(2), "C:");
        let list = units("C:\\\0D:\\data\\\0\0junk");
        let paths: Vec<String> = multi_sz(&list).map(String::from_utf16_lossy).collect();
        assert_eq!(paths, [r"C:\", r"D:\data\"]);
        assert_eq!(multi_sz(&[0, 0]).count(), 0);
        assert_eq!(multi_sz(&[]).count(), 0);
        assert_eq!(drive_kind(ffi::DRIVE_FIXED), DiskKind::Unknown);
        assert_eq!(drive_kind(ffi::DRIVE_CDROM), DiskKind::Optical);
        assert_eq!(drive_kind(ffi::DRIVE_REMOVABLE), DiskKind::Removable);
    }

    #[test]
    fn test_disk_windows_drive_path_on_the_stack() {
        let mut buf = [0xFFFFu16; 32];
        drive_path(0, &mut buf);
        assert_eq!(from_wide(&buf), r"\\.\PhysicalDrive0");
        drive_path(u32::MAX, &mut buf);
        assert_eq!(from_wide(&buf), r"\\.\PhysicalDrive4294967295");
        drive_path(17, &mut buf);
        assert_eq!(from_wide(&buf), r"\\.\PhysicalDrive17");
    }

    #[test]
    fn test_disk_windows_busy_time_from_a_fixed_origin() {
        // 100 ns units: 10 s on the clock since the origin, 4 s idle.
        let origin = 1_000_000_000;
        let busy = busy_time(origin + 100_000_000, 40_000_000, origin);
        assert_eq!(busy, Duration::from_secs(6));
        // Idle above the span saturates.
        assert_eq!(busy_time(origin + 1, 5, origin), Duration::ZERO);
        assert_eq!(busy_time(0, 0, origin), Duration::ZERO);
    }

    #[test]
    fn test_disk_windows_wide_paths_short_long_and_nul() {
        let short = with_wide_path(Path::new(r"C:\x"), |p| Ok(p.to_vec())).unwrap();
        assert_eq!(short, units("C:\\x\0"));
        let long = "a".repeat(STACK_PATH + 10);
        let n = with_wide_path(Path::new(&long), |p| Ok(p.len())).unwrap();
        assert_eq!(n, STACK_PATH + 11);
        let edge = "b".repeat(STACK_PATH - 1);
        let n = with_wide_path(Path::new(&edge), |p| Ok(p.len())).unwrap();
        assert_eq!(n, STACK_PATH);
        assert!(matches!(
            with_wide_path(Path::new("a\0b"), |_| Ok(())),
            Err(Error::InvalidInput { .. })
        ));
    }

    #[test]
    fn test_disk_windows_live_space_list_and_counters() {
        let s = disk_space(Path::new(".")).unwrap();
        assert!(s.total.raw() > 0 && s.available <= s.free && s.free <= s.total);
        // A file: the space of the volume holding it.
        let file = disk_space(Path::new("Cargo.toml")).unwrap();
        assert_eq!(file.total, s.total);
        assert!(
            disk_space(Path::new(r"C:\definitely\not\here\sys-lib"))
                .is_err_and(|e| e.is_unavailable())
        );
        let disks = disks(&ListOptions::default()).unwrap();
        assert!(!disks.is_empty());
        for d in &disks {
            assert!(!d.filesystem.is_empty() && !d.device.is_empty(), "{d:?}");
            assert!(d.mount_point.to_string_lossy().ends_with('\\'), "{d:?}");
            assert_ne!(d.kind, DiskKind::Network);
        }
        let system = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_owned());
        assert!(
            disks.iter().any(|d| d.device.eq_ignore_ascii_case(&system)),
            "{disks:?}"
        );
        let small = ListOptions {
            max_mounts: 0,
            ..ListOptions::default()
        };
        assert!(matches!(super::disks(&small), Err(Error::TooLarge { .. })));
        match disk_io_counters() {
            Ok(drives) => {
                assert!(!drives.is_empty());
                let total = disk_io_totals().unwrap();
                assert!(total.busy.is_some() && total.op_time.is_some());
                assert!(
                    drives
                        .iter()
                        .all(|(name, _)| name.starts_with("PhysicalDrive"))
                );
            }
            Err(e) => assert!(e.is_unavailable(), "{e}"),
        }
        assert!(disk_pressure().unwrap().is_none());
    }
}
