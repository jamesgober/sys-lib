//! Environment and reports on Linux (0.6): the file-tree logic in
//! `crate::linux::report` applied to the real `/`, plus `getrlimit` for the
//! locked-memory limit.

use std::path::Path;

use super::{ROOT, ffi, live::rlimit};
use crate::{
    Result,
    bench::BenchFacts,
    disk::Disk,
    gpu::Gpu,
    linux::report as tree,
    readiness::{DataDirTuning, Fact, MemLock, Tuning},
    security::Posture,
    sensors::Temperature,
};

pub(crate) fn temperatures() -> Result<Vec<Temperature>> {
    tree::temperatures(&ROOT)
}

pub(crate) fn gpus() -> Result<Vec<Gpu>> {
    tree::gpus(&ROOT)
}

pub(crate) fn tuning() -> Tuning {
    Tuning {
        memlock: Fact::of(rlimit(ffi::RLIMIT_MEMLOCK)).map(|l| MemLock::Limit { soft: l.soft }),
        ..tree::tuning(&ROOT)
    }
}

pub(crate) fn data_dir_tuning(path: &Path, _disk: &Disk) -> DataDirTuning {
    match std::fs::canonicalize(path) {
        Ok(path) => tree::data_dir_tuning(&ROOT, &path),
        Err(e) => {
            let why = format!("the data directory: {e}");
            DataDirTuning {
                atime: Fact::Unknown(why.clone()),
                scheduler: Fact::Unknown(why),
            }
        }
    }
}

pub(crate) fn bench_facts() -> BenchFacts {
    tree::bench_facts(&ROOT)
}

pub(crate) fn posture() -> Posture {
    tree::posture(&ROOT, crate::virt::is_container())
}
