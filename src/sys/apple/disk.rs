//! Disks: `statfs` for one path, `getfsstat` for the mount table (macOS), and
//! on macOS the IOKit block-storage statistics for I/O and device
//! characteristics for the medium, and `getattrlist` for the volume name.

use std::{
    ffi::CStr,
    mem::MaybeUninit,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::Path,
};

use super::{ffi, last_errno, os_error};
use crate::{
    Error, Pressure, Result,
    disk::{Disk, DiskKind, DiskRaw, ListOptions, Space},
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
        // A network mount is here only when the caller included it, accepting
        // the risk of a hang on a dead one: its space is read like the rest.
        let space = fresh_space(&mount_point_of(st)).unwrap_or_else(|| space_of(st));
        out.push(disk_from(st, class, space));
    }
    Ok(out)
}

/// A `statfs`'s mount point.
fn mount_point_of(st: &ffi::Statfs) -> std::path::PathBuf {
    std::path::PathBuf::from(std::ffi::OsString::from_vec(super::c_array_bytes(
        &st.mntonname,
    )))
}

/// The `Disk` for a mount, the same from the list and from `disk_of`.
fn disk_from(st: &ffi::Statfs, class: DiskKind, space: Space) -> Disk {
    Disk {
        device: crate::parse::lossy(&super::c_array_bytes(&st.mntfromname), PATH_MAX),
        mount_point: mount_point_of(st),
        label: volume_label(st, class),
        filesystem: crate::parse::lossy(&super::c_array_bytes(&st.fstypename), 16),
        kind: medium(st, class),
        space: Some(space),
        read_only: st.flags & ffi::MNT_RDONLY != 0,
        removable: st.flags & ffi::MNT_REMOVABLE != 0,
    }
}

/// The mount holding `path`, from one `statfs`, which names the mount without
/// reading a table. Every kind of mount is answered: the caller chose the path.
pub(crate) fn disk_of(path: &Path) -> Result<Disk> {
    const WHAT: &str = "disk of a path";
    let mut buf = [0u8; PATH_MAX];
    let path = c_path(path, &mut buf)?;
    let st = statfs(path).map_err(|code| os_error(WHAT, "statfs", code))?;
    let every = ListOptions {
        include_network: true,
        include_pseudo: true,
        include_duplicates: true,
        ..ListOptions::default()
    };
    let class = classify(&st, &every).unwrap_or(DiskKind::Unknown);
    Ok(disk_from(&st, class, space_of(&st)))
}

/// `Ssd` or `Hdd` for a fixed local disk, from IOKit; every other kind stays.
#[cfg(target_os = "macos")]
fn medium(st: &ffi::Statfs, class: DiskKind) -> DiskKind {
    if class != DiskKind::Unknown {
        return class;
    }
    bsd_name(&super::c_array_bytes(&st.mntfromname))
        .and_then(|name| iokit::medium(&name))
        .unwrap_or(class)
}

/// iOS has no public IOKit.
#[cfg(not(target_os = "macos"))]
fn medium(_st: &ffi::Statfs, class: DiskKind) -> DiskKind {
    class
}

/// The BSD name of a mount's device: `/dev/disk3s1s1` is `disk3s1s1`, and an
/// APFS snapshot names its volume after an `@` (`snapshot@/dev/disk1s5`).
/// Anything else (`map auto_home`, a server path) has none.
#[cfg(any(target_os = "macos", test))]
fn bsd_name(device: &[u8]) -> Option<std::ffi::CString> {
    let device = device.rsplit(|&b| b == b'@').next()?;
    let name = device.strip_prefix(b"/dev/")?;
    let valid = !name.is_empty() && name.iter().all(u8::is_ascii_alphanumeric);
    if valid {
        std::ffi::CString::new(name).ok()
    } else {
        None
    }
}

