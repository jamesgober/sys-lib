//! Power on Linux: `/sys/class/power_supply`. A battery counts when its `type`
//! is `Battery` and its `scope` is not `Device`, which is how the kernel marks a
//! wireless mouse, keyboard, or headset (E-81).

use core::time::Duration;

use super::Root;
use crate::{
    Error, Percent, Result,
    parse::uevent::{self, Status, Supply},
    power::{Battery, BatteryState, RawBattery},
};

const CLASS: &str = "/sys/class/power_supply";
/// The most supplies looked at (T7).
const MAX_SUPPLIES: usize = 256;

fn path(name: &str, file: &str) -> String {
    format!("{CLASS}/{name}/{file}")
}

/// A name the kernel lists: never a path.
fn valid(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.starts_with('.')
}

fn is_system_battery(root: &Root, name: &str) -> bool {
    root.text(&path(name, "type")).as_deref().map(str::trim) == Some("Battery")
        && root.text(&path(name, "scope")).as_deref().map(str::trim) != Some("Device")
}

fn supplies(root: &Root) -> Vec<String> {
    let mut names = root.dir_names(CLASS, MAX_SUPPLIES);
    names.retain(|n| valid(n));
    names.sort();
    names
}

/// Whether a system battery exists.
pub(crate) fn has_battery(root: &Root) -> bool {
    supplies(root).iter().any(|n| is_system_battery(root, n))
}

/// A share of two figures, when the second is not zero.
fn share(part: Option<u64>, whole: Option<u64>) -> Option<Percent> {
    Some(Percent::from_ratio_u128(
        u128::from(part?),
        u128::from(whole.filter(|&w| w != 0)?),
    ))
}

/// `amount` at `rate`: micro-unit hours over micro-units, so hours.
fn duration(amount: Option<u64>, rate: Option<u64>) -> Option<Duration> {
    let (amount, rate) = (amount?, rate.filter(|&r| r != 0)?);
    let seconds = u128::from(amount) * 3600 / u128::from(rate);
    Some(Duration::from_secs(u64::try_from(seconds).ok()?))
}

/// One battery from its `uevent`.
pub(crate) fn battery(s: &Supply) -> RawBattery {
    // Energy (micro-watt hours) where the driver reports it, else charge
    // (micro-amp hours).
    let (now, full, design, rate) = if s.energy_full.is_some() {
        (s.energy_now, s.energy_full, s.energy_design, s.power_now)
    } else {
        (s.charge_now, s.charge_full, s.charge_design, s.current_now)
    };
    let charge = s
        .capacity
        .map(|c| Percent::from_value(c.min(100) as f32))
        .or_else(|| share(now, full))
        .unwrap_or(Percent::ZERO);
    let state = match s.status {
        Status::Charging => BatteryState::Charging,
        Status::Discharging => BatteryState::Discharging,
        Status::Full => BatteryState::Full,
        Status::NotCharging => BatteryState::NotCharging,
        Status::Unknown => BatteryState::Unknown,
    };
    let to_full = match (now, full) {
        (Some(n), Some(f)) => Some(f.saturating_sub(n)),
        _ => None,
    };
    RawBattery {
        battery: Battery {
            charge,
            state,
            time_to_empty: (state == BatteryState::Discharging)
                .then(|| duration(now, rate))
                .flatten(),
            time_to_full: (state == BatteryState::Charging)
                .then(|| duration(to_full, rate))
                .flatten(),
            health: share(full, design),
            cycles: s.cycles.and_then(|c| u32::try_from(c).ok()).filter(|&c| c != 0),
        },
        energy: now.zip(full),
    }
}

