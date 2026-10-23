//! 64 threads read every static fact at once, at first use, and all see the
//! same value (PLAN.md 19.1). One test in this file, so first use really is
//! first: nothing else in this process has read a fact before it.

use std::sync::{Arc, Barrier};

use sys_lib::{cpu, host, os, process};

fn snapshot() -> String {
    format!(
        "{:?}",
        (
            (
                os::info(),
                os::name(),
                os::version(),
                os::kernel(),
                os::distro(),
                os::distro_id()
            ),
            (
                cpu::logical(),
                cpu::physical(),
                cpu::name(),
                cpu::raw_name(),
                cpu::vendor()
            ),
            (cpu::is_64bit(), cpu::arch()),
            (
                host::name().ok(),
                process::user().ok(),
                process::is_elevated().ok()
            ),
        )
    )
}

#[test]
fn test_concurrency_64_threads_at_first_use_see_one_value() {
    const THREADS: usize = 64;
    let start = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let start = Arc::clone(&start);
            std::thread::spawn(move || {
                start.wait();
                // Static facts are cached: every thread gets the same &'static.
                let name: *const str = os::name();
                (snapshot(), name.addr())
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let (first, addr) = &results[0];
    for (snap, a) in &results {
        assert_eq!(snap, first);
        assert_eq!(a, addr, "os::name() was computed more than once");
    }
}
