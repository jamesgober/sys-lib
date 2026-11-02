//! Detection and identity on Linux (0.5): the file-tree logic in `crate::linux`
//! applied to the real `/`, the environment it is told about, and the calls no
//! file answers (`localtime_r`, `adjtimex`).

use core::time::Duration;
use std::{
    ffi::{CStr, c_char},
    mem::MaybeUninit,
    time::SystemTime,
};

use super::{ROOT, ffi};
use crate::{
    Error, Result,
    cpu::Arch,
    host::{ClockSync, Session, TimeZone},
    linux as tree,
    mem::MemoryModule,
    net::Dns,
    os::{Detection, FormFactor},
    power::RawBattery,
    process::LaunchFacts,
    sys::{HardwareFacts, TopologyFacts},
    virt::{Cloud, Virtualization},
};

fn env_set(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|v| !v.is_empty())
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn cpuid_signature() -> Option<[u8; 12]> {
    crate::sys::x86::hypervisor_signature()
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn cpuid_signature() -> Option<[u8; 12]> {
    None
}

pub(crate) fn virtualization() -> Virtualization {
    tree::virt::detect(
        &ROOT,
        cpuid_signature(),
        tree::virt::Env {
            kubernetes_host: env_set("KUBERNETES_SERVICE_HOST"),
            wsl_distro: env_set("WSL_DISTRO_NAME"),
        },
    )
}

pub(crate) fn cloud() -> Option<Cloud> {
    tree::virt::cloud(&ROOT, crate::virt::hypervisor())
}

#[cfg(not(target_os = "android"))]
pub(crate) fn form_factor() -> Detection<FormFactor> {
    tree::hardware::form_factor(&ROOT, crate::virt::is_vm(), crate::power::has_battery())
}

/// `ro.build.characteristics`, else a phone: Android's own default.
#[cfg(target_os = "android")]
pub(crate) fn form_factor() -> Detection<FormFactor> {
    use crate::os::Confidence::{High, Medium};

    let traits = super::property(c"ro.build.characteristics").unwrap_or_default();
    let has = |t: &str| traits.split(',').any(|v| v == t);
    if has("tablet") {
        Detection::new(FormFactor::Tablet, "ro.build.characteristics", High)
    } else if has("watch") {
        Detection::new(FormFactor::Wearable, "ro.build.characteristics", High)
    } else if has("tv") {
        Detection::new(FormFactor::Tv, "ro.build.characteristics", High)
    } else {
        Detection::new(FormFactor::Phone, "Android default", Medium)
    }
}

pub(crate) fn hardware() -> HardwareFacts {
    tree::hardware::facts(&ROOT)
}

pub(crate) fn is_server_edition() -> Option<bool> {
    None
}

/// The running kernel's machine, which differs from this binary's under qemu
/// user emulation or Rosetta for Linux.
pub(crate) fn native_arch() -> Arch {
    let arch = ROOT.text("/proc/sys/kernel/arch");
    match arch.as_deref() {
        Some("x86_64") => Arch::X86_64,
        Some("aarch64") => Arch::Aarch64,
        Some("riscv64") => Arch::Riscv64,
        Some("ppc64le" | "ppc64") => Arch::Powerpc64,
        Some("s390x") => Arch::S390x,
        Some("loongarch64") => Arch::Loongarch64,
        _ => crate::cpu::arch(),
    }
}

#[cfg(all(feature = "device-id", any(target_arch = "x86", target_arch = "x86_64")))]
pub(crate) fn cpu_signature() -> u32 {
    crate::sys::x86::signature()
}

#[cfg(all(
    feature = "device-id",
    not(any(target_arch = "x86", target_arch = "x86_64"))
))]
pub(crate) fn cpu_signature() -> u32 {
    0
}

pub(crate) fn cpu_topology() -> TopologyFacts {
    tree::topology::read(&ROOT)
}

pub(crate) fn memory_modules() -> Result<Vec<MemoryModule>> {
    tree::hardware::modules(&ROOT)
}

