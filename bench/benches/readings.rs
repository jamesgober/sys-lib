//! Warm numbers: each reading in a criterion loop, next to what a caller would
//! write with the competing crate to get the same answer (dev/PLAN.md section
//! 18). Each group is one reading; each function in it is one implementation.

use std::{hint::black_box, time::Duration};

use criterion::{Criterion, criterion_group, criterion_main};
use sys_lib as sys;
use sysinfo::{
    Disks, MemoryRefreshKind, Networks, Pid, ProcessRefreshKind, ProcessesToUpdate, RefreshKind,
    System,
};

fn static_facts(c: &mut Criterion) {
    let mut g = c.benchmark_group("static_fact");
    let _ = sys::os::name();
    g.bench_function("sys-lib os::name", |b| {
        b.iter(|| black_box(sys::os::name()))
    });
    g.bench_function("sysinfo System::name", |b| {
        b.iter(|| black_box(System::name().ok()))
    });
    g.finish();

    let mut g = c.benchmark_group("cpu_physical");
    let _ = sys::cpu::physical();
    g.bench_function("sys-lib cpu::physical", |b| {
        b.iter(|| black_box(sys::cpu::physical()))
    });
    g.bench_function("num_cpus get_physical", |b| {
        b.iter(|| black_box(num_cpus::get_physical()))
    });
    g.finish();

    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        let mut g = c.benchmark_group("cpu_has_avx2");
        let _ = sys::cpu::has(sys::cpu::Feature::Avx2);
        g.bench_function("sys-lib cpu::has", |b| {
            b.iter(|| black_box(sys::cpu::has(black_box(sys::cpu::Feature::Avx2))))
        });
        g.bench_function("raw-cpuid has_avx2", |b| {
            b.iter(|| {
                black_box(
                    raw_cpuid::CpuId::new()
                        .get_extended_feature_info()
                        .is_some_and(|f| f.has_avx2()),
                )
            })
        });
        g.finish();
    }
}

fn memory(c: &mut Criterion) {
    let mut g = c.benchmark_group("mem_snapshot");
    g.bench_function("sys-lib mem::snapshot", |b| {
        b.iter(|| black_box(sys::mem::snapshot().unwrap()))
    });
    // The equal answer: totals, available, swap, and the container's limit.
    let mut s = System::new_with_specifics(RefreshKind::nothing()).unwrap();
    g.bench_function("sysinfo refresh_memory + cgroup_limits", |b| {
        b.iter(|| {
            s.refresh_memory_specifics(MemoryRefreshKind::everything());
            black_box((
                s.total_memory(),
                s.available_memory(),
                s.total_swap(),
                s.cgroup_limits(),
            ))
        })
    });
    g.finish();
}

fn cpu_usage(c: &mut Criterion) {
    let mut g = c.benchmark_group("cpu_sample");
    let mut sampler = sys::cpu::Sampler::new().unwrap();
    // A sample taken sooner than one OS tick is TooSoon, after the same reads.
    g.bench_function("sys-lib Sampler::sample", |b| {
        b.iter(|| black_box(sampler.sample().ok()))
    });
    // sysinfo skips the read when called within its minimum interval
    // (200 ms), so each timed refresh follows a wait outside the timing:
    // the same work, a fresh reading, on both sides.
    let mut s = System::new_with_specifics(RefreshKind::nothing()).unwrap();
    g.sample_size(10);
    g.bench_function("sysinfo refresh_cpu_usage + global", |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters.min(8) {
                std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
                let start = std::time::Instant::now();
                s.refresh_cpu_usage();
                black_box(s.global_cpu_usage());
                total += start.elapsed();
            }
            total * u32::try_from(iters.div_ceil(iters.min(8))).unwrap_or(u32::MAX)
        })
    });
    g.finish();
}

