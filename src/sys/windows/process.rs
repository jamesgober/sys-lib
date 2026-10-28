//! This process: parent, memory, CPU time, threads, handles, job limits,
//! start time, and I/O.

use std::{mem::size_of, sync::OnceLock, time::SystemTime};

use super::{Owned, ffi, filetime_to_system, hundred_ns, job_limits, memory_status, os_error};
use crate::{
    Bytes, Error, Result,
    process::{CpuTime, Limit, Limits, ProcessIo, ProcessMemory},
};

/// The most snapshot entries walked: a bound on a corrupt or endless list.
const MAX_ENTRIES: usize = 4 << 20;

/// This process's entry in a process snapshot, read once: the parent of a
/// running process never changes.
pub(crate) fn parent_id() -> Option<u32> {
    static PARENT: OnceLock<Option<u32>> = OnceLock::new();
    *PARENT.get_or_init(read_parent_id)
}

fn read_parent_id() -> Option<u32> {
    // SAFETY: plain flags; the returned handle is checked before use.
    let snapshot = unsafe { ffi::CreateToolhelp32Snapshot(ffi::TH32CS_SNAPPROCESS, 0) };
    if snapshot == ffi::INVALID_HANDLE_VALUE {
        return None;
    }
    let snapshot = Owned(snapshot);
    let pid = std::process::id();
    let mut entry = ffi::ProcessEntry32W {
        size: size_of::<ffi::ProcessEntry32W>() as u32,
        cnt_usage: 0,
        th32_process_id: 0,
        th32_default_heap_id: 0,
        th32_module_id: 0,
        cnt_threads: 0,
        th32_parent_process_id: 0,
        pc_pri_class_base: 0,
        flags: 0,
        exe_file: [0; 260],
    };
    // SAFETY: `entry` is a PROCESSENTRY32W with its size set.
    let mut more = unsafe { ffi::Process32FirstW(snapshot.0, &raw mut entry) } != 0;
    let mut seen = 0usize;
    while more && seen < MAX_ENTRIES {
        if entry.th32_process_id == pid {
            // The System Idle Process (0) is nobody's parent.
            return (entry.th32_parent_process_id != 0).then_some(entry.th32_parent_process_id);
        }
        seen += 1;
        // SAFETY: as above.
        more = unsafe { ffi::Process32NextW(snapshot.0, &raw mut entry) } != 0;
    }
    None
}

/// `GetProcessMemoryInfo` for this process.
pub(super) fn memory_counters() -> Result<ffi::ProcessMemoryCountersEx> {
    let size = size_of::<ffi::ProcessMemoryCountersEx>() as u32;
    let mut counters = ffi::ProcessMemoryCountersEx {
        cb: size,
        ..ffi::ProcessMemoryCountersEx::default()
    };
    // SAFETY: the pseudo-handle of this process is always valid; `counters` is
    // a PROCESS_MEMORY_COUNTERS_EX of the size passed.
    let ok =
        unsafe { ffi::K32GetProcessMemoryInfo(ffi::GetCurrentProcess(), &raw mut counters, size) };
    if ok == 0 {
        return Err(os_error("process memory", "GetProcessMemoryInfo"));
    }
    Ok(counters)
}

pub(crate) fn process_memory() -> Result<Bytes> {
    memory_counters().map(|c| Bytes::new(c.working_set_size as u64))
}

pub(crate) fn peak_memory() -> Result<Bytes> {
    memory_counters().map(|c| Bytes::new(c.peak_working_set_size as u64))
}

/// The working set, the commit charge as private memory, and the address
/// space in use (reserved or committed) as the virtual size.
pub(crate) fn memory_detail() -> Result<ProcessMemory> {
    let c = memory_counters()?;
    let status = memory_status()?;
    Ok(ProcessMemory {
        resident: Bytes::new(c.working_set_size as u64),
        peak_resident: Bytes::new(c.peak_working_set_size as u64),
        virtual_size: Bytes::new(status.total_virtual.saturating_sub(status.avail_virtual)),
        private: Some(Bytes::new(c.private_usage as u64)),
        shared: None,
        swapped: None,
    })
}

