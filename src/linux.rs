use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::Path;

use crate::{CoreType, CpuInfo, Error};

/// Parse a CPU list string like "0-7,16-23" into a set of CPU IDs.
fn parse_cpu_list(s: &str) -> HashSet<usize> {
    let mut set = HashSet::new();
    for part in s.trim().split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(s), Ok(e)) = (start.trim().parse::<usize>(), end.trim().parse::<usize>()) {
                for i in s..=e {
                    set.insert(i);
                }
            }
        } else if let Ok(id) = part.parse::<usize>() {
            set.insert(id);
        }
    }
    set
}

/// Get the number of online logical CPUs.
fn online_cpu_count() -> usize {
    // Try sysconf first
    unsafe {
        let n = libc::sysconf(libc::_SC_NPROCESSORS_ONLN);
        if n > 0 {
            return n as usize;
        }
    }
    1
}

/// Read max frequency for a CPU from sysfs (in MHz).
fn read_max_frequency(cpu_id: usize) -> Option<u64> {
    let path = format!("/sys/devices/system/cpu/cpu{cpu_id}/cpufreq/cpuinfo_max_freq");
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|khz| khz / 1000) // Convert KHz to MHz
}

/// Detect core types on x86_64 Intel hybrid CPUs using PMU sysfs.
/// Returns None if the system is not a hybrid Intel CPU.
fn detect_x86_hybrid() -> Option<Vec<(usize, CoreType)>> {
    let p_core_path = Path::new("/sys/devices/cpu_core/cpus");
    let e_core_path = Path::new("/sys/devices/cpu_atom/cpus");

    // Both files must exist for Intel hybrid detection
    if !p_core_path.exists() || !e_core_path.exists() {
        return None;
    }

    let p_cores_str = fs::read_to_string(p_core_path).ok()?;
    let e_cores_str = fs::read_to_string(e_core_path).ok()?;

    let p_cores = parse_cpu_list(&p_cores_str);
    let e_cores = parse_cpu_list(&e_cores_str);

    if p_cores.is_empty() && e_cores.is_empty() {
        return None;
    }

    let mut result = Vec::new();
    for &id in &p_cores {
        result.push((id, CoreType::Performance));
    }
    for &id in &e_cores {
        result.push((id, CoreType::Efficiency));
    }
    Some(result)
}

/// Detect core types using cpu_capacity sysfs.
/// Works for ARM big.LITTLE/DynamIQ and AMD hybrid (Zen 5 + Zen 5c) on kernels 6.x+.
/// Returns None if all cores have the same capacity (homogeneous).
fn detect_capacity_hybrid(cpu_count: usize) -> Option<Vec<(usize, CoreType)>> {
    let mut capacities: Vec<(usize, u64)> = Vec::new();

    for id in 0..cpu_count {
        let path = format!("/sys/devices/system/cpu/cpu{id}/cpu_capacity");
        if let Ok(s) = fs::read_to_string(&path) {
            if let Ok(cap) = s.trim().parse::<u64>() {
                capacities.push((id, cap));
            }
        }
    }

    if capacities.is_empty() {
        return None;
    }

    let max_cap = capacities.iter().map(|(_, c)| *c).max().unwrap_or(0);
    let min_cap = capacities.iter().map(|(_, c)| *c).min().unwrap_or(0);

    // If all capacities are the same, it's homogeneous
    if max_cap == min_cap {
        return None;
    }

    // Use a threshold: cores with capacity >= 80% of max are Performance
    let threshold = max_cap * 80 / 100;
    let result: Vec<(usize, CoreType)> = capacities
        .into_iter()
        .map(|(id, cap)| {
            let core_type = if cap >= threshold {
                CoreType::Performance
            } else {
                CoreType::Efficiency
            };
            (id, core_type)
        })
        .collect();

    Some(result)
}

