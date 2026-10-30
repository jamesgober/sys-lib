//! CPU usage: the share of non-idle time between two readings of the OS's
//! cumulative counters.

use core::time::Duration;
use std::time::Instant;

use crate::{Error, Percent, Result};

/// The interval [`usage`] measures over: 100 ms.
pub const DEFAULT_WINDOW: Duration = Duration::from_millis(100);

const MIN_WINDOW: Duration = Duration::from_millis(10);
const MAX_WINDOW: Duration = Duration::from_secs(10);

/// The OS's cumulative CPU time counters, since boot (Tier 3). States an OS
/// does not keep are zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Times {
    /// Running user code.
    pub user: Duration,
    /// Running user code at a lowered priority (Unix).
    pub nice: Duration,
    /// Running kernel code (Windows: kernel time minus idle).
    pub system: Duration,
    /// Doing nothing.
    pub idle: Duration,
    /// Idle while I/O was outstanding (Linux).
    pub iowait: Duration,
    /// Serving hardware interrupts (Linux).
    pub irq: Duration,
    /// Serving software interrupts (Linux).
    pub softirq: Duration,
    /// Taken by the hypervisor for other guests (Linux virtual machines).
    pub steal: Duration,
}

impl Times {
    fn total(&self) -> Duration {
        self.busy() + self.idle + self.iowait
    }

    /// Busy time: steal counts as busy (the CPU was not ours to use).
    fn busy(&self) -> Duration {
        self.user + self.nice + self.system + self.irq + self.softirq + self.steal
    }

    /// `self - earlier`, field by field, or `None` when any counter went
    /// backwards (a CPU offlined and back, a VM migration, a wrap: E-20).
    fn since(&self, earlier: &Times) -> Option<Times> {
        Some(Times {
            user: self.user.checked_sub(earlier.user)?,
            nice: self.nice.checked_sub(earlier.nice)?,
            system: self.system.checked_sub(earlier.system)?,
            idle: self.idle.checked_sub(earlier.idle)?,
            iowait: self.iowait.checked_sub(earlier.iowait)?,
            irq: self.irq.checked_sub(earlier.irq)?,
            softirq: self.softirq.checked_sub(earlier.softirq)?,
            steal: self.steal.checked_sub(earlier.steal)?,
        })
    }
}

/// Which states a platform's counters distinguish.
#[derive(Clone, Copy, Debug)]
pub(crate) struct States {
    pub(crate) iowait: bool,
    pub(crate) steal: bool,
    pub(crate) irq: bool,
}

/// CPU usage over an interval.
///
/// Steal counts as busy (the CPU was not available to us) and iowait as idle
/// (the CPU was free to run other work); both are also reported on their own
/// where the OS keeps them (E-24, E-25).
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct Usage {
    /// Running user code (nice included).
    pub user: Percent,
    /// Running kernel code.
    pub system: Percent,
    /// Doing nothing, iowait excluded.
    pub idle: Percent,
    /// Idle while I/O was outstanding (Linux).
    pub iowait: Option<Percent>,
    /// Taken by the hypervisor for other guests (Linux virtual machines).
    pub steal: Option<Percent>,
    /// Serving interrupts (Linux: hardware and software).
    pub irq: Option<Percent>,
    /// The time the values cover.
    pub interval: Duration,
}

impl Usage {
    /// The busy share: everything but idle and iowait.
    #[must_use]
    pub fn used(&self) -> Percent {
        self.free().complement()
    }

    /// The idle share, iowait included.
    #[must_use]
    pub fn free(&self) -> Percent {
        let iowait = self.iowait.map_or(0.0, Percent::value);
        Percent::from_value(self.idle.value() + iowait)
    }

