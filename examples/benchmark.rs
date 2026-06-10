//! Example: Run a workload pinned to a specific core type.
//!
//! Demonstrates how to use cpu-pin to run compute work exclusively on
//! performance cores (or fall back to any core on homogeneous systems).

use std::time::Instant;

use cpu_pin::{pin_cpu, topology, CoreType};

fn workload() -> u64 {
    // Simple compute-bound work
    let mut sum: u64 = 0;
    for i in 0..10_000_000u64 {
        sum = sum.wrapping_add(i.wrapping_mul(i));
    }
    sum
}

fn main() {
    let topo = topology().expect("failed to enumerate CPUs");

    // Pick a P-core and an E-core (if available)
    let p_core = topo
        .cores
        .iter()
        .find(|c| c.core_type == CoreType::Performance);
    let e_core = topo
        .cores
        .iter()
        .find(|c| c.core_type == CoreType::Efficiency);

    // Benchmark on a P-core
    if let Some(core) = p_core {
        let lp = core.logical_cpus[0];
        pin_cpu(lp).expect("failed to pin");
        let start = Instant::now();
        let result = workload();
        let elapsed = start.elapsed();
        println!(
            "P-core {} (logical CPU {}): {:?} (result={})",
            core.id, lp, elapsed, result
        );
    }

    // Benchmark on an E-core
    if let Some(core) = e_core {
        let lp = core.logical_cpus[0];
        pin_cpu(lp).expect("failed to pin");
        let start = Instant::now();
        let result = workload();
        let elapsed = start.elapsed();
        println!(
            "E-core {} (logical CPU {}): {:?} (result={})",
            core.id, lp, elapsed, result
        );
    }

    // If no hybrid cores detected, just run on first core
    if p_core.is_none() && e_core.is_none() {
        println!("Homogeneous system — no P/E distinction");
        let lp = topo.cores[0].logical_cpus[0];
        pin_cpu(lp).expect("failed to pin");
        let start = Instant::now();
        let result = workload();
        let elapsed = start.elapsed();
        println!(
            "Core 0 (logical CPU {}): {:?} (result={})",
            lp, elapsed, result
        );
    }
}