/// The name of the volume mounted here, as Finder shows it. Network and pseudo
/// mounts are not asked: `getattrlist` would wait on a dead server (threat T8).
#[cfg(target_os = "macos")]
fn volume_label(st: &ffi::Statfs, class: DiskKind) -> Option<String> {
    if matches!(class, DiskKind::Network | DiskKind::Virtual) {
        return None;
    }
    let mut path = [0u8; PATH_MAX];
    let path = c_path(&mount_point_of(st), &mut path).ok()?;
    let mut list = ffi::Attrlist {
        bitmapcount: ffi::ATTR_BIT_MAP_COUNT,
        volattr: ffi::ATTR_VOL_INFO | ffi::ATTR_VOL_NAME,
        ..ffi::Attrlist::default()
    };
    // Room for the length, the `attrreference_t`, and a name of `MAXPATHLEN`.
    let mut buf = [0u8; LABEL_NAME + PATH_MAX];
    // SAFETY: `path` is NUL-terminated, `list` is a valid `attrlist`, and `buf`
    // is `buf.len()` writable bytes, of which the kernel writes at most that many.
    let rc = unsafe {
        ffi::getattrlist(
            path.as_ptr(),
            (&raw mut list).cast::<core::ffi::c_void>(),
            buf.as_mut_ptr().cast::<core::ffi::c_void>(),
            buf.len(),
            ffi::FSOPT_NOFOLLOW,
        )
    };
    if rc != 0 {
        return None;
    }
    parse_label(&buf)
}

/// iOS keeps the volumes to itself.
#[cfg(not(target_os = "macos"))]
fn volume_label(_st: &ffi::Statfs, _class: DiskKind) -> Option<String> {
    None
}

/// Where an `ATTR_VOL_NAME` answer's `attrreference_t` ends.
#[cfg(any(target_os = "macos", test))]
const LABEL_NAME: usize = 12;

