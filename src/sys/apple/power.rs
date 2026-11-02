//! The battery and mains power: the IOKit power sources (macOS). Health and
//! cycles come from the `AppleSmartBattery` service. iOS has no public source.

#[cfg(any(target_os = "macos", test))]
use core::time::Duration;

#[cfg(target_os = "macos")]
use super::{
    ffi,
    iokit::{Cf, Io, Ref},
};
#[cfg(any(target_os = "macos", test))]
use crate::{
    Percent,
    power::{Battery, BatteryState},
};
use crate::{Result, power::RawBattery};

#[cfg(target_os = "macos")]
const WHAT: &str = "batteries";

/// The most power sources read: a Mac has one, and a UPS or two.
#[cfg(target_os = "macos")]
const MAX_SOURCES: usize = 64;

/// What powers the machine, as a power source says it.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Feed {
    Mains,
    Battery,
    #[default]
    Other,
}

/// What the power source dictionary says about one battery. Capacities are in
/// the same unit as each other, percent on current systems; times are minutes,
/// with -1 for "still calculating".
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pack {
    current: Option<i64>,
    max: Option<i64>,
    charging: bool,
    charged: bool,
    feed: Feed,
    to_empty: Option<i64>,
    to_full: Option<i64>,
}

#[cfg(any(target_os = "macos", test))]
impl Pack {
    /// Capacity now and when full; `None` without a usable pair (E-83).
    fn energy(&self) -> Option<(u64, u64)> {
        let current = u64::try_from(self.current?).ok()?;
        let max = u64::try_from(self.max?).ok()?;
        (max > 0).then_some((current, max))
    }

    fn state(&self) -> BatteryState {
        let Some((current, max)) = self.energy() else {
            return BatteryState::Unknown;
        };
        if self.charged {
            BatteryState::Full
        } else if self.charging {
            BatteryState::Charging
        } else {
            match self.feed {
                Feed::Battery => BatteryState::Discharging,
                Feed::Mains if current >= max => BatteryState::Full,
                Feed::Mains => BatteryState::NotCharging,
                Feed::Other => BatteryState::Unknown,
            }
        }
    }

    /// The figures as `power` wants them. A time is meaningful only in the
    /// state it describes: at rest the dictionary carries 0 or -1.
    fn raw(&self, health: Option<Percent>, cycles: Option<u32>) -> RawBattery {
        let energy = self.energy();
        let state = self.state();
        let time = |minutes: Option<i64>, wanted: BatteryState| {
            let minutes = u64::try_from(minutes?).ok().filter(|&m| m > 0)?;
            (state == wanted).then(|| Duration::from_secs(minutes.saturating_mul(60)))
        };
        RawBattery {
            battery: Battery {
                charge: energy.map_or(Percent::ZERO, |(now, full)| Percent::from_ratio(now, full)),
                state,
                time_to_empty: time(self.to_empty, BatteryState::Discharging),
                time_to_full: time(self.to_full, BatteryState::Charging),
                health,
                cycles,
            },
            energy,
        }
    }
}

/// Full capacity over design capacity. Raw capacity can read above design on a
/// new battery; `Percent` caps it at 100.
#[cfg(any(target_os = "macos", test))]
fn health_of(full: i64, design: i64) -> Option<Percent> {
    let full = u64::try_from(full).ok().filter(|&n| n > 0)?;
    let design = u64::try_from(design).ok().filter(|&n| n > 0)?;
    Some(Percent::from_ratio(full, design))
}

/// What `IOPSGetProvidingPowerSourceType` says, as "on mains".
#[cfg(any(target_os = "macos", test))]
fn mains_from(source: &[u8]) -> Option<bool> {
    match source {
        b"AC Power" => Some(true),
        b"Battery Power" | b"UPS Power" => Some(false),
        _ => None,
    }
}

/// The power source dictionary's keys (IOPSKeys.h).
#[cfg(target_os = "macos")]
struct Keys {
    kind: Cf,
    present: Cf,
    state: Cf,
    current: Cf,
    max: Cf,
    charging: Cf,
    charged: Cf,
    to_empty: Cf,
    to_full: Cf,
}

#[cfg(target_os = "macos")]
impl Keys {
    fn new() -> Result<Keys> {
        let key = |text| Cf::string(WHAT, text);
        Ok(Keys {
            kind: key("Type")?,
            present: key("Is Present")?,
            state: key("Power Source State")?,
            current: key("Current Capacity")?,
            max: key("Max Capacity")?,
            charging: key("Is Charging")?,
            charged: key("Is Charged")?,
            to_empty: key("Time to Empty")?,
            to_full: key("Time to Full Charge")?,
        })
    }
}

