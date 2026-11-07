//! The system battery and mains power.
//!
//! Only batteries that power the machine count: wireless mice, keyboards, and
//! headsets report batteries too and are left out (E-81). Several internal
//! batteries are combined by energy, not by averaging percentages (E-82).
//!
//! ```
//! use sys_lib as sys;
//!
//! if let Some(b) = sys::power::battery()? {
//!     println!("battery {} {:?}, empty in {:?}", b.charge, b.state, b.time_to_empty);
//! }
//! if sys::power::on_ac()? == Some(false) {
//!     // on battery: back off background work
//! }
//! # Ok::<(), sys_lib::Error>(())
//! ```

use core::time::Duration;
use std::sync::OnceLock;

use crate::{Percent, Result};

/// A battery, or several combined.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Battery {
    /// How full it is.
    pub charge: Percent,
    /// What it is doing.
    pub state: BatteryState,
    /// How long it lasts at the present draw, when the OS can tell.
    pub time_to_empty: Option<Duration>,
    /// How long until it is full, when the OS can tell.
    pub time_to_full: Option<Duration>,
    /// Full capacity over design capacity.
    pub health: Option<Percent>,
    /// Charge cycles.
    pub cycles: Option<u32>,
}

/// What a battery is doing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum BatteryState {
    /// Charging.
    Charging,
    /// Powering the machine.
    Discharging,
    /// Full.
    Full,
    /// On mains power and not charging (a charge limit, or a full pack).
    NotCharging,
    /// The OS does not say (E-83).
    Unknown,
}

/// A battery as a platform reports it, with the energy figures
/// [`combine`] needs. `energy` is a platform's own unit, compared only with
/// itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RawBattery {
    pub(crate) battery: Battery,
    /// Energy now and when full; `None` when the OS gives only a percentage.
    pub(crate) energy: Option<(u64, u64)>,
}

/// Whether the machine has a system battery. Cached.
#[must_use]
pub fn has_battery() -> bool {
    static FACT: OnceLock<bool> = OnceLock::new();
    *FACT.get_or_init(crate::sys::has_battery)
}

/// The system battery, several combined; `None` when there is none.
///
/// # Errors
///
/// `PermissionDenied` where the OS refuses (SELinux on Android, E-90);
/// `Unsupported` on iOS.
pub fn battery() -> Result<Option<Battery>> {
    Ok(combine(&crate::sys::batteries()?))
}

/// Every system battery separately, in the order the OS lists them.
///
/// # Errors
///
/// As [`battery`].
pub fn batteries() -> Result<Vec<Battery>> {
    Ok(crate::sys::batteries()?.iter().map(|b| b.battery).collect())
}

/// Whether the machine runs on mains power; `None` when it cannot be told,
/// which is the answer on a desktop with no power information.
///
/// # Errors
///
/// As [`battery`].
pub fn on_ac() -> Result<Option<bool>> {
    crate::sys::on_ac()
}

/// Combines batteries by energy (E-82): the sum of energy now over the sum of
/// energy when full. Without energy figures on every battery, percentages are
/// averaged. Times add when discharging together.
pub(crate) fn combine(all: &[RawBattery]) -> Option<Battery> {
    let (first, rest) = all.split_first()?;
    if rest.is_empty() {
        return Some(first.battery);
    }
    let energies: Option<Vec<(u64, u64)>> = all.iter().map(|b| b.energy).collect();
    let charge = match energies {
        Some(e) => {
            let now: u128 = e.iter().map(|&(n, _)| u128::from(n)).sum();
            let full: u128 = e.iter().map(|&(_, f)| u128::from(f)).sum();
            if full == 0 {
                first.battery.charge
            } else {
                Percent::from_ratio_u128(now, full)
            }
        }
        None => {
            let sum: f32 = all.iter().map(|b| b.battery.charge.value()).sum();
            Percent::from_value(sum / all.len() as f32)
        }
    };
    let state = combined_state(all);
    let total = |pick: fn(&Battery) -> Option<Duration>| {
        all.iter()
            .map(|b| pick(&b.battery))
            .try_fold(Duration::ZERO, |sum, t| Some(sum.saturating_add(t?)))
    };
    Some(Battery {
        charge,
        state,
        time_to_empty: total(|b| b.time_to_empty),
        time_to_full: all.iter().filter_map(|b| b.battery.time_to_full).max(),
        // One pack's health or cycles does not describe the set.
        health: None,
        cycles: None,
    })
}

/// Charging if any pack charges, discharging if any pack discharges, full when
/// all are.
fn combined_state(all: &[RawBattery]) -> BatteryState {
    let any = |s: BatteryState| all.iter().any(|b| b.battery.state == s);
    if any(BatteryState::Charging) {
        BatteryState::Charging
    } else if any(BatteryState::Discharging) {
        BatteryState::Discharging
    } else if all.iter().all(|b| b.battery.state == BatteryState::Full) {
        BatteryState::Full
    } else if any(BatteryState::NotCharging) {
        BatteryState::NotCharging
    } else {
        BatteryState::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(charge: f32, state: BatteryState, energy: Option<(u64, u64)>) -> RawBattery {
        RawBattery {
            battery: Battery {
                charge: Percent::from_value(charge),
                state,
                time_to_empty: Some(Duration::from_secs(600)),
                time_to_full: None,
                health: Some(Percent::from_value(90.0)),
                cycles: Some(10),
            },
            energy,
        }
    }

    #[test]
    fn test_power_combine_by_energy_not_percentage() {
        // E-82: 10 of 100 and 90 of 100 is 50%; 99 of 100 and 1 of 10 is 90%,
        // where averaging would say 54.5%.
        let c = combine(&[
            raw(99.0, BatteryState::Discharging, Some((99, 100))),
            raw(10.0, BatteryState::Discharging, Some((1, 10))),
        ])
        .unwrap();
        assert_eq!(c.charge.to_string(), "90.9%");
        assert_eq!(c.state, BatteryState::Discharging);
        assert_eq!(c.time_to_empty, Some(Duration::from_secs(1200)));
        assert_eq!((c.health, c.cycles), (None, None));
    }

    #[test]
    fn test_power_combine_edge_cases() {
        assert_eq!(combine(&[]), None);
        let one = raw(40.0, BatteryState::Charging, None);
        assert_eq!(combine(&[one]), Some(one.battery));
        // No energy figures: the mean of the percentages.
        let c = combine(&[
            raw(40.0, BatteryState::Full, None),
            raw(60.0, BatteryState::Full, None),
        ])
        .unwrap();
        assert_eq!(c.charge.to_string(), "50.0%");
        assert_eq!(c.state, BatteryState::Full);
        // Empty packs do not divide by zero.
        let c = combine(&[
            raw(0.0, BatteryState::Unknown, Some((0, 0))),
            raw(0.0, BatteryState::NotCharging, Some((0, 0))),
        ])
        .unwrap();
        assert_eq!(c.state, BatteryState::NotCharging);
    }

    #[test]
    fn test_power_live_never_fails_oddly() {
        let _ = has_battery();
        if let Ok(Some(b)) = battery() {
            assert!(b.charge <= Percent::FULL);
        }
        let _ = on_ac();
    }
}
