//! The benchmark environment: what a result needs printed next to it so it can
//! be trusted and repeated.
//!
//! Whether the CPU frequency could wander (the governor, turbo), whether the
//! machine was busy, on battery, throttled, or a VM, and whether the timer is
//! stable. Benchmark harnesses print it at the start of a run. Read-only, like
//! the [`readiness`](crate::readiness) report.
//!
//! ```
//! use sys_lib::bench;
//!
//! let env = bench::environment();
//! println!("{env}");
//! if !env.is_reproducible() {
//!     eprintln!("results may be noisy: {:?}", env.warnings);
//! }
//! ```

use core::{fmt, time::Duration};

use crate::cpu::Load;

/// The machine, as a benchmark sees it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Environment {
    /// The CPU frequency governor (`"performance"`, `"powersave"`,
    /// `"schedutil"`), or the Windows power plan's name.
    pub governor: Option<String>,
    /// Turbo or boost is enabled.
    pub turbo: Option<bool>,
    /// Simultaneous multithreading (hyper-threading) is active.
    pub smt: Option<bool>,
    /// The time-stamp counter runs at a constant rate (x86 only).
    pub invariant_tsc: Option<bool>,
    /// The kernel's clock source (Linux): `"tsc"`, `"hpet"`.
    pub clock_source: Option<String>,
    /// The CPU was throttled for heat or power: since boot on Linux (the
    /// kernel's counters), right now on Windows.
    pub throttled: Option<bool>,
    /// The CPUs kept away from the scheduler (Linux `isolcpus`, `nohz_full`),
    /// as the kernel lists them: `"2-3"`.
    pub isolated_cpus: Option<String>,
    /// Address-space randomisation is on: run-to-run layout noise.
    pub aslr: Option<bool>,
    /// On mains power.
    pub on_ac: Option<bool>,
    /// The load average, where the OS keeps one.
    pub load: Option<Load>,
    /// Running in a virtual machine.
    pub virtualized: bool,
    /// What makes results noisy, in the order of the variants.
    pub warnings: Vec<Warning>,
}

/// Something that makes benchmark results noisy.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum Warning {
    /// The governor or power plan lowers the frequency when idle.
    PowerSaving,
    /// Turbo is on: the frequency depends on temperature and on other cores.
    TurboEnabled,
    /// The CPU has been throttled.
    Throttled,
    /// On battery, where the OS saves power.
    OnBattery,
    /// Other work is running: the load is over a quarter of the CPUs.
    Busy,
    /// A VM: other guests share the hardware.
    Virtualized,
    /// The clock is slow to read or drifts with the frequency.
    UnstableClock,
}

impl Warning {
    /// A short lowercase name: `"power-saving"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::PowerSaving => "power-saving",
            Self::TurboEnabled => "turbo",
            Self::Throttled => "throttled",
            Self::OnBattery => "on-battery",
            Self::Busy => "busy",
            Self::Virtualized => "virtualized",
            Self::UnstableClock => "unstable-clock",
        }
    }
}

/// What a platform reports for [`environment`]; the rest comes from the
/// public modules.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct BenchFacts {
    pub(crate) governor: Option<String>,
    /// The governor or plan lets the frequency drop (anything but Linux
    /// `performance` or a Windows high-performance plan).
    pub(crate) power_saving: Option<bool>,
    pub(crate) turbo: Option<bool>,
    pub(crate) smt: Option<bool>,
    pub(crate) clock_source: Option<String>,
    pub(crate) throttled: Option<bool>,
    pub(crate) isolated_cpus: Option<String>,
    pub(crate) aslr: Option<bool>,
}

/// The load per CPU above which the machine counts as busy.
const BUSY: f32 = 0.25;
/// How long [`environment`] samples the CPU where there is no load average.
const BUSY_WINDOW: Duration = Duration::from_millis(100);

