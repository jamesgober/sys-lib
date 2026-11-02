//! Detection and identity (0.5), answered "unknown", for the targets sys-lib has
//! no backend for yet (the `unsupported` module glob-imports it). Linux, macOS,
//! and Windows define every item themselves, so a missing item there is a build
//! error. The list below is the interface each backend provides.

#![allow(dead_code)]

use std::path::Path;

use crate::{
    Error, Result,
    cpu::Arch,
    disk::Disk,
    host::{ClockSync, Session, TimeZone},
    mem::MemoryModule,
    net::Dns,
    os::{Confidence, Detection, FormFactor},
    power::RawBattery,
    process::LaunchFacts,
    sys::{HardwareFacts, TopologyFacts},
    virt::{Cloud, Virtualization},
};

fn no(what: &'static str) -> Error {
    Error::Unsupported {
        what,
        platform: crate::os::kind().name(),
    }
}

pub(crate) fn virtualization() -> Virtualization {
    Virtualization::default()
}

pub(crate) fn cloud() -> Option<Cloud> {
    None
}

pub(crate) fn form_factor() -> Detection<FormFactor> {
    Detection::new(FormFactor::Unknown, "none", Confidence::None)
}

pub(crate) fn hardware() -> HardwareFacts {
    HardwareFacts::default()
}

pub(crate) fn is_server_edition() -> Option<bool> {
    None
}

pub(crate) fn native_arch() -> Arch {
    crate::cpu::arch()
}

/// A word identifying the CPU's model: CPUID leaf 1 EAX on x86, 0 elsewhere.
pub(crate) fn cpu_signature() -> u32 {
    0
}

pub(crate) fn cpu_topology() -> TopologyFacts {
    TopologyFacts::default()
}

pub(crate) fn memory_modules() -> Result<Vec<MemoryModule>> {
    Err(no("memory modules"))
}

pub(crate) fn disk_of(_path: &Path) -> Result<Disk> {
    Err(no("disk of a path"))
}

/// The index of the interface holding the default route.
pub(crate) fn primary_interface() -> Result<Option<u32>> {
    Err(no("default route"))
}

pub(crate) fn dns() -> Result<Dns> {
    Err(no("DNS configuration"))
}

pub(crate) fn timezone() -> Result<TimeZone> {
    Err(no("time zone"))
}

/// The BCP 47 tag and the raw value.
pub(crate) fn locale() -> Option<(String, String)> {
    None
}

pub(crate) fn clock_sync() -> Result<Option<ClockSync>> {
    Ok(None)
}

pub(crate) fn sessions() -> Result<Vec<Session>> {
    Err(no("sessions"))
}

pub(crate) fn launch() -> LaunchFacts {
    LaunchFacts::default()
}

pub(crate) fn has_battery() -> bool {
    false
}

pub(crate) fn batteries() -> Result<Vec<RawBattery>> {
    Err(no("batteries"))
}

pub(crate) fn on_ac() -> Result<Option<bool>> {
    Ok(None)
}

/// Reads an anchor into `out`, in canonical form (`crate::device::canon`).
/// `CpuSignature` and `PermanentMac` are read by `crate::device` itself.
#[cfg(feature = "device-id")]
pub(crate) fn device_anchor(
    _anchor: crate::device::Anchor,
    _out: &mut [u8; crate::device::ANCHOR_BUF],
) -> crate::device::Reading {
    crate::device::Reading::Unsupported
}
