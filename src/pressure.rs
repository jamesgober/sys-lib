//! [`Pressure`]: how much work is stalled waiting for a resource.

use core::time::Duration;

use crate::Percent;

/// How much work is stalled waiting for a resource (CPU, memory, or I/O).
///
/// The best single sign that a machine, or a container, is overloaded: it
/// measures waiting rather than busyness. A CPU at 100% with nothing stalled is
/// doing its job; with 30% of the time stalled it is turning work away.
/// [`cpu::pressure`](crate::cpu::pressure), [`mem::pressure`](crate::mem::pressure),
/// and [`disk::pressure`](crate::disk::pressure) return it.
///
/// On Linux it is the kernel's pressure-stall information (PSI, kernel 4.20 and
/// later), read from the process's own cgroup when it has one, so a container
/// sees its own pressure, and from `/proc/pressure/` otherwise. Where an OS
/// reports only a level (macOS and Windows memory), the stall shares are zero
/// and `total` is `None`.
///
/// ```
/// use sys_lib::{self as sys, PressureLevel};
///
/// if let Some(p) = sys::cpu::pressure()? {
///     if p.level >= PressureLevel::Warning {
///         println!("tasks waited for a CPU {} of the last 10 s", p.some.avg10);
///     }
/// }
/// # Ok::<(), sys_lib::Error>(())
/// ```
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub struct Pressure {
    /// The level the stall shares amount to (see the thresholds).
    pub level: PressureLevel,
    /// The share of time at least one task was stalled.
    pub some: Stall,
    /// The share of time every non-idle task was stalled at once (`None` for
    /// CPU at the host level, where the kernel does not report it).
    pub full: Option<Stall>,
    /// The cumulative time tasks were stalled (`some`), when reported.
    pub total: Option<Duration>,
}

impl Pressure {
    /// `some.avg10` at or above this is [`PressureLevel::Warning`].
    pub const WARNING_SOME_AVG10: f32 = 10.0;
    /// `some.avg10` at or above this is [`PressureLevel::Critical`].
    pub const CRITICAL_SOME_AVG10: f32 = 40.0;
    /// `full.avg10` above this is [`PressureLevel::Critical`].
    pub const CRITICAL_FULL_AVG10: f32 = 10.0;

    /// A pressure reading from its stall shares, with the level they amount to.
    pub(crate) fn from_stalls(
        some: Stall,
        full: Option<Stall>,
        total: Option<Duration>,
    ) -> Pressure {
        let full_critical = full.is_some_and(|f| f.avg10.value() > Self::CRITICAL_FULL_AVG10);
        let level = if full_critical || some.avg10.value() >= Self::CRITICAL_SOME_AVG10 {
            PressureLevel::Critical
        } else if some.avg10.value() >= Self::WARNING_SOME_AVG10 {
            PressureLevel::Warning
        } else {
            PressureLevel::Normal
        };
        Pressure {
            level,
            some,
            full,
            total,
        }
    }

    /// A level-only reading, for an OS that reports no stall shares.
    #[allow(dead_code)] // the macOS and Windows memory readings
    pub(crate) fn from_level(level: PressureLevel) -> Pressure {
        Pressure {
            level,
            some: Stall::ZERO,
            full: None,
            total: None,
        }
    }
}

/// The share of time tasks were stalled, averaged over the last 10, 60, and
/// 300 seconds.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[non_exhaustive]
pub struct Stall {
    /// Over the last 10 seconds.
    pub avg10: Percent,
    /// Over the last 60 seconds.
    pub avg60: Percent,
    /// Over the last 300 seconds.
    pub avg300: Percent,
}

impl Stall {
    pub(crate) const ZERO: Stall = Stall {
        avg10: Percent::ZERO,
        avg60: Percent::ZERO,
        avg300: Percent::ZERO,
    };

    pub(crate) fn new(avg10: Percent, avg60: Percent, avg300: Percent) -> Stall {
        Stall {
            avg10,
            avg60,
            avg300,
        }
    }
}

/// How serious a [`Pressure`] reading is. Ordered: `Normal < Warning < Critical`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[non_exhaustive]
pub enum PressureLevel {
    /// Little or no stall.
    Normal,
    /// Work is waiting often enough to notice: trim caches, postpone batch work.
    Warning,
    /// Work is waiting most of the time: shed load.
    Critical,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stall(avg10: f32) -> Stall {
        Stall::new(Percent::from_value(avg10), Percent::ZERO, Percent::ZERO)
    }

    #[test]
    fn test_pressure_level_thresholds() {
        let level = |some: f32, full: Option<f32>| {
            Pressure::from_stalls(stall(some), full.map(stall), None).level
        };
        assert_eq!(level(0.0, None), PressureLevel::Normal);
        assert_eq!(level(9.99, Some(10.0)), PressureLevel::Normal);
        assert_eq!(level(10.0, None), PressureLevel::Warning);
        assert_eq!(level(39.9, Some(5.0)), PressureLevel::Warning);
        assert_eq!(level(40.0, None), PressureLevel::Critical);
        assert_eq!(level(1.0, Some(10.01)), PressureLevel::Critical);
        assert!(
            PressureLevel::Normal < PressureLevel::Warning
                && PressureLevel::Warning < PressureLevel::Critical
        );
    }

    #[test]
    fn test_pressure_from_level_has_no_shares() {
        let p = Pressure::from_level(PressureLevel::Critical);
        assert_eq!(p.level, PressureLevel::Critical);
        assert_eq!(p.some, Stall::ZERO);
        assert_eq!((p.full, p.total), (None, None));
    }
}
