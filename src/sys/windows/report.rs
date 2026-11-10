//! Environment and reports (0.6): temperatures, graphics adapters, and the
//! settings the readiness, benchmark, and security reports read.
//!
//! user32, tbs, and the powrprof calls added here are loaded at run time
//! (D-070): a Nano Server or Server Core image may lack them, and a missing
//! import would stop the program before `main`. Each reading then answers
//! unknown.

use std::{
    ffi::c_void,
    mem::{size_of, transmute},
    path::Path,
    ptr::{null, null_mut},
    sync::OnceLock,
};

use super::{
    Key, Owned,
    cpu::power_info,
    detect::firmware_kind,
    disk::{MAX_DRIVES, MISSES, drive_path},
    export, ffi, from_wide, last_error, os_error, system_library,
    topology::{Records, smt_of},
    unsupported, wide,
};
use crate::{
    Bytes, Result,
    bench::BenchFacts,
    disk::Disk,
    gpu::{Gpu, GpuKind, GpuVendor},
    host::FirmwareKind,
    readiness::{DataDirTuning, Fact, Governor, MemLock, Tuning},
    security::{Lockdown, Posture, Tpm, TpmVersion},
    sensors::{SensorKind, Temperature},
};

// ---------------------------------------------------------------------------
// Temperatures
// ---------------------------------------------------------------------------

/// An IOCTL answer, aligned for the structs in it. 512 bytes hold the
/// temperature descriptor with 30 sensors (NVMe has at most 9) and a device
/// descriptor with its strings.
#[repr(C, align(8))]
struct Answer([u8; 512]);

/// `IOCTL_STORAGE_QUERY_PROPERTY` for `property`: the bytes the driver wrote.
fn storage_property(drive: &Owned, property: i32, answer: &mut Answer) -> Option<usize> {
    let query = ffi::StoragePropertyQuery {
        property_id: property,
        query_type: ffi::PROPERTY_STANDARD_QUERY,
        additional_parameters: [0],
    };
    let mut returned = 0u32;
    // SAFETY: `drive` is open; `query` and `answer` are buffers of the sizes
    // passed; the call is synchronous (no OVERLAPPED).
    let ok = unsafe {
        ffi::DeviceIoControl(
            drive.0,
            ffi::IOCTL_STORAGE_QUERY_PROPERTY,
            (&raw const query).cast::<c_void>(),
            size_of::<ffi::StoragePropertyQuery>() as u32,
            answer.0.as_mut_ptr().cast::<c_void>(),
            answer.0.len() as u32,
            &raw mut returned,
            null_mut(),
        )
    };
    (ok != 0).then_some((returned as usize).min(answer.0.len()))
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_ne_bytes([s[0], s[1]]))
}

fn i16_at(b: &[u8], at: usize) -> Option<i16> {
    u16_at(b, at).map(|v| v as i16)
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_ne_bytes([s[0], s[1], s[2], s[3]]))
}

/// A reported, plausible reading in degrees Celsius.
fn celsius(raw: i16) -> Option<f32> {
    let c = f32::from(raw);
    (raw != ffi::STORAGE_TEMPERATURE_VALUE_NOT_REPORTED && crate::sensors::plausible(c))
        .then_some(c)
}

/// A threshold: reported, plausible, and above freezing (drives that do not
/// set one report 0).
fn threshold(raw: i16) -> Option<f32> {
    celsius(raw).filter(|&c| c > 0.0)
}

/// The sensors of one `STORAGE_TEMPERATURE_DATA_DESCRIPTOR`. Sensor 0 is the
/// drive's own reading (the NVMe composite temperature) and takes `name`;
/// the others are named by their index. The walk stops at the first entry
/// the answer does not hold.
fn drive_temperatures(bytes: &[u8], name: &str, out: &mut Vec<Temperature>) {
    use ffi::temperature as t;
    let high = i16_at(bytes, t::WARNING).and_then(threshold);
    let critical = i16_at(bytes, t::CRITICAL).and_then(threshold);
    let count = usize::from(u16_at(bytes, t::INFO_COUNT).unwrap_or(0));
    for i in 0..count {
        let at = t::INFO + i * t::INFO_ENTRY;
        let (Some(index), Some(raw)) = (
            u16_at(bytes, at + t::INFO_INDEX),
            i16_at(bytes, at + t::INFO_TEMPERATURE),
        ) else {
            return;
        };
        let Some(reading) = celsius(raw) else {
            continue;
        };
        out.push(Temperature {
            label: if index == 0 {
                name.to_owned()
            } else {
                format!("{name} sensor {index}")
            },
            kind: SensorKind::Storage,
            celsius: reading,
            high,
            critical,
        });
    }
}

/// The product id of a `STORAGE_DEVICE_DESCRIPTOR` answer ("Samsung SSD 990
/// PRO 2TB"): printable ASCII, whitespace collapsed, from at most 64 bytes.
/// The serial number in the same answer is never read.
fn product_of(bytes: &[u8]) -> Option<String> {
    let at = u32_at(bytes, ffi::sdd::PRODUCT_ID_OFFSET)? as usize;
    // An offset into the fixed part is not a string.
    if at < size_of::<ffi::StorageDeviceDescriptor>() - 1 {
        return None;
    }
    let rest = bytes.get(at..)?;
    let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
    let text: String = rest[..end.min(64)]
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { ' ' })
        .collect();
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    (!text.is_empty()).then_some(text)
}

/// The name of physical drive `n`: its model and number,
/// "Samsung SSD 990 PRO 2TB (PhysicalDrive0)", so identical drives differ.
fn drive_name(n: u32, model: Option<&str>) -> String {
    match model {
        Some(m) => format!("{m} (PhysicalDrive{n})"),
        None => format!("PhysicalDrive{n}"),
    }
}

/// Opens physical drive `n` with no access rights: the property query is
/// `FILE_ANY_ACCESS`, so this works unprivileged and disturbs no other user
/// of the disk. `Err` carries the Win32 error.
fn open_drive(n: u32) -> core::result::Result<Owned, u32> {
    let mut path = [0u16; 32];
    drive_path(n, &mut path);
    // SAFETY: `path` is NUL-terminated; no security attributes or template.
    let handle = unsafe {
        ffi::CreateFileW(
            path.as_ptr(),
            0,
            ffi::FILE_SHARE_READ | ffi::FILE_SHARE_WRITE,
            null_mut(),
            ffi::OPEN_EXISTING,
            0,
            null_mut(),
        )
    };
    // Only an opened handle gets an owner, which closes it.
    if handle == ffi::INVALID_HANDLE_VALUE {
        Err(last_error() as u32)
    } else {
        Ok(Owned(handle))
    }
}