/// Detect core types using per-CPU core_type sysfs attribute.
/// Available on some kernels that expose /sys/devices/system/cpu/cpuN/core_type.
/// Known values: "intel_atom" (E-core), "intel_core" (P-core).
fn detect_core_type_sysfs(cpu_count: usize) -> Option<Vec<(usize, CoreType)>> {
    let mut result = Vec::new();
    let mut any_found = false;

    for id in 0..cpu_count {
        let path = format!("/sys/devices/system/cpu/cpu{id}/core_type");
        if let Ok(s) = fs::read_to_string(&path) {
            any_found = true;
            let core_type = match s.trim() {
                "intel_core" => CoreType::Performance,
                "intel_atom" => CoreType::Efficiency,
                _ => CoreType::Unknown,
            };
            result.push((id, core_type));
        }
    }

    if !any_found {
        return None;
    }

    // Only return if there's actual heterogeneity
    let has_p = result.iter().any(|(_, ct)| *ct == CoreType::Performance);
    let has_e = result.iter().any(|(_, ct)| *ct == CoreType::Efficiency);
    if has_p || has_e {
        Some(result)
    } else {
        None
    }
}

/// Enumerate all physical CPU cores on Linux.
pub fn cpus() -> Result<Vec<CpuInfo>, Error> {
    let cpu_count = online_cpu_count();

    // Detection order:
    // 1. Intel PMU sysfs (cpu_core/cpu_atom)
    // 2. Per-CPU core_type sysfs attribute
    // 3. cpu_capacity (AMD hybrid / ARM big.LITTLE)
    let type_map: Vec<(usize, CoreType)> = detect_x86_hybrid()
        .or_else(|| detect_core_type_sysfs(cpu_count))
        .or_else(|| detect_capacity_hybrid(cpu_count))
        .unwrap_or_default();

    // Group logical CPUs by physical core using topology sysfs.
    // Key: (package_id, core_id) -> Vec<logical_cpu_id>
    let mut physical_cores: BTreeMap<(usize, usize), Vec<usize>> = BTreeMap::new();

    for id in 0..cpu_count {
        let topo_path = format!("/sys/devices/system/cpu/cpu{id}/topology");
        let package_id = fs::read_to_string(format!("{topo_path}/physical_package_id"))
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let core_id = fs::read_to_string(format!("{topo_path}/core_id"))
            .ok()
            .and_then(|s| s.trim().parse::<usize>().ok())
            .unwrap_or(id);

        physical_cores
            .entry((package_id, core_id))
            .or_default()
            .push(id);
    }

    // Build CpuInfo for each physical core
    let mut result = Vec::with_capacity(physical_cores.len());
    for (idx, ((_pkg, _core), logical_cpus)) in physical_cores.into_iter().enumerate() {
        // Core type from the first logical CPU in this physical core
        let core_type = logical_cpus
            .iter()
            .find_map(|&lp| type_map.iter().find(|(i, _)| *i == lp).map(|(_, ct)| *ct))
            .unwrap_or(CoreType::Unknown);

        // Frequency from the first logical CPU
        let max_frequency_mhz = logical_cpus.iter().find_map(|&lp| read_max_frequency(lp));

        result.push(CpuInfo {
            id: idx,
            core_type,
            max_frequency_mhz,
            logical_cpus,
        });
    }

    Ok(result)
}

/// Pin the current thread to the specified logical CPU.
pub fn validate_cpu_id(cpu_id: usize) -> Result<(), Error> {
    let cpu_count = online_cpu_count();
    if cpu_id >= cpu_count {
        return Err(Error::InvalidCpuId(cpu_id));
    }
    Ok(())
}

pub fn pin_cpu(cpu_id: usize) -> Result<(), Error> {
    validate_cpu_id(cpu_id)?;

    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu_id, &mut set);

        let ret = libc::sched_setaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &set);
        if ret != 0 {
            return Err(Error::Os(std::io::Error::last_os_error()));
        }
    }

    Ok(())
}

#[cfg(test)] // only used in tests
pub fn get_current_cpu_affinity() -> Result<Vec<usize>, Error> {
    unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        let ret = libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set);
        if ret != 0 {
            return Err(Error::Os(std::io::Error::last_os_error()));
        }

        let max_cpus = libc::CPU_SETSIZE as usize;
        let cpus = (0..max_cpus)
            .filter(|&i| libc::CPU_ISSET(i, &set))
            .collect();
        Ok(cpus)
    }
}
