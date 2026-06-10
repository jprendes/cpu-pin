//! # cpu-pin
//!
//! Cross-platform CPU core type detection and thread pinning.
//!
//! This crate provides:
//! - Detection of physical CPU cores with their type (Performance/Efficiency)
//! - Mapping of logical CPUs (hardware threads) to physical cores
//! - Per-core maximum frequency information (where available)
//! - Thread pinning to specific logical CPUs
//!
//! ## Platform Support
//!
//! | Platform    | Core Detection | Pinning |
//! |-------------|---------------|---------|
//! | Linux x86   | Intel PMU sysfs (`cpu_core`/`cpu_atom`) | `sched_setaffinity` |
//! | Linux ARM   | `cpu_capacity` sysfs | `sched_setaffinity` |
//! | macOS ARM   | `hw.nperflevels` sysctl | Not supported (returns error) |
//! | Windows x86 | `EfficiencyClass` via `GetLogicalProcessorInformationEx` | `SetThreadAffinityMask` |
//! | Windows ARM | `EfficiencyClass` via `GetLogicalProcessorInformationEx` | `SetThreadAffinityMask` |
//!
//! ## Example
//!
//! ```no_run
//! use cpu_pin::{topology, pin_cpu, CoreType};
//!
//! // Discover the CPU topology
//! let topo = topology().unwrap();
//! println!("Hybrid: {}", topo.is_hybrid);
//!
//! // List all physical cores
//! for core in &topo.cores {
//!     println!("Core {}: {} threads={:?} freq={:?}",
//!         core.id, core.core_type, core.logical_cpus, core.max_frequency_mhz);
//! }
//!
//! // Pin current thread to first logical CPU of first P-core
//! if let Some(p_core) = topo.performance_cores().first() {
//!     pin_cpu(p_core.logical_cpus[0]).unwrap();
//! }
//! ```

mod types;

use once_cell::sync::OnceCell;
pub use types::{CoreType, CpuInfo, CpuTopology, Error};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as platform;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
use windows as platform;

static TOPOLOGY: OnceCell<CpuTopology> = OnceCell::new();

/// Discover the CPU topology of the system.
///
/// The topology is detected once and cached for the lifetime of the process.
/// Subsequent calls return a reference to the cached result.
///
/// Returns a [`CpuTopology`] containing all physical cores and whether the
/// system has a hybrid architecture (mix of Performance and Efficiency cores).
///
/// # Errors
///
/// Returns [`Error::Os`] if an OS-level error occurs during detection.
pub fn topology() -> Result<&'static CpuTopology, Error> {
    TOPOLOGY.get_or_try_init(|| {
        let cores = platform::cpus()?;
        let has_p = cores.iter().any(|c| c.core_type == CoreType::Performance);
        let has_non_p = cores.iter().any(|c| c.core_type != CoreType::Performance);
        let is_hybrid = has_p && has_non_p;
        Ok(CpuTopology { is_hybrid, cores })
    })
}

/// Pin the current thread to the specified logical CPU.
///
/// After a successful call, the current thread will only execute on the specified CPU.
///
/// # Errors
///
/// - [`Error::InvalidCpuId`] if `cpu_id` does not correspond to a valid logical CPU.
/// - [`Error::PinningNotSupported`] on macOS, which does not support hard CPU affinity.
/// - [`Error::Os`] if the OS call fails.
pub fn pin_cpu(cpu_id: usize) -> Result<(), Error> {
    platform::pin_cpu(cpu_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topology_returns_nonempty() {
        let topo = topology().unwrap();
        assert!(
            !topo.cores.is_empty(),
            "should detect at least one physical core"
        );
    }

    #[test]
    fn ids_are_sequential() {
        let topo = topology().unwrap();
        for (i, core) in topo.cores.iter().enumerate() {
            assert_eq!(core.id, i);
        }
    }

    #[test]
    fn every_core_has_at_least_one_logical_cpu() {
        let topo = topology().unwrap();
        for core in &topo.cores {
            assert!(
                !core.logical_cpus.is_empty(),
                "core {} has no logical CPUs",
                core.id
            );
        }
    }

    #[test]
    fn logical_cpus_are_unique_across_cores() {
        let topo = topology().unwrap();
        let mut seen = std::collections::HashSet::new();
        for core in &topo.cores {
            for &lp in &core.logical_cpus {
                assert!(
                    seen.insert(lp),
                    "logical CPU {} appears in multiple physical cores",
                    lp
                );
            }
        }
    }

    #[test]
    fn total_logical_cpus_matches_system() {
        let topo = topology().unwrap();
        let total: usize = topo.cores.iter().map(|c| c.logical_cpus.len()).sum();
        let expected = num_cpus_online();
        assert_eq!(
            total, expected,
            "total logical CPUs should match system count"
        );
    }

    #[test]
    fn performance_and_efficiency_are_subsets() {
        let topo = topology().unwrap();
        let p = topo.performance_cores();
        let e = topo.efficiency_cores();
        assert!(p.len() + e.len() <= topo.cores.len());
        for core in &p {
            assert_eq!(core.core_type, CoreType::Performance);
        }
        for core in &e {
            assert_eq!(core.core_type, CoreType::Efficiency);
        }
    }

    #[test]
    fn is_hybrid_consistent_with_core_types() {
        let topo = topology().unwrap();
        let has_p = topo
            .cores
            .iter()
            .any(|c| c.core_type == CoreType::Performance);
        let has_non_p = topo
            .cores
            .iter()
            .any(|c| c.core_type != CoreType::Performance);
        assert_eq!(topo.is_hybrid, has_p && has_non_p);
    }

    #[test]
    fn pin_to_valid_cpu_succeeds() {
        let topo = topology().unwrap();
        let lp = topo.cores[0].logical_cpus[0];
        // On macOS this will return PinningNotSupported, which is still correct behavior
        let result = pin_cpu(lp);
        assert!(
            result.is_ok() || matches!(result, Err(Error::PinningNotSupported)),
            "pin_cpu should succeed or return PinningNotSupported, got: {:?}",
            result
        );
    }

    #[test]
    fn pin_to_invalid_cpu_fails() {
        let result = pin_cpu(99999);
        assert!(result.is_err());
    }

    fn num_cpus_online() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }
}