/// The drives that report a temperature (`StorageDeviceTemperatureProperty`:
/// NVMe, and some SATA drives). The CPU has no unprivileged documented
/// source on Windows, so none is listed.
pub(crate) fn temperatures() -> Result<Vec<Temperature>> {
    let mut out = Vec::new();
    let mut misses = 0;
    for n in 0..MAX_DRIVES {
        let drive = match open_drive(n) {
            Ok(drive) => drive,
            Err(ffi::ERROR_ACCESS_DENIED) => {
                misses = 0;
                continue;
            }
            Err(_) => {
                misses += 1;
                if misses == MISSES {
                    break;
                }
                continue;
            }
        };
        misses = 0;
        let mut reading = Answer([0; 512]);
        let Some(len) = storage_property(
            &drive,
            ffi::STORAGE_DEVICE_TEMPERATURE_PROPERTY,
            &mut reading,
        ) else {
            continue;
        };
        let mut device = Answer([0; 512]);
        let model = storage_property(&drive, ffi::STORAGE_DEVICE_PROPERTY, &mut device)
            .and_then(|len| product_of(&device.0[..len]));
        drive_temperatures(
            &reading.0[..len],
            &drive_name(n, model.as_deref()),
            &mut out,
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Graphics adapters
// ---------------------------------------------------------------------------

/// The most display devices enumerated (threat T7).
const MAX_DISPLAY_DEVICES: u32 = 256;

fn enum_display_devices() -> Option<ffi::EnumDisplayDevicesW> {
    static USER32: OnceLock<Option<ffi::EnumDisplayDevicesW>> = OnceLock::new();
    *USER32.get_or_init(|| {
        let address = export(system_library("user32.dll")?, c"EnumDisplayDevicesW")?;
        // SAFETY: the address is the export of that name, whose signature is
        // the type it becomes (winuser.h), and the module is never unloaded.
        Some(unsafe { transmute::<*mut c_void, ffi::EnumDisplayDevicesW>(address) })
    })
}

/// The PCI vendor and device ids of a device instance id,
/// `PCI\VEN_10DE&DEV_2786&SUBSYS_...`; `None` for other buses.
fn pci_ids(device_id: &str) -> (Option<u16>, Option<u16>) {
    let upper = device_id.to_ascii_uppercase();
    let Some(rest) = upper.strip_prefix(r"PCI\") else {
        return (None, None);
    };
    let field = |name: &str| {
        rest.split('&')
            .find_map(|part| part.strip_prefix(name))
            .filter(|hex| hex.len() == 4)
            .and_then(|hex| u16::from_str_radix(hex, 16).ok())
    };
    (field("VEN_"), field("DEV_"))
}

/// PCI vendors whose display devices are always emulated: QEMU and Bochs,
/// virtio, Red Hat (QXL), VirtualBox, Parallels, Xen.
const VIRTUAL_PCI_VENDORS: [u16; 6] = [0x1234, 0x1af4, 0x1b36, 0x80ee, 0x1ab8, 0x5853];

/// The adapter part of a display device's registry key: the outputs of one
/// adapter are `...\Video\{GUID}\0000`, `\0001`, and so on.
fn adapter_of(device_key: &str) -> &str {
    device_key
        .rsplit_once('\\')
        .map_or(device_key, |(adapter, _)| adapter)
}

/// What the registry says about an adapter.
#[derive(Default)]
struct AdapterKey {
    memory: Option<u64>,
    driver: Option<String>,
    /// Run by the basic display driver: no acceleration, the firmware's
    /// frame buffer.
    basic: bool,
}

/// The adapter's registry key (`DeviceKey`): memory, driver version, and the
/// driver. `HardwareInformation.MemorySize` is 32 bits and stops below 4 GiB,
/// so the 64-bit value comes first.
fn adapter_key(device_key: &str) -> AdapterKey {
    let Some(key) = Key::kernel_path(device_key) else {
        return AdapterKey::default();
    };
    let memory = key
        .qword("HardwareInformation.qwMemorySize")
        .or_else(|| {
            key.dword_or_binary("HardwareInformation.MemorySize")
                .map(u64::from)
        })
        .filter(|&m| m > 0);
    let service = Key::kernel_path(&format!(r"{}\Video", adapter_of(device_key)))
        .and_then(|k| k.string("Service"));
    AdapterKey {
        memory,
        driver: key.string("DriverVersion"),
        basic: is_basic_display(
            service.as_deref(),
            key.string("MatchingDeviceId").as_deref(),
        ),
    }
}

/// The basic display driver runs the adapter: its service, or a match on the
/// PCI display class (`PCI\CC_0300`) rather than on the vendor and device.
/// Both are locale-independent, unlike the adapter's name.
fn is_basic_display(service: Option<&str>, matching_id: Option<&str>) -> bool {
    service.is_some_and(|s| s.eq_ignore_ascii_case("BasicDisplay"))
        || matching_id.is_some_and(|m| {
            m.get(..7)
                .is_some_and(|head| head.eq_ignore_ascii_case(r"pci\cc_"))
        })
}

/// The vendor and kind of an adapter. The basic display adapter is
/// Microsoft's and virtual (as `GpuVendor::Microsoft` documents); so are
/// Hyper-V's (`VMBUS\`); software display devices (`ROOT\`, `SWD\`) are
/// virtual.
fn classify(device_id: &str, vendor_id: Option<u16>, basic: bool) -> (GpuVendor, GpuKind) {
    if basic {
        return (GpuVendor::Microsoft, GpuKind::Virtual);
    }
    if let Some(id) = vendor_id {
        let vendor = GpuVendor::from_pci(id);
        if VIRTUAL_PCI_VENDORS.contains(&id) {
            return (vendor, GpuKind::Virtual);
        }
        return (vendor, crate::gpu::kind_of(vendor));
    }
    let bus = device_id
        .split_once('\\')
        .map_or("", |(bus, _)| bus)
        .to_ascii_uppercase();
    match bus.as_str() {
        "VMBUS" => (GpuVendor::Microsoft, GpuKind::Virtual),
        "ROOT" | "SWD" => (GpuVendor::Other, GpuKind::Virtual),
        _ => (GpuVendor::Other, GpuKind::Unknown),
    }
}

/// The adapters `EnumDisplayDevicesW` lists, once each: mirror drivers (remote
/// desktop, screen capture) are left out, and an adapter's outputs are folded
/// into one entry.
pub(crate) fn gpus() -> Result<Vec<Gpu>> {
    let enumerate = enum_display_devices().ok_or_else(|| unsupported("graphics adapters"))?;
    let mut out = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for index in 0..MAX_DISPLAY_DEVICES {
        let mut device = ffi::DisplayDeviceW {
            cb: size_of::<ffi::DisplayDeviceW>() as u32,
            device_name: [0; 32],
            device_string: [0; 128],
            state_flags: 0,
            device_id: [0; 128],
            device_key: [0; 128],
        };
        // SAFETY: a null device lists the adapters; `device` is a writable
        // DISPLAY_DEVICEW with its size set.
        let ok = unsafe { enumerate(null(), index, &raw mut device, 0) };
        if ok == 0 {
            break;
        }
        if device.state_flags & ffi::DISPLAY_DEVICE_MIRRORING_DRIVER != 0 {
            continue;
        }
        let name = from_wide(&device.device_string).trim().to_owned();
        if name.is_empty() {
            continue;
        }
        let id = from_wide(&device.device_id);
        let key = from_wide(&device.device_key);
        let identity = if key.is_empty() {
            format!("{id}|{name}")
        } else {
            adapter_of(&key).to_ascii_lowercase()
        };
        if seen.contains(&identity) {
            continue;
        }
        seen.push(identity);
        let registry = adapter_key(&key);
        let (vendor_id, device_id) = pci_ids(&id);
        let (vendor, kind) = classify(&id, vendor_id, registry.basic);
        out.push(Gpu {
            name,
            vendor,
            kind,
            vendor_id,
            device_id,
            memory: registry.memory.map(Bytes::new),
            driver: registry.driver,
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Power plans
// ---------------------------------------------------------------------------

/// The powrprof calls this module makes, resolved at run time.
struct PowerApi {
    active_scheme: ffi::PowerGetActiveScheme,
    friendly_name: ffi::PowerReadFriendlyName,
    ac_value: ffi::PowerReadValueIndex,
    dc_value: ffi::PowerReadValueIndex,
}

fn power_api() -> Option<&'static PowerApi> {
    static API: OnceLock<Option<PowerApi>> = OnceLock::new();
    API.get_or_init(|| {
        let module = system_library("powrprof.dll")?;
        let active_scheme = export(module, c"PowerGetActiveScheme")?;
        let friendly_name = export(module, c"PowerReadFriendlyName")?;
        let ac_value = export(module, c"PowerReadACValueIndex")?;
        let dc_value = export(module, c"PowerReadDCValueIndex")?;
        // SAFETY: each address is the export of that name, whose signature is
        // the type it becomes (powersetting.h, powrprof.h), and the module is
        // never unloaded.
        let active_scheme =
            unsafe { transmute::<*mut c_void, ffi::PowerGetActiveScheme>(active_scheme) };
        // SAFETY: as above.
        let friendly_name =
            unsafe { transmute::<*mut c_void, ffi::PowerReadFriendlyName>(friendly_name) };
        // SAFETY: as above.
        let ac_value = unsafe { transmute::<*mut c_void, ffi::PowerReadValueIndex>(ac_value) };
        // SAFETY: as above.
        let dc_value = unsafe { transmute::<*mut c_void, ffi::PowerReadValueIndex>(dc_value) };
        Some(PowerApi {
            active_scheme,
            friendly_name,
            ac_value,
            dc_value,
        })
    })
    .as_ref()
}

/// Memory from `LocalAlloc`, freed on drop.
struct LocalMemory(*mut c_void);

impl Drop for LocalMemory {
    fn drop(&mut self) {
        // SAFETY: the OS allocated the memory with LocalAlloc; it is freed once.
        unsafe { ffi::LocalFree(self.0) };
    }
}

/// The `PERSONALITY` value of a plan based on High performance
/// (`powercfg /qh`: 0 Power saver, 1 High performance, 2 Balanced).
const PERSONALITY_HIGH_PERFORMANCE: u32 = 1;

/// The active power plan.
struct Plan {
    guid: ffi::Guid,
    name: Option<String>,
    personality: Option<u32>,
    /// `PERFBOOSTMODE` for the current power source: 0 is disabled.
    boost_mode: Option<u32>,
    /// `PROCTHROTTLEMAX` for the current power source, in percent.
    max_state: Option<u32>,
}

impl Plan {
    /// Every CPU may run at its highest frequency policy.
    fn performance(&self) -> bool {
        is_performance(self.guid, self.personality)
    }

    /// The plan's name, else a built-in plan's English name, else its GUID.
    fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| plan_name(self.guid))
    }

    /// The plan lets the processor boost: a boost mode other than disabled,
    /// and no cap below 100% (which keeps the processor out of boost).
    fn turbo(&self) -> Option<bool> {
        let boost = self.boost_mode?;
        Some(boost != 0 && self.max_state.is_none_or(|m| m >= 100))
    }
}

/// High performance and Ultimate Performance, by GUID (names are localized),
/// or a plan based on High performance: `powercfg -duplicatescheme` copies a
/// plan under a new GUID with the same personality.
fn is_performance(guid: ffi::Guid, personality: Option<u32>) -> bool {
    guid == ffi::GUID_MIN_POWER_SAVINGS
        || guid == ffi::GUID_ULTIMATE_PERFORMANCE
        || personality == Some(PERSONALITY_HIGH_PERFORMANCE)
}

/// The name of a built-in plan, or the GUID in registry form.
fn plan_name(guid: ffi::Guid) -> String {
    match guid {
        g if g == ffi::GUID_MIN_POWER_SAVINGS => "High performance".to_owned(),
        g if g == ffi::GUID_ULTIMATE_PERFORMANCE => "Ultimate Performance".to_owned(),
        g if g == ffi::GUID_TYPICAL_POWER_SAVINGS => "Balanced".to_owned(),
        g if g == ffi::GUID_MAX_POWER_SAVINGS => "Power saver".to_owned(),
        g => {
            let d = g.data4;
            format!(
                "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
                g.data1, g.data2, g.data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
            )
        }
    }
}

/// The active plan and the settings read from it, for the current power
/// source.
fn active_plan() -> Result<Plan> {
    let api = power_api().ok_or_else(|| unsupported("power plan"))?;
    let mut scheme: *mut ffi::Guid = null_mut();
    // SAFETY: a null root key means the system's; `scheme` is a writable
    // pointer slot the call fills with memory it allocates.
    let rc = unsafe { (api.active_scheme)(null_mut(), &raw mut scheme) };
    if rc != ffi::ERROR_SUCCESS || scheme.is_null() {
        return Err(crate::Error::Os {
            what: "power plan",
            call: "PowerGetActiveScheme",
            code: rc as i32,
        });
    }
    let memory = LocalMemory(scheme.cast::<c_void>());
    // SAFETY: the call wrote a GUID there, alive until `memory` drops; it is
    // read unaligned, which needs no alignment promise.
    let guid = unsafe { memory.0.cast::<ffi::Guid>().read_unaligned() };
    drop(memory);
    let read = if super::on_ac().ok().flatten() == Some(false) {
        api.dc_value
    } else {
        api.ac_value
    };
    let value = |subgroup: &ffi::Guid, setting: &ffi::Guid| {
        let mut value = 0u32;
        // SAFETY: a null root key means the system's; the GUIDs are valid
        // for the call; `value` is writable.
        let rc = unsafe {
            read(
                null_mut(),
                &raw const guid,
                subgroup,
                setting,
                &raw mut value,
            )
        };
        (rc == ffi::ERROR_SUCCESS).then_some(value)
    };
    Ok(Plan {
        guid,
        name: friendly_name(api, &guid),
        personality: value(&ffi::NO_SUBGROUP_GUID, &ffi::GUID_POWERSCHEME_PERSONALITY),
        boost_mode: value(
            &ffi::GUID_PROCESSOR_SETTINGS_SUBGROUP,
            &ffi::GUID_PROCESSOR_PERF_BOOST_MODE,
        ),
        max_state: value(
            &ffi::GUID_PROCESSOR_SETTINGS_SUBGROUP,
            &ffi::GUID_PROCESSOR_THROTTLE_MAXIMUM,
        ),
    })
}

/// The plan's name in the user's language: "Balanced", "Ausbalanciert".
fn friendly_name(api: &PowerApi, guid: &ffi::Guid) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: a null root key means the system's; the scheme GUID is valid;
    // null subgroup and setting ask for the scheme's own name; `buf` is
    // `size` writable bytes.
    let rc = unsafe {
        (api.friendly_name)(
            null_mut(),
            guid,
            null(),
            null(),
            buf.as_mut_ptr().cast::<u8>(),
            &raw mut size,
        )
    };
    if rc != ffi::ERROR_SUCCESS {
        return None;
    }
    let units = (size as usize / 2).min(buf.len());
    let name = from_wide(&buf[..units]).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

// ---------------------------------------------------------------------------
// Readiness
// ---------------------------------------------------------------------------

/// This process's access token, opened for queries.
fn process_token(what: &'static str) -> Result<Owned> {
    let mut token: ffi::Handle = null_mut();
    // SAFETY: the pseudo-handle of this process is always valid; `token` is
    // a writable handle slot.
    let ok = unsafe {
        ffi::OpenProcessToken(ffi::GetCurrentProcess(), ffi::TOKEN_QUERY, &raw mut token)
    };
    // Only an opened token gets an owner, which closes it.
    if ok == 0 {
        Err(os_error(what, "OpenProcessToken"))
    } else {
        Ok(Owned(token))
    }
}

/// The largest privilege list read (threat T7); a token holds about 40.
const MAX_PRIVILEGES: usize = 64 << 10;

/// Whether a `TOKEN_PRIVILEGES` answer holds `luid`, enabled or not (a
/// process enables a privilege it holds before using it). A removed one does
/// not count. The walk stops at the first entry the answer does not hold.
fn holds_privilege(bytes: &[u8], luid: ffi::Luid) -> bool {
    use ffi::tp;
    let count = u32_at(bytes, tp::COUNT).unwrap_or(0) as usize;
    (0..count)
        .map_while(|i| {
            let at = i.checked_mul(tp::ENTRY)?.checked_add(tp::PRIVILEGES)?;
            Some((
                u32_at(bytes, at + tp::LUID_LOW)?,
                u32_at(bytes, at + tp::LUID_HIGH)? as i32,
                u32_at(bytes, at + tp::ATTRIBUTES)?,
            ))
        })
        .any(|(low, high, attributes)| {
            low == luid.low_part
                && high == luid.high_part
                && attributes & ffi::SE_PRIVILEGE_REMOVED == 0
        })
}

/// The token holds `SeLockMemoryPrivilege` ("Lock pages in memory").
fn lock_memory_privilege() -> Result<bool> {
    const WHAT: &str = "lock-pages privilege";
    let name = wide("SeLockMemoryPrivilege");
    let mut luid = ffi::Luid::default();
    // SAFETY: a null system name means this machine; `name` is NUL-terminated;
    // `luid` is writable.
    let ok = unsafe { ffi::LookupPrivilegeValueW(null(), name.as_ptr(), &raw mut luid) };
    if ok == 0 {
        return Err(os_error(WHAT, "LookupPrivilegeValueW"));
    }
    let token = process_token(WHAT)?;
    let mut buf: Vec<u32> = vec![0; 256];
    for _ in 0..3 {
        let mut needed = 0u32;
        let bytes = u32::try_from(buf.len() * 4).unwrap_or(u32::MAX);
        // SAFETY: `token` is open for query; `buf` is `bytes` writable bytes,
        // aligned for TOKEN_PRIVILEGES.
        let ok = unsafe {
            ffi::GetTokenInformation(
                token.0,
                ffi::TOKEN_PRIVILEGES_CLASS,
                buf.as_mut_ptr().cast::<c_void>(),
                bytes,
                &raw mut needed,
            )
        };
        if ok != 0 {
            let len = (needed as usize).min(buf.len() * 4);
            let view: Vec<u8> = buf.iter().flat_map(|w| w.to_ne_bytes()).collect();
            return Ok(holds_privilege(&view[..len], luid));
        }
        let needed = needed as usize;
        if last_error() as u32 != ffi::ERROR_INSUFFICIENT_BUFFER
            || needed == 0
            || needed > MAX_PRIVILEGES
        {
            break;
        }
        buf = vec![0; needed.div_ceil(4)];
    }
    Err(os_error(WHAT, "GetTokenInformation"))
}

/// Why the ephemeral port range is unknown (D-073).
const PORTS_UNKNOWN: &str =
    "no documented call reads the dynamic port range; netsh int ipv4 show dynamicport tcp shows it";

pub(crate) fn tuning() -> Tuning {
    let memlock = match lock_memory_privilege() {
        Ok(held) => Fact::Value(MemLock::Privilege(held)),
        Err(e) => Fact::Unknown(e.to_string()),
    };
    let governor = Fact::of(active_plan().map(|plan| Governor {
        name: plan.display_name(),
        performance: plan.performance(),
    }));
    Tuning {
        handles_unlimited: true,
        memlock,
        ports: Fact::Unknown(PORTS_UNKNOWN.to_owned()),
        governor,
        ..Tuning::default()
    }
}

/// How NTFS keeps access times, from `NtfsDisableLastAccessUpdate`: 0 and 1
/// are set by the administrator, `0x8000000x` by the system (Windows 10 1803
/// and later decide by volume size); the lowest bit says updates are off.
/// `fsutil behavior query disablelastaccess` reads 0x80000002 as 2, "System
/// Managed, Last Access Time Updates ENABLED".
fn atime_of(value: u32) -> Option<&'static str> {
    if value & !0x8000_0003 != 0 {
        return None;
    }
    Some(if value & 1 != 0 {
        "noatime"
    } else {
        "strictatime"
    })
}

fn ntfs_atime() -> Fact<String> {
    let Some(key) = Key::local_machine(r"SYSTEM\CurrentControlSet\Control\FileSystem") else {
        return Fact::Unknown("the FileSystem key cannot be read".to_owned());
    };
    match key.dword("NtfsDisableLastAccessUpdate") {
        None => Fact::Unknown("NtfsDisableLastAccessUpdate is not set".to_owned()),
        Some(v) => match atime_of(v) {
            Some(a) => Fact::Value(a.to_owned()),
            None => Fact::Unknown(format!("NtfsDisableLastAccessUpdate is {v:#x}")),
        },
    }
}

/// The access-time policy is system-wide and applies to NTFS only; Windows
/// has no I/O scheduler to choose.
pub(crate) fn data_dir_tuning(_path: &Path, disk: &Disk) -> DataDirTuning {
    DataDirTuning {
        atime: if disk.filesystem.eq_ignore_ascii_case("NTFS") {
            ntfs_atime()
        } else {
            Fact::Skip
        },
        scheduler: Fact::Skip,
    }
}

// ---------------------------------------------------------------------------
// Benchmark environment
// ---------------------------------------------------------------------------

/// A processor is held below its maximum now: `MhzLimit < MaxMhz` on any
/// entry the OS filled. `None` when none is filled.
fn throttled_of(cpus: &[ffi::ProcessorPowerInformation]) -> Option<bool> {
    let mut filled = cpus.iter().filter(|c| c.max_mhz > 0).peekable();
    filled.peek()?;
    Some(filled.any(|c| c.mhz_limit < c.max_mhz))
}

/// ASLR is on for this process: bottom-up randomization, or forced relocation
/// of images.
fn aslr_on(flags: u32) -> bool {
    flags & (ffi::ASLR_ENABLE_BOTTOM_UP_RANDOMIZATION | ffi::ASLR_ENABLE_FORCE_RELOCATE_IMAGES) != 0
}

/// `GetProcessMitigationPolicy(ProcessASLRPolicy)` for this process.
fn aslr() -> Option<bool> {
    let mut policy = ffi::ProcessMitigationAslrPolicy::default();
    // SAFETY: the pseudo-handle of this process is always valid; `policy` is
    // the PROCESS_MITIGATION_ASLR_POLICY buffer of the size passed.
    let ok = unsafe {
        ffi::GetProcessMitigationPolicy(
            ffi::GetCurrentProcess(),
            ffi::PROCESS_ASLR_POLICY,
            (&raw mut policy).cast::<c_void>(),
            size_of::<ffi::ProcessMitigationAslrPolicy>(),
        )
    };
    (ok != 0).then(|| aslr_on(policy.flags))
}

fn smt() -> Option<bool> {
    smt_of(Records::read(ffi::lpi::RELATION_PROCESSOR_CORE)?.bytes())
}

pub(crate) fn bench_facts() -> BenchFacts {
    let plan = active_plan().ok();
    BenchFacts {
        governor: plan.as_ref().map(Plan::display_name),
        power_saving: plan.as_ref().map(|p| !p.performance()),
        turbo: plan.as_ref().and_then(Plan::turbo),
        smt: smt(),
        clock_source: None,
        throttled: power_info(throttled_of).ok().flatten(),
        isolated_cpus: None,
        aslr: aslr(),
    }
}

// ---------------------------------------------------------------------------
// Security posture
// ---------------------------------------------------------------------------

fn tbs_device_info() -> Option<ffi::TbsiGetDeviceInfo> {
    static TBS: OnceLock<Option<ffi::TbsiGetDeviceInfo>> = OnceLock::new();
    *TBS.get_or_init(|| {
        let address = export(system_library("tbs.dll")?, c"Tbsi_GetDeviceInfo")?;
        // SAFETY: the address is the export of that name, whose signature is
        // the type it becomes (tbs.h), and the module is never unloaded.
        Some(unsafe { transmute::<*mut c_void, ffi::TbsiGetDeviceInfo>(address) })
    })
}

fn tpm_of(version: u32) -> Option<Tpm> {
    let version = match version {
        ffi::TPM_VERSION_20 => TpmVersion::V2_0,
        ffi::TPM_VERSION_12 => TpmVersion::V1_2,
        _ => return None,
    };
    Some(Tpm { version })
}

/// `Tbsi_GetDeviceInfo`: unprivileged, and it reads no identifier. No TPM
/// (`TBS_E_TPM_NOT_FOUND`) and no answer are both `None`.
fn tpm() -> Option<Tpm> {
    let device_info = tbs_device_info()?;
    let mut info = ffi::TpmDeviceInfo::default();
    // SAFETY: `info` is a writable TPM_DEVICE_INFO of the size passed.
    let rc = unsafe {
        device_info(
            size_of::<ffi::TpmDeviceInfo>() as u32,
            (&raw mut info).cast::<c_void>(),
        )
    };
    if rc != ffi::TBS_SUCCESS {
        return None;
    }
    tpm_of(info.tpm_version)
}

/// `UEFISecureBootEnabled`; a BIOS boot has no Secure Boot.
fn secure_boot() -> Option<bool> {
    let enabled = Key::local_machine(r"SYSTEM\CurrentControlSet\Control\SecureBoot\State")
        .and_then(|k| k.dword("UEFISecureBootEnabled"));
    match enabled {
        Some(v) => Some(v != 0),
        None if firmware_kind() == Some(FirmwareKind::Bios) => Some(false),
        None => None,
    }
}

const DEVICE_GUARD: &str = r"SYSTEM\CurrentControlSet\Control\DeviceGuard";

/// Virtualization-based security and HVCI from the DeviceGuard key. Whether
/// the secure kernel runs comes from `Scenarios\KeyGuard\Status`
/// `IsSecureKernelRunning` (written at boot) when present, else from the
/// configuration (`EnableVirtualizationBasedSecurity`), which can say only
/// that it is off. HVCI (`Scenarios\HypervisorEnforcedCodeIntegrity`
/// `Enabled`) runs only under VBS.
fn vbs_state(
    configured: Option<u32>,
    running: Option<u32>,
    hvci: Option<u32>,
) -> (Option<Lockdown>, Option<bool>) {
    let vbs = match (running, configured) {
        (Some(r), _) => Some(r != 0),
        (None, None | Some(0)) => Some(false),
        // Configured on, and the system does not say whether it started.
        (None, Some(_)) => None,
    };
    let lockdown = vbs.map(|on| {
        if on {
            Lockdown::Integrity
        } else {
            Lockdown::Off
        }
    });
    let code_integrity = match (hvci, vbs) {
        (None | Some(0), _) | (_, Some(false)) => Some(false),
        (Some(_), Some(true)) => Some(true),
        (Some(_), None) => None,
    };
    (lockdown, code_integrity)
}

fn device_guard() -> (Option<Lockdown>, Option<bool>) {
    let Some(key) = Key::local_machine(DEVICE_GUARD) else {
        return (None, None);
    };
    let running = Key::local_machine(&format!(r"{DEVICE_GUARD}\Scenarios\KeyGuard\Status"))
        .and_then(|k| k.dword("IsSecureKernelRunning"));
    let hvci = Key::local_machine(&format!(
        r"{DEVICE_GUARD}\Scenarios\HypervisorEnforcedCodeIntegrity"
    ))
    .and_then(|k| k.dword("Enabled"));
    vbs_state(
        key.dword("EnableVirtualizationBasedSecurity"),
        running,
        hvci,
    )
}

/// On when every profile is on, off when every one is off; `None` when they
/// differ or one cannot be read: which profile is active is known only
/// through COM (`INetFwPolicy2`), which sys-lib does not call.
fn firewall_of(profiles: [Option<u32>; 3]) -> Option<bool> {
    let mut on = 0;
    for p in profiles {
        on += usize::from(p? != 0);
    }
    match on {
        0 => Some(false),
        3 => Some(true),
        _ => None,
    }
}

/// Each profile's `EnableFirewall`: Group Policy's value when it sets one,
/// else the local one.
fn firewall() -> Option<bool> {
    const POLICY: &str = r"SOFTWARE\Policies\Microsoft\WindowsFirewall";
    const LOCAL: &str = r"SYSTEM\CurrentControlSet\Services\SharedAccess\Parameters\FirewallPolicy";
    let read = |policy: &str, local: &str| {
        Key::local_machine(&format!(r"{POLICY}\{policy}"))
            .and_then(|k| k.dword("EnableFirewall"))
            .or_else(|| {
                Key::local_machine(&format!(r"{LOCAL}\{local}"))
                    .and_then(|k| k.dword("EnableFirewall"))
            })
    };
    firewall_of([
        read("DomainProfile", "DomainProfile"),
        read("PrivateProfile", "StandardProfile"),
        read("PublicProfile", "PublicProfile"),
    ])
}

pub(crate) fn posture() -> Posture {
    let (kernel_lockdown, code_integrity) = device_guard();
    Posture {
        secure_boot: secure_boot(),
        tpm: tpm(),
        // No unprivileged source outside COM and WMI (D-077).
        disk_encryption: None,
        aslr: aslr(),
        kernel_lockdown,
        code_integrity,
        firewall: firewall(),
        ..Posture::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_report_windows_pci_ids_from_device_ids() {
        assert_eq!(
            pci_ids(r"PCI\VEN_10DE&DEV_2786&SUBSYS_88A01043&REV_A1"),
            (Some(0x10de), Some(0x2786))
        );
        assert_eq!(
            pci_ids(r"pci\ven_8086&dev_a780&subsys_7d251462&rev_04"),
            (Some(0x8086), Some(0xa780))
        );
        assert_eq!(
            pci_ids(r"PCI\VEN_1414&DEV_008E"),
            (Some(0x1414), Some(0x008e))
        );
        assert_eq!(
            pci_ids(r"PCI\DEV_2786&VEN_10DE"),
            (Some(0x10de), Some(0x2786))
        );
        assert_eq!(pci_ids(r"PCI\VEN_10D&DEV_27860"), (None, None));
        assert_eq!(pci_ids(r"PCI\VEN_ZZZZ&DEV_2786"), (None, Some(0x2786)));
        assert_eq!(
            pci_ids(r"VMBUS\{DA0A7802-E377-4AAC-8E77-0558EB1073F8}"),
            (None, None)
        );
        assert_eq!(pci_ids(r"ROOT\BasicDisplay\0000"), (None, None));
        assert_eq!(pci_ids(""), (None, None));
    }

    #[test]
    fn test_report_windows_adapters_classified() {
        let nvidia = classify(r"PCI\VEN_10DE&DEV_2B85", Some(0x10de), false);
        assert_eq!(nvidia, (GpuVendor::Nvidia, GpuKind::Discrete));
        let intel = classify(r"PCI\VEN_8086&DEV_A780", Some(0x8086), false);
        assert_eq!(intel, (GpuVendor::Intel, GpuKind::Integrated));
        let amd = classify(r"PCI\VEN_1002&DEV_164E", Some(0x1002), false);
        assert_eq!(amd, (GpuVendor::Amd, GpuKind::Unknown));
        // The basic display driver on real hardware.
        let basic = classify(r"PCI\VEN_10DE&DEV_2B85", Some(0x10de), true);
        assert_eq!(basic, (GpuVendor::Microsoft, GpuKind::Virtual));
        let qemu = classify(r"PCI\VEN_1234&DEV_1111", Some(0x1234), false);
        assert_eq!(qemu, (GpuVendor::Other, GpuKind::Virtual));
        let vmware = classify(r"PCI\VEN_15AD&DEV_0405", Some(0x15ad), false);
        assert_eq!(vmware, (GpuVendor::Vmware, GpuKind::Virtual));
        let hyperv = classify(r"VMBUS\{DA0A7802-E377-4AAC-8E77-0558EB1073F8}", None, false);
        assert_eq!(hyperv, (GpuVendor::Microsoft, GpuKind::Virtual));
        let indirect = classify(r"ROOT\DISPLAY\0000", None, false);
        assert_eq!(indirect, (GpuVendor::Other, GpuKind::Virtual));
        let usb = classify(r"USB\VID_17E9&PID_6006", None, false);
        assert_eq!(usb, (GpuVendor::Other, GpuKind::Unknown));
    }

    #[test]
    fn test_report_windows_basic_display_and_adapter_keys() {
        assert!(is_basic_display(Some("BasicDisplay"), None));
        assert!(is_basic_display(
            Some("basicdisplay"),
            Some(r"pci\ven_10de&dev_2b85")
        ));
        assert!(is_basic_display(None, Some(r"PCI\CC_0300")));
        assert!(is_basic_display(None, Some(r"pci\cc_0301")));
        assert!(!is_basic_display(
            Some("nvlddmkm"),
            Some(r"pci\ven_10de&dev_2b85")
        ));
        assert!(!is_basic_display(None, Some("pci")));
        assert!(!is_basic_display(None, None));
        let video = r"\Registry\Machine\System\CurrentControlSet\Control\Video\{55B83094-C14A-11F1-A463-581031FEE709}";
        assert_eq!(adapter_of(&format!(r"{video}\0001")), video);
        assert_eq!(adapter_of("0000"), "0000");
        assert!(Key::kernel_path(r"\Registry\User\S-1-5-18").is_none());
        assert!(Key::kernel_path("").is_none());
        assert!(Key::kernel_path(r"\REGISTRY\MACHINE\SYSTEM\CurrentControlSet\Control").is_some());
    }

    #[test]
    fn test_report_windows_plan_mapping() {
        assert!(is_performance(ffi::GUID_MIN_POWER_SAVINGS, None));
        assert!(is_performance(ffi::GUID_ULTIMATE_PERFORMANCE, Some(1)));
        assert!(!is_performance(ffi::GUID_TYPICAL_POWER_SAVINGS, Some(2)));
        assert!(!is_performance(ffi::GUID_MAX_POWER_SAVINGS, Some(0)));
        // A copy of Ultimate Performance under a new GUID.
        let copy = ffi::Guid::from_u128(0x1111_2222_3333_4444_5555_6666_7777_8888);
        assert!(is_performance(copy, Some(PERSONALITY_HIGH_PERFORMANCE)));
        assert!(!is_performance(copy, None));
        assert_eq!(plan_name(ffi::GUID_TYPICAL_POWER_SAVINGS), "Balanced");
        assert_eq!(plan_name(ffi::GUID_MIN_POWER_SAVINGS), "High performance");
        assert_eq!(plan_name(copy), "{11112222-3333-4444-5555-666677778888}");
        let plan = |boost_mode, max_state| Plan {
            guid: copy,
            name: None,
            personality: None,
            boost_mode,
            max_state,
        };
        assert_eq!(plan(Some(2), Some(100)).turbo(), Some(true));
        assert_eq!(plan(Some(1), None).turbo(), Some(true));
        assert_eq!(plan(Some(2), Some(99)).turbo(), Some(false));
        assert_eq!(plan(Some(0), Some(100)).turbo(), Some(false));
        assert_eq!(plan(None, Some(100)).turbo(), None);
        assert_eq!(plan(None, None).display_name(), plan_name(copy));
    }

    #[test]
    fn test_report_windows_last_access_values() {
        assert_eq!(atime_of(0), Some("strictatime"));
        assert_eq!(atime_of(1), Some("noatime"));
        assert_eq!(atime_of(0x8000_0000), Some("strictatime"));
        assert_eq!(atime_of(0x8000_0001), Some("noatime"));
        // fsutil's 2 and 3: system managed, enabled and disabled.
        assert_eq!(atime_of(0x8000_0002), Some("strictatime"));
        assert_eq!(atime_of(0x8000_0003), Some("noatime"));
        assert_eq!(atime_of(2), Some("strictatime"));
        assert_eq!(atime_of(4), None);
        assert_eq!(atime_of(0x8000_0004), None);
        assert_eq!(atime_of(u32::MAX), None);
    }

    /// A temperature answer: thresholds, then (index, reading) sensors.
    fn temperature_answer(critical: i16, warning: i16, sensors: &[(u16, i16)]) -> Vec<u8> {
        use ffi::temperature as t;
        let mut v = vec![0u8; t::INFO + sensors.len() * t::INFO_ENTRY];
        v[t::CRITICAL..t::CRITICAL + 2].copy_from_slice(&critical.to_ne_bytes());
        v[t::WARNING..t::WARNING + 2].copy_from_slice(&warning.to_ne_bytes());
        let count = sensors.len() as u16;
        v[t::INFO_COUNT..t::INFO_COUNT + 2].copy_from_slice(&count.to_ne_bytes());
        for (i, &(index, reading)) in sensors.iter().enumerate() {
            let at = t::INFO + i * t::INFO_ENTRY;
            v[at + t::INFO_INDEX..at + t::INFO_INDEX + 2].copy_from_slice(&index.to_ne_bytes());
            v[at + t::INFO_TEMPERATURE..at + t::INFO_TEMPERATURE + 2]
                .copy_from_slice(&reading.to_ne_bytes());
        }
        v
    }

    #[test]
    fn test_report_windows_drive_temperatures_parsed() {
        let mut out = Vec::new();
        let answer = temperature_answer(84, 80, &[(0, 41), (1, 39), (2, i16::MIN)]);
        drive_temperatures(&answer, "Disk (PhysicalDrive0)", &mut out);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].label, "Disk (PhysicalDrive0)");
        assert_eq!(out[0].celsius, 41.0);
        assert_eq!(out[0].high, Some(80.0));
        assert_eq!(out[0].critical, Some(84.0));
        assert_eq!(out[0].kind, SensorKind::Storage);
        assert_eq!(out[1].label, "Disk (PhysicalDrive0) sensor 1");

        // Thresholds not reported, or zero; a placeholder reading.
        let mut out = Vec::new();
        let answer = temperature_answer(i16::MIN, 0, &[(0, 35), (1, 600)]);
        drive_temperatures(&answer, "d", &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].high, out[0].critical), (None, None));

        // A count the answer does not hold stops at the end of the bytes.
        let mut answer = temperature_answer(70, 65, &[(0, 30)]);
        let count = ffi::temperature::INFO_COUNT;
        answer[count..count + 2].copy_from_slice(&u16::MAX.to_ne_bytes());
        let mut out = Vec::new();
        drive_temperatures(&answer, "d", &mut out);
        assert_eq!(out.len(), 1);
        for cut in 0..answer.len() {
            drive_temperatures(&answer[..cut], "d", &mut Vec::new());
        }
    }

    #[test]
    fn test_report_windows_drive_names_without_serials() {
        let offset = ffi::sdd::PRODUCT_ID_OFFSET;
        let with = |at: u32, tail: &[u8]| {
            let mut answer = vec![0u8; 40];
            answer[offset..offset + 4].copy_from_slice(&at.to_ne_bytes());
            answer.extend_from_slice(tail);
            answer
        };
        let answer = with(40, b"Samsung SSD 990 PRO 2TB   \0S7KHNJ0X000000\0");
        assert_eq!(
            product_of(&answer).as_deref(),
            Some("Samsung SSD 990 PRO 2TB")
        );
        // An offset into the fixed part, past the end, or a blank string.
        assert_eq!(product_of(&with(4, b"x\0")), None);
        assert_eq!(product_of(&with(999, b"x\0")), None);
        assert_eq!(product_of(&with(40, b"  \x01 \0")), None);
        assert_eq!(product_of(&[]), None);
        let long = with(40, &[b'A'; 100]);
        assert_eq!(product_of(&long).map(|s| s.len()), Some(64));
        assert_eq!(drive_name(3, Some("X")), "X (PhysicalDrive3)");
        assert_eq!(drive_name(0, None), "PhysicalDrive0");
    }

    #[test]
    fn test_report_windows_token_privileges_parsed() {
        let entry = |low: u32, high: i32, attributes: u32| {
            let mut e = Vec::new();
            e.extend_from_slice(&low.to_ne_bytes());
            e.extend_from_slice(&high.to_ne_bytes());
            e.extend_from_slice(&attributes.to_ne_bytes());
            e
        };
        let lock = ffi::Luid {
            low_part: 4,
            high_part: 0,
        };
        let mut answer = 2u32.to_ne_bytes().to_vec();
        answer.extend(entry(23, 0, 3));
        answer.extend(entry(4, 0, 0));
        assert!(holds_privilege(&answer, lock), "held, not enabled");
        let mut removed = 1u32.to_ne_bytes().to_vec();
        removed.extend(entry(4, 0, ffi::SE_PRIVILEGE_REMOVED));
        assert!(!holds_privilege(&removed, lock));
        let mut other = 1u32.to_ne_bytes().to_vec();
        other.extend(entry(4, 1, 0));
        assert!(!holds_privilege(&other, lock));
        // A count past the bytes stops at the end.
        let mut short = 1000u32.to_ne_bytes().to_vec();
        short.extend(entry(23, 0, 0));
        assert!(!holds_privilege(&short, lock));
        assert!(!holds_privilege(&[], lock));
    }

    #[test]
    fn test_report_windows_posture_rules() {
        let on = Some(1);
        let off = Some(0);
        assert_eq!(
            vbs_state(on, on, on),
            (Some(Lockdown::Integrity), Some(true))
        );
        assert_eq!(vbs_state(on, off, on), (Some(Lockdown::Off), Some(false)));
        assert_eq!(
            vbs_state(off, None, None),
            (Some(Lockdown::Off), Some(false))
        );
        assert_eq!(
            vbs_state(None, None, on),
            (Some(Lockdown::Off), Some(false))
        );
        assert_eq!(vbs_state(on, None, on), (None, None));
        assert_eq!(vbs_state(on, None, off), (None, Some(false)));
        assert_eq!(
            vbs_state(None, on, None),
            (Some(Lockdown::Integrity), Some(false))
        );

        assert_eq!(firewall_of([on, on, on]), Some(true));
        assert_eq!(firewall_of([off, off, off]), Some(false));
        assert_eq!(firewall_of([on, off, on]), None);
        assert_eq!(firewall_of([on, None, on]), None);

        assert_eq!(tpm_of(2).map(|t| t.version), Some(TpmVersion::V2_0));
        assert_eq!(tpm_of(1).map(|t| t.version), Some(TpmVersion::V1_2));
        assert_eq!(tpm_of(0), None);

        assert!(aslr_on(ffi::ASLR_ENABLE_BOTTOM_UP_RANDOMIZATION));
        assert!(aslr_on(ffi::ASLR_ENABLE_FORCE_RELOCATE_IMAGES));
        assert!(!aslr_on(ffi::ASLR_ENABLE_HIGH_ENTROPY));
        assert!(!aslr_on(0));
    }

    #[test]
    fn test_report_windows_throttling() {
        let cpu = |max_mhz, mhz_limit| ffi::ProcessorPowerInformation {
            max_mhz,
            mhz_limit,
            ..ffi::ProcessorPowerInformation::default()
        };
        assert_eq!(
            throttled_of(&[cpu(3000, 3000), cpu(3000, 3000)]),
            Some(false)
        );
        assert_eq!(
            throttled_of(&[cpu(3000, 3000), cpu(3000, 1200)]),
            Some(true)
        );
        assert_eq!(throttled_of(&[cpu(0, 0)]), None);
        assert_eq!(throttled_of(&[]), None);
    }

    #[test]
    fn test_report_windows_live_temperatures() {
        match temperatures() {
            Ok(list) => {
                for t in &list {
                    assert!(crate::sensors::plausible(t.celsius), "{t:?}");
                    assert!(t.label.contains("PhysicalDrive"), "{t:?}");
                    assert_eq!(t.kind, SensorKind::Storage);
                }
                println!("temperatures: {list:?}");
            }
            Err(e) => assert!(e.is_unavailable(), "{e}"),
        }
    }

    #[test]
    fn test_report_windows_live_gpus() {
        match gpus() {
            Ok(list) => {
                for g in &list {
                    assert!(!g.name.is_empty(), "{g:?}");
                    assert_eq!(g.vendor_id.is_some(), g.device_id.is_some(), "{g:?}");
                }
                println!("gpus: {list:?}");
            }
            Err(e) => assert!(e.is_unavailable(), "{e}"),
        }
    }

    #[test]
    fn test_report_windows_live_tuning() {
        let t = tuning();
        assert!(t.handles_unlimited);
        assert!(
            matches!(t.memlock, Fact::Value(MemLock::Privilege(_))),
            "{:?}",
            t.memlock
        );
        assert!(matches!(t.ports, Fact::Unknown(_)));
        match &t.governor {
            Fact::Value(g) => assert!(!g.name.is_empty()),
            Fact::Unknown(why) => assert!(!why.is_empty()),
            Fact::Skip => panic!("the governor applies on Windows"),
        }
        assert_eq!(t.swappiness, Fact::Skip);
        assert_eq!(t.clock_source, Fact::Skip);
        println!("tuning: {t:?}");
    }

    #[test]
    fn test_report_windows_live_data_dir() {
        let dir = std::env::temp_dir();
        let disk = crate::disk::of(&dir).unwrap();
        let tuning = data_dir_tuning(&dir, &disk);
        assert_eq!(tuning.scheduler, Fact::Skip);
        if disk.filesystem.eq_ignore_ascii_case("NTFS") {
            assert_ne!(tuning.atime, Fact::Skip);
        } else {
            assert_eq!(tuning.atime, Fact::Skip);
        }
        println!("data dir {}: {tuning:?}", disk.filesystem);
    }

    #[test]
    fn test_report_windows_live_bench_facts() {
        let facts = bench_facts();
        assert_eq!(facts.governor.is_some(), facts.power_saving.is_some());
        assert_eq!(facts.clock_source, None);
        assert_eq!(facts.isolated_cpus, None);
        if let Some(smt) = facts.smt {
            let cores = &crate::cpu::topology().cores;
            if !cores.is_empty() {
                assert_eq!(smt, cores.iter().any(|c| c.logical.len() > 1));
            }
        }
        println!("bench: {facts:?}");
    }

    #[test]
    fn test_report_windows_live_posture() {
        let p = posture();
        assert_eq!(p.disk_encryption, None);
        assert_eq!(p.mandatory_access_control, None);
        assert!(p.cpu_vulnerabilities.is_empty());
        assert_eq!(p.elevated, None);
        assert_eq!(p.aslr, aslr());
        println!("posture: {p:?}");
    }
}