    /// The usage a counter difference amounts to, or `None` when no time
    /// passed on the OS's counters.
    fn from_delta(d: &Times, states: States, interval: Duration) -> Option<Usage> {
        let total = d.total().as_nanos();
        if total == 0 {
            return None;
        }
        let share = |part: Duration| Percent::from_ratio_u128(part.as_nanos(), total);
        Some(Usage {
            user: share(d.user + d.nice),
            system: share(d.system),
            idle: share(d.idle),
            iowait: states.iowait.then(|| share(d.iowait)),
            steal: states.steal.then(|| share(d.steal)),
            irq: states.irq.then(|| share(d.irq + d.softirq)),
            interval,
        })
    }
}

/// The OS's cumulative CPU time counters (Tier 3). One reading only gives the
/// average since boot; [`Sampler`] gives usage.
///
/// # Errors
///
/// `NotFound` or `PermissionDenied` when a sandbox hides the counters;
/// `Malformed` when they do not parse.
pub fn times() -> Result<Times> {
    crate::sys::cpu_times()
}

/// CPU usage over the next 100 ms. **Blocks for [`DEFAULT_WINDOW`]**; a
/// monitor keeps a [`Sampler`] instead.
///
/// ```
/// let u = sys_lib::cpu::usage()?;
/// println!("CPU {} (user {}, system {})", u.used(), u.user, u.system);
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`Sampler::sample`].
pub fn usage() -> Result<Usage> {
    usage_over(DEFAULT_WINDOW)
}

/// CPU usage over `window`, clamped to 10 ms ..= 10 s. **Blocks for the
/// window.**
///
/// # Errors
///
/// As [`Sampler::sample`].
pub fn usage_over(window: Duration) -> Result<Usage> {
    let mut sampler = Sampler::total_only()?;
    std::thread::sleep(window.clamp(MIN_WINDOW, MAX_WINDOW));
    sampler.sample()
}

/// The busy share over the next 100 ms: `usage()?.used()`. **Blocks for
/// [`DEFAULT_WINDOW`].**
///
/// ```
/// println!("CPU {}", sys_lib::cpu::used()?);   // "12.4%"
/// # Ok::<(), sys_lib::Error>(())
/// ```
///
/// # Errors
///
/// As [`Sampler::sample`].
pub fn used() -> Result<Percent> {
    usage().map(|u| u.used())
}

/// The idle share over the next 100 ms: `usage()?.free()`. **Blocks for
/// [`DEFAULT_WINDOW`].**
///
/// # Errors
///
/// As [`Sampler::sample`].
pub fn free() -> Result<Percent> {
    usage().map(|u| u.free())
}

/// Measures CPU usage between samples. Owned by the caller: two samplers never
/// disturb each other (there is no hidden global baseline). `sample` never
/// blocks and allocates nothing; `sample_per_core_into` allocates nothing
/// unless CPUs were hot-plugged since the previous sample.
///
/// ```no_run
/// use std::time::Duration;
/// use sys_lib as sys;
///
/// let mut cpu = sys::cpu::Sampler::new()?;
/// let mut per_core = vec![sys::Percent::ZERO; sys::cpu::logical()];
/// loop {
///     std::thread::sleep(Duration::from_secs(5));
///     let u = cpu.sample_per_core_into(&mut per_core)?;
///     println!("used {} (user {}, system {}, steal {:?})", u.used(), u.user, u.system, u.steal);
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Debug)]
pub struct Sampler {
    /// The baseline of `sample`, from `times()`.
    total: Times,
    at: Instant,
    /// The baseline of `sample_per_core_into`: its own total (from the same
    /// read as the cores, which on macOS is a wider sum than `times()`), the
    /// cores, and a scratch buffer of the same length. Kept apart from the
    /// total's, so mixing the two calls never compares unlike counters.
    cores_total: Times,
    cores: Vec<Times>,
    scratch: Vec<Times>,
    cores_at: Instant,
}

