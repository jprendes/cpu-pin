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
//! ## Features
//!
//! | Feature | Description |
//! |---------|-------------|
//! | `tokio` | Implements [`PinnedCommand`] for [`tokio::process::Command`] |
//!
//! ```toml
//! [dependencies]
//! cpu-pin = { version = "0.1", features = ["tokio"] }
//! ```
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
//!
//! ### Spawn a process pinned to a CPU (tokio)
//!
//! Requires the `tokio` feature.
//!
//! ```no_run
//! use tokio::process::Command;
//! use cpu_pin::{topology, PinnedCommand};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() {
//! let topo = topology().unwrap();
//! let cpu = topo.best_cores()[0].logical_cpus[0];
//!
//! let child = Command::new("my-program")
//!     .spawn_pinned(cpu)
//!     .unwrap();
//! let output = child.wait_with_output().await.unwrap();
//! # }
//! ```

mod pinned_command;
#[cfg(feature = "tokio")]
mod tokio_pinned_command;
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
mod tests;