/// The name in a `getattrlist` answer: a `u32` total length, then an
/// `attrreference_t` (the data's offset from the reference itself, and its
/// length with the NUL), then the data. Everything is checked against the
/// total and the buffer.
#[cfg(any(target_os = "macos", test))]
fn parse_label(buf: &[u8]) -> Option<String> {
    const REFERENCE: usize = 4;
    let total = usize::try_from(super::u32_at(buf, 0)?).ok()?;
    let buf = buf.get(..total)?;
    let offset = usize::try_from(super::i32_at(buf, REFERENCE)?).ok()?;
    let len = usize::try_from(super::u32_at(buf, REFERENCE + 4)?).ok()?;
    let start = REFERENCE.checked_add(offset)?;
    let name = buf.get(start..start.checked_add(len)?)?;
    let name = name.split(|&b| b == 0).next()?;
    let text = crate::parse::lossy(name, PATH_MAX);
    (!text.is_empty()).then_some(text)
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
/// - Removable media (`MNT_REMOVABLE`): `Removable`; anything else `Unknown`,
///   which `medium` then refines.
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

/// The IOKit side of disks: every object is released by its owner on every
/// path.
#[cfg(target_os = "macos")]
mod iokit {
    use core::time::Duration;
    use std::ffi::CStr;

    use super::super::{
        ffi,
        iokit::{Cf, Io, Ref},
        kern_error,
    };
    use crate::{
        Error, Result,
        disk::{DiskKind, DiskRaw},
    };

    const WHAT: &str = "disk I/O";

    /// The most block-storage drivers read; more is `TooLarge`.
    const MAX_DRIVERS: usize = 4_096;

    /// The most registry levels walked from a volume to its device: an APFS
    /// volume sits on a container on a partition on a whole disk, a few levels
    /// each.
    const MAX_DEPTH: usize = 32;

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
            let key = |text| Cf::string(WHAT, text);
            Ok(Keys {
                statistics: key("Statistics")?,
                bytes_read: key("Bytes (Read)")?,
                bytes_written: key("Bytes (Write)")?,
                reads: key("Operations (Read)")?,
                writes: key("Operations (Write)")?,
                read_time: key("Total Time (Read)")?,
                write_time: key("Total Time (Write)")?,
                bsd_name: key("BSD Name")?,
            })
        }
    }

    /// Calls `f` with each `IOBlockStorageDriver` and its position.
    pub(super) fn for_each_driver(mut f: impl FnMut(&Io, usize) -> Result<()>) -> Result<()> {
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
        let Some(iterator) = Io::new(iterator) else {
            return Ok(());
        };
        for n in 0..=MAX_DRIVERS {
            // SAFETY: `iterator` is a live iterator this function owns.
            let Some(driver) = Io::new(unsafe { ffi::IOIteratorNext(iterator.0) }) else {
                return Ok(());
            };
            if n == MAX_DRIVERS {
                break;
            }
            f(&driver, n)?;
        }
        Err(Error::TooLarge {
            what: "block storage drivers",
            limit: MAX_DRIVERS as u64,
        })
    }

    /// A driver's `Statistics` dictionary, or `None` when it has none.
    pub(super) fn statistics(driver: &Io, keys: &Keys) -> Option<DiskRaw> {
        let stats = driver.property(&keys.statistics)?;
        let stats = stats.get();
        if !stats.is_dictionary() {
            return None;
        }
        let read_time = number(stats, &keys.read_time);
        let write_time = number(stats, &keys.write_time);
        Some(DiskRaw {
            read: number(stats, &keys.bytes_read),
            written: number(stats, &keys.bytes_written),
            reads: number(stats, &keys.reads),
            writes: number(stats, &keys.writes),
            busy: None,
            op_time: Some(Duration::from_nanos(read_time.saturating_add(write_time))),
            disks: 1,
        })
    }

    /// An integer value of a dictionary; 0 when missing, negative, or not a
    /// number.
    fn number(dictionary: Ref<'_>, key: &Cf) -> u64 {
        dictionary
            .value(key)
            .and_then(Ref::int)
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0)
    }

    /// The BSD name (`disk0`) of the driver's whole-disk media, its first child
    /// in the service plane, as `iostat` finds it.
    pub(super) fn bsd_name(driver: &Io, keys: &Keys) -> Option<String> {
        let media = driver.child()?;
        let name = media.property(&keys.bsd_name)?;
        let mut buf = [0u8; 64];
        let text = crate::parse::lossy(name.get().text(&mut buf)?, 64);
        (!text.is_empty()).then_some(text)
    }

    /// What a BSD device is stored on: `Medium Type` in the `Device
    /// Characteristics` of the nearest ancestor that has them. `None` when none
    /// says (a virtual disk).
    pub(super) fn medium(name: &CStr) -> Option<DiskKind> {
        let characteristics = Cf::string(WHAT, "Device Characteristics").ok()?;
        let medium_type = Cf::string(WHAT, "Medium Type").ok()?;
        let mut entry = Io::bsd_device(name)?;
        for _ in 0..MAX_DEPTH {
            entry = entry.parent()?;
            if let Some(found) = entry.property(&characteristics) {
                let mut buf = [0u8; 32];
                let value = found.get().value(&medium_type)?.text(&mut buf)?;
                return super::medium_kind(value);
            }
        }
        None
    }
}

