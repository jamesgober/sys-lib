//! Memory: physical memory, the job object's limit, pressure, and large pages.

use std::{mem::MaybeUninit, sync::OnceLock};

use super::{
    Owned, ffi, job_limits, memory_status, os_error, performance_info, process::memory_counters,
    unsupported,
};
use crate::{
    Bytes, Pressure, PressureLevel, Result,
    mem::{HugePages, LimitSource, MemRaw, MemoryEvents, MemoryLimit, MemorySource, PagingRaw},
};

pub(crate) fn memory(with_swap: bool) -> Result<MemRaw> {
    let status = memory_status()?;
    let mut raw = MemRaw {
        host_total: status.total_phys,
        host_available: status.avail_phys,
        source: Some(MemorySource::Host),
        ..MemRaw::default()
    };
    // The cache and the commit counters come from GetPerformanceInfo, which
    // counts handles across the machine: only the full snapshot pays for it.
    if with_swap {
        let info = performance_info("memory")?;
        let page = info.page_size as u64;
        raw.cached = Some((info.system_cache as u64).saturating_mul(page));
        raw.swap = Some(swap_from_commit(
            info.commit_total as u64,
            info.commit_limit as u64,
            info.physical_total as u64,
            page,
        ));
    }
    if let Some(job) = job_limits() {
        let flags = job.basic_limit_information.limit_flags;
        if let Some((hard, per_process)) = job_memory_limit(
            flags,
            job.process_memory_limit as u64,
            job.job_memory_limit as u64,
        ) {
            raw.limit = Some(MemoryLimit {
                hard: Bytes::new(hard),
                soft: None,
                swap: None,
                source: LimitSource::JobObject,
            });
            // A per-process limit counts this process's commit charge. The
            // job's current total has no reliable source, so a job limit gets
            // the host's usage, capped at the limit (`mem::from_raw`).
            raw.limited_used = if per_process {
                memory_counters().ok().map(|c| c.private_usage as u64)
            } else {
                None
            };
        }
    }
    Ok(raw)
}

/// Swap `(total, free)` in bytes from the commit counters, in pages. The
/// commit limit beyond physical memory is the paging files' size, and commit
/// charged beyond physical memory is the least of it that must be in use: an
/// approximation, because Windows reports paging-file usage only through
/// performance counters.
fn swap_from_commit(commit_total: u64, commit_limit: u64, physical: u64, page: u64) -> (u64, u64) {
    let total = commit_limit.saturating_sub(physical);
    let used = commit_total.saturating_sub(physical).min(total);
    (
        total.saturating_mul(page),
        (total - used).saturating_mul(page),
    )
}

/// The memory limit a job sets, and whether it is the per-process one: the
/// smaller of the process and job limits whose flags are set.
fn job_memory_limit(flags: u32, process: u64, job: u64) -> Option<(u64, bool)> {
    let process = (flags & ffi::JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0 && process > 0)
        .then_some((process, true));
    let job = (flags & ffi::JOB_OBJECT_LIMIT_JOB_MEMORY != 0 && job > 0).then_some((job, false));
    match (process, job) {
        (Some(p), Some(j)) => Some(if j.0 < p.0 { j } else { p }),
        (p, j) => p.or(j),
    }
}

pub(crate) fn page_size() -> usize {
    static PAGE: OnceLock<usize> = OnceLock::new();
    *PAGE.get_or_init(|| {
        let mut info = MaybeUninit::<ffi::SystemInfo>::zeroed();
        // SAFETY: `info` is a writable SYSTEM_INFO; the call fills it.
        unsafe { ffi::GetSystemInfo(info.as_mut_ptr()) };
        // SAFETY: zeroed before the call, so every field is initialised.
        let info = unsafe { info.assume_init() };
        usize::try_from(info.page_size)
            .ok()
            .filter(|p| p.is_power_of_two())
            .unwrap_or(4096)
    })
}

/// `Critical` while the system signals low memory, else `Normal`.
pub(crate) fn mem_pressure() -> Result<Option<Pressure>> {
    // SAFETY: plain enum argument; the returned handle is checked before use.
    let handle =
        unsafe { ffi::CreateMemoryResourceNotification(ffi::LOW_MEMORY_RESOURCE_NOTIFICATION) };
    if handle.is_null() {
        return Err(os_error(
            "memory pressure",
            "CreateMemoryResourceNotification",
        ));
    }
    let handle = Owned(handle);
    let mut low: ffi::Bool = 0;
    // SAFETY: `handle` is the notification just created; `low` is writable.
    let ok = unsafe { ffi::QueryMemoryResourceNotification(handle.0, &raw mut low) };
    if ok == 0 {
        return Err(os_error(
            "memory pressure",
            "QueryMemoryResourceNotification",
        ));
    }
    Ok(Some(Pressure::from_level(if low != 0 {
        PressureLevel::Critical
    } else {
        PressureLevel::Normal
    })))
}

