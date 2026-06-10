//! # cpu-pin
//!
//! Cross-platform CPU core type detection and thread pinning.
//!
//! This crate provides:
//! - Detection of physical CPU cores with their type (Performance/Efficiency)
//! - Mapping of logical CPUs (hardware threads) to physical cores
//! - Per-core maximum frequency information (where available)
//! - Thread pinning to specific logical CPUs
//! - Spawning child processes with CPU affinity pre-set ([`PinnedCommand`] trait)
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
//!
//! ### Spawn a process pinned to a CPU
//!
//! ```no_run
//! use std::process::Command;
//! use cpu_pin::{topology, PinnedCommand};
//!
//! let topo = topology().unwrap();
//! let cpu = topo.best_cores()[0].logical_cpus[0];
//!
//! let mut child = Command::new("my-program")
//!     .spawn_pinned(cpu)
//!     .unwrap();
//! child.wait().unwrap();
//! ```

mod pinned_command;
mod types;

use once_cell::sync::OnceCell;
pub use pinned_command::PinnedCommand;
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

    #[test]
    #[cfg(any(target_os = "linux", target_os = "windows"))] // "macOS pinning is a no-op"
    fn spawn_pinned_runs_on_correct_cpu() {
        use std::process::Command;

        let topo = topology().unwrap();
        let lp = topo
            .cores
            .last()
            .unwrap()
            .logical_cpus
            .last()
            .copied()
            .unwrap();

        // Spawn the test binary itself, running the ignored helper test pinned to `lp`
        let test_bin = std::env::current_exe().unwrap();
        let output = Command::new(&test_bin)
            .args([
                "--exact",
                "tests::report_cpu_affinity",
                "--ignored",
                "--nocapture",
            ])
            .stdout(std::process::Stdio::piped())
            .spawn_pinned(lp)
            .unwrap()
            .wait_with_output()
            .unwrap();

        assert!(
            output.status.success(),
            "helper test failed: {:?}",
            output.status
        );
        let stdout = String::from_utf8_lossy(&output.stdout);

        // The helper prints "CPU_AFFINITY=<list>"
        let expected = format!("CPU_AFFINITY={lp}\n");
        println!("Expected to find: {expected}");
        println!("Full output:\n{stdout}");
        assert!(
            stdout.contains(&expected),
            "expected CPU {lp} in affinity, got: {stdout}"
        );
    }

    /// Helper test that reports its own CPU affinity. Not run normally.
    #[test]
    #[ignore = "used as target for spawn_pinned test"]
    fn report_cpu_affinity() {
        #[cfg(target_os = "linux")]
        {
            let content = std::fs::read_to_string("/proc/self/status").unwrap();
            let line = content
                .lines()
                .find(|l| l.starts_with("Cpus_allowed_list:"))
                .unwrap();
            let list = line.split(':').nth(1).unwrap().trim();
            println!("CPU_AFFINITY={list}");
        }

        #[cfg(target_os = "macos")]
        {
            // macOS doesn't support hard affinity
            // report something that would never match
            println!("CPU_AFFINITY=<unknown>");
        }

        #[cfg(windows)]
        {
            use ::windows::Win32::System::Threading::{GetCurrentProcess, GetProcessAffinityMask};
            let mut process_mask: usize = 0;
            let mut system_mask: usize = 0;
            unsafe {
                let _ = GetProcessAffinityMask(
                    GetCurrentProcess(),
                    &mut process_mask,
                    &mut system_mask,
                );
            }
            // Convert bitmask to comma-separated list of CPU IDs
            let cpus: Vec<String> = (0..usize::BITS)
                .filter(|&i| process_mask & (1 << i) != 0)
                .map(|i| i.to_string())
                .collect();
            println!("CPU_AFFINITY={}", cpus.join(","));
        }
    }

    #[test]
    fn spawn_pinned_invalid_cpu_fails() {
        use std::process::Command;

        let result = Command::new("echo").arg("hi").spawn_pinned(99999);
        assert!(matches!(result, Err(Error::InvalidCpuId(99999))));
    }

    #[test]
    fn spawn_pinned_captures_output() {
        use std::process::Command;

        let topo = topology().unwrap();
        let lp = topo.cores[0].logical_cpus[0];

        let test_bin = std::env::current_exe().unwrap();
        let output = Command::new(&test_bin)
            .args(["--exact", "tests::print_pinned", "--ignored", "--nocapture"])
            .stdout(std::process::Stdio::piped())
            .spawn_pinned(lp)
            .unwrap()
            .wait_with_output()
            .unwrap();

        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("pinned"));
    }

    /// Helper test that prints "pinned". Not run normally.
    #[test]
    #[ignore = "used as target for spawn_pinned test"]
    fn print_pinned() {
        println!("pinned");
    }

    fn num_cpus_online() -> usize {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }
}
