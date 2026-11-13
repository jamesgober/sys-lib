//! No reading holds a descriptor or handle between calls (PLAN.md section 18:
//! 0 descriptors held; only the process guard keeps its lock file open). Every
//! reading is called twice, then this process's open descriptors (Unix) or
//! handles (Windows) are counted with sys-lib's own reading and must be what
//! they were before.
//!
//! One test in this file, so no other test opens files while it counts.

use std::time::Duration;

use sys_lib as sys;

fn every_reading() {
    let _ = (sys::os::info(), sys::os::is_64bit(), sys::os::form_factor());
    let _ = (sys::cpu::count(), sys::cpu::name(), sys::cpu::topology());
    let _ = (sys::cpu::load(), sys::cpu::times(), sys::cpu::frequency());
    let _ = (
        sys::cpu::current_frequency(),
        sys::cpu::quota(),
        sys::cpu::pressure(),
    );
    let _ = (
        sys::mem::snapshot(),
        sys::mem::activity(),
        sys::mem::events(),
    );
    let _ = (
        sys::mem::huge_pages(),
        sys::mem::modules(),
        sys::mem::pressure(),
    );
    let _ = (sys::disk::list(), sys::disk::of("."), sys::disk::space("."));
    let _ = (
        sys::disk::io_totals(),
        sys::disk::io_counters(),
        sys::disk::pressure(),
    );
    let _ = (
        sys::net::interfaces(),
        sys::net::totals(),
        sys::net::counters(),
    );
    let _ = (
        sys::net::connections(),
        sys::net::primary(),
        sys::net::dns(),
    );
    let _ = (
        sys::host::name(),
        sys::host::uptime(),
        sys::host::processes(),
    );
    let _ = (
        sys::host::open_files(),
        sys::host::timezone(),
        sys::host::sessions(),
    );
    let _ = (
        sys::host::clock_sync(),
        sys::host::model(),
        sys::host::firmware(),
    );
    let _ = (
        sys::process::user(),
        sys::process::is_elevated(),
        sys::process::limits(),
    );
    let _ = (
        sys::process::memory_detail(),
        sys::process::cpu_time(),
        sys::process::io(),
    );
    let _ = (sys::process::threads(), sys::process::launch());
    let _ = (sys::virt::detect(), sys::virt::cloud());
    let _ = (sys::power::batteries(), sys::power::on_ac());
    let _ = (sys::sensors::temperatures(), sys::gpu::list());
    let _ = (
        sys::security::posture(),
        sys::bench::environment(),
        sys::summary(),
    );
    let _ = sys::readiness::report_for(sys::readiness::Profile::Database, ".");
    #[cfg(feature = "device-id")]
    let _ = (
        sys::device::id("com.example.descriptors"),
        sys::device::report(),
    );
    let mut cpu = sys::cpu::Sampler::new().unwrap();
    let mut disk = sys::disk::IoSampler::new().unwrap();
    let mut net = sys::net::Sampler::new().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let _ = (cpu.sample(), disk.sample(), net.sample());
}

#[test]
fn test_no_reading_holds_a_descriptor() {
    // The first pass opens what std and the OS open once for a process (the
    // loader's libraries, std's random seed): those stay, and are not
    // sys-lib's. The second pass must open nothing that outlives it.
    every_reading();
    let before = sys::process::open_files().unwrap();
    every_reading();
    every_reading();
    let after = sys::process::open_files().unwrap();
    assert!(
        after <= before,
        "descriptors or handles grew from {before} to {after}"
    );
}