/// The batteries that power this machine: `kIOPSInternalBatteryType` and
/// present. A UPS and accessories are other types (E-81).
#[cfg(target_os = "macos")]
fn is_internal(description: Ref<'_>, keys: &Keys) -> bool {
    let mut buf = [0u8; 32];
    let kind = description.value(&keys.kind).and_then(|k| k.text(&mut buf));
    let present = description.value(&keys.present).and_then(Ref::flag);
    kind == Some(&b"InternalBattery"[..]) && present != Some(false)
}

#[cfg(target_os = "macos")]
fn pack_from(description: Ref<'_>, keys: &Keys) -> Pack {
    let int = |key| description.value(key).and_then(Ref::int);
    let flag = |key| description.value(key).and_then(Ref::flag) == Some(true);
    let mut buf = [0u8; 32];
    let feed = description
        .value(&keys.state)
        .and_then(|s| s.text(&mut buf));
    Pack {
        current: int(&keys.current),
        max: int(&keys.max),
        charging: flag(&keys.charging),
        charged: flag(&keys.charged),
        feed: match feed {
            Some(b"AC Power") => Feed::Mains,
            Some(b"Battery Power") => Feed::Battery,
            _ => Feed::Other,
        },
        to_empty: int(&keys.to_empty),
        to_full: int(&keys.to_full),
    }
}

/// `IOPSCopyPowerSourcesInfo`: `None` when the system gives no snapshot.
#[cfg(target_os = "macos")]
fn snapshot() -> Option<Cf> {
    // SAFETY: the call takes nothing; the result, if any, carries one reference.
    Cf::new(unsafe { ffi::IOPSCopyPowerSourcesInfo() })
}

/// Every internal battery, or `None` when the system gives no snapshot.
#[cfg(target_os = "macos")]
fn packs(keys: &Keys) -> Option<Vec<Pack>> {
    let blob = snapshot()?;
    // SAFETY: `blob` is a live snapshot; the result, if any, carries one
    // reference (the Copy rule).
    let list = Cf::new(unsafe { ffi::IOPSCopyPowerSourcesList(blob.raw()) })?;
    let mut out = Vec::new();
    for source in list.get().items(MAX_SOURCES) {
        // SAFETY: `blob` is a live snapshot and `source` an element of its list;
        // the dictionary is borrowed from `blob` (the Get rule).
        let description = unsafe { ffi::IOPSGetPowerSourceDescription(blob.raw(), source.raw()) };
        if let Some(description) = blob.lent(description)
            && is_internal(description, keys)
        {
            out.push(pack_from(description, keys));
        }
    }
    Some(out)
}

/// Whether the machine has an internal battery; `None` when the power sources
/// could not be read.
#[cfg(target_os = "macos")]
pub(super) fn internal_battery() -> Option<bool> {
    let keys = Keys::new().ok()?;
    packs(&keys).map(|p| !p.is_empty())
}

#[cfg(target_os = "macos")]
pub(crate) fn has_battery() -> bool {
    internal_battery() == Some(true)
}

/// Health and cycles from `AppleSmartBattery`. On Apple silicon its `MaxCapacity`
/// is a percentage, so the capacity in mAh is `AppleRawMaxCapacity`, compared
/// with `DesignCapacity`. These are registry properties, not API: a missing one
/// is `None`.
#[cfg(target_os = "macos")]
fn smart_battery() -> (Option<Percent>, Option<u32>) {
    let Some(battery) = Io::service(c"AppleSmartBattery") else {
        return (None, None);
    };
    let int = |name: &str| {
        let key = Cf::string(WHAT, name).ok()?;
        battery.property(&key)?.get().int()
    };
    let health = int("AppleRawMaxCapacity")
        .zip(int("DesignCapacity"))
        .and_then(|(full, design)| health_of(full, design));
    let cycles = int("CycleCount").and_then(|n| u32::try_from(n).ok());
    (health, cycles)
}

/// Each internal battery. No snapshot counts as no battery: a desktop has none
/// to report. `AppleSmartBattery` is one service, so its figures apply only
/// when there is one pack.
#[cfg(target_os = "macos")]
pub(crate) fn batteries() -> Result<Vec<RawBattery>> {
    let keys = Keys::new()?;
    let packs = packs(&keys).unwrap_or_default();
    let (health, cycles) = match packs.as_slice() {
        [_] => smart_battery(),
        _ => (None, None),
    };
    Ok(packs.iter().map(|p| p.raw(health, cycles)).collect())
}

