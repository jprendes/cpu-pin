# cpu-pin

Cross-platform CPU core type detection and thread pinning.

## Features

- Detection of physical CPU cores with their type (Performance / Efficiency)
- Mapping of logical CPUs (hardware threads) to physical cores
- Per-core maximum frequency information (where available)
- Hybrid topology detection (`is_hybrid`)
- Thread pinning to specific logical CPUs
- Spawning child processes with CPU affinity pre-set (`PinnedCommand` trait)

## Platform Support

| Platform    | Core Detection | Pinning |
|-------------|---------------|---------|
| Linux x86   | Intel PMU sysfs (`cpu_core`/`cpu_atom`) | `sched_setaffinity` |
| Linux ARM   | `cpu_capacity` sysfs | `sched_setaffinity` |
| macOS ARM   | `hw.nperflevels` sysctl | Not supported (returns error) |
| Windows x86 | `EfficiencyClass` via `GetLogicalProcessorInformationEx` | `SetThreadAffinityMask` |
| Windows ARM | `EfficiencyClass` via `GetLogicalProcessorInformationEx` | `SetThreadAffinityMask` |

## Features

| Feature | Description |
|---------|-------------|
| `tokio` | Implements `PinnedCommand` for `tokio::process::Command` |

## Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
cpu-pin = "0.1"
```

### Discover topology

```rust
use cpu_pin::{topology, CoreType};

let topo = topology().unwrap();
println!("Hybrid: {}", topo.is_hybrid);

for core in &topo.cores {
    println!("Core {}: {} threads={:?} freq={:?}",
        core.id, core.core_type, core.logical_cpus, core.max_frequency_mhz);
}

// Access P-cores and E-cores directly
let p_cores = topo.performance_cores();
let e_cores = topo.efficiency_cores();
```

### Pin current thread

```rust
use cpu_pin::{topology, pin_cpu};

let topo = topology().unwrap();

// Pin to the first logical CPU of the first P-core
if let Some(p_core) = topo.performance_cores().first() {
    pin_cpu(p_core.logical_cpus[0]).unwrap();
}
```

### Spawn a process pinned to a CPU

```rust
use std::process::Command;
use cpu_pin::{topology, PinnedCommand};

let topo = topology().unwrap();
let cpu = topo.best_cores()[0].logical_cpus[0];

let mut child = Command::new("my-program")
    .spawn_pinned(cpu)
    .unwrap();
child.wait().unwrap();
```

### Spawn a process pinned to a CPU (tokio)

Enable the `tokio` feature:

```toml
[dependencies]
cpu-pin = { version = "0.1", features = ["tokio"] }
```

```rust
use tokio::process::Command;
use cpu_pin::{topology, PinnedCommand};

let topo = topology().unwrap();
let cpu = topo.best_cores()[0].logical_cpus[0];

let child = Command::new("my-program")
    .spawn_pinned(cpu)
    .unwrap();
let output = child.wait_with_output().await.unwrap();
```

On Linux and macOS, `spawn_pinned` uses `pre_exec` to call `sched_setaffinity` before
the child executes. On Windows, the process is created suspended, its affinity mask is
set, and then it is resumed.

## License

MIT
