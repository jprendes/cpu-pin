use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadGroupAffinity};

use crate::{CoreType, CpuInfo, Error};

/// Information about a single physical core from Windows.
struct PhysicalCoreRaw {
    efficiency_class: u8,
    logical_cpus: Vec<usize>,
}

/// Enumerate all physical CPU cores and their core types on Windows.
///
/// Uses `GetLogicalProcessorInformationEx` with `RelationProcessorCore` to read
/// the `EfficiencyClass` field for each physical core.
pub fn cpus() -> Result<Vec<CpuInfo>, Error> {
    let raw_cores = get_processor_info()?;

    // Determine max/min efficiency class to map to Performance/Efficiency
    let max_efficiency = raw_cores
        .iter()
        .map(|c| c.efficiency_class)
        .max()
        .unwrap_or(0);
    let min_efficiency = raw_cores
        .iter()
        .map(|c| c.efficiency_class)
        .min()
        .unwrap_or(0);

    let cpus: Vec<CpuInfo> = raw_cores
        .into_iter()
        .enumerate()
        .map(|(idx, core)| {
            let core_type = if max_efficiency == min_efficiency {
                CoreType::Unknown
            } else if core.efficiency_class == max_efficiency {
                CoreType::Performance
            } else {
                CoreType::Efficiency
            };

            CpuInfo {
                id: idx,
                core_type,
                max_frequency_mhz: None,
                logical_cpus: core.logical_cpus,
            }
        })
        .collect();

    Ok(cpus)
}

/// Pin the current thread to the specified logical CPU.
pub fn validate_cpu_id(cpu_id: usize) -> Result<(), Error> {
    let topo = crate::topology().map_err(|_| Error::InvalidCpuId(cpu_id))?;
    let valid = topo.cores.iter().any(|c| c.logical_cpus.contains(&cpu_id));
    if !valid {
        return Err(Error::InvalidCpuId(cpu_id));
    }
    Ok(())
}

pub fn pin_cpu(cpu_id: usize) -> Result<(), Error> {
    validate_cpu_id(cpu_id)?;

    let group = (cpu_id / 64) as u16;
    let bit = cpu_id % 64;

    unsafe {
        let thread = GetCurrentThread();
        let affinity = windows::Win32::System::SystemInformation::GROUP_AFFINITY {
            Mask: 1usize << bit,
            Group: group,
            Reserved: [0; 3],
        };
        let result = SetThreadGroupAffinity(thread, &affinity, None);
        if !result.as_bool() {
            return Err(Error::Os(std::io::Error::last_os_error()));
        }
    }

    Ok(())
}

/// Query processor information from Windows, returning one entry per physical core.
fn get_processor_info() -> Result<Vec<PhysicalCoreRaw>, Error> {
    // First call to get buffer size
    let mut buffer_size: u32 = 0;
    unsafe {
        let _ = GetLogicalProcessorInformationEx(RelationProcessorCore, None, &mut buffer_size);
    }

    if buffer_size == 0 {
        return Err(Error::Os(std::io::Error::last_os_error()));
    }

    // Allocate buffer and make the real call
    let mut buffer: Vec<u8> = vec![0u8; buffer_size as usize];
    unsafe {
        GetLogicalProcessorInformationEx(
            RelationProcessorCore,
            Some(buffer.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX),
            &mut buffer_size,
        )
        .map_err(|e| Error::Os(std::io::Error::from_raw_os_error(e.code().0)))?;
    }

    // Parse the variable-length entries — one per physical core
    let mut result = Vec::new();
    let mut offset = 0usize;

    while offset < buffer_size as usize {
        let entry = unsafe {
            &*(buffer.as_ptr().add(offset) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX)
        };

        if entry.Relationship == RelationProcessorCore {
            let proc_info = unsafe { &entry.Anonymous.Processor };
            let efficiency_class = proc_info.EfficiencyClass;
            let mut logical_cpus = Vec::new();

            let group_count = proc_info.GroupCount as usize;
            for g in 0..group_count {
                let group_affinity = &proc_info.GroupMask[g];
                let group_num = group_affinity.Group as usize;
                let mut mask = group_affinity.Mask;
                let mut bit_pos = 0usize;
                while mask != 0 {
                    if mask & 1 != 0 {
                        logical_cpus.push(group_num * 64 + bit_pos);
                    }
                    bit_pos += 1;
                    mask >>= 1;
                }
            }

            result.push(PhysicalCoreRaw {
                efficiency_class,
                logical_cpus,
            });
        }

        offset += entry.Size as usize;
    }

    Ok(result)
}
