//! A program that uses every module, for the binary-size measurement against
//! `baseline` (dev/PLAN.md section 18).

use std::hint::black_box;

use sys_lib as sys;

fn main() {
    println!("{}", sys::summary());
    println!("{}", sys::readiness::report(sys::readiness::Profile::Database));
    println!("{}", sys::bench::environment());
    println!("{}", sys::security::posture());
    black_box((sys::os::info(), sys::cpu::topology(), sys::mem::snapshot().ok()));
    black_box((sys::disk::list().ok(), sys::net::interfaces().ok(), sys::host::sessions().ok()));
    black_box((sys::process::info().ok(), sys::virt::detect(), sys::power::batteries().ok()));
    black_box((sys::sensors::temperatures().ok(), sys::gpu::list().ok(), sys::net::dns().ok()));
    black_box((sys::device::id("com.example.footprint").ok(), sys::device::report()));
    black_box(sys::process::guard("footprint").ok());
    black_box(sys::Instance::start());
}
