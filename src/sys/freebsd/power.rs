//! The battery and mains power: the ACPI driver's combined battery sysctls
//! (`hw.acpi.battery.*`) and the AC adapter (`hw.acpi.acline`). Per-battery
//! figures need `/dev/acpi`, which only the operator group may open, so the
//! machine's batteries are one entry, as `acpiconf -i` shows them combined.

use core::time::Duration;

use super::{SysctlError, ffi, sysctl_signed};
use crate::{
    Percent, Result,
    power::{Battery, BatteryState, RawBattery},
};

const WHAT: &str = "batteries";

// `acpi_bst` state bits (dev/acpica/acpiio.h).
const STAT_DISCHARGING: i64 = 0x1;
const STAT_CHARGING: i64 = 0x2;
/// `ACPI_BATT_STAT_NOT_PRESENT`: every state bit set.
const STAT_NOT_PRESENT: i64 = 0x7;

/// A sysctl that may be absent: `Ok(None)` without the ACPI driver.
fn optional(name: &'static core::ffi::CStr) -> Result<Option<i64>> {
    match sysctl_signed(name) {
        Ok(n) => Ok(Some(n)),
        Err(SysctlError::Errno(ffi::ENOENT)) => Ok(None),
        Err(e) => Err(e.into_error(WHAT, name)),
    }
}

/// `hw.acpi.battery.units`: the batteries the ACPI driver found.
pub(crate) fn has_battery() -> bool {
    matches!(optional(c"hw.acpi.battery.units"), Ok(Some(n)) if n > 0)
}

/// What the sysctls say, before it becomes a battery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Acpi {
    units: i64,
    /// Percent, -1 when unknown.
    life: i64,
    /// Minutes to empty, -1 when unknown.
    minutes: i64,
    state: i64,
    acline: Option<bool>,
}

/// The combined battery, or `None` when there is none.
fn battery_from(a: Acpi) -> Option<RawBattery> {
    if a.units <= 0 || a.state == STAT_NOT_PRESENT || !(0..=100).contains(&a.life) {
        return None;
    }
    let state = if a.state & STAT_CHARGING != 0 {
        BatteryState::Charging
    } else if a.state & STAT_DISCHARGING != 0 {
        BatteryState::Discharging
    } else {
        match a.acline {
            Some(true) if a.life >= 100 => BatteryState::Full,
            Some(true) => BatteryState::NotCharging,
            _ => BatteryState::Unknown,
        }
    };
    let time_to_empty = (state == BatteryState::Discharging)
        .then(|| u64::try_from(a.minutes).ok())
        .flatten()
        .map(|m| Duration::from_secs(m.saturating_mul(60)));
    Some(RawBattery {
        battery: Battery {
            charge: Percent::from_value(a.life as f32),
            state,
            time_to_empty,
            time_to_full: None,
            health: None,
            cycles: None,
        },
        energy: None,
    })
}

pub(crate) fn batteries() -> Result<Vec<RawBattery>> {
    let Some(units) = optional(c"hw.acpi.battery.units")? else {
        return Ok(Vec::new());
    };
    let acpi = Acpi {
        units,
        life: optional(c"hw.acpi.battery.life")?.unwrap_or(-1),
        minutes: optional(c"hw.acpi.battery.time")?.unwrap_or(-1),
        state: optional(c"hw.acpi.battery.state")?.unwrap_or(STAT_NOT_PRESENT),
        acline: on_ac()?,
    };
    Ok(battery_from(acpi).into_iter().collect())
}

/// `hw.acpi.acline`: 1 on mains power; `None` without an AC adapter driver.
pub(crate) fn on_ac() -> Result<Option<bool>> {
    Ok(optional(c"hw.acpi.acline")?.map(|n| n == 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAPTOP: Acpi = Acpi {
        units: 1,
        life: 80,
        minutes: 95,
        state: STAT_DISCHARGING,
        acline: Some(false),
    };

    #[test]
    fn test_power_freebsd_battery_states() {
        let b = battery_from(LAPTOP).unwrap().battery;
        assert_eq!(b.state, BatteryState::Discharging);
        assert_eq!(b.charge.to_string(), "80.0%");
        assert_eq!(b.time_to_empty, Some(Duration::from_secs(95 * 60)));
        let charging = Acpi {
            state: STAT_CHARGING,
            acline: Some(true),
            ..LAPTOP
        };
        let b = battery_from(charging).unwrap().battery;
        assert_eq!((b.state, b.time_to_empty), (BatteryState::Charging, None));
        let full = Acpi {
            state: 0,
            life: 100,
            acline: Some(true),
            ..LAPTOP
        };
        assert_eq!(
            battery_from(full).unwrap().battery.state,
            BatteryState::Full
        );
        let held = Acpi {
            state: 0,
            life: 60,
            acline: Some(true),
            ..LAPTOP
        };
        let held = battery_from(held).unwrap().battery.state;
        assert_eq!(held, BatteryState::NotCharging);
        let unknown_time = Acpi {
            minutes: -1,
            ..LAPTOP
        };
        assert_eq!(
            battery_from(unknown_time).unwrap().battery.time_to_empty,
            None
        );
    }

    #[test]
    fn test_power_freebsd_no_battery() {
        assert_eq!(battery_from(Acpi { units: 0, ..LAPTOP }), None);
        let absent = Acpi {
            state: STAT_NOT_PRESENT,
            ..LAPTOP
        };
        assert_eq!(battery_from(absent), None);
        assert_eq!(battery_from(Acpi { life: -1, ..LAPTOP }), None);
        assert_eq!(
            battery_from(Acpi {
                life: 101,
                ..LAPTOP
            }),
            None
        );
    }

    #[test]
    fn test_power_freebsd_live_readings() {
        let list = batteries().unwrap();
        assert!(list.len() <= 1);
        if !has_battery() {
            assert!(list.is_empty());
        }
        let _ = on_ac().unwrap();
    }
}