fn process(c: &mut Criterion) {
    let mut g = c.benchmark_group("process_memory");
    g.bench_function("sys-lib process::memory", |b| {
        b.iter(|| black_box(sys::process::memory().unwrap()))
    });
    let pid = Pid::from_u32(std::process::id());
    let mut s = System::new_with_specifics(RefreshKind::nothing()).unwrap();
    g.bench_function("sysinfo refresh this pid", |b| {
        b.iter(|| {
            s.refresh_processes_specifics(
                ProcessesToUpdate::Some(&[pid]),
                false,
                ProcessRefreshKind::nothing().with_memory(),
            );
            black_box(s.process(pid).map(|p| p.memory()))
        })
    });
    g.finish();
}

fn disks_and_network(c: &mut Criterion) {
    let mut g = c.benchmark_group("disk_list");
    g.bench_function("sys-lib disk::list", |b| {
        b.iter(|| black_box(sys::disk::list().unwrap()))
    });
    g.bench_function("sysinfo Disks::new_with_refreshed_list", |b| {
        b.iter(|| black_box(Disks::new_with_refreshed_list().ok()))
    });
    g.finish();

    let mut g = c.benchmark_group("net_interfaces");
    g.bench_function("sys-lib net::interfaces", |b| {
        b.iter(|| black_box(sys::net::interfaces().unwrap()))
    });
    g.bench_function("sysinfo Networks::new_with_refreshed_list", |b| {
        b.iter(|| black_box(Networks::new_with_refreshed_list().ok()))
    });
    g.finish();

    let mut g = c.benchmark_group("disk_io");
    let mut io = sys::disk::IoSampler::new().unwrap();
    g.bench_function("sys-lib IoSampler::sample", |b| {
        b.iter(|| black_box(io.sample().ok()))
    });
    let mut disks = Disks::new_with_refreshed_list().unwrap();
    g.bench_function("sysinfo Disks::refresh (usage)", |b| {
        b.iter(|| {
            disks.refresh(false);
            black_box(disks.iter().map(|d| d.usage().read_bytes).sum::<u64>())
        })
    });
    g.finish();
}

fn host(c: &mut Criterion) {
    let mut g = c.benchmark_group("host_uptime");
    g.bench_function("sys-lib host::uptime", |b| {
        b.iter(|| black_box(sys::host::uptime().unwrap()))
    });
    g.bench_function("sysinfo System::uptime", |b| {
        b.iter(|| black_box(System::uptime().ok()))
    });
    g.finish();

    let mut g = c.benchmark_group("host_processes");
    g.bench_function("sys-lib host::processes", |b| {
        b.iter(|| black_box(sys::host::processes().unwrap()))
    });
    let mut s = System::new_with_specifics(RefreshKind::nothing()).unwrap();
    g.bench_function("sysinfo refresh_processes count", |b| {
        b.iter(|| {
            s.refresh_processes_specifics(
                ProcessesToUpdate::All,
                true,
                ProcessRefreshKind::nothing(),
            );
            black_box(s.processes().len())
        })
    });
    g.finish();
}

fn device_id(c: &mut Criterion) {
    let mut g = c.benchmark_group("device_id");
    g.bench_function("sys-lib device::id", |b| {
        b.iter(|| black_box(sys::device::id("com.example.bench").unwrap()))
    });
    // machine-uid gives the raw id; an application keys it per application,
    // as sys-lib does on every call: the key is derived inside the loop too.
    g.bench_function("machine-uid get + keyed blake3", |b| {
        b.iter(|| {
            let key = blake3::derive_key("sys-lib-bench app key", b"com.example.bench");
            let id = machine_uid::get().unwrap();
            black_box(blake3::keyed_hash(&key, id.as_bytes()))
        })
    });
    g.finish();
}

/// The 0.6 reports: no competitor; measured so a regression shows.
fn reports(c: &mut Criterion) {
    let mut g = c.benchmark_group("reports");
    g.sample_size(20);
    g.bench_function("readiness::report(Database)", |b| {
        b.iter(|| black_box(sys::readiness::report(sys::readiness::Profile::Database)))
    });
    g.bench_function("security::posture", |b| {
        b.iter(|| black_box(sys::security::posture()))
    });
    g.bench_function("summary", |b| b.iter(|| black_box(sys::summary())));
    g.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2));
    targets = static_facts, memory, cpu_usage, process, disks_and_network, host, device_id, reports
}
criterion_main!(benches);
