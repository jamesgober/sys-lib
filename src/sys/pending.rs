//! The live-reading interface (0.4), answered `Unsupported`, for the targets
//! sys-lib has no backend for yet (the `unsupported` module glob-imports it).
//! Linux, macOS, and Windows define every item themselves and do not import
//! this module, so a missing item there is a build error. The list below is
//! the interface each backend provides.
//!
//! The interface, by area (each function's contract is its caller's
//! documentation in the public module):
//!
//! - memory: `memory`, `page_size`, `mem_pressure`, `mem_events`, `paging`,
//!   `huge_pages`
//! - CPU: `cpu_online`, `cpu_quota`, `frequency`, `current_frequency`, `load`,
//!   `cpu_pressure`, `cpu_times`, `cpu_times_per_core`, `CPU_TICK`,
//!   `CPU_STATES`
//! - disks: `disk_space`, `disks`, `disk_io_totals`, `disk_io_counters`,
//!   `disk_pressure`
//! - network: `interfaces`, `net_totals`, `net_counters`, `connections`
//! - this process: `parent_id`, `process_memory`, `peak_memory`,
//!   `memory_detail`, `cpu_time`, `threads`, `open_files`, `limits`,
//!   `started_at`, `process_io`
//! - host: `uptime`, `boot_time`, `processes`, `host_open_files`

#![allow(dead_code)]

use core::time::Duration;
use std::{path::Path, time::SystemTime};

use crate::{
    Bytes, Error, Mhz, Pressure, Result,
    cpu::{CpuQuota, Frequency, Load, States, Times},
    disk::{Disk, DiskRaw, ListOptions, Space},
    host::{OpenFiles, Processes},
    mem::{HugePages, MemRaw, MemWant, MemoryEvents, PagingRaw},
    net::{Connections, Counters, Interface, InterfaceOptions, Totals},
    process::{CpuTime, Limits, ProcessIo, ProcessMemory},
};

fn no(what: &'static str) -> Error {
    Error::Unsupported {
        what,
        platform: crate::os::kind().name(),
    }
}

// Memory.

pub(crate) fn memory(_want: MemWant) -> Result<MemRaw> {
    Err(no("memory"))
}

pub(crate) fn page_size() -> usize {
    4096
}

pub(crate) fn mem_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

pub(crate) fn mem_events() -> Result<Option<MemoryEvents>> {
    Ok(None)
}

pub(crate) fn paging() -> Result<PagingRaw> {
    Err(no("paging activity"))
}

pub(crate) fn huge_pages() -> Result<Option<HugePages>> {
    Ok(None)
}

// CPU.

/// The resolution of the CPU time counters: the shortest interval a sampler
/// can measure.
pub(crate) const CPU_TICK: Duration = Duration::from_millis(10);

/// Which states the CPU time counters distinguish.
pub(crate) const CPU_STATES: States = States {
    iowait: false,
    steal: false,
    irq: false,
};

pub(crate) fn cpu_online(logical: usize) -> usize {
    logical
}

pub(crate) fn cpu_quota() -> Result<Option<CpuQuota>> {
    Ok(None)
}

pub(crate) fn frequency() -> Frequency {
    Frequency::default()
}

pub(crate) fn current_frequency() -> Result<Option<Mhz>> {
    Ok(None)
}

pub(crate) fn load() -> Result<Option<Load>> {
    Ok(None)
}

pub(crate) fn cpu_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

pub(crate) fn cpu_times() -> Result<Times> {
    Err(no("CPU times"))
}

/// The total and each core's counters into `out`; the number of cores
/// written. `TooLarge` when more cores report than `out` holds.
pub(crate) fn cpu_times_per_core(_out: &mut [Times]) -> Result<(Times, usize)> {
    Err(no("CPU times"))
}

// Disks.

pub(crate) fn disk_space(_path: &Path) -> Result<Space> {
    Err(no("disk space"))
}

pub(crate) fn disks(_options: &ListOptions) -> Result<Vec<Disk>> {
    Err(no("disk list"))
}

pub(crate) fn disk_io_totals() -> Result<DiskRaw> {
    Err(no("disk I/O"))
}

pub(crate) fn disk_io_counters() -> Result<Vec<(String, DiskRaw)>> {
    Err(no("disk I/O"))
}

pub(crate) fn disk_pressure() -> Result<Option<Pressure>> {
    Ok(None)
}

// Network.

pub(crate) fn interfaces(_options: &InterfaceOptions) -> Result<Vec<Interface>> {
    Err(no("network interfaces"))
}

pub(crate) fn net_totals() -> Result<Totals> {
    Err(no("network counters"))
}

pub(crate) fn net_counters() -> Result<Vec<Counters>> {
    Err(no("network counters"))
}

pub(crate) fn connections() -> Result<Connections> {
    Err(no("connection counts"))
}

// This process.

pub(crate) fn parent_id() -> Option<u32> {
    None
}

pub(crate) fn process_memory() -> Result<Bytes> {
    Err(no("process memory"))
}

pub(crate) fn peak_memory() -> Result<Bytes> {
    Err(no("process memory"))
}

pub(crate) fn memory_detail() -> Result<ProcessMemory> {
    Err(no("process memory"))
}

pub(crate) fn cpu_time() -> Result<CpuTime> {
    Err(no("process CPU time"))
}

pub(crate) fn threads() -> Result<usize> {
    Err(no("thread count"))
}

pub(crate) fn open_files() -> Result<usize> {
    Err(no("open files"))
}

pub(crate) fn limits() -> Result<Limits> {
    Ok(Limits::default())
}

pub(crate) fn started_at() -> Result<SystemTime> {
    Err(no("process start time"))
}

pub(crate) fn process_io() -> Result<ProcessIo> {
    Err(no("process I/O"))
}

// Host.

pub(crate) fn uptime() -> Result<Duration> {
    Err(no("uptime"))
}

pub(crate) fn boot_time() -> Result<SystemTime> {
    Err(no("boot time"))
}

pub(crate) fn processes() -> Result<Processes> {
    Err(no("process count"))
}

pub(crate) fn host_open_files() -> Result<OpenFiles> {
    Err(no("open file count"))
}
