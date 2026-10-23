//! The boot kit, as a long-running service uses it in its first milliseconds:
//! the guard before any file is opened, then build and instance info on the
//! first log line, then a worker pool sized to what this process may use.
//!
//! ```text
//! cargo run --example boot
//! ```
//!
//! A second copy started while the first runs is refused with the first
//! copy's process id.

use sys_lib as sys;

fn main() -> Result<(), sys::Error> {
    let guard = sys::process::guard("sys-lib-boot-example")?;
    let build = sys::build_info!();
    let instance = sys::Instance::start();

    println!("{build}");
    println!("{instance}");
    let report = guard.report();
    println!(
        "lock      {:?} on {:?}",
        report.lock_file, report.lock_filesystem
    );
    println!(
        "umask     {:?} -> {:?}",
        report.umask_before, report.umask_after
    );
    println!(
        "open files {:?} -> {:?}",
        report.open_files_before, report.open_files_after
    );
    for warning in &report.warnings {
        println!("warning   {warning:?}");
    }

    let workers = sys::cpu::available();
    println!(
        "workers   {workers} (of {} logical CPUs)",
        sys::cpu::logical()
    );
    println!("public    {}", build.public());

    std::thread::sleep(std::time::Duration::from_secs(2));
    Ok(())
}
