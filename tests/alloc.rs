//! Every scalar live reading and every sampler step allocates nothing after
//! warm-up (PLAN.md 19.1, the counting-allocator layer). A global allocator
//! counts allocations made on this test's own thread while counting is on;
//! each reading is warmed up once (static facts, cached paths), then called
//! again with counting on, and must have allocated zero times.
//!
//! One test in this file, so no other test's thread allocates under the count.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    time::Duration,
};

use sys_lib::{self as sys, Percent};

struct Counting;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: every call is forwarded to `System` unchanged; the counter is a
// thread-local `Cell` that never allocates.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's contract is `System::alloc`'s.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract is `System::dealloc`'s.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's contract is `System::alloc_zeroed`'s.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: the caller's contract is `System::realloc`'s.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

fn note() {
    // `try_with`: the thread-locals may be gone while a thread exits.
    let on = ON.try_with(Cell::get).unwrap_or(false);
    if on {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Allocations `f` made on this thread.
fn allocations<T>(f: impl FnOnce() -> T) -> (u64, T) {
    COUNT.with(|c| c.set(0));
    ON.with(|o| o.set(true));
    let value = f();
    ON.with(|o| o.set(false));
    (COUNT.with(Cell::get), value)
}

/// Warms `f` up, then checks a second call allocates nothing. Readings an OS
/// does not offer (`Unsupported`, hidden files) are skipped, not failed.
fn check<T, E: std::fmt::Display>(
    name: &str,
    mut f: impl FnMut() -> Result<T, E>,
    failures: &mut Vec<String>,
) {
    if let Err(e) = f() {
        eprintln!("skipped {name}: {e}");
        return;
    }
    let (n, _) = allocations(&mut f);
    if n != 0 {
        failures.push(format!("{name}: {n} allocations"));
    }
}

#[test]
fn test_alloc_scalar_readings_and_samplers_allocate_nothing() {
    let mut failures = Vec::new();
    let here = std::env::temp_dir();
    check("mem::total", sys::mem::total, &mut failures);
    check("mem::used", sys::mem::used, &mut failures);
    check("mem::free", sys::mem::free, &mut failures);
    check("mem::used_percent", sys::mem::used_percent, &mut failures);
    check("mem::snapshot", sys::mem::snapshot, &mut failures);
    check("mem::swap", sys::mem::swap, &mut failures);
    check("cpu::times", sys::cpu::times, &mut failures);
    check("cpu::load", sys::cpu::load, &mut failures);
    check("process::memory", sys::process::memory, &mut failures);
    check(
        "process::peak_memory",
        sys::process::peak_memory,
        &mut failures,
    );
    check("process::cpu_time", sys::process::cpu_time, &mut failures);
    check(
        "process::open_files",
        sys::process::open_files,
        &mut failures,
    );
    check("process::limits", sys::process::limits, &mut failures);
    check("process::io", sys::process::io, &mut failures);
    if !cfg!(windows) {
        // Windows walks a system-wide thread snapshot (documented).
        check("process::threads", sys::process::threads, &mut failures);
    }
    check("disk::space", || sys::disk::space(&here), &mut failures);
    check("net::totals", sys::net::totals, &mut failures);
    check("host::uptime", sys::host::uptime, &mut failures);

    // Samplers: one sample after warm-up allocates nothing.
    if let Ok(mut s) = sys::cpu::Sampler::new() {
        let mut per_core = vec![Percent::ZERO; sys::cpu::logical()];
        std::thread::sleep(Duration::from_millis(25));
        let _ = s.sample();
        let _ = s.sample_per_core_into(&mut per_core);
        std::thread::sleep(Duration::from_millis(25));
        let (n, _) = allocations(|| s.sample());
        if n != 0 {
            failures.push(format!("cpu::Sampler::sample: {n} allocations"));
        }
        std::thread::sleep(Duration::from_millis(25));
        let (n, r) = allocations(|| s.sample_per_core_into(&mut per_core));
        if n != 0 && r.is_ok() {
            failures.push(format!(
                "cpu::Sampler::sample_per_core_into: {n} allocations"
            ));
        }
    }
    let mut sample = |name: &str, n: u64| {
        if n != 0 {
            failures.push(format!("{name}: {n} allocations"));
        }
    };
    if let Ok(mut s) = sys::net::Sampler::new() {
        std::thread::sleep(Duration::from_millis(2));
        let _ = s.sample();
        std::thread::sleep(Duration::from_millis(2));
        sample("net::Sampler::sample", allocations(|| s.sample()).0);
    }
    if let Ok(mut s) = sys::disk::IoSampler::new() {
        std::thread::sleep(Duration::from_millis(2));
        let _ = s.sample();
        std::thread::sleep(Duration::from_millis(2));
        sample("disk::IoSampler::sample", allocations(|| s.sample()).0);
    }
    if let Ok(mut s) = sys::mem::Sampler::new() {
        std::thread::sleep(Duration::from_millis(2));
        let _ = s.sample();
        std::thread::sleep(Duration::from_millis(2));
        sample("mem::Sampler::sample", allocations(|| s.sample()).0);
    }
    if let Ok(mut s) = sys::process::CpuSampler::new() {
        std::thread::sleep(Duration::from_millis(2));
        let _ = s.sample();
        std::thread::sleep(Duration::from_millis(2));
        sample("process::CpuSampler::sample", allocations(|| s.sample()).0);
    }
    assert!(
        failures.is_empty(),
        "readings that allocated:\n{}",
        failures.join("\n")
    );
}