impl Sampler {
    /// Takes the first reading, of the total and of every core.
    ///
    /// # Errors
    ///
    /// As [`times`].
    pub fn new() -> Result<Sampler> {
        let total = times()?;
        let mut cores = vec![Times::default(); super::logical()];
        let (cores_total, n) = match read_cores(&mut cores) {
            Ok(r) => r,
            // No per-core counters here (Windows past 64 processors): the
            // total still works; the per-core calls say Unsupported.
            Err(Error::Unsupported { .. }) => (total, 0),
            Err(e) => return Err(e),
        };
        cores.truncate(n);
        let now = Instant::now();
        Ok(Sampler {
            total,
            at: now,
            cores_total,
            scratch: vec![Times::default(); cores.len()],
            cores,
            cores_at: now,
        })
    }

    /// A sampler for the total only, for the blocking functions.
    fn total_only() -> Result<Sampler> {
        let now = Instant::now();
        let total = times()?;
        Ok(Sampler {
            total,
            at: now,
            cores_total: total,
            cores: Vec::new(),
            scratch: Vec::new(),
            cores_at: now,
        })
    }

    /// Usage since the previous sample.
    ///
    /// # Errors
    ///
    /// `TooSoon` when no tick passed on the OS's counters since the previous
    /// sample (about 10 ms on Linux: E-21), or when they went backwards, which
    /// re-bases the sampler (E-20). Never a meaningless 0% or 100%.
    pub fn sample(&mut self) -> Result<Usage> {
        let now = times()?;
        let at = Instant::now();
        let usage = between(&now, &self.total, at.duration_since(self.at));
        self.total = now;
        self.at = at;
        usage
    }

    /// Usage since the previous sample, total and per core (allocates the
    /// `Vec`).
    ///
    /// # Errors
    ///
    /// As [`sample_per_core_into`](Sampler::sample_per_core_into).
    pub fn sample_per_core(&mut self) -> Result<(Usage, Vec<Percent>)> {
        let mut out = vec![Percent::ZERO; self.scratch.len().max(super::logical())];
        let usage = self.sample_per_core_into(&mut out)?;
        out.truncate(self.cores.len());
        Ok((usage, out))
    }

    /// Usage since the previous sample, writing each core's busy share into
    /// `out` (Tier 3: no allocation). Entries past the number of cores are left
    /// as they were. A core whose counters went backwards, or did not move,
    /// reads 0% for this interval and is re-based (E-20); the total is
    /// unaffected.
    ///
    /// # Errors
    ///
    /// `TooLarge` without writing anything when `out` is shorter than the
    /// number of cores (on Linux, the highest CPU number plus one: an offline
    /// CPU keeps its slot and reads 0%); `TooSoon` as
    /// [`sample`](Sampler::sample); `Unsupported` per core on Windows with
    /// more than 64 logical processors or more than one processor group.
    pub fn sample_per_core_into(&mut self, out: &mut [Percent]) -> Result<Usage> {
        let (total, n) = read_cores(&mut self.scratch)?;
        let at = Instant::now();
        if out.len() < n {
            return Err(Error::TooLarge {
                what: "per-core CPU usage",
                limit: out.len() as u64,
            });
        }
        let interval = at.duration_since(self.cores_at);
        let usage = between(&total, &self.cores_total, interval);
        for (i, now) in self.scratch[..n].iter().enumerate() {
            out[i] = self
                .cores
                .get(i)
                .and_then(|old| now.since(old))
                .and_then(|d| Usage::from_delta(&d, crate::sys::CPU_STATES, interval))
                .map_or(Percent::ZERO, |u| u.used());
        }
        // The new readings become the baseline; both buffers keep their size,
        // so nothing is allocated in the steady state.
        self.cores.resize(n, Times::default());
        self.cores.copy_from_slice(&self.scratch[..n]);
        self.cores_at = at;
        self.cores_total = total;
        usage
    }
}

/// The usage between two readings `interval` apart.
fn between(now: &Times, then: &Times, interval: Duration) -> Result<Usage> {
    let too_soon = || Error::TooSoon {
        elapsed: interval,
        minimum: crate::sys::CPU_TICK,
    };
    let delta = now.since(then).ok_or_else(too_soon)?;
    Usage::from_delta(&delta, crate::sys::CPU_STATES, interval).ok_or_else(too_soon)
}