/// Windows keeps no out-of-memory counters.
pub(crate) fn mem_events() -> Result<Option<MemoryEvents>> {
    Ok(None)
}

/// The SDK declares `SYSTEM_PERFORMANCE_INFORMATION` as 312 reserved bytes,
/// so the paging counters inside it have no documented offsets to read.
pub(crate) fn paging() -> Result<PagingRaw> {
    Err(unsupported("paging activity"))
}

/// The large page minimum; Windows has no huge-page pool to count.
pub(crate) fn huge_pages() -> Result<Option<HugePages>> {
    Ok(large_pages(ffi::GetLargePageMinimum()))
}

fn large_pages(minimum: usize) -> Option<HugePages> {
    (minimum > 0).then(|| HugePages {
        page_size: Bytes::new(minimum as u64),
        total: 0,
        free: 0,
        reserved: 0,
        transparent: None,
        large_page_minimum: Some(Bytes::new(minimum as u64)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: u64 = 4096;

    #[test]
    fn test_mem_windows_swap_from_commit_bounded() {
        // 16 GiB of RAM (4 Mi pages) and a 4 GiB paging file (1 Mi pages).
        let ram = 4 << 20;
        let (total, free) = swap_from_commit(3 << 20, ram + (1 << 20), ram, PAGE);
        assert_eq!((total, free), (4 << 30, 4 << 30));
        // Commit 1 GiB past RAM: at least that much is paged out.
        let (total, free) = swap_from_commit(ram + (1 << 18), ram + (1 << 20), ram, PAGE);
        assert_eq!(total - free, 1 << 30);
        // No paging file, and commit read above the limit: all zero, no underflow.
        assert_eq!(swap_from_commit(ram * 2, ram, ram, PAGE), (0, 0));
        assert_eq!(swap_from_commit(ram * 2, ram - 1, ram, PAGE), (0, 0));
    }

    #[test]
    fn test_mem_windows_job_limit_smaller_wins_flags_required() {
        let process = ffi::JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        let job = ffi::JOB_OBJECT_LIMIT_JOB_MEMORY;
        assert_eq!(job_memory_limit(0, 100, 200), None);
        assert_eq!(job_memory_limit(process, 100, 200), Some((100, true)));
        assert_eq!(job_memory_limit(job, 100, 200), Some((200, false)));
        assert_eq!(
            job_memory_limit(process | job, 300, 200),
            Some((200, false))
        );
        assert_eq!(job_memory_limit(process | job, 100, 200), Some((100, true)));
        assert_eq!(job_memory_limit(process, 0, 200), None);
    }

    #[test]
    fn test_mem_windows_large_pages_only_when_supported() {
        assert_eq!(large_pages(0), None);
        let h = large_pages(2 << 20).unwrap();
        assert_eq!(h.page_size, Bytes::from_mib(2));
        assert_eq!(h.large_page_minimum, Some(Bytes::from_mib(2)));
        assert_eq!(
            (h.total, h.free, h.reserved, h.transparent),
            (0, 0, 0, None)
        );
    }

    #[test]
    fn test_mem_windows_live_readings() {
        let m = memory(true).unwrap();
        assert!(m.host_total > 0 && m.host_available <= m.host_total);
        assert!(m.cached.is_some_and(|c| c <= m.host_total));
        let (total, free) = m.swap.unwrap();
        assert!(free <= total);
        if let (Some(limit), Some(used)) = (m.limit, m.limited_used) {
            assert!(limit.hard.raw() > 0 && used > 0);
        }
        let quick = memory(false).unwrap();
        assert!(quick.swap.is_none() && quick.cached.is_none());
        assert!(page_size().is_power_of_two() && page_size() >= 4096);
        let p = mem_pressure().unwrap().unwrap();
        assert!(p.full.is_none() && p.total.is_none());
        assert!(mem_events().unwrap().is_none());
        assert!(matches!(paging(), Err(crate::Error::Unsupported { .. })));
        if let Some(h) = huge_pages().unwrap() {
            assert!(h.page_size.raw().is_power_of_two());
        }
    }
}
