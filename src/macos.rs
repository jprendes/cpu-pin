use std::ffi::CStr;
use std::mem;

use crate::{CoreType, CpuInfo, Error};

/// Call sysctlbyname and return the value as a u32.
fn sysctl_u32(name: &CStr) -> Option<u32> {
    let mut value: u32 = 0;
    let mut size = mem::size_of::<u32>();
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut value as *mut u32 as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 {
        Some(value)
    } else {
        None
    }
}

/// Call sysctlbyname and return the value as a u64.
fn sysctl_u64(name: &CStr) -> Option<u64> {
    let mut value: u64 = 0;
    let mut size = mem::size_of::<u64>();
    let ret = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut value as *mut u64 as *mut libc::c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if ret == 0 {
        Some(value)
    } else {
        None
    }
}

/// Get the total number of logical CPUs.
fn logical_cpu_count() -> usize {
    sysctl_u32(c"hw.logicalcpu_max")
        .or_else(|| sysctl_u32(c"hw.logicalcpu"))
        .unwrap_or(1) as usize
}

/// Get the number of physical CPUs.
fn physical_cpu_count() -> usize {
    sysctl_u32(c"hw.physicalcpu_max")
        .or_else(|| sysctl_u32(c"hw.physicalcpu"))
        .unwrap_or(1) as usize
}

/// Detect hybrid core layout on Apple Silicon using hw.nperflevels and hw.perflevelN.
/// Returns (p_physical, p_logical, e_physical, e_logical) or None if not hybrid.
fn detect_apple_silicon_hybrid() -> Option<(usize, usize, usize, usize)> {
    let nperflevels = sysctl_u32(c"hw.nperflevels")?;
    if nperflevels < 2 {
        return None;
    }

    // perflevel0 = highest performance (P-cores)
    // perflevel1 = lowest performance (E-cores)
    let p_physical = sysctl_u32(c"hw.perflevel0.physicalcpu")? as usize;
    let p_logical = sysctl_u32(c"hw.perflevel0.logicalcpu")? as usize;
    let e_physical = sysctl_u32(c"hw.perflevel1.physicalcpu")? as usize;
    let e_logical = sysctl_u32(c"hw.perflevel1.logicalcpu")? as usize;

    if p_physical == 0 && e_physical == 0 {
        return None;
    }

    Some((p_physical, p_logical, e_physical, e_logical))
}

/// Get CPU frequency for a performance level in MHz.
///
/// Tries multiple sysctl keys since availability varies across Apple Silicon generations.
/// Returns None if no frequency information is available for the given level.
fn perflevel_frequency_mhz(level: u32) -> Option<u64> {
    // Try the direct per-level frequency sysctl (in Hz)
    let hz = match level {
        0 => sysctl_u64(c"hw.perflevel0.cpuspeeds"),
        1 => sysctl_u64(c"hw.perflevel1.cpuspeeds"),
        _ => None,
    };
    if let Some(hz) = hz {
        return Some(hz / 1_000_000);
    }

    // Fallback: hw.cpufrequency_max gives the system-wide max (only useful for level 0)
    if level == 0 {
        if let Some(hz) = sysctl_u64(c"hw.cpufrequency_max") {
            return Some(hz / 1_000_000);
        }
    }

    None
}

/// Enumerate all physical CPU cores and their core types on macOS.
pub fn cpus() -> Result<Vec<CpuInfo>, Error> {
    let logical_count = logical_cpu_count();
    let physical_count = physical_cpu_count();
    let hybrid = detect_apple_silicon_hybrid();

    let mut cores = Vec::with_capacity(physical_count);

    match hybrid {
        Some((p_physical, p_logical, e_physical, e_logical)) => {
            let p_freq = perflevel_frequency_mhz(0);
            let e_freq = perflevel_frequency_mhz(1);

            // On Apple Silicon, P-cores come first, then E-cores.
            // Logical CPUs are distributed evenly across physical cores.
            let p_threads_per_core = p_logical.checked_div(p_physical).unwrap_or(1);
            let e_threads_per_core = e_logical.checked_div(e_physical).unwrap_or(1);

            let mut logical_id = 0usize;

            // P-cores
            for idx in 0..p_physical {
                let thread_count = p_threads_per_core;
                let logical_cpus: Vec<usize> = (logical_id..logical_id + thread_count).collect();
                logical_id += thread_count;

                cores.push(CpuInfo {
                    id: idx,
                    core_type: CoreType::Performance,
                    max_frequency_mhz: p_freq,
                    logical_cpus,
                });
            }

            // E-cores
            for i in 0..e_physical {
                let idx = p_physical + i;
                let thread_count = e_threads_per_core;
                let logical_cpus: Vec<usize> = (logical_id..logical_id + thread_count).collect();
                logical_id += thread_count;

                cores.push(CpuInfo {
                    id: idx,
                    core_type: CoreType::Efficiency,
                    max_frequency_mhz: e_freq,
                    logical_cpus,
                });
            }
        }
        None => {
            // Homogeneous system (Intel Mac or single perf level)
            let threads_per_core = logical_count.checked_div(physical_count).unwrap_or(1);

            for idx in 0..physical_count {
                let start = idx * threads_per_core;
                let logical_cpus: Vec<usize> = (start..start + threads_per_core).collect();

                cores.push(CpuInfo {
                    id: idx,
                    core_type: CoreType::Unknown,
                    max_frequency_mhz: None,
                    logical_cpus,
                });
            }
        }
    }

    Ok(cores)
}

pub fn validate_cpu_id(cpu_id: usize) -> Result<(), Error> {
    let cpu_count = logical_cpu_count();
    if cpu_id >= cpu_count {
        return Err(Error::InvalidCpuId(cpu_id));
    }
    Ok(())
}

/// Pin the current thread to the specified logical CPU.
///
/// **Note:** macOS does not support hard CPU affinity binding.
/// This function returns `Error::PinningNotSupported`.
/// On macOS, use QoS classes (`pthread_set_qos_class_np`) for soft scheduling hints.
pub fn pin_cpu(cpu_id: usize) -> Result<(), Error> {
    validate_cpu_id(cpu_id)?;

    // macOS does not support hard thread-to-CPU pinning.
    // thread_policy_set with THREAD_AFFINITY_POLICY only provides a "tag"
    // that hints the scheduler to co-locate threads with the same tag,
    // but does NOT pin to a specific CPU.
    Err(Error::PinningNotSupported)
}