/// Every system battery, in name order.
///
/// # Errors
///
/// `PermissionDenied` when SELinux refuses the class directory or a `uevent`
/// (E-90).
pub(crate) fn batteries(root: &Root) -> Result<Vec<RawBattery>> {
    let mut out = Vec::new();
    for name in supplies(root) {
        if !is_system_battery(root, &name) {
            continue;
        }
        let file = path(&name, "uevent");
        match root.with(&file, "battery", "/sys/class/power_supply", uevent::parse) {
            Ok(s) => out.push(battery(&s)),
            Err(Error::NotFound { .. }) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

/// Whether mains or USB power is present; `None` with no such supply.
pub(crate) fn on_ac(root: &Root) -> Option<bool> {
    let mut seen = false;
    for name in supplies(root) {
        let kind = root.text(&path(&name, "type")).unwrap_or_default();
        if !matches!(kind.trim(), "Mains" | "USB" | "USB_C" | "USB_PD") {
            continue;
        }
        seen = true;
        if root.text(&path(&name, "online")).as_deref().map(str::trim) == Some("1") {
            return Some(true);
        }
    }
    seen.then_some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux::hostile::Tree;

    fn laptop(tag: &str) -> Tree {
        let t = Tree::new(tag);
        t.put("/sys/class/power_supply/BAT0/type", b"Battery\n");
        t.put("/sys/class/power_supply/BAT0/scope", b"System\n");
        t.put(
            "/sys/class/power_supply/BAT0/uevent",
            b"POWER_SUPPLY_STATUS=Discharging\nPOWER_SUPPLY_ENERGY_NOW=40000000\n\
              POWER_SUPPLY_ENERGY_FULL=50000000\nPOWER_SUPPLY_ENERGY_FULL_DESIGN=60000000\n\
              POWER_SUPPLY_POWER_NOW=10000000\nPOWER_SUPPLY_CAPACITY=80\n\
              POWER_SUPPLY_CYCLE_COUNT=312\n",
        );
        t.put("/sys/class/power_supply/AC/type", b"Mains\n");
        t.put("/sys/class/power_supply/AC/online", b"0\n");
        // E-81: a wireless mouse.
        t.put("/sys/class/power_supply/hidpp_battery_0/type", b"Battery\n");
        t.put("/sys/class/power_supply/hidpp_battery_0/scope", b"Device\n");
        t.put(
            "/sys/class/power_supply/hidpp_battery_0/uevent",
            b"POWER_SUPPLY_CAPACITY=3\n",
        );
        t
    }

    #[test]
    fn test_power_laptop_tree() {
        let t = laptop("power-tree");
        let root = t.root();
        assert!(has_battery(&root));
        let all = batteries(&root).unwrap();
        assert_eq!(all.len(), 1, "the mouse is left out");
        let b = all[0].battery;
        assert_eq!(b.charge.to_string(), "80.0%");
        assert_eq!(b.state, BatteryState::Discharging);
        assert_eq!(b.time_to_empty, Some(Duration::from_secs(4 * 3600)));
        assert_eq!(b.time_to_full, None);
        assert_eq!(b.health.unwrap().to_string(), "83.3%");
        assert_eq!(b.cycles, Some(312));
        assert_eq!(all[0].energy, Some((40_000_000, 50_000_000)));
        assert_eq!(on_ac(&root), Some(false));
    }

    #[test]
    fn test_power_on_mains() {
        let t = laptop("power-mains");
        t.put("/sys/class/power_supply/AC/online", b"1\n");
        assert_eq!(on_ac(&t.root()), Some(true));
    }

    #[test]
    fn test_power_desktop_says_nothing() {
        let t = Tree::new("desktop");
        assert!(!has_battery(&t.root()));
        assert!(batteries(&t.root()).unwrap().is_empty());
        assert_eq!(on_ac(&t.root()), None);
    }

    #[test]
    fn test_power_charge_units_and_unknown_status() {
        // E-83: charge figures, no capacity, unknown status.
        let s = uevent::parse(
            b"POWER_SUPPLY_CHARGE_NOW=1000000\nPOWER_SUPPLY_CHARGE_FULL=2000000\n\
              POWER_SUPPLY_CURRENT_NOW=500000\n",
        );
        let b = battery(&s).battery;
        assert_eq!(b.charge.to_string(), "50.0%");
        assert_eq!(b.state, BatteryState::Unknown);
        assert_eq!(b.time_to_empty, None);
        // Charging: the time to the top.
        let s = uevent::parse(
            b"POWER_SUPPLY_STATUS=Charging\nPOWER_SUPPLY_CHARGE_NOW=1000000\n\
              POWER_SUPPLY_CHARGE_FULL=2000000\nPOWER_SUPPLY_CURRENT_NOW=500000\n",
        );
        assert_eq!(battery(&s).battery.time_to_full, Some(Duration::from_secs(2 * 3600)));
        // Nothing at all, and a zero rate.
        let b = battery(&Supply::default()).battery;
        assert_eq!((b.charge, b.health, b.cycles), (Percent::ZERO, None, None));
        assert_eq!(duration(Some(5), Some(0)), None);
    }
}
