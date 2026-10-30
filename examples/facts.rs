//! Prints every fact and live reading sys-lib reads about this machine.
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

    // Live readings (0.4): each is read now, and an error means unknown here.
    let c = sys::cpu::count();
    println!(
        "cpus      {} packages, {} online, quota {:?}",
        c.packages,
        c.online,
        sys::cpu::quota().ok().flatten()
    );
    println!(
        "  freq    {:?} now {:?}",
        sys::cpu::frequency(),
        sys::cpu::current_frequency().ok().flatten()
    );
    println!(
        "  load    {:?}",
        sys::cpu::load().ok().flatten().map(|l| l.to_string())
    );
    println!("  feats   {}", sys::cpu::features());
    if let Ok(m) = sys::mem::snapshot() {
        println!(
            "memory    {} of {} used ({}), source {:?}",
            m.used,
            m.total,
            m.used_percent(),
            m.source
        );
        println!(
            "  host    {} free of {}; swap {} of {}",
            m.host_free, m.host_total, m.swap.used, m.swap.total
        );
        if let Some(l) = m.limit {
            println!("  limit   {} ({:?})", l.hard, l.source);
        }
    }
    for (what, p) in [
        ("cpu", sys::cpu::pressure()),
        ("memory", sys::mem::pressure()),
        ("io", sys::disk::pressure()),
    ] {
        if let Ok(Some(p)) = p {
            println!(
                "pressure  {what}: {:?}, {} over 10 s",
                p.level, p.some.avg10
            );
        }
    }
    if let Ok(disks) = sys::disk::list() {
        for d in disks {
            let space = d
                .space
                .map(|s| format!("{} free of {}", s.available, s.total))
                .unwrap_or_default();
            println!(
                "disk      {} ({}, {:?}) {space}",
                d.mount_point.display(),
                d.filesystem,
                d.kind
            );
        }
    }
    if let Ok(list) = sys::net::interfaces() {
        for i in list {
            let mac = i.mac.map(|m| m.to_string()).unwrap_or_default();
            println!(
                "net       {} {:?} up={} {mac} {:?}",
                i.name,
                i.kind,
                i.running,
                i.addresses
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
            );
        }
    }
    println!("connections {:?}", sys::net::connections().ok());
    println!(
        "uptime    {:?}, {:?}",
        sys::host::uptime().ok(),
        sys::host::processes().ok()
    );
    if let Ok(info) = sys::process::info() {
        println!(
            "this      rss {} threads {:?} files {:?}",
            info.memory.resident, info.threads, info.open_files
        );
    }
}
