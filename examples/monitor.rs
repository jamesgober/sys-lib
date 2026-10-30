//! A monitor loop built on the samplers: CPU, memory paging, disk, and
//! network, every second, for five seconds. Each sampler is owned by the loop,
//! never blocks, and allocates nothing per sample.
//!
//! ```text
//! cargo run --example monitor
//! ```

use std::time::Duration;

use sys_lib as sys;

fn main() -> Result<(), sys::Error> {
    let mut cpu = sys::cpu::Sampler::new()?;
    let mut net = sys::net::Sampler::new()?;
    let mut disk = sys::disk::IoSampler::new().ok();
    let mut paging = sys::mem::Sampler::new().ok();
    let mut me = sys::process::CpuSampler::new()?;
    for _ in 0..5 {
        std::thread::sleep(Duration::from_secs(1));
        let u = cpu.sample()?;
        let t = net.sample()?;
        let used = sys::mem::used_percent()?;
        print!(
            "cpu {} (user {}, system {}) | mem {used} | down {} up {}",
            u.used(),
            u.user,
            u.system,
            t.download,
            t.upload
        );
        if let Some(d) = disk.as_mut().and_then(|d| d.sample().ok()) {
            print!(" | disk r {} w {}", d.read, d.write);
        }
        if let Some(a) = paging.as_mut().and_then(|p| p.sample().ok()) {
            print!(" | swap out {}", a.swap_out);
        }
        println!(" | this {:.1}%", me.sample()?.of_one_core);
    }
    Ok(())
}
