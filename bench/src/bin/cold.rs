//! Cold numbers: the first call in a fresh process, which is what a service
//! pays at start-up and what a cached `OnceLock` cannot hide. Each sample is a
//! new process running one case once (dev/PLAN.md section 18).
//!
//! ```text
//! cargo run --release --bin cold [samples]
//! ```

use std::{hint::black_box, process::Command, time::Instant};

use sys_lib as sys;
use sysinfo::{MemoryRefreshKind, RefreshKind, System};

type Case = (&'static str, &'static str, fn());

const CASES: &[Case] = &[
    ("os name", "sys-lib os::name", || {
        black_box(sys::os::name());
    }),
    ("os name", "os_info get", || {
        black_box(os_info::get());
    }),
    ("os name", "sysinfo System::name", || {
        black_box(System::name().ok());
    }),
    ("cpu physical", "sys-lib cpu::physical", || {
        black_box(sys::cpu::physical());
    }),
    ("cpu physical", "num_cpus get_physical", || {
        black_box(num_cpus::get_physical());
    }),
    ("cpu logical", "sys-lib cpu::logical", || {
        black_box(sys::cpu::logical());
    }),
    ("cpu logical", "num_cpus get", || {
        black_box(num_cpus::get());
    }),
    ("mem snapshot", "sys-lib mem::snapshot", || {
        black_box(sys::mem::snapshot().ok());
    }),
    ("mem snapshot", "sysinfo memory + cgroup_limits", || {
        let mut s = System::new_with_specifics(RefreshKind::nothing()).unwrap();
        s.refresh_memory_specifics(MemoryRefreshKind::everything());
        black_box((s.total_memory(), s.available_memory(), s.cgroup_limits()));
    }),
    ("device id", "sys-lib device::id", || {
        black_box(sys::device::id("com.example.bench").ok());
    }),
    ("device id", "machine-uid get + keyed blake3", || {
        let key = blake3::derive_key("sys-lib-bench app key", b"com.example.bench");
        let id = machine_uid::get().unwrap_or_default();
        black_box(blake3::keyed_hash(&key, id.as_bytes()));
    }),
    ("summary", "sys-lib summary", || {
        black_box(sys::summary());
    }),
];

fn percentile(sorted: &[u128], p: usize) -> u128 {
    sorted[(sorted.len() - 1) * p / 100]
}

fn main() {
    let mut args = std::env::args().skip(1);
    if let Some(flag) = args.next() {
        if flag == "--one" {
            // A child: run one case, print its time in nanoseconds.
            let i: usize = args.next().and_then(|a| a.parse().ok()).unwrap();
            let start = Instant::now();
            (CASES[i].2)();
            println!("{}", start.elapsed().as_nanos());
            return;
        }
    }
    let samples: usize = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(200);
    let me = std::env::current_exe().unwrap();
    println!(
        "{:<14} {:<34} {:>10} {:>10} {:>10}",
        "reading", "implementation", "median µs", "p90 µs", "min µs"
    );
    for (i, (reading, name, _)) in CASES.iter().enumerate() {
        let mut times: Vec<u128> = (0..samples)
            .map(|_| {
                let out = Command::new(&me)
                    .args(["--one", &i.to_string()])
                    .output()
                    .unwrap();
                String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
            })
            .collect();
        times.sort_unstable();
        let us = |ns: u128| ns as f64 / 1000.0;
        println!(
            "{:<14} {:<34} {:>10.1} {:>10.1} {:>10.1}",
            reading,
            name,
            us(percentile(&times, 50)),
            us(percentile(&times, 90)),
            us(times[0])
        );
    }
}