/// Clock sources that are slow to read or not constant-rate.
pub(crate) fn slow_clock(source: &str) -> bool {
    matches!(source, "hpet" | "acpi_pm" | "jiffies" | "pit" | "xen")
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
fn invariant_tsc() -> Option<bool> {
    use std::sync::OnceLock;
    static TSC: OnceLock<Option<bool>> = OnceLock::new();
    *TSC.get_or_init(crate::sys::x86_invariant_tsc)
}

#[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
fn invariant_tsc() -> Option<bool> {
    None
}

/// The benchmark environment, read now. Never fails: each value the OS does
/// not answer is `None`. Where the OS keeps no load average (Windows), the CPU
/// is sampled for 100 ms to tell whether it is busy, so the call takes that
/// long; elsewhere it costs 0.1 to 1 ms [est.].
#[must_use]
pub fn environment() -> Environment {
    let facts = crate::sys::bench_facts();
    let load = crate::cpu::load().ok().flatten();
    let busy = match load {
        Some(l) => Some(l.per_cpu().one > BUSY),
        None => crate::cpu::usage_over(BUSY_WINDOW)
            .ok()
            .map(|u| u.used().value() > BUSY * 100.0),
    };
    let on_ac = crate::power::on_ac().ok().flatten();
    assess(
        facts,
        invariant_tsc(),
        load,
        busy,
        on_ac,
        crate::virt::is_vm(),
    )
}

fn assess(
    facts: BenchFacts,
    invariant_tsc: Option<bool>,
    load: Option<Load>,
    busy: Option<bool>,
    on_ac: Option<bool>,
    virtualized: bool,
) -> Environment {
    let mut warnings = Vec::new();
    if facts.power_saving == Some(true) {
        warnings.push(Warning::PowerSaving);
    }
    if facts.turbo == Some(true) {
        warnings.push(Warning::TurboEnabled);
    }
    if facts.throttled == Some(true) {
        warnings.push(Warning::Throttled);
    }
    if on_ac == Some(false) {
        warnings.push(Warning::OnBattery);
    }
    if busy == Some(true) {
        warnings.push(Warning::Busy);
    }
    if virtualized {
        warnings.push(Warning::Virtualized);
    }
    if invariant_tsc == Some(false) || facts.clock_source.as_deref().is_some_and(slow_clock) {
        warnings.push(Warning::UnstableClock);
    }
    Environment {
        governor: facts.governor,
        turbo: facts.turbo,
        smt: facts.smt,
        invariant_tsc,
        clock_source: facts.clock_source,
        throttled: facts.throttled,
        isolated_cpus: facts.isolated_cpus,
        aslr: facts.aslr,
        on_ac,
        load,
        virtualized,
        warnings,
    }
}

impl Environment {
    /// No warning that makes results noisy.
    #[must_use]
    pub fn is_reproducible(&self) -> bool {
        self.warnings.is_empty()
    }

    fn pairs<W: fmt::Write>(&self, out: &mut W) -> fmt::Result {
        fn flag(v: Option<bool>) -> &'static str {
            match v {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            }
        }
        let load = self
            .load
            .map_or_else(|| "unknown".to_owned(), |l| format!("{:.2}", l.one));
        let warnings = if self.warnings.is_empty() {
            "none".to_owned()
        } else {
            let names: Vec<&str> = self.warnings.iter().map(|w| w.name()).collect();
            names.join(",")
        };
        let items = [
            ("governor", self.governor.as_deref().unwrap_or("unknown")),
            ("turbo", flag(self.turbo)),
            ("smt", flag(self.smt)),
            ("invariant_tsc", flag(self.invariant_tsc)),
            (
                "clock_source",
                self.clock_source.as_deref().unwrap_or("unknown"),
            ),
            ("throttled", flag(self.throttled)),
            (
                "isolated_cpus",
                self.isolated_cpus.as_deref().unwrap_or("none"),
            ),
            ("aslr", flag(self.aslr)),
            ("on_ac", flag(self.on_ac)),
            ("load", &load),
            ("virtualized", flag(Some(self.virtualized))),
            ("warnings", &warnings),
            ("reproducible", flag(Some(self.is_reproducible()))),
        ];
        for (i, (key, value)) in items.iter().enumerate() {
            if i > 0 {
                out.write_char(' ')?;
            }
            crate::fmt::kv(out, key, value)?;
        }
        Ok(())
    }

    /// One line of `key=value` pairs; values with spaces are quoted.
    ///
    /// # Errors
    ///
    /// Whatever `out` returns.
    pub fn write_kv(&self, out: &mut impl fmt::Write) -> fmt::Result {
        self.pairs(out)
    }

    /// One JSON object; unknown values are `null`.
    ///
    /// # Errors
    ///
    /// Whatever `out` returns.
    pub fn write_json(&self, out: &mut impl fmt::Write) -> fmt::Result {
        fn text(out: &mut impl fmt::Write, v: Option<&str>) -> fmt::Result {
            match v {
                Some(s) => crate::fmt::json_str(out, s),
                None => out.write_str("null"),
            }
        }
        fn flag(out: &mut impl fmt::Write, v: Option<bool>) -> fmt::Result {
            match v {
                Some(b) => write!(out, "{b}"),
                None => out.write_str("null"),
            }
        }
        out.write_str("{\"governor\":")?;
        text(out, self.governor.as_deref())?;
        out.write_str(",\"turbo\":")?;
        flag(out, self.turbo)?;
        out.write_str(",\"smt\":")?;
        flag(out, self.smt)?;
        out.write_str(",\"invariant_tsc\":")?;
        flag(out, self.invariant_tsc)?;
        out.write_str(",\"clock_source\":")?;
        text(out, self.clock_source.as_deref())?;
        out.write_str(",\"throttled\":")?;
        flag(out, self.throttled)?;
        out.write_str(",\"isolated_cpus\":")?;
        text(out, self.isolated_cpus.as_deref())?;
        out.write_str(",\"aslr\":")?;
        flag(out, self.aslr)?;
        out.write_str(",\"on_ac\":")?;
        flag(out, self.on_ac)?;
        out.write_str(",\"load\":")?;
        match self.load {
            Some(l) => write!(out, "[{},{},{}]", l.one, l.five, l.fifteen)?,
            None => out.write_str("null")?,
        }
        write!(out, ",\"virtualized\":{},\"warnings\":[", self.virtualized)?;
        for (i, w) in self.warnings.iter().enumerate() {
            if i > 0 {
                out.write_char(',')?;
            }
            crate::fmt::json_str(out, w.name())?;
        }
        write!(out, "],\"reproducible\":{}}}", self.is_reproducible())
    }
}

