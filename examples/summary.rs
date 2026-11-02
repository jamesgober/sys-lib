//! The facts for a start-up log line, as key-value pairs and as JSON.
//!
//! ```text
//! cargo run --example summary
//! ```

use sys_lib as sys;

fn main() -> Result<(), std::fmt::Error> {
    let summary = sys::summary();
    println!("{summary}");

    let mut json = String::new();
    summary.write_json(&mut json)?;
    println!("{json}");

    let v = sys::virt::detect();
    println!("cloud: {:?}", sys::virt::cloud());
    for e in &v.evidence {
        println!("  {} ({:?})", e.check, e.strength);
    }
    let d = sys::os::detect_form_factor();
    println!("form factor {} (from {})", d.value.name(), d.source);
    Ok(())
}
