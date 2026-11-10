//! Where a sampler reads from: this machine, through the platform backend, or
//! on Linux a host's `/proc` and `/sys` mounted elsewhere, given explicitly in
//! code through `probe::Probe` (never through the environment: threat T4).

use crate::{
    Result,
    cpu::Times,
    disk::DiskRaw,
    net::{Counters, Totals},
};

#[derive(Clone, Debug, Default)]
pub(crate) struct Origin {
    #[cfg(target_os = "linux")]
    host: Option<std::sync::Arc<Host>>,
}

/// A host tree, and the files that differ from this process's own view.
#[cfg(target_os = "linux")]
#[derive(Debug)]
pub(crate) struct Host {
    pub(crate) root: crate::linux::Root,
    /// `/proc/1/net/dev` when the host's process 1 is visible (its network
    /// namespace), else `/proc/net/dev`.
    pub(crate) net_dev: &'static str,
}

/// The most interfaces a host reading keeps (T7).
#[cfg(target_os = "linux")]
const MAX_INTERFACES: usize = 4096;

impl Origin {
    /// This machine.
    pub(crate) fn local() -> Origin {
        Origin::default()
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn host(host: std::sync::Arc<Host>) -> Origin {
        Origin { host: Some(host) }
    }

    pub(crate) fn cpu_times(&self) -> Result<Times> {
        #[cfg(target_os = "linux")]
        if let Some(h) = &self.host {
            return crate::linux::stat::cpu_times(&h.root, crate::sys::clk_tck());
        }
        crate::sys::cpu_times()
    }

    pub(crate) fn cpu_times_per_core(&self, out: &mut [Times]) -> Result<(Times, usize)> {
        #[cfg(target_os = "linux")]
        if let Some(h) = &self.host {
            return crate::linux::stat::cpu_times_per_core(&h.root, crate::sys::clk_tck(), out);
        }
        crate::sys::cpu_times_per_core(out)
    }

    pub(crate) fn disk_io_totals(&self) -> Result<DiskRaw> {
        #[cfg(target_os = "linux")]
        if let Some(h) = &self.host {
            return crate::linux::disk::io_totals(&h.root);
        }
        crate::sys::disk_io_totals()
    }

    pub(crate) fn disk_io_counters(&self) -> Result<Vec<(String, DiskRaw)>> {
        #[cfg(target_os = "linux")]
        if let Some(h) = &self.host {
            return crate::linux::disk::io_counters(&h.root);
        }
        crate::sys::disk_io_counters()
    }

    pub(crate) fn net_totals(&self) -> Result<Totals> {
        #[cfg(target_os = "linux")]
        if let Some(h) = &self.host {
            return crate::linux::net::totals_at(&h.root, h.net_dev);
        }
        crate::sys::net_totals()
    }

    pub(crate) fn net_counters(&self) -> Result<Vec<Counters>> {
        #[cfg(target_os = "linux")]
        if let Some(h) = &self.host {
            return crate::linux::net::counters_at(&h.root, h.net_dev, MAX_INTERFACES);
        }
        crate::sys::net_counters()
    }
}