/// The kind a `Medium Type` value names (IOStorageDeviceCharacteristics.h).
#[cfg(any(target_os = "macos", test))]
fn medium_kind(value: &[u8]) -> Option<DiskKind> {
    match value {
        b"Solid State" => Some(DiskKind::Ssd),
        b"Rotational" => Some(DiskKind::Hdd),
        _ => None,
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

    #[test]
    fn test_disk_apple_bsd_name_from_device() {
        assert_eq!(
            bsd_name(b"/dev/disk3s1s1").unwrap().as_c_str(),
            c"disk3s1s1"
        );
        let snapshot = bsd_name(b"com.apple.os.update-0A1B@/dev/disk1s5");
        assert_eq!(snapshot.unwrap().as_c_str(), c"disk1s5");
        for device in [
            &b"map auto_home"[..],
            b"devfs",
            b"//user@server/share",
            b"/dev/",
            b"/dev/../etc",
            b"/dev/disk0 s1",
        ] {
            assert_eq!(bsd_name(device), None, "{device:?}");
        }
    }

    #[test]
    fn test_disk_apple_medium_values() {
        assert_eq!(medium_kind(b"Solid State"), Some(DiskKind::Ssd));
        assert_eq!(medium_kind(b"Rotational"), Some(DiskKind::Hdd));
        assert_eq!(medium_kind(b"Tape"), None);
        assert_eq!(medium_kind(b""), None);
    }

    /// A `getattrlist` answer holding `name` with its NUL.
    fn label_answer(name: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        let total = (LABEL_NAME + name.len() + 1) as u32;
        buf.extend(total.to_ne_bytes());
        // The data follows the eight-byte reference.
        buf.extend(8i32.to_ne_bytes());
        buf.extend((name.len() as u32 + 1).to_ne_bytes());
        buf.extend(name);
        buf.push(0);
        buf
    }

    #[test]
    fn test_disk_apple_label_from_attribute_buffer() {
        let mut buf = label_answer("Macintosh HD".as_bytes());
        assert_eq!(parse_label(&buf).as_deref(), Some("Macintosh HD"));
        buf.resize(1036, 0);
        assert_eq!(parse_label(&buf).as_deref(), Some("Macintosh HD"));
        assert_eq!(
            parse_label(&label_answer("\u{e9}t\u{e9}".as_bytes())).as_deref(),
            Some("\u{e9}t\u{e9}")
        );
        assert_eq!(parse_label(&label_answer(b"")), None, "an unnamed volume");
    }

    #[test]
    fn test_disk_apple_label_rejects_bad_offsets() {
        let good = label_answer(b"Data");
        assert_eq!(parse_label(&good[..6]), None);
        assert_eq!(parse_label(&[]), None);
        // The total runs past the buffer.
        let mut long = good.clone();
        long[..4].copy_from_slice(&5000u32.to_ne_bytes());
        assert_eq!(parse_label(&long), None);
        // A negative offset, and one past the end.
        let mut negative = good.clone();
        negative[4..8].copy_from_slice(&(-8i32).to_ne_bytes());
        assert_eq!(parse_label(&negative), None);
        let mut past = good.clone();
        past[4..8].copy_from_slice(&4000i32.to_ne_bytes());
        assert_eq!(parse_label(&past), None);
        // A length that overflows the end.
        let mut huge = good;
        huge[8..12].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert_eq!(parse_label(&huge), None);
    }

    fn mount(fstype: &[u8], flags: u32) -> ffi::Statfs {
        // SAFETY: a zeroed `statfs` is valid (integers and arrays only).
        let mut st: ffi::Statfs = unsafe { MaybeUninit::zeroed().assume_init() };
        for (dst, &src) in st.fstypename.iter_mut().zip(fstype) {
            *dst = src.cast_signed();
        }
        st.flags = flags;
        st
    }

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

    #[cfg(target_os = "macos")]
    #[test]
    fn test_disk_apple_live_disk_of() {
        let root = disk_of(Path::new("/")).unwrap();
        assert_eq!(root.mount_point, Path::new("/"));
        assert!(root.device.starts_with("/dev/"), "{root:?}");
        assert!(root.space.is_some_and(|s| s.total.raw() > 0));
        let listed = disks(&ListOptions::default()).unwrap();
        let listed = listed.iter().find(|d| d.mount_point == Path::new("/"));
        assert_eq!(root.kind, listed.unwrap().kind);
        let dev = disk_of(Path::new("/dev")).unwrap();
        assert_eq!((dev.kind, dev.label), (DiskKind::Virtual, None));
        assert!(matches!(
            disk_of(Path::new("/definitely/not/here/sys-lib")),
            Err(Error::NotFound { .. })
        ));
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
