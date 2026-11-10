//! The 0.6 reports: server readiness, the benchmark environment, the security
//! posture, temperatures, and graphics adapters.
//!
//! ```text
//! cargo run --example reports [data-directory]
//! ```

use sys_lib::{
    bench, gpu,
    readiness::{self, Profile},
    security, sensors,
};

fn main() {
    let data_dir = std::env::args_os()
        .nth(1)
        .map_or_else(std::env::temp_dir, Into::into);

    for profile in [
        Profile::General,
        Profile::Database,
        Profile::WebServer,
        Profile::Benchmark,
    ] {
        let report = readiness::report_for(profile, &data_dir);
        println!(
            "== readiness: {} ({})",
            profile.name(),
            if report.passed() { "passed" } else { "failed" }
        );
        println!("{report}\n");
    }

    let env = bench::environment();
    println!("== benchmark environment\n{env}\n");

    println!("== security posture\n{}\n", security::posture());

    println!("== temperatures");
    match sensors::temperatures() {
        Ok(list) if list.is_empty() => println!("none reported"),
        Ok(list) => {
            for t in list {
                println!(
                    "{:<24}{:<8}{:>6.1} C  high {:?} critical {:?}",
                    t.label,
                    t.kind.name(),
                    t.celsius,
                    t.high,
                    t.critical
                );
            }
        }
        Err(e) => println!("{e}"),
    }

    println!("\n== graphics adapters");
    match gpu::list() {
        Ok(list) if list.is_empty() => println!("none reported"),
        Ok(list) => {
            for g in list {
                println!(
                    "{} ({}, {:?}) memory {:?} driver {:?}",
                    g.name,
                    g.vendor.name(),
                    g.kind,
                    g.memory,
                    g.driver
                );
            }
        }
        Err(e) => println!("{e}"),
    }
}
