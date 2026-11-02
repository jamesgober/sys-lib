//! `/sys/class/power_supply/*/uevent`: `POWER_SUPPLY_KEY=value` lines, read once
//! per battery so one reading is one consistent read.

use super::{lines, parse_u64, trim};

/// At most this many lines are read.
const MAX_LINES: usize = 256;

/// A battery's fields; the sysfs units (micro-amp and micro-watt hours,
/// microseconds never appear here) are kept as they are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Supply {
    pub(crate) status: Status,
    pub(crate) capacity: Option<u64>,
    pub(crate) energy_now: Option<u64>,
    pub(crate) energy_full: Option<u64>,
    pub(crate) energy_design: Option<u64>,
    pub(crate) charge_now: Option<u64>,
    pub(crate) charge_full: Option<u64>,
    pub(crate) charge_design: Option<u64>,
    pub(crate) power_now: Option<u64>,
    pub(crate) current_now: Option<u64>,
    pub(crate) cycles: Option<u64>,
}

/// `POWER_SUPPLY_STATUS`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Status {
    Charging,
    Discharging,
    Full,
    NotCharging,
    #[default]
    Unknown,
}

/// Parses a `uevent` file. Values that do not parse stay `None` (E-83).
pub(crate) fn parse(data: &[u8]) -> Supply {
    let mut s = Supply::default();
    for line in lines(data, MAX_LINES) {
        let Some(pos) = line.iter().position(|&b| b == b'=') else {
            continue;
        };
        let (key, value) = (&line[..pos], trim(&line[pos + 1..]));
        let Some(key) = key.strip_prefix(b"POWER_SUPPLY_") else {
            continue;
        };
        let n = parse_u64(value);
        match key {
            b"STATUS" => {
                s.status = match value {
                    b"Charging" => Status::Charging,
                    b"Discharging" => Status::Discharging,
                    b"Full" => Status::Full,
                    b"Not charging" => Status::NotCharging,
                    _ => Status::Unknown,
                };
            }
            b"CAPACITY" => s.capacity = n,
            b"ENERGY_NOW" => s.energy_now = n,
            b"ENERGY_FULL" => s.energy_full = n,
            b"ENERGY_FULL_DESIGN" => s.energy_design = n,
            b"CHARGE_NOW" => s.charge_now = n,
            b"CHARGE_FULL" => s.charge_full = n,
            b"CHARGE_FULL_DESIGN" => s.charge_design = n,
            b"POWER_NOW" => s.power_now = n,
            b"CURRENT_NOW" => s.current_now = n,
            b"CYCLE_COUNT" => s.cycles = n,
            _ => {}
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uevent_energy_battery() {
        let s = parse(
            b"POWER_SUPPLY_NAME=BAT0\nPOWER_SUPPLY_STATUS=Discharging\n\
              POWER_SUPPLY_CAPACITY=80\nPOWER_SUPPLY_ENERGY_NOW=40000000\n\
              POWER_SUPPLY_ENERGY_FULL=50000000\nPOWER_SUPPLY_ENERGY_FULL_DESIGN=60000000\n\
              POWER_SUPPLY_POWER_NOW=10000000\nPOWER_SUPPLY_CYCLE_COUNT=312\n",
        );
        assert_eq!(s.status, Status::Discharging);
        assert_eq!(s.capacity, Some(80));
        assert_eq!(
            (s.energy_now, s.energy_full, s.energy_design),
            (Some(40_000_000), Some(50_000_000), Some(60_000_000))
        );
        assert_eq!((s.power_now, s.cycles), (Some(10_000_000), Some(312)));
    }

    #[test]
    fn test_uevent_unknown_and_junk() {
        let s = parse(b"POWER_SUPPLY_STATUS=Weird\nPOWER_SUPPLY_CAPACITY=abc\nnoise\n=x\n\xff=\xff\n");
        assert_eq!(s, Supply::default());
        assert_eq!(parse(b"POWER_SUPPLY_STATUS=Not charging").status, Status::NotCharging);
    }
}