pub(crate) fn primary_interface() -> Result<Option<u32>> {
    Ok(tree::net::default_route(&ROOT).and_then(|name| tree::net::index(&ROOT, &name)))
}

pub(crate) fn dns() -> Result<Dns> {
    tree::net::dns(&ROOT)
}

pub(crate) fn timezone() -> Result<TimeZone> {
    let env = std::env::var("TZ").ok();
    let iana = tree::host::zone_name(&ROOT, env.as_deref());
    let now: ffi::Time = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as ffi::Time);
    let mut tm = MaybeUninit::<ffi::Tm>::zeroed();
    // SAFETY: both pointers are valid for the call; `tm` is zeroed, so it is a
    // valid `Tm` whether or not the call fills it.
    let filled = unsafe { ffi::localtime_r(&raw const now, tm.as_mut_ptr()) };
    if filled.is_null() {
        return Err(Error::Os {
            what: "time zone",
            call: "localtime_r",
            code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
        });
    }
    // SAFETY: zeroed above, so initialised either way.
    let tm = unsafe { tm.assume_init() };
    let zone: *const c_char = tm.zone;
    let abbreviation = (!zone.is_null()).then(|| {
        // SAFETY: libc's `tm_zone` is a NUL-terminated string that lives as
        // long as the zone data; it is copied before anything can change it.
        unsafe { CStr::from_ptr(zone) }.to_string_lossy().into_owned()
    });
    Ok(TimeZone {
        iana,
        windows: None,
        utc_offset_seconds: i32::try_from(tm.gmtoff).unwrap_or(0),
        abbreviation: abbreviation.filter(|a| !a.is_empty() && a.len() <= 16),
    })
}

pub(crate) fn locale() -> Option<(String, String)> {
    crate::host::posix_locale()
}

#[cfg(not(target_os = "android"))]
pub(crate) fn clock_sync() -> Result<Option<ClockSync>> {
    let mut t = MaybeUninit::<ffi::Timex>::zeroed();
    // SAFETY: a zeroed `Timex` has `modes == 0`: a read-only query the kernel
    // writes into; `t` is valid for the whole structure.
    let state = unsafe { ffi::adjtimex(t.as_mut_ptr()) };
    if state < 0 {
        // A sandbox refusing the call is not a fact about the clock.
        return Ok(None);
    }
    // SAFETY: zeroed above, and the kernel wrote only valid integers.
    let t = unsafe { t.assume_init() };
    let micros = |v: core::ffi::c_long| {
        u64::try_from(v).ok().map(Duration::from_micros)
    };
    Ok(Some(ClockSync {
        synchronized: state != ffi::TIME_ERROR && t.status & ffi::STA_UNSYNC == 0,
        max_error: micros(t.maxerror),
        estimated_error: micros(t.esterror),
    }))
}

#[cfg(target_os = "android")]
pub(crate) fn clock_sync() -> Result<Option<ClockSync>> {
    Ok(None)
}

pub(crate) fn sessions() -> Result<Vec<Session>> {
    tree::host::sessions(&ROOT)
}

pub(crate) fn launch() -> LaunchFacts {
    tree::process::launch(
        &ROOT,
        tree::process::LaunchEnv {
            invocation: env_set("INVOCATION_ID"),
            notify_socket: env_set("NOTIFY_SOCKET"),
            ssh: env_set("SSH_CONNECTION"),
        },
        std::process::id(),
    )
}

pub(crate) fn has_battery() -> bool {
    tree::power::has_battery(&ROOT)
}

pub(crate) fn batteries() -> Result<Vec<RawBattery>> {
    tree::power::batteries(&ROOT)
}

pub(crate) fn on_ac() -> Result<Option<bool>> {
    Ok(tree::power::on_ac(&ROOT))
}

#[cfg(feature = "device-id")]
pub(crate) fn device_anchor(
    anchor: crate::device::Anchor,
    out: &mut [u8; crate::device::ANCHOR_BUF],
) -> crate::device::Reading {
    tree::device::anchor(&ROOT, anchor, out)
}
