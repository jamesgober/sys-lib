//! Disks: `statfs` for one path, `getfsstat` for the mount table (macOS), and
//! the IOKit block-storage statistics for I/O (macOS).

#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStringExt;
use std::{ffi::CStr, mem::MaybeUninit, os::unix::ffi::OsStrExt, path::Path};

use super::{ffi, last_errno, os_error};
#[cfg(target_os = "macos")]
use crate::disk::DiskKind;
use crate::{
    Error, Pressure, Result,
    disk::{Disk, DiskRaw, ListOptions, Space},
};

/// `MAXPATHLEN`: the kernel refuses longer paths, so they never need the heap.
const PATH_MAX: usize = 1024;

/// `path` as a NUL-terminated C string in `buf`, without allocating.
fn c_path<'a>(path: &Path, buf: &'a mut [u8; PATH_MAX]) -> Result<&'a CStr> {
    let bytes = path.as_os_str().as_bytes();
    if bytes.contains(&0) {
        return Err(Error::InvalidInput {
            what: "disk path",
            reason: "it contains a NUL byte",
        });
    }
    if bytes.len() >= buf.len() {
        return Err(Error::InvalidInput {
            what: "disk path",
            reason: "it is longer than the 1 023-byte path limit (MAXPATHLEN)",
        });
    }
    buf[..bytes.len()].copy_from_slice(bytes);
    buf[bytes.len()] = 0;
    CStr::from_bytes_with_nul(&buf[..=bytes.len()]).map_err(|_| Error::InvalidInput {
        what: "disk path",
        reason: "it contains a NUL byte",
    })
}

