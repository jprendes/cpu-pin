use cpu_pin::{pin_cpu, topology};

fn main() {
    let topo = topology().expect("failed to enumerate CPUs");

    println!("=== CPU Topology ===\n");
    println!("  Hybrid: {}\n", topo.is_hybrid);

    println!("=== Physical CPU Cores ===\n");
    for core in &topo.cores {
        let freq = core
            .max_frequency_mhz
            .map(|f| format!("{f} MHz"))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "  Core {:>2}: {:>12}  max_freq={}  threads={:?}",
            core.id,
            core.core_type.to_string(),
            freq,
            core.logical_cpus
        );
    }

    println!("\n=== Summary ===\n");
    let p_cores = topo.performance_cores();
    let e_cores = topo.efficiency_cores();
    println!(
        "  Performance cores: {} (threads: {:?})",
        p_cores.len(),
        p_cores
            .iter()
            .flat_map(|c| &c.logical_cpus)
            .collect::<Vec<_>>()
    );
    println!(
        "  Efficiency cores:  {} (threads: {:?})",
        e_cores.len(),
        e_cores
            .iter()
            .flat_map(|c| &c.logical_cpus)
            .collect::<Vec<_>>()
    );

    println!("\n=== Pinning Test ===\n");
    if let Some(first_core) = topo.cores.first() {
        let lp = first_core.logical_cpus[0];
        match pin_cpu(lp) {
            Ok(()) => println!(
                "  Successfully pinned to logical CPU {} (core {})",
                lp, first_core.id
            ),
            Err(e) => println!("  Failed to pin to logical CPU {}: {}", lp, e),
        }
    }
}
