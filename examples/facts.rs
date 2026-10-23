//! Prints every fact sys-lib 0.3 reads about this machine.
//!
//! ```text
//! cargo run --example facts
//! ```

use sys_lib as sys;

fn main() {
    let os = sys::os::info();
    println!("os        {}", sys::os::name());
    println!("  kind    {:?}", sys::os::kind());
    println!("  distro  {} ({})", sys::os::distro(), sys::os::distro_id());
    match sys::os::version() {
        Some(v) => println!(
            "  version {v} ({}.{}.{} build {:?})",
            v.major, v.minor, v.patch, v.build
        ),
        None => println!("  version unknown"),
    }
    println!("  kernel  {}", sys::os::kernel());
    if let Some(r) = sys::os::os_release() {
        println!("  like    {:?}", r.id_like);
    }
    println!("  source  {:?}", os.name_source);

    let arch = sys::cpu::arch();
    println!("cpu       {}", sys::cpu::name());
    println!("  raw     {:?}", sys::cpu::raw_name());
    println!("  vendor  {}", sys::cpu::vendor().name());
    println!(
        "  arch    {} (build is {}-bit; processor 64-bit: {})",
        arch.name(),
        if arch.is_64bit() { 64 } else { 32 },
        sys::cpu::is_64bit()
    );
    println!(
        "  counts  {} logical, {} physical, {} available to this process",
        sys::cpu::logical(),
        sys::cpu::physical(),
        sys::cpu::available()
    );

    match sys::host::name() {
        Ok(name) => println!("host      {name}"),
        Err(e) => println!("host      unknown: {e}"),
    }
    match sys::process::user() {
        Ok(u) => println!("user      {} (id {:?}, admin {})", u.name, u.id, u.is_admin),
        Err(e) => println!("user      unknown: {e}"),
    }
    println!(
        "process   {} (elevated: {:?})",
        sys::process::id(),
        sys::process::is_elevated().ok()
    );
    println!("build     {}", sys::build_info!());
}
