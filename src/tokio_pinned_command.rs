use tokio::process::Command;

use crate::{platform, Error, PinnedCommand};

#[cfg(unix)]
impl PinnedCommand for Command {
    type Child = tokio::process::Child;

    fn spawn_pinned(&mut self, cpu_id: usize) -> Result<Self::Child, Error> {
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
    type Child = tokio::process::Child;

    fn spawn_pinned(&mut self, cpu_id: usize) -> Result<Self::Child, Error> {
        use windows::Win32::System::Threading::CREATE_SUSPENDED;

        platform::validate_cpu_id(cpu_id)?;

        // Spawn suspended so we can set affinity before it runs
        self.creation_flags(CREATE_SUSPENDED.0);
        let child = self.spawn().map_err(Error::Os)?;

        if let (Some(pid), Some(handle)) = (child.id(), child.raw_handle()) {
            unsafe {
                crate::platform::pin_and_resume_windows(cpu_id, handle, pid);
            }
        }

        Ok(child)
    }
}
