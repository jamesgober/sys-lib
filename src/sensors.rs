//! Temperatures.
//!
//! Only documented, unprivileged sources are read. Linux has them for most
//! sensors (`hwmon`, thermal zones); Windows has them for NVMe drives only, and
//! macOS for none (Apple silicon sensors need private HID APIs and Intel SMC
//! keys are undocumented), so both answer what they can and nothing else.
//! Inside Docker and WSL the sensors are usually hidden: an empty list, not an
//! error.
//!
//! ```
//! use sys_lib as sys;
//!
//! match sys::sensors::temperatures() {
//!     Ok(list) => {
//!         for t in list {
//!             println!("{:<16} {:?} {:.1} C (critical {:?})", t.label, t.kind, t.celsius, t.critical);
//!         }
//!     }
//!     Err(e) if e.is_unavailable() => println!("no temperatures here"),
//!     Err(e) => return Err(e),
//! }
//! # Ok::<(), sys_lib::Error>(())
//! ```

use crate::Result;

/// One temperature sensor.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub struct Temperature {
    /// What the OS calls it: "Package id 0", "Composite", "edge".
    pub label: String,
    /// What it measures.
    pub kind: SensorKind,
    /// The reading.
    pub celsius: f32,
    /// Where the hardware starts to complain, when it says.
    pub high: Option<f32>,
    /// Where the hardware shuts down or throttles hard, when it says.
    pub critical: Option<f32>,
}

/// What a sensor measures.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum SensorKind {
    /// The CPU package or a core.
    Cpu,
    /// A graphics adapter.
    Gpu,
    /// A drive.
    Storage,
    /// A battery.
    Battery,
    /// The board, the chassis, or an ACPI zone.
    Ambient,
    /// Anything else.
    Other,
}

impl SensorKind {
    /// A short lowercase name: `"cpu"`, `"storage"`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "gpu",
            Self::Storage => "storage",
            Self::Battery => "battery",
            Self::Ambient => "ambient",
            Self::Other => "other",
        }
    }
}

/// Every temperature the OS reports without privileges, in the order it lists
/// them. Readings outside -60 to 250 degrees are left out: they are absent
/// sensors that report a placeholder.
///
/// # Errors
///
/// `Unsupported` on macOS, iOS, and targets without a backend; `Io` when the
/// sensor directory cannot be listed.
pub fn temperatures() -> Result<Vec<Temperature>> {
    crate::sys::temperatures()
}

/// Whether a reading is a temperature rather than a placeholder.
#[allow(dead_code)] // unused on targets without a backend
pub(crate) fn plausible(celsius: f32) -> bool {
    (-60.0..=250.0).contains(&celsius)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sensors_live_list_or_unavailable() {
        match temperatures() {
            Ok(list) => {
                for t in list {
                    assert!(plausible(t.celsius), "{t:?}");
                    assert!(!t.label.is_empty());
                }
            }
            Err(e) => assert!(e.is_unavailable(), "{e}"),
        }
    }

    #[test]
    fn test_sensors_plausible_bounds() {
        assert!(plausible(45.0) && plausible(-20.0));
        assert!(!plausible(-273.0) && !plausible(511.0) && !plausible(f32::NAN));
    }
}
