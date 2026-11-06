//! The system battery and mains power, from `GetSystemPowerStatus`.
//!
//! The call reports the system batteries as one: `IOCTL_BATTERY_QUERY_INFORMATION`
//! per battery device could list them apart, and gives health and cycles, but it
//! has not been verified on a machine with a battery. So one combined entry
//! comes back, with no energy figures.

use std::time::Duration;

use super::{ffi, os_error};
use crate::{
    Percent, Result,
    power::{Battery, BatteryState, RawBattery},
};

fn status() -> Result<ffi::SystemPowerStatus> {
    let mut status = ffi::SystemPowerStatus {
        ac_line_status: ffi::AC_LINE_UNKNOWN,
        battery_flag: ffi::BATTERY_FLAG_UNKNOWN,
        battery_life_percent: ffi::BATTERY_PERCENTAGE_UNKNOWN,
        system_status_flag: 0,
        battery_life_time: ffi::BATTERY_LIFE_UNKNOWN,
        battery_full_life_time: ffi::BATTERY_LIFE_UNKNOWN,
    };
    // SAFETY: `status` is a writable SYSTEM_POWER_STATUS.
    let ok = unsafe { ffi::GetSystemPowerStatus(&raw mut status) };
    if ok == 0 {
        return Err(os_error("batteries", "GetSystemPowerStatus"));
    }
    Ok(status)
}

/// A battery flag saying one is there: not "no battery", not "unknown". The
/// value is a bit set, in which 128 stands alone.
const fn present(flag: u8) -> bool {
    flag != ffi::BATTERY_FLAG_UNKNOWN && flag & ffi::BATTERY_FLAG_NO_BATTERY == 0
}

fn battery_state(s: &ffi::SystemPowerStatus) -> BatteryState {
    if s.battery_flag & ffi::BATTERY_FLAG_CHARGING != 0 {
        return BatteryState::Charging;
    }
    match s.ac_line_status {
        ffi::AC_LINE_ONLINE if s.battery_life_percent == 100 => BatteryState::Full,
        ffi::AC_LINE_ONLINE => BatteryState::NotCharging,
        ffi::AC_LINE_UNKNOWN => BatteryState::Unknown,
        _ => BatteryState::Discharging,
    }
}

/// The battery a status describes. An unknown percentage reads as zero with
/// the state unknown (E-83): `charge` is not optional, and the state says it
/// is not to be believed.
fn battery_of(s: &ffi::SystemPowerStatus) -> Option<RawBattery> {
    if !present(s.battery_flag) {
        return None;
    }
    let known = s.battery_life_percent <= 100;
    let charge = if known {
        Percent::from_value(f32::from(s.battery_life_percent))
    } else {
        Percent::ZERO
    };
    let state = if known {
        battery_state(s)
    } else {
        BatteryState::Unknown
    };
    Some(RawBattery {
        battery: Battery {
            charge,
            state,
            time_to_empty: (state == BatteryState::Discharging
                && s.battery_life_time != ffi::BATTERY_LIFE_UNKNOWN)
                .then(|| Duration::from_secs(u64::from(s.battery_life_time))),
            time_to_full: None,
            health: None,
            cycles: None,
        },
        energy: None,
    })
}

pub(crate) fn has_battery() -> bool {
    status().is_ok_and(|s| present(s.battery_flag))
}

pub(crate) fn batteries() -> Result<Vec<RawBattery>> {
    Ok(battery_of(&status()?).into_iter().collect())
}

pub(crate) fn on_ac() -> Result<Option<bool>> {
    Ok(match status()?.ac_line_status {
        ffi::AC_LINE_UNKNOWN => None,
        line => Some(line == ffi::AC_LINE_ONLINE),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(ac: u8, flag: u8, percent: u8, seconds: u32) -> ffi::SystemPowerStatus {
        ffi::SystemPowerStatus {
            ac_line_status: ac,
            battery_flag: flag,
            battery_life_percent: percent,
            system_status_flag: 0,
            battery_life_time: seconds,
            battery_full_life_time: ffi::BATTERY_LIFE_UNKNOWN,
        }
    }

    #[test]
    fn test_power_windows_flags_mean_what_they_say() {
        assert!(!present(ffi::BATTERY_FLAG_NO_BATTERY));
        assert!(!present(ffi::BATTERY_FLAG_UNKNOWN));
        assert!(present(0), "zero: between high and low");
        assert!(present(1) && present(2) && present(4));
        assert!(present(ffi::BATTERY_FLAG_CHARGING | 1));
        // The desktop this crate is mostly built on.
        assert_eq!(battery_of(&with(1, 128, 255, u32::MAX)), None);
        assert_eq!(battery_of(&with(255, 255, 255, u32::MAX)), None);
    }

    #[test]
    fn test_power_windows_states() {
        let state = |ac, flag, percent| battery_of(&with(ac, flag, percent, 60)).unwrap();
        assert_eq!(state(1, 9, 40).battery.state, BatteryState::Charging);
        assert_eq!(state(1, 1, 100).battery.state, BatteryState::Full);
        assert_eq!(state(1, 1, 80).battery.state, BatteryState::NotCharging);
        assert_eq!(state(255, 1, 80).battery.state, BatteryState::Unknown);
        let on_battery = state(0, 1, 42);
        assert_eq!(on_battery.battery.state, BatteryState::Discharging);
        assert_eq!(on_battery.battery.charge.to_string(), "42.0%");
        assert_eq!(
            on_battery.battery.time_to_empty,
            Some(Duration::from_secs(60))
        );
        assert_eq!(on_battery.energy, None);
        // E-83: present, but nothing known about it.
        let unknown = state(0, 1, ffi::BATTERY_PERCENTAGE_UNKNOWN);
        assert_eq!(unknown.battery.state, BatteryState::Unknown);
        assert_eq!(unknown.battery.charge, Percent::ZERO);
        // No time on mains, and none when the OS has no estimate.
        assert_eq!(state(1, 9, 40).battery.time_to_empty, None);
        let no_estimate = battery_of(&with(0, 1, 42, ffi::BATTERY_LIFE_UNKNOWN)).unwrap();
        assert_eq!(no_estimate.battery.time_to_empty, None);
    }

    #[test]
    fn test_power_windows_live_agrees_with_itself() {
        let list = batteries().unwrap();
        assert_eq!(has_battery(), !list.is_empty());
        assert!(list.len() <= 1);
        let _ = on_ac().unwrap();
    }
}