/// `GetProcessTimes`: (creation, kernel, user), `FILETIME` values.
fn process_times() -> Result<(u64, u64, u64)> {
    let (mut creation, mut exit, mut kernel, mut user) = (
        ffi::FileTime::default(),
        ffi::FileTime::default(),
        ffi::FileTime::default(),
        ffi::FileTime::default(),
    );
    // SAFETY: the pseudo-handle of this process is always valid; the four
    // outputs are writable locals.
    let ok = unsafe {
        ffi::GetProcessTimes(
            ffi::GetCurrentProcess(),
            &raw mut creation,
            &raw mut exit,
            &raw mut kernel,
            &raw mut user,
        )
    };
    if ok == 0 {
        return Err(os_error("process times", "GetProcessTimes"));
    }
    Ok((creation.get(), kernel.get(), user.get()))
}

pub(crate) fn cpu_time() -> Result<CpuTime> {
    let (_, kernel, user) = process_times()?;
    Ok(CpuTime {
        user: hundred_ns(user),
        system: hundred_ns(kernel),
    })
}

/// This process's threads, from a system-wide thread snapshot.
pub(crate) fn threads() -> Result<usize> {
    // SAFETY: plain flags; the returned handle is checked before use.
    let snapshot = unsafe { ffi::CreateToolhelp32Snapshot(ffi::TH32CS_SNAPTHREAD, 0) };
    if snapshot == ffi::INVALID_HANDLE_VALUE {
        return Err(os_error("thread count", "CreateToolhelp32Snapshot"));
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
    let (mut count, mut seen) = (0usize, 0usize);
    // SAFETY: `entry` is a THREADENTRY32 with its size set.
    let mut more = unsafe { ffi::Thread32First(snapshot.0, &raw mut entry) } != 0;
    while more && seen < MAX_ENTRIES {
        if entry.th32_owner_process_id == pid {
            count += 1;
        }
        seen += 1;
        // SAFETY: as above.
        more = unsafe { ffi::Thread32Next(snapshot.0, &raw mut entry) } != 0;
    }
    if count == 0 {
        // The calling thread is always in the snapshot.
        return Err(Error::Malformed {
            what: "thread count",
            source: "CreateToolhelp32Snapshot",
            line: None,
        });
    }
    Ok(count)
}

/// The guard counts threads on Unix only (Windows has no umask race); its
/// tests compile the call on every platform.
#[cfg(all(test, feature = "guard"))]
pub(crate) fn thread_count() -> Option<usize> {
    threads().ok()
}

/// Open handles: files, but also events, keys, threads, and the rest.
pub(crate) fn open_files() -> Result<usize> {
    let mut count = 0u32;
    // SAFETY: the pseudo-handle of this process is always valid; `count` is
    // writable.
    let ok = unsafe { ffi::GetProcessHandleCount(ffi::GetCurrentProcess(), &raw mut count) };
    if ok == 0 {
        return Err(os_error("open files", "GetProcessHandleCount"));
    }
    Ok(count as usize)
}

/// Windows has no `rlimit`: the job object's active-process and per-process
/// memory limits when they are set, every other value `None`. A 32-bit
/// process is also bound by its own address space (2 GiB, or 4 GiB when it is
/// large-address aware on 64-bit Windows: E-34).
pub(crate) fn limits() -> Result<Limits> {
    let mut limits = job_limits().map_or_else(Limits::default, |job| {
        let flags = job.basic_limit_information.limit_flags;
        job_process_limits(
            flags,
            job.basic_limit_information.active_process_limit,
            job.process_memory_limit as u64,
        )
    });
    if cfg!(target_pointer_width = "32") {
        let space = memory_status()?.total_virtual;
        let cap = limits.address_space.hard.map_or(space, |h| h.min(space));
        limits.address_space = Limit {
            soft: Some(cap),
            hard: Some(cap),
        };
    }
    Ok(limits)
}

/// The limits a job's flags and values set. A job limit cannot be raised from
/// inside the job, so soft and hard are the same.
fn job_process_limits(flags: u32, active_processes: u32, process_memory: u64) -> Limits {
    let both = |n: u64| Limit {
        soft: Some(n),
        hard: Some(n),
    };
    let mut limits = Limits::default();
    if flags & ffi::JOB_OBJECT_LIMIT_ACTIVE_PROCESS != 0 {
        limits.processes = both(u64::from(active_processes));
    }
    if flags & ffi::JOB_OBJECT_LIMIT_PROCESS_MEMORY != 0 && process_memory > 0 {
        limits.address_space = both(process_memory);
    }
    limits
}

pub(crate) fn started_at() -> Result<SystemTime> {
    let (creation, _, _) = process_times()?;
    filetime_to_system(creation).ok_or(Error::Malformed {
        what: "process start time",
        source: "GetProcessTimes",
        line: None,
    })
}

/// Every read and write this process made, files, devices, and network alike.
pub(crate) fn process_io() -> Result<ProcessIo> {
    let mut c = ffi::IoCounters::default();
    // SAFETY: the pseudo-handle of this process is always valid; `c` is a
    // writable IO_COUNTERS.
    let ok = unsafe { ffi::GetProcessIoCounters(ffi::GetCurrentProcess(), &raw mut c) };
    if ok == 0 {
        return Err(os_error("process I/O", "GetProcessIoCounters"));
    }
    Ok(ProcessIo {
        read: Bytes::new(c.read_transfer_count),
        written: Bytes::new(c.write_transfer_count),
        read_ops: Some(c.read_operation_count),
        write_ops: Some(c.write_operation_count),
        storage_only: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_windows_job_limits_follow_flags() {
        assert_eq!(job_process_limits(0, 5, 1 << 30), Limits::default());
        let l = job_process_limits(
            ffi::JOB_OBJECT_LIMIT_ACTIVE_PROCESS | ffi::JOB_OBJECT_LIMIT_PROCESS_MEMORY,
            5,
            1 << 30,
        );
        assert_eq!((l.processes.soft, l.processes.hard), (Some(5), Some(5)));
        assert_eq!(l.address_space.hard, Some(1 << 30));
        assert_eq!(l.open_files, Limit::default());
        let l = job_process_limits(ffi::JOB_OBJECT_LIMIT_PROCESS_MEMORY, 0, 0);
        assert_eq!(l.address_space, Limit::default());
    }

    #[test]
    fn test_process_windows_live_readings() {
        let parent = parent_id().unwrap();
        assert!(parent != 0 && parent != std::process::id());
        assert_eq!(parent_id(), Some(parent));
        let rss = process_memory().unwrap();
        assert!(rss.raw() > 0 && peak_memory().unwrap() >= rss);
        let d = memory_detail().unwrap();
        assert!(d.virtual_size >= d.resident);
        assert!(d.private.is_some_and(|p| p.raw() > 0));
        let t = cpu_time().unwrap();
        assert!(t.total() >= t.user);
        let spawned = std::thread::spawn(threads);
        let with_two = spawned.join().unwrap().unwrap();
        assert!(with_two >= 2, "{with_two}");
        assert!(threads().unwrap() >= 1);
        assert!(open_files().unwrap() >= 1);
        let started = started_at().unwrap();
        assert!(started <= SystemTime::now());
        assert!(SystemTime::now().duration_since(started).unwrap().as_secs() < 24 * 3600);
        let io = process_io().unwrap();
        assert!(!io.storage_only && io.read_ops.is_some());
        let l = limits().unwrap();
        assert_eq!(
            l.address_space.hard.is_some(),
            cfg!(target_pointer_width = "32")
                || job_limits().is_some_and(|j| {
                    j.basic_limit_information.limit_flags & ffi::JOB_OBJECT_LIMIT_PROCESS_MEMORY
                        != 0
                })
        );
    }
}
