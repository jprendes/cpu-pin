use std::os::windows::io::{AsRawHandle, OwnedHandle, RawHandle};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
};
use windows::Win32::System::SystemInformation::{
    GetLogicalProcessorInformationEx, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows::Win32::System::Threading::{
    GetCurrentThread, OpenThread, ResumeThread, SetProcessDefaultCpuSetMasks,
    SetThreadGroupAffinity, THREAD_SET_INFORMATION, THREAD_SUSPEND_RESUME,
};

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

    unsafe {
        let thread = GetCurrentThread();
        pin_thread_to_cpu(thread.0 as RawHandle, cpu_id)?;
    }

    Ok(())
}

#[cfg(test)] // only used in tests
pub fn get_current_cpu_affinity() -> Result<Vec<usize>, Error> {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessDefaultCpuSetMasks};

    let process = unsafe { GetCurrentProcess() };
    let mut count: u16 = 0;
    unsafe {
        let _ = GetProcessDefaultCpuSetMasks(process, None, &mut count);
    }

    if count == 0 {
        // No CPU set masks configured — all CPUs are available.
        let topo = crate::topology()?;
        let mut all_cpus: Vec<usize> = topo
            .cores
            .iter()
            .flat_map(|c| c.logical_cpus.iter().copied())
            .collect();
        all_cpus.sort();
        return Ok(all_cpus);
    }

    let mut masks =
        vec![windows::Win32::System::SystemInformation::GROUP_AFFINITY::default(); count as usize];
    unsafe {
        let _ = GetProcessDefaultCpuSetMasks(process, Some(&mut masks), &mut count);
    }

    let mut cpus: Vec<usize> = masks
        .iter()
        .flat_map(|m| {
            let group = m.Group as usize;
            (0..usize::BITS as usize)
                .filter(move |&i| m.Mask & (1 << i) != 0)
                .map(move |i| group * 64 + i)
        })
        .collect();
    cpus.sort();
    Ok(cpus)
}

/// Pin a thread to the specified logical CPU.
///
/// # Safety
///
/// `thread` must be a valid thread handle with permission to set affinity.
unsafe fn pin_thread_to_cpu(thread: RawHandle, cpu_id: usize) -> Result<(), Error> {
    let group = (cpu_id / 64) as u16;
    let bit = cpu_id % 64;
    let affinity = windows::Win32::System::SystemInformation::GROUP_AFFINITY {
        Mask: 1usize << bit,
        Group: group,
        Reserved: [0; 3],
    };
    let result = SetThreadGroupAffinity(HANDLE(thread as _), &affinity, None);
    if !result.as_bool() {
        return Err(Error::Os(std::io::Error::last_os_error()));
    }
    Ok(())
}

/// Set CPU affinity on a suspended Windows process and resume its main thread.
///
/// # Safety
///
/// The process identified by `pid` must have been created suspended.
/// `process_handle` must be a valid handle to the process with PROCESS_SET_INFORMATION access.
pub(crate) unsafe fn pin_and_resume_windows(cpu_id: usize, process_handle: RawHandle, pid: u32) {
    let group = (cpu_id / 64) as u16;
    let bit = cpu_id % 64;
    let affinity = windows::Win32::System::SystemInformation::GROUP_AFFINITY {
        Mask: 1usize << bit,
        Group: group,
        Reserved: [0; 3],
    };
    // Set process-level CPU affinity (group-aware, Windows 11+)
    let _ = SetProcessDefaultCpuSetMasks(HANDLE(process_handle as _), Some(&[affinity]));

    if let Some(thread) = find_main_thread_handle(pid) {
        let raw = thread.as_raw_handle();
        // Best effort thread pinning
        let _ = pin_thread_to_cpu(raw, cpu_id);
        ResumeThread(HANDLE(raw as _));
    }
}

/// Find the main thread of a process and return an owned handle to it.
fn find_main_thread_handle(pid: u32) -> Option<OwnedHandle> {
    use std::os::windows::io::FromRawHandle;

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).ok()?;
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        if Thread32First(snapshot, &mut entry).is_err() {
            let _ = windows::Win32::Foundation::CloseHandle(snapshot);
            return None;
        }
        loop {
            if entry.th32OwnerProcessID == pid {
                let _ = windows::Win32::Foundation::CloseHandle(snapshot);
                let thread = OpenThread(
                    THREAD_SUSPEND_RESUME | THREAD_SET_INFORMATION,
                    false,
                    entry.th32ThreadID,
                )
                .ok()?;
                return Some(OwnedHandle::from_raw_handle(thread.0 as RawHandle));
            }
            if Thread32Next(snapshot, &mut entry).is_err() {
                break;
            }
        }
        let _ = windows::Win32::Foundation::CloseHandle(snapshot);
        None
    }
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