/// `statfs` on a C path.
fn statfs(path: &CStr) -> core::result::Result<ffi::Statfs, i32> {
    let mut buf = MaybeUninit::<ffi::Statfs>::zeroed();
    // SAFETY: `path` is NUL-terminated and `buf` is a writable `statfs`.
    let rc = unsafe { ffi::statfs(path.as_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return Err(last_errno());
    }
    // SAFETY: zeroed is a valid `statfs` (integers and character arrays only),
    // and the call succeeded.
    Ok(unsafe { buf.assume_init() })
}

/// The space a `statfs` reports, in bytes.
fn space_of(st: &ffi::Statfs) -> Space {
    let block = u64::from(st.bsize);
    Space::new(
        st.blocks.saturating_mul(block),
        st.bfree.saturating_mul(block),
        st.bavail.saturating_mul(block),
    )
}

/// One `statfs` call; no allocation.
pub(crate) fn disk_space(path: &Path) -> Result<Space> {
    let mut buf = [0u8; PATH_MAX];
    let path = c_path(path, &mut buf)?;
    let st = statfs(path).map_err(|code| os_error("disk space", "statfs", code))?;
    Ok(space_of(&st))
}

/// iOS keeps the mount table inside the sandbox's reach only partly; the list
/// is macOS only.
#[cfg(not(target_os = "macos"))]
pub(crate) fn disks(_options: &ListOptions) -> Result<Vec<Disk>> {
    Err(super::unsupported("disk list"))
}

/// The mount table from `getfsstat(MNT_NOWAIT)`, which never waits on a
/// network filesystem (threat T8); the space of each local filesystem is then
/// read fresh with `statfs` (the table's own numbers can be stale).
#[cfg(target_os = "macos")]
pub(crate) fn disks(options: &ListOptions) -> Result<Vec<Disk>> {
    let table = mount_table(options.max_mounts)?;
    let mut out = Vec::new();
    for st in &table {
        let Some(class) = classify(st, options) else {
            continue;
        };
        let mount_point = std::path::PathBuf::from(std::ffi::OsString::from_vec(
            super::c_array_bytes(&st.mntonname),
        ));
        let space = match class {
            DiskKind::Network => None,
            _ => Some(fresh_space(&mount_point).unwrap_or_else(|| space_of(st))),
        };
        out.push(Disk {
            device: crate::parse::lossy(&super::c_array_bytes(&st.mntfromname), PATH_MAX),
            mount_point,
            label: None,
            filesystem: crate::parse::lossy(&super::c_array_bytes(&st.fstypename), 16),
            kind: class,
            space,
            read_only: st.flags & ffi::MNT_RDONLY != 0,
            removable: st.flags & ffi::MNT_REMOVABLE != 0,
        });
    }
    Ok(out)
}

/// `statfs` on a mount point, for numbers the kernel refreshes.
#[cfg(target_os = "macos")]
fn fresh_space(mount_point: &Path) -> Option<Space> {
    let mut buf = [0u8; PATH_MAX];
    let path = c_path(mount_point, &mut buf).ok()?;
    statfs(path).ok().map(|st| space_of(&st))
}

/// What a mount is, or `None` when `options` leave it out.
///
/// - Pseudo filesystems: `devfs` and `autofs` (`Virtual`).
/// - Network filesystems: no `MNT_LOCAL`, or a known network type (`Network`).
/// - Duplicates: the volumes the system hides from browsing (`MNT_DONTBROWSE`):
///   the APFS helper volumes (VM, Preboot, Update, xarts, iSCPreboot,
///   Hardware) and the Data volume, which share the root volume's container,
///   so its free space is counted once (E-39).
/// - Removable media (`MNT_REMOVABLE`): `Removable`; anything else `Unknown`
///   until solid-state detection (0.5).
#[cfg(target_os = "macos")]
fn classify(st: &ffi::Statfs, options: &ListOptions) -> Option<DiskKind> {
    let fstype = super::c_array_bytes(&st.fstypename);
    let pseudo = matches!(fstype.as_slice(), b"devfs" | b"autofs");
    if pseudo {
        return options.include_pseudo.then_some(DiskKind::Virtual);
    }
    let network = st.flags & ffi::MNT_LOCAL == 0
        || matches!(
            fstype.as_slice(),
            b"nfs" | b"smbfs" | b"afpfs" | b"webdav" | b"cifs" | b"ftp"
        );
    if network {
        return options.include_network.then_some(DiskKind::Network);
    }
    if st.flags & ffi::MNT_DONTBROWSE != 0 && !options.include_duplicates {
        return None;
    }
    Some(if st.flags & ffi::MNT_REMOVABLE != 0 {
        DiskKind::Removable
    } else {
        DiskKind::Unknown
    })
}

/// The mount table, bounded at `max` entries (checked before the table is
/// allocated). A table that fills the buffer is read again, at most three
/// times, in case mounts appeared between the two calls.
#[cfg(target_os = "macos")]
fn mount_table(max: usize) -> Result<Vec<ffi::Statfs>> {
    const WHAT: &str = "disk list";
    let too_large = || Error::TooLarge {
        what: "mounted filesystems",
        limit: max as u64,
    };
    let mut table: Vec<ffi::Statfs> = Vec::new();
    for _ in 0..3 {
        // SAFETY: a null buffer asks only for the number of mounts.
        let n = unsafe { ffi::getfsstat(core::ptr::null_mut(), 0, ffi::MNT_NOWAIT) };
        let n = usize::try_from(n).map_err(|_| os_error(WHAT, "getfsstat", last_errno()))?;
        if n > max {
            return Err(too_large());
        }
        // Room for a few mounts more, and never more than one past the budget.
        let room = n.saturating_add(8).min(max.saturating_add(1));
        let bytes = room
            .checked_mul(core::mem::size_of::<ffi::Statfs>())
            .and_then(|b| core::ffi::c_int::try_from(b).ok())
            .ok_or_else(too_large)?;
        table.clear();
        table.reserve_exact(room);
        // SAFETY: `table` has room for `room` structs, `bytes` in all; the
        // call writes at most `bytes`.
        let got = unsafe { ffi::getfsstat(table.as_mut_ptr(), bytes, ffi::MNT_NOWAIT) };
        let got = usize::try_from(got)
            .map_err(|_| os_error(WHAT, "getfsstat", last_errno()))?
            .min(room);
        // SAFETY: the call initialised the first `got` structs, `got <= room`
        // and `room` is within the capacity.
        unsafe { table.set_len(got) };
        if got > max {
            return Err(too_large());
        }
        if got < room {
            break;
        }
    }
    Ok(table)
}

/// IOKit is macOS only.
#[cfg(not(target_os = "macos"))]
pub(crate) fn disk_io_totals() -> Result<DiskRaw> {
    Err(super::unsupported("disk I/O"))
}

/// IOKit is macOS only.
#[cfg(not(target_os = "macos"))]
pub(crate) fn disk_io_counters() -> Result<Vec<(String, DiskRaw)>> {
    Err(super::unsupported("disk I/O"))
}

/// Every physical disk's statistics, summed.
#[cfg(target_os = "macos")]
pub(crate) fn disk_io_totals() -> Result<DiskRaw> {
    let keys = iokit::Keys::new()?;
    let mut total = DiskRaw::default();
    iokit::for_each_driver(|driver, _| {
        if let Some(raw) = iokit::statistics(driver, &keys) {
            total.add(&raw);
        }
        Ok(())
    })?;
    Ok(total)
}

/// Each physical disk's statistics, named by the BSD name of its whole-disk
/// media (`disk0`), or `disk<N>` by order when the registry has none.
#[cfg(target_os = "macos")]
pub(crate) fn disk_io_counters() -> Result<Vec<(String, DiskRaw)>> {
    let keys = iokit::Keys::new()?;
    let mut out = Vec::new();
    iokit::for_each_driver(|driver, n| {
        if let Some(raw) = iokit::statistics(driver, &keys) {
            let name = iokit::bsd_name(driver, &keys).unwrap_or_else(|| format!("disk{n}"));
            out.push((name, raw));
        }
        Ok(())
    })?;
    Ok(out)
}

/// macOS and iOS have no I/O stall measure.
pub(crate) fn disk_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

/// The IOKit side of disk I/O: every object is released by its owner on every
/// path.
#[cfg(target_os = "macos")]
mod iokit {
    use core::{
        ffi::{c_char, c_void},
        time::Duration,
    };

    use super::super::{ffi, kern_error};
    use crate::{Error, Result, disk::DiskRaw};

    const WHAT: &str = "disk I/O";

    /// The most block-storage drivers read; more is `TooLarge`.
    const MAX_DRIVERS: usize = 4_096;

    /// A CoreFoundation object this code owns (one reference), released on drop.
    pub(super) struct Cf(ffi::CfRef);

    impl Cf {
        fn new(object: ffi::CfRef) -> Option<Cf> {
            (!object.is_null()).then_some(Cf(object))
        }

        /// An immutable UTF-8 `CFString`.
        fn string(text: &str) -> Result<Cf> {
            let len = ffi::CfIndex::try_from(text.len()).unwrap_or(0);
            // SAFETY: `text` is `len` readable bytes; the null allocator is
            // the default one.
            let object = unsafe {
                ffi::CFStringCreateWithBytes(
                    core::ptr::null(),
                    text.as_ptr(),
                    len,
                    ffi::CF_STRING_ENCODING_UTF8,
                    0,
                )
            };
            Cf::new(object).ok_or_else(|| kern_error(WHAT, "CFStringCreateWithBytes", 0))
        }

        fn is(&self, type_id: ffi::CfTypeId) -> bool {
            // SAFETY: `self.0` is a live CoreFoundation object.
            unsafe { ffi::CFGetTypeID(self.0) == type_id }
        }
    }

    impl Drop for Cf {
        fn drop(&mut self) {
            // SAFETY: this value owns one reference, released once.
            unsafe { ffi::CFRelease(self.0) };
        }
    }

    /// An IOKit object this code owns, released on drop.
    pub(super) struct Io(ffi::IoObject);

    impl Drop for Io {
        fn drop(&mut self) {
            if self.0 != 0 {
                // SAFETY: this value owns the object, released once.
                unsafe { ffi::IOObjectRelease(self.0) };
            }
        }
    }

    /// The property names, created once per reading.
    pub(super) struct Keys {
        statistics: Cf,
        bytes_read: Cf,
        bytes_written: Cf,
        reads: Cf,
        writes: Cf,
        read_time: Cf,
        write_time: Cf,
        bsd_name: Cf,
    }

    impl Keys {
        /// The `IOBlockStorageDriver.h` statistics keys (times in nanoseconds)
        /// and `kIOBSDNameKey`.
        pub(super) fn new() -> Result<Keys> {
            Ok(Keys {
                statistics: Cf::string("Statistics")?,
                bytes_read: Cf::string("Bytes (Read)")?,
                bytes_written: Cf::string("Bytes (Write)")?,
                reads: Cf::string("Operations (Read)")?,
                writes: Cf::string("Operations (Write)")?,
                read_time: Cf::string("Total Time (Read)")?,
                write_time: Cf::string("Total Time (Write)")?,
                bsd_name: Cf::string("BSD Name")?,
            })
        }
    }

    /// Calls `f` with each `IOBlockStorageDriver` and its position.
    pub(super) fn for_each_driver(
        mut f: impl FnMut(ffi::IoObject, usize) -> Result<()>,
    ) -> Result<()> {
        // SAFETY: the class name is NUL-terminated.
        let matching = unsafe { ffi::IOServiceMatching(c"IOBlockStorageDriver".as_ptr()) };
        if matching.is_null() {
            return Err(kern_error(WHAT, "IOServiceMatching", 0));
        }
        let mut iterator: ffi::IoObject = 0;
        // SAFETY: `matching` is a dictionary with one reference, which the
        // call consumes on every path; `iterator` is writable.
        let kr = unsafe {
            ffi::IOServiceGetMatchingServices(
                ffi::IO_MAIN_PORT_DEFAULT,
                matching,
                &raw mut iterator,
            )
        };
        if kr != ffi::KERN_SUCCESS {
            return Err(kern_error(WHAT, "IOServiceGetMatchingServices", kr));
        }
        let iterator = Io(iterator);
        for n in 0..=MAX_DRIVERS {
            // SAFETY: `iterator` is a live iterator this function owns.
            let driver = Io(unsafe { ffi::IOIteratorNext(iterator.0) });
            if driver.0 == 0 {
                return Ok(());
            }
            if n == MAX_DRIVERS {
                break;
            }
            f(driver.0, n)?;
        }
        Err(Error::TooLarge {
            what: "block storage drivers",
            limit: MAX_DRIVERS as u64,
        })
    }

    /// A driver's `Statistics` dictionary, or `None` when it has none.
    pub(super) fn statistics(driver: ffi::IoObject, keys: &Keys) -> Option<DiskRaw> {
        // SAFETY: `driver` is a live registry entry; the key is a live
        // `CFString`; the null allocator is the default one. The result, if
        // any, carries one reference, owned by `stats`.
        let stats = Cf::new(unsafe {
            ffi::IORegistryEntryCreateCFProperty(driver, keys.statistics.0, core::ptr::null(), 0)
        })?;
        if !stats.is(ffi::CFDictionaryGetTypeID()) {
            return None;
        }
        let read_time = number(&stats, &keys.read_time);
        let write_time = number(&stats, &keys.write_time);
        Some(DiskRaw {
            read: number(&stats, &keys.bytes_read),
            written: number(&stats, &keys.bytes_written),
            reads: number(&stats, &keys.reads),
            writes: number(&stats, &keys.writes),
            busy: None,
            op_time: Some(Duration::from_nanos(read_time.saturating_add(write_time))),
        })
    }

    /// An integer value of a dictionary; 0 when missing, negative, or not a
    /// number.
    fn number(dictionary: &Cf, key: &Cf) -> u64 {
        // SAFETY: both are live objects; the value is borrowed (get rule), valid
        // while `dictionary` is.
        let value = unsafe { ffi::CFDictionaryGetValue(dictionary.0, key.0) };
        if value.is_null() {
            return 0;
        }
        // SAFETY: `value` is a live object borrowed from `dictionary`.
        let type_id = unsafe { ffi::CFGetTypeID(value) };
        if type_id != ffi::CFNumberGetTypeID() {
            return 0;
        }
        let mut n: i64 = 0;
        // SAFETY: `value` is a `CFNumber` and `n` is a writable 64-bit integer,
        // the size of `kCFNumberSInt64Type`.
        let ok = unsafe {
            ffi::CFNumberGetValue(
                value,
                ffi::CF_NUMBER_SINT64_TYPE,
                (&raw mut n).cast::<c_void>(),
            )
        };
        if ok == 0 {
            return 0;
        }
        u64::try_from(n).unwrap_or(0)
    }

    /// The BSD name (`disk0`) of the driver's whole-disk media, its first child
    /// in the service plane, as `iostat` finds it.
    pub(super) fn bsd_name(driver: ffi::IoObject, keys: &Keys) -> Option<String> {
        let mut child: ffi::IoObject = 0;
        // SAFETY: `driver` is a live registry entry, the plane name is
        // NUL-terminated, and `child` is writable; a child returned is owned by
        // `media` below.
        let kr = unsafe {
            ffi::IORegistryEntryGetChildEntry(driver, c"IOService".as_ptr(), &raw mut child)
        };
        let media = Io(child);
        if kr != ffi::KERN_SUCCESS || media.0 == 0 {
            return None;
        }
        // SAFETY: as in `statistics`, for the media entry.
        let name = Cf::new(unsafe {
            ffi::IORegistryEntryCreateCFProperty(media.0, keys.bsd_name.0, core::ptr::null(), 0)
        })?;
        if !name.is(ffi::CFStringGetTypeID()) {
            return None;
        }
        let mut buf = [0 as c_char; 64];
        // SAFETY: `name` is a `CFString` and `buf` is 64 writable bytes.
        let ok = unsafe {
            ffi::CFStringGetCString(
                name.0,
                buf.as_mut_ptr(),
                buf.len() as ffi::CfIndex,
                ffi::CF_STRING_ENCODING_UTF8,
            )
        };
        if ok == 0 {
            return None;
        }
        let bytes = super::super::c_array_bytes(&buf);
        let text = crate::parse::lossy(&bytes, 64);
        (!text.is_empty()).then_some(text)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_disk_apple_c_path_bounds_and_nul() {
        let mut buf = [0u8; PATH_MAX];
        assert_eq!(c_path(Path::new("/tmp"), &mut buf).unwrap(), c"/tmp");
        let long = PathBuf::from("a".repeat(PATH_MAX));
        assert!(matches!(
            c_path(&long, &mut buf),
            Err(Error::InvalidInput { .. })
        ));
        let nul = PathBuf::from(std::ffi::OsStr::from_bytes(b"/a\0b"));
        assert!(matches!(
            c_path(&nul, &mut buf),
            Err(Error::InvalidInput { .. })
        ));
    }

    #[test]
    fn test_disk_apple_space_multiplies_blocks() {
        // SAFETY: a zeroed `statfs` is valid (integers and arrays only).
        let mut st: ffi::Statfs = unsafe { MaybeUninit::zeroed().assume_init() };
        st.bsize = 4096;
        st.blocks = 1000;
        st.bfree = 400;
        st.bavail = 300;
        let s = space_of(&st);
        assert_eq!(
            (s.total.raw(), s.free.raw(), s.available.raw()),
            (4_096_000, 1_638_400, 1_228_800)
        );
        st.blocks = u64::MAX;
        assert_eq!(space_of(&st).total.raw(), u64::MAX);
    }

    #[cfg(target_os = "macos")]
    fn mount(fstype: &[u8], flags: u32) -> ffi::Statfs {
        // SAFETY: a zeroed `statfs` is valid (integers and arrays only).
        let mut st: ffi::Statfs = unsafe { MaybeUninit::zeroed().assume_init() };
        for (dst, &src) in st.fstypename.iter_mut().zip(fstype) {
            *dst = src.cast_signed();
        }
        st.flags = flags;
        st
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_disk_apple_classify_by_type_and_flags() {
        use ffi::{MNT_DONTBROWSE, MNT_LOCAL, MNT_REMOVABLE};
        let default = ListOptions::default();
        let all = ListOptions {
            include_network: true,
            include_pseudo: true,
            include_duplicates: true,
            ..ListOptions::default()
        };
        let root = mount(b"apfs", MNT_LOCAL);
        assert_eq!(classify(&root, &default), Some(DiskKind::Unknown));
        let usb = mount(b"msdos", MNT_LOCAL | MNT_REMOVABLE);
        assert_eq!(classify(&usb, &default), Some(DiskKind::Removable));
        // E-39: the Data and helper volumes share the root's container.
        let data = mount(b"apfs", MNT_LOCAL | MNT_DONTBROWSE);
        assert_eq!(classify(&data, &default), None);
        assert_eq!(classify(&data, &all), Some(DiskKind::Unknown));
        // T8: network filesystems, by flag or by type.
        let nfs = mount(b"nfs", 0);
        assert_eq!(classify(&nfs, &default), None);
        assert_eq!(classify(&nfs, &all), Some(DiskKind::Network));
        let smb = mount(b"smbfs", MNT_LOCAL);
        assert_eq!(classify(&smb, &all), Some(DiskKind::Network));
        // autofs has no MNT_LOCAL but is pseudo, not network.
        let auto = mount(b"autofs", MNT_DONTBROWSE);
        assert_eq!(classify(&auto, &default), None);
        assert_eq!(classify(&auto, &all), Some(DiskKind::Virtual));
        let dev = mount(b"devfs", MNT_LOCAL | MNT_DONTBROWSE);
        assert_eq!(classify(&dev, &all), Some(DiskKind::Virtual));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_disk_apple_live_list_and_io() {
        let list = disks(&ListOptions::default()).unwrap();
        assert!(list.iter().any(|d| d.mount_point == Path::new("/")));
        for d in &list {
            assert!(!d.filesystem.is_empty() && !d.device.is_empty(), "{d:?}");
            assert_ne!(d.kind, DiskKind::Network);
        }
        let tight = ListOptions {
            max_mounts: 0,
            ..ListOptions::default()
        };
        assert!(matches!(disks(&tight), Err(Error::TooLarge { .. })));
        let total = disk_io_totals().unwrap();
        let each = disk_io_counters().unwrap();
        let sum: u64 = each.iter().map(|(_, r)| r.read).sum();
        assert!(sum <= disk_io_totals().unwrap().read);
        assert!(total.read <= disk_io_totals().unwrap().read);
        for (name, _) in &each {
            assert!(name.starts_with("disk"), "{name}");
        }
    }

    #[test]
    fn test_disk_apple_live_space() {
        let s = disk_space(Path::new("/")).unwrap();
        assert!(s.total.raw() > 0 && s.available <= s.free && s.free <= s.total);
        assert!(matches!(
            disk_space(Path::new("/definitely/not/here/sys-lib")),
            Err(Error::NotFound { .. })
        ));
        assert_eq!(disk_pressure().unwrap(), None);
    }
}
