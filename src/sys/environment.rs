//! Environment and reports (0.6), answered "unknown", for the targets sys-lib
//! has no backend for yet (the `unsupported` module glob-imports it). Linux,
//! macOS, and Windows define every item themselves, so a missing item there is
//! a build error. The list below is the interface each backend provides.

#![allow(dead_code)]

use std::path::Path;

use crate::{
    Error, Result,
    bench::BenchFacts,
    disk::Disk,
    gpu::Gpu,
    readiness::{DataDirTuning, Tuning},
    security::Posture,
    sensors::Temperature,
};

fn no(what: &'static str) -> Error {
    Error::Unsupported {
        what,
        platform: crate::os::kind().name(),
    }
}

pub(crate) fn temperatures() -> Result<Vec<Temperature>> {
    Err(no("temperatures"))
}

pub(crate) fn gpus() -> Result<Vec<Gpu>> {
    Err(no("graphics adapters"))
}

/// The system settings the readiness report checks; `Fact::Skip` for each one
/// this platform does not have.
pub(crate) fn tuning() -> Tuning {
    Tuning::default()
}

/// The settings of the data directory `path`, on `disk` (from `disk::of`).
pub(crate) fn data_dir_tuning(_path: &Path, _disk: &Disk) -> DataDirTuning {
    DataDirTuning::default()
}

pub(crate) fn bench_facts() -> BenchFacts {
    BenchFacts::default()
}

/// The posture, `elevated` left `None` (`crate::security` fills it).
pub(crate) fn posture() -> Posture {
    Posture::default()
}

/// `USER_HZ`, for a probe on a Linux target without a backend: the value the
/// kernel fixes for every architecture sys-lib builds for.
pub(crate) fn clk_tck() -> u64 {
    100
}