/// The total and per-core counters, into `buf`. When more cores report than
/// `buf` holds (hot-plug, E-22), it grows once and reads again: the only
/// allocation in a sampler's steady state.
fn read_cores(buf: &mut Vec<Times>) -> Result<(Times, usize)> {
    let (total, n) = match crate::sys::cpu_times_per_core(buf) {
        Err(Error::TooLarge { .. }) => {
            buf.resize(buf.len() * 2 + super::logical(), Times::default());
            crate::sys::cpu_times_per_core(buf)?
        }
        r => r?,
    };
    Ok((total, n.min(buf.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: States = States {
        iowait: true,
        steal: true,
        irq: true,
    };

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn times(user: u64, system: u64, idle: u64, iowait: u64, steal: u64) -> Times {
        Times {
            user: ms(user),
            system: ms(system),
            idle: ms(idle),
            iowait: ms(iowait),
            steal: ms(steal),
            ..Times::default()
        }
    }

    #[test]
    fn test_cpu_usage_steal_busy_iowait_idle() {
        // E-24, E-25: 1000 ms: user 300, system 100, idle 400, iowait 100, steal 100.
        let d = times(300, 100, 400, 100, 100);
        let u = Usage::from_delta(&d, ALL, ms(1000)).unwrap();
        assert_eq!(u.user.to_string(), "30.0%");
        assert_eq!(u.idle.to_string(), "40.0%");
        assert_eq!(u.iowait.unwrap().to_string(), "10.0%");
        assert_eq!(u.steal.unwrap().to_string(), "10.0%");
        assert_eq!(u.free().to_string(), "50.0%");
        assert_eq!(u.used().to_string(), "50.0%");
    }

    #[test]
    fn test_cpu_usage_states_a_platform_lacks_are_none() {
        let u = Usage::from_delta(
            &times(1, 1, 2, 0, 0),
            States {
                iowait: false,
                steal: false,
                irq: false,
            },
            ms(4),
        )
        .unwrap();
        assert_eq!((u.iowait, u.steal, u.irq), (None, None, None));
        assert_eq!(u.used().to_string(), "50.0%");
    }

    #[test]
    fn test_cpu_usage_no_time_and_backwards_counters() {
        // E-21: no tick passed. E-20: a counter went backwards.
        assert!(Usage::from_delta(&Times::default(), ALL, ms(5)).is_none());
        assert!(times(5, 0, 0, 0, 0).since(&times(6, 0, 0, 0, 0)).is_none());
        let d = times(7, 1, 2, 0, 0).since(&times(5, 1, 1, 0, 0)).unwrap();
        assert_eq!((d.user, d.idle), (ms(2), ms(1)));
    }

    #[test]
    fn test_cpu_sampler_live_never_out_of_range() {
        let mut s = Sampler::new().unwrap();
        let mut out = vec![Percent::FULL; crate::cpu::logical() + 4];
        std::thread::sleep(ms(30));
        match s.sample_per_core_into(&mut out) {
            Ok(u) => {
                assert!(u.used() <= Percent::FULL && u.interval >= ms(25));
                assert_eq!(
                    out[crate::cpu::logical() + 3],
                    Percent::FULL,
                    "past the cores is untouched"
                );
            }
            Err(Error::TooSoon { .. } | Error::Unsupported { .. }) => {}
            Err(e) => panic!("{e}"),
        }
        let mut short = [Percent::ZERO; 0];
        if crate::cpu::logical() > 0 {
            assert!(matches!(
                s.sample_per_core_into(&mut short),
                Err(Error::TooLarge { .. }) | Err(Error::Unsupported { .. })
            ));
        }
        std::thread::sleep(ms(30));
        if let Ok(u) = s.sample() {
            assert!(u.used() <= Percent::FULL);
        }
        let u = usage_over(ms(30)).unwrap();
        assert!(u.used() <= Percent::FULL);
    }
}