/// `IOPSGetProvidingPowerSourceType`; `None` when it gives no answer.
#[cfg(target_os = "macos")]
pub(crate) fn on_ac() -> Result<Option<bool>> {
    let Some(blob) = snapshot() else {
        return Ok(None);
    };
    // SAFETY: `blob` is a live snapshot; the string is borrowed, not owned.
    let source = unsafe { ffi::IOPSGetProvidingPowerSourceType(blob.raw()) };
    let mut buf = [0u8; 32];
    Ok(blob
        .lent(source)
        .and_then(|s| s.text(&mut buf))
        .and_then(mains_from))
}

/// iOS answers no public call.
#[cfg(not(target_os = "macos"))]
pub(crate) fn has_battery() -> bool {
    false
}

/// iOS answers no public call.
#[cfg(not(target_os = "macos"))]
pub(crate) fn batteries() -> Result<Vec<RawBattery>> {
    Err(super::unsupported("batteries"))
}

/// iOS answers no public call.
#[cfg(not(target_os = "macos"))]
pub(crate) fn on_ac() -> Result<Option<bool>> {
    Err(super::unsupported("mains power"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(current: i64, feed: Feed) -> Pack {
        Pack {
            current: Some(current),
            max: Some(100),
            feed,
            ..Pack::default()
        }
    }

    #[test]
    fn test_power_apple_states() {
        assert_eq!(pack(50, Feed::Battery).state(), BatteryState::Discharging);
        assert_eq!(pack(50, Feed::Mains).state(), BatteryState::NotCharging);
        assert_eq!(pack(100, Feed::Mains).state(), BatteryState::Full);
        assert_eq!(pack(50, Feed::Other).state(), BatteryState::Unknown);
        let charging = Pack {
            charging: true,
            ..pack(50, Feed::Mains)
        };
        assert_eq!(charging.state(), BatteryState::Charging);
        let charged = Pack {
            charged: true,
            charging: true,
            ..pack(100, Feed::Mains)
        };
        assert_eq!(charged.state(), BatteryState::Full);
    }

    #[test]
    fn test_power_apple_missing_capacity_is_unknown() {
        // E-83.
        for p in [
            Pack::default(),
            Pack {
                max: Some(0),
                ..pack(50, Feed::Mains)
            },
            Pack {
                current: Some(-1),
                ..pack(50, Feed::Battery)
            },
            Pack {
                max: None,
                ..pack(50, Feed::Battery)
            },
        ] {
            let raw = p.raw(None, None);
            assert_eq!(raw.battery.state, BatteryState::Unknown, "{p:?}");
            assert_eq!(raw.battery.charge, Percent::ZERO);
            assert_eq!(raw.energy, None);
        }
    }

    #[test]
    fn test_power_apple_times_only_in_their_state() {
        let discharging = Pack {
            to_empty: Some(90),
            to_full: Some(0),
            ..pack(42, Feed::Battery)
        };
        let raw = discharging.raw(None, Some(7));
        assert_eq!(raw.energy, Some((42, 100)));
        assert_eq!(raw.battery.charge.to_string(), "42.0%");
        assert_eq!(raw.battery.time_to_empty, Some(Duration::from_secs(5400)));
        assert_eq!(raw.battery.time_to_full, None);
        assert_eq!(raw.battery.cycles, Some(7));

        // -1 is "still calculating".
        let calculating = Pack {
            to_empty: Some(-1),
            ..pack(42, Feed::Battery)
        };
        assert_eq!(calculating.raw(None, None).battery.time_to_empty, None);

        let charging = Pack {
            charging: true,
            to_full: Some(30),
            to_empty: Some(500),
            ..pack(42, Feed::Mains)
        };
        let raw = charging.raw(None, None);
        assert_eq!(raw.battery.time_to_full, Some(Duration::from_secs(1800)));
        assert_eq!(raw.battery.time_to_empty, None);
    }

    #[test]
    fn test_power_apple_health_and_mains() {
        assert_eq!(health_of(4200, 5000).unwrap().to_string(), "84.0%");
        // Calibration can read above design.
        assert_eq!(health_of(4395, 4382), Some(Percent::FULL));
        assert_eq!(health_of(0, 5000), None);
        assert_eq!(health_of(100, 0), None);
        assert_eq!(health_of(-1, 5000), None);
        assert_eq!(mains_from(b"AC Power"), Some(true));
        assert_eq!(mains_from(b"Battery Power"), Some(false));
        assert_eq!(mains_from(b"UPS Power"), Some(false));
        assert_eq!(mains_from(b"Off Line"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn test_power_apple_live_readings() {
        let list = batteries().unwrap();
        assert_eq!(has_battery(), !list.is_empty());
        for b in &list {
            assert!(b.battery.charge <= Percent::FULL);
        }
        on_ac().unwrap();
    }
}