impl fmt::Display for Environment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.pairs(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> BenchFacts {
        BenchFacts {
            governor: Some("performance".into()),
            power_saving: Some(false),
            turbo: Some(false),
            smt: Some(true),
            clock_source: Some("tsc".into()),
            throttled: Some(false),
            isolated_cpus: None,
            aslr: Some(true),
        }
    }

    #[test]
    fn test_bench_quiet_machine_is_reproducible() {
        let env = assess(quiet(), Some(true), None, Some(false), Some(true), false);
        assert!(env.is_reproducible(), "{:?}", env.warnings);
    }

    #[test]
    fn test_bench_every_warning() {
        let facts = BenchFacts {
            governor: Some("powersave".into()),
            power_saving: Some(true),
            turbo: Some(true),
            throttled: Some(true),
            clock_source: Some("hpet".into()),
            ..BenchFacts::default()
        };
        let env = assess(facts, Some(true), None, Some(true), Some(false), true);
        assert_eq!(
            env.warnings,
            vec![
                Warning::PowerSaving,
                Warning::TurboEnabled,
                Warning::Throttled,
                Warning::OnBattery,
                Warning::Busy,
                Warning::Virtualized,
                Warning::UnstableClock
            ]
        );
        assert!(!env.is_reproducible());
        let env = assess(quiet(), Some(false), None, None, None, false);
        assert_eq!(env.warnings, vec![Warning::UnstableClock]);
    }

    #[test]
    fn test_bench_unknown_is_not_a_warning() {
        let env = assess(BenchFacts::default(), None, None, None, None, false);
        assert!(env.is_reproducible());
        let mut j = String::new();
        env.write_json(&mut j).unwrap();
        assert!(j.contains("\"governor\":null") && j.ends_with("\"reproducible\":true}"));
        assert!(env.to_string().contains("warnings=none"));
    }

    #[test]
    fn test_bench_live_outputs() {
        let env = environment();
        let mut kv = String::new();
        env.write_kv(&mut kv).unwrap();
        assert_eq!(kv, env.to_string());
        assert!(kv.starts_with("governor=") && kv.contains(" reproducible="));
        let mut j = String::new();
        env.write_json(&mut j).unwrap();
        assert!(j.starts_with("{\"governor\":") && j.ends_with('}'));
    }
}
