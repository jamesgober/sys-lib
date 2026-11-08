//! Disks: `statfs` for one path, `getfsstat` for the mount table, the
//! `kern.devstat.all` records for I/O, and the disk drivers' `rotating`
//! sysctls for the medium.

use core::time::Duration;
use std::{
    ffi::{CStr, CString},
    mem::MaybeUninit,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

use super::{Key, ffi, last_errno, os_error};
use crate::{
    Error, Pressure, Result,
    disk::{Disk, DiskKind, DiskRaw, ListOptions, Space},
};

/// `MAXPATHLEN`: the kernel refuses longer paths, so they never need the heap.
pub(super) const PATH_MAX: usize = 1024;

/// `path` as a NUL-terminated C string in `buf`, without allocating.
pub(super) fn c_path<'a>(path: &Path, buf: &'a mut [u8; PATH_MAX]) -> Result<&'a CStr> {
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
pub(super) fn statfs(path: &CStr) -> core::result::Result<ffi::Statfs, i32> {
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

/// The space a `statfs` reports, in bytes. UFS lets root fill the reserve, so
/// `f_bavail` can be negative: no space available.
fn space_of(st: &ffi::Statfs) -> Space {
    let block = st.bsize;
    Space::new(
        st.blocks.saturating_mul(block),
        st.bfree.saturating_mul(block),
        u64::try_from(st.bavail).unwrap_or(0).saturating_mul(block),
    )
}

/// One `statfs` call; no allocation.
pub(crate) fn disk_space(path: &Path) -> Result<Space> {
    let mut buf = [0u8; PATH_MAX];
    let path = c_path(path, &mut buf)?;
    let st = statfs(path).map_err(|code| os_error("disk space", "statfs", code))?;
    Ok(space_of(&st))
}

/// The mount table from `getfsstat(MNT_NOWAIT)`, which never waits on a
/// network filesystem (threat T8); the space of each listed filesystem is then
/// read fresh with `statfs` (the table's own numbers can be stale).
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
fn mount_point_of(st: &ffi::Statfs) -> PathBuf {
    PathBuf::from(std::ffi::OsString::from_vec(super::c_array_bytes(
        &st.mntonname,
    )))
}

/// The `Disk` for a mount, the same from the list and from `disk_of`.
fn disk_from(st: &ffi::Statfs, class: DiskKind, space: Space) -> Disk {
    let device = super::c_array_bytes(&st.mntfromname);
    let kind = if class == DiskKind::Unknown {
        medium(&device).unwrap_or(class)
    } else {
        class
    };
    Disk {
        device: crate::parse::lossy(&device, PATH_MAX),
        mount_point: mount_point_of(st),
        label: None,
        filesystem: crate::parse::lossy(&super::c_array_bytes(&st.fstypename), 16),
        kind,
        space: Some(space),
        read_only: st.flags & ffi::MNT_RDONLY != 0,
        removable: false,
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

/// `statfs` on a mount point, for numbers the kernel refreshes.
fn fresh_space(mount_point: &Path) -> Option<Space> {
    let mut buf = [0u8; PATH_MAX];
    let path = c_path(mount_point, &mut buf).ok()?;
    statfs(path).ok().map(|st| space_of(&st))
}

/// The disk driver and unit a mount's device names: `/dev/ada0p2` is
/// (`ada`, 0), `/dev/nda1` is (`nda`, 1). A ZFS dataset (`zroot/ROOT/default`),
/// a label (`/dev/gpt/rootfs`), or a server path names none.
fn disk_unit(device: &[u8]) -> Option<(&[u8], u32)> {
    let name = device.strip_prefix(b"/dev/")?;
    let letters = name.iter().take_while(|b| b.is_ascii_lowercase()).count();
    let digits = name[letters..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if letters == 0 || digits == 0 || digits > 6 {
        return None;
    }
    let driver = &name[..letters];
    let unit = core::str::from_utf8(&name[letters..letters + digits])
        .ok()?
        .parse()
        .ok()?;
    // A partition or slice may follow (`p2`, `s1a`); a further directory may not.
    let rest = &name[letters + digits..];
    rest.iter()
        .all(u8::is_ascii_alphanumeric)
        .then_some((driver, unit))
}

/// The sysctl saying whether a disk spins, for the drivers that keep one.
fn rotating_sysctl(driver: &[u8], unit: u32) -> Option<CString> {
    let name = match driver {
        b"ada" => format!("dev.ada.{unit}.rotating"),
        b"da" => format!("kern.cam.da.{unit}.rotating"),
        _ => return None,
    };
    CString::new(name).ok()
}

/// `Ssd` or `Hdd` for a mount on a whole-disk device: NVMe disks are solid
/// state; SATA and SCSI disks say whether they spin. Anything else (a virtual
/// disk, a pool, a label) stays unknown.
fn medium(device: &[u8]) -> Option<DiskKind> {
    let (driver, unit) = disk_unit(device)?;
    if matches!(driver, b"nvd" | b"nda") {
        return Some(DiskKind::Ssd);
    }
    let name = rotating_sysctl(driver, unit)?;
    match super::sysctl_u64(&name)? {
        0 => Some(DiskKind::Ssd),
        1 => Some(DiskKind::Hdd),
        _ => None,
    }
}

/// What a mount is, or `None` when `options` leave it out.
///
/// - Pseudo filesystems: `devfs`, `fdescfs`, `procfs`, `linprocfs`,
///   `linsysfs`, `tmpfs`, `mqueuefs`, `autofs` (`Virtual`).
/// - Network filesystems: no `MNT_LOCAL`, or a known network type (`Network`).
/// - Duplicates: `nullfs` (a second view of a mounted directory, as a bind
///   mount is on Linux) and mounts hidden from `df` (`MNT_IGNORE`).
/// - Anything else `Unknown`, which `medium` then refines.
fn classify(st: &ffi::Statfs, options: &ListOptions) -> Option<DiskKind> {
    let fstype = super::c_array_bytes(&st.fstypename);
    let pseudo = matches!(
        fstype.as_slice(),
        b"devfs"
            | b"fdescfs"
            | b"procfs"
            | b"linprocfs"
            | b"linsysfs"
            | b"tmpfs"
            | b"mqueuefs"
            | b"autofs"
    );
    if pseudo {
        return options.include_pseudo.then_some(DiskKind::Virtual);
    }
    let network = st.flags & ffi::MNT_LOCAL == 0 || matches!(fstype.as_slice(), b"nfs" | b"smbfs");
    if network {
        return options.include_network.then_some(DiskKind::Network);
    }
    let duplicate = fstype == b"nullfs" || st.flags & ffi::MNT_IGNORE != 0;
    if duplicate && !options.include_duplicates {
        return None;
    }
    Some(DiskKind::Unknown)
}

/// The mount table, bounded at `max` entries (checked before the table is
/// allocated). A table that fills the buffer is read again, at most three
/// times, in case mounts appeared between the two calls.
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
            .and_then(|b| core::ffi::c_long::try_from(b).ok())
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

const IO: &str = "disk I/O";
const DEVSTAT_ALL: &str = "kern.devstat.all";
/// The most bytes of device records read: 16 384 devices.
const MAX_DEVSTAT: usize = 8 + ffi::DEVSTAT_SIZE * 16_384;

/// A `struct bintime` (seconds and a 64-bit binary fraction) as a duration.
fn bintime_at(data: &[u8], at: usize) -> Option<Duration> {
    let sec = u64::try_from(super::i64_at(data, at)?).unwrap_or(0);
    let frac = super::u64_at(data, at.checked_add(8)?)?;
    // The fraction's top 32 bits are ample: 2^-32 s is a quarter nanosecond.
    let nanos = ((frac >> 32) * 1_000_000_000) >> 32;
    Some(Duration::new(sec, nanos as u32))
}

/// A disk's name as the kernel keeps it: the driver and the unit (`ada`, 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DiskName<'a> {
    driver: &'a [u8],
    unit: i32,
}

impl DiskName<'_> {
    /// `ada0`. Only the per-disk list builds it; the totals allocate nothing.
    fn text(self) -> String {
        format!(
            "{}{}",
            crate::parse::lossy(self.driver, ffi::DEVSTAT_NAME_LEN),
            self.unit
        )
    }
}

/// One device's name and counters, from its `struct devstat` record; `None`
/// for anything that is not a disk (a CD drive, a tape, a `pass` device) or
/// a memory disk.
fn device_from(record: &[u8]) -> Option<(DiskName<'_>, DiskRaw)> {
    let kind = super::i32_at(record, ffi::DS_DEVICE_TYPE)?;
    if kind & ffi::DEVSTAT_TYPE_MASK != ffi::DEVSTAT_TYPE_DIRECT
        || kind & ffi::DEVSTAT_TYPE_PASS != 0
    {
        return None;
    }
    let name = record.get(ffi::DS_DEVICE_NAME..ffi::DS_DEVICE_NAME + ffi::DEVSTAT_NAME_LEN)?;
    let driver = name.split(|&b| b == 0).next()?;
    if driver.is_empty() || driver == b"md" || !driver.iter().all(u8::is_ascii_alphanumeric) {
        return None;
    }
    let unit = super::i32_at(record, ffi::DS_UNIT_NUMBER)?;
    let counter = |base: usize, which: usize| super::u64_at(record, base + which * 8);
    let time = |which: usize| bintime_at(record, ffi::DS_DURATION + which * ffi::BINTIME_SIZE);
    let read_time = time(ffi::DEVSTAT_READ)?;
    let write_time = time(ffi::DEVSTAT_WRITE)?;
    let raw = DiskRaw {
        read: counter(ffi::DS_BYTES, ffi::DEVSTAT_READ)?,
        written: counter(ffi::DS_BYTES, ffi::DEVSTAT_WRITE)?,
        reads: counter(ffi::DS_OPERATIONS, ffi::DEVSTAT_READ)?,
        writes: counter(ffi::DS_OPERATIONS, ffi::DEVSTAT_WRITE)?,
        busy: Some(bintime_at(record, ffi::DS_BUSY_TIME)?),
        op_time: Some(read_time.saturating_add(write_time)),
        disks: 1,
    };
    Some((DiskName { driver, unit }, raw))
}

/// Calls `f` with each disk of a `kern.devstat.all` answer: a `long`
/// generation number, then one record per device.
fn walk_devstat(data: &[u8], mut f: impl FnMut(DiskName<'_>, DiskRaw)) -> Result<()> {
    let malformed = || Error::Malformed {
        what: IO,
        source: DEVSTAT_ALL,
        line: None,
    };
    let records = data.get(8..).ok_or_else(malformed)?;
    if !records.len().is_multiple_of(ffi::DEVSTAT_SIZE) {
        return Err(malformed());
    }
    let (records, _) = records.as_chunks::<{ ffi::DEVSTAT_SIZE }>();
    for record in records {
        if let Some((name, raw)) = device_from(record) {
            f(name, raw);
        }
    }
    Ok(())
}

/// Calls `f` with every disk's counters, after checking that the records have
/// the layout read here (`kern.devstat.version`). Up to 56 disks fit the
/// stack buffer, so the totals allocate nothing.
fn with_devices(f: impl FnMut(DiskName<'_>, DiskRaw)) -> Result<()> {
    const VERSION: &CStr = c"kern.devstat.version";
    let version = super::sysctl_int(VERSION).map_err(|e| e.into_error(IO, VERSION))?;
    if version != ffi::DEVSTAT_VERSION {
        return Err(super::unsupported("disk I/O (another devstat version)"));
    }
    super::with_sysctl(
        IO,
        DEVSTAT_ALL,
        Key::Name(c"kern.devstat.all"),
        MAX_DEVSTAT,
        |data| walk_devstat(data, f),
    )
}

/// Every disk's statistics, summed.
pub(crate) fn disk_io_totals() -> Result<DiskRaw> {
    let mut total = DiskRaw::default();
    with_devices(|_, raw| total.add(&raw))?;
    Ok(total)
}

/// Each disk's statistics, named as the kernel names the disk (`ada0`).
pub(crate) fn disk_io_counters() -> Result<Vec<(String, DiskRaw)>> {
    let mut out = Vec::new();
    with_devices(|name, raw| out.push((name.text(), raw)))?;
    Ok(out)
}

/// FreeBSD has no I/O stall measure.
pub(crate) fn disk_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disk_freebsd_c_path_bounds_and_nul() {
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

    fn mount(fstype: &[u8], flags: u64) -> ffi::Statfs {
        // SAFETY: a zeroed `statfs` is valid (integers and arrays only).
        let mut st: ffi::Statfs = unsafe { MaybeUninit::zeroed().assume_init() };
        for (dst, &src) in st.fstypename.iter_mut().zip(fstype) {
            *dst = core::ffi::c_char::from_ne_bytes([src]);
        }
        st.flags = flags;
        st
    }

    #[test]
    fn test_disk_freebsd_space_multiplies_blocks() {
        let mut st = mount(b"ufs", ffi::MNT_LOCAL);
        st.bsize = 4096;
        st.blocks = 1000;
        st.bfree = 400;
        st.bavail = 300;
        let s = space_of(&st);
        assert_eq!(
            (s.total.raw(), s.free.raw(), s.available.raw()),
            (4_096_000, 1_638_400, 1_228_800)
        );
        // Root dipped into the reserve.
        st.bavail = -20;
        assert_eq!(space_of(&st).available.raw(), 0);
        st.blocks = u64::MAX;
        assert_eq!(space_of(&st).total.raw(), u64::MAX);
    }

    #[test]
    fn test_disk_freebsd_classify_by_type_and_flags() {
        let default = ListOptions::default();
        let all = ListOptions {
            include_network: true,
            include_pseudo: true,
            include_duplicates: true,
            ..ListOptions::default()
        };
        let root = mount(b"zfs", ffi::MNT_LOCAL);
        assert_eq!(classify(&root, &default), Some(DiskKind::Unknown));
        let dev = mount(b"devfs", ffi::MNT_LOCAL);
        assert_eq!(classify(&dev, &default), None);
        assert_eq!(classify(&dev, &all), Some(DiskKind::Virtual));
        let tmp = mount(b"tmpfs", ffi::MNT_LOCAL);
        assert_eq!(classify(&tmp, &all), Some(DiskKind::Virtual));
        // T8: network filesystems, by flag or by type.
        let nfs = mount(b"nfs", 0);
        assert_eq!(classify(&nfs, &default), None);
        assert_eq!(classify(&nfs, &all), Some(DiskKind::Network));
        let smb = mount(b"smbfs", ffi::MNT_LOCAL);
        assert_eq!(classify(&smb, &all), Some(DiskKind::Network));
        // A second view of a directory, and a mount hidden from df.
        let null = mount(b"nullfs", ffi::MNT_LOCAL);
        assert_eq!(classify(&null, &default), None);
        assert_eq!(classify(&null, &all), Some(DiskKind::Unknown));
        let hidden = mount(b"ufs", ffi::MNT_LOCAL | ffi::MNT_IGNORE);
        assert_eq!(classify(&hidden, &default), None);
    }

    #[test]
    fn test_disk_freebsd_disk_unit_from_device() {
        assert_eq!(disk_unit(b"/dev/ada0p2"), Some((&b"ada"[..], 0)));
        assert_eq!(disk_unit(b"/dev/nda12"), Some((&b"nda"[..], 12)));
        assert_eq!(disk_unit(b"/dev/da3s1a"), Some((&b"da"[..], 3)));
        assert_eq!(disk_unit(b"/dev/vtbd0p3"), Some((&b"vtbd"[..], 0)));
        for device in [
            &b"zroot/ROOT/default"[..],
            b"/dev/gpt/rootfs",
            b"/dev/",
            b"/dev/ada",
            b"/dev/0",
            b"devfs",
            b"server:/export",
            b"/dev/ada0p2/../x",
            b"/dev/ada9999999",
        ] {
            assert_eq!(disk_unit(device), None, "{device:?}");
        }
        assert_eq!(medium(b"/dev/nvd0p2"), Some(DiskKind::Ssd));
        assert_eq!(medium(b"/dev/vtbd0p2"), None);
        assert_eq!(
            rotating_sysctl(b"ada", 1).unwrap().as_c_str(),
            c"dev.ada.1.rotating"
        );
        assert_eq!(rotating_sysctl(b"md", 0), None);
    }

    /// A `struct devstat` record as the kernel lays it out.
    fn record(name: &[u8], unit: i32, kind: i32, read: u64, written: u64) -> Vec<u8> {
        let mut r = vec![0u8; ffi::DEVSTAT_SIZE];
        let put = |r: &mut Vec<u8>, at: usize, b: &[u8]| r[at..at + b.len()].copy_from_slice(b);
        put(&mut r, ffi::DS_DEVICE_NAME, name);
        put(&mut r, ffi::DS_UNIT_NUMBER, &unit.to_ne_bytes());
        put(&mut r, ffi::DS_DEVICE_TYPE, &kind.to_ne_bytes());
        put(&mut r, ffi::DS_BYTES + 8, &read.to_ne_bytes());
        put(&mut r, ffi::DS_BYTES + 16, &written.to_ne_bytes());
        put(&mut r, ffi::DS_OPERATIONS + 8, &7u64.to_ne_bytes());
        put(&mut r, ffi::DS_OPERATIONS + 16, &9u64.to_ne_bytes());
        // Read time 1.5 s, write time 0.25 s, busy 2 s.
        put(&mut r, ffi::DS_DURATION + 16, &1i64.to_ne_bytes());
        put(&mut r, ffi::DS_DURATION + 24, &(1u64 << 63).to_ne_bytes());
        put(&mut r, ffi::DS_DURATION + 40, &(1u64 << 62).to_ne_bytes());
        put(&mut r, ffi::DS_BUSY_TIME, &2i64.to_ne_bytes());
        r
    }

    #[test]
    fn test_disk_freebsd_devstat_records() {
        let mut data = 42i64.to_ne_bytes().to_vec();
        data.extend(record(b"ada", 0, 0x020, 1000, 2000));
        data.extend(record(b"cd", 0, 0x025, 5, 5));
        data.extend(record(b"pass", 1, 0x110, 5, 5));
        data.extend(record(b"md", 0, 0x030, 5, 5));
        data.extend(record(b"nvd", 1, 0x040, 3, 4));
        let mut seen = Vec::new();
        walk_devstat(&data, |name, raw| seen.push((name.text(), raw))).unwrap();
        let names: Vec<&str> = seen.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["ada0", "nvd1"]);
        let ada = seen[0].1;
        assert_eq!(
            (ada.read, ada.written, ada.reads, ada.writes),
            (1000, 2000, 7, 9)
        );
        assert_eq!(ada.op_time, Some(Duration::from_millis(1750)));
        assert_eq!(ada.busy, Some(Duration::from_secs(2)));
        assert_eq!(ada.disks, 1);
    }

    #[test]
    fn test_disk_freebsd_devstat_rejects_bad_lengths() {
        assert!(walk_devstat(&[0; 4], |_, _| {}).is_err());
        let mut data = vec![0u8; 8];
        data.extend(record(b"ada", 0, 0, 1, 1));
        data.pop();
        assert!(matches!(
            walk_devstat(&data, |_, _| {}),
            Err(Error::Malformed { .. })
        ));
        walk_devstat(&[0; 8], |_, _| panic!("no records")).unwrap();
        // A name with no NUL in its 16 bytes, and one with a stray byte.
        assert!(device_from(&record(b"abcdefghijklmnop", 0, 0, 1, 1)).is_some());
        assert_eq!(device_from(&record(b"ad/a", 0, 0, 1, 1)), None);
        assert_eq!(device_from(&[0u8; 10]), None);
    }

    #[test]
    fn test_disk_freebsd_bintime() {
        let mut b = 3i64.to_ne_bytes().to_vec();
        b.extend((1u64 << 63).to_ne_bytes());
        assert_eq!(bintime_at(&b, 0), Some(Duration::from_millis(3500)));
        assert_eq!(bintime_at(&b[..15], 0), None);
        let mut negative = (-1i64).to_ne_bytes().to_vec();
        negative.extend(u64::MAX.to_ne_bytes());
        assert!(bintime_at(&negative, 0).unwrap() < Duration::from_secs(1));
    }

    #[test]
    fn test_disk_freebsd_live_list_and_io() {
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
        assert!(!each.is_empty());
        assert_eq!(disk_pressure().unwrap(), None);
    }

    #[test]
    fn test_disk_freebsd_live_disk_of_and_space() {
        let root = disk_of(Path::new("/")).unwrap();
        assert_eq!(root.mount_point, Path::new("/"));
        assert!(root.space.is_some_and(|s| s.total.raw() > 0));
        let dev = disk_of(Path::new("/dev")).unwrap();
        assert_eq!(dev.kind, DiskKind::Virtual);
        assert!(matches!(
            disk_of(Path::new("/definitely/not/here/sys-lib")),
            Err(Error::NotFound { .. })
        ));
        let s = disk_space(Path::new("/")).unwrap();
        assert!(s.total.raw() > 0 && s.available <= s.free && s.free <= s.total);
    }
}
