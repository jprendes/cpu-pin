use std::process::{Child, Command};

use crate::{platform, Error};

/// Extension trait for [`Command`] that adds the ability to spawn a process
/// pinned to a specific logical CPU.
pub trait PinnedCommand {
    /// Spawn the command with CPU affinity set to the specified logical CPU.
    ///
    /// The child process will be pinned to `cpu_id` before it begins executing.
    /// If the OS-level pinning call fails, the error is ignored and the process
    /// runs without affinity constraints.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidCpuId`] if `cpu_id` does not correspond to a valid logical CPU.
    /// - [`Error::Os`] if spawning the process fails.
    fn spawn_pinned(&mut self, cpu_id: usize) -> Result<Child, Error>;
}

#[cfg(unix)]
impl PinnedCommand for Command {
    fn spawn_pinned(&mut self, cpu_id: usize) -> Result<Child, Error> {
        use std::os::unix::process::CommandExt;

        // Validate cpu_id before spawning
        platform::validate_cpu_id(cpu_id)?;

        unsafe {
            self.pre_exec(move || {
                // Ignore pinning failure — best effort
                let _ = platform::pin_cpu(cpu_id);
                Ok(())
            });
        }

        self.spawn().map_err(Error::Os)
    }
}

#[cfg(windows)]
impl PinnedCommand for Command {
    fn spawn_pinned(&mut self, cpu_id: usize) -> Result<Child, Error> {
        use std::os::windows::io::AsRawHandle;
        use std::os::windows::process::CommandExt;

        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::Threading::{
            ResumeThread, SetProcessAffinityMask, CREATE_SUSPENDED,
        };

        platform::validate_cpu_id(cpu_id)?;

        // Spawn suspended so we can set affinity before it runs
        self.creation_flags(CREATE_SUSPENDED.0);
        let child = self.spawn().map_err(Error::Os)?;

        let handle = HANDLE(child.as_raw_handle() as _);
        let mask = 1usize << (cpu_id % 64);

        unsafe {
            // Ignore failure — best effort pinning
            let _ = SetProcessAffinityMask(handle, mask);

            // Resume the main thread
            use windows::Win32::System::Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD,
                THREADENTRY32,
            };
            use windows::Win32::System::Threading::{OpenThread, THREAD_SUSPEND_RESUME};

            let pid = child.id();
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if let Ok(snapshot) = snapshot {
                let mut entry = THREADENTRY32 {
                    dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
                    ..Default::default()
                };
                if Thread32First(snapshot, &mut entry).is_ok() {
                    loop {
                        if entry.th32OwnerProcessID == pid {
                            if let Ok(thread) =
                                OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)
                            {
                                ResumeThread(thread);
                                let _ = windows::Win32::Foundation::CloseHandle(thread);
                            }
                            break;
                        }
                        if Thread32Next(snapshot, &mut entry).is_err() {
                            break;
                        }
                    }
                }
                let _ = windows::Win32::Foundation::CloseHandle(snapshot);
            }
        }

        Ok(child)
    }
}
