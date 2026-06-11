use std::process::Command;

use crate::{platform, Error};

/// Extension trait for [`Command`] that adds the ability to spawn a process
/// pinned to a specific logical CPU.
///
/// Implemented for [`std::process::Command`] and, with the `tokio` feature,
/// for [`tokio::process::Command`].
pub trait PinnedCommand {
    /// The child process type returned on success.
    type Child;

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
    fn spawn_pinned(&mut self, cpu_id: usize) -> Result<Self::Child, Error>;
}

#[cfg(unix)]
impl PinnedCommand for Command {
    type Child = std::process::Child;

    fn spawn_pinned(&mut self, cpu_id: usize) -> Result<Self::Child, Error> {
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
    type Child = std::process::Child;

    fn spawn_pinned(&mut self, cpu_id: usize) -> Result<Self::Child, Error> {
        use std::os::windows::process::CommandExt;

        use windows::Win32::System::Threading::CREATE_SUSPENDED;

        platform::validate_cpu_id(cpu_id)?;

        // Spawn suspended so we can set affinity before it runs
        self.creation_flags(CREATE_SUSPENDED.0);
        let child = self.spawn().map_err(Error::Os)?;

        unsafe {
            platform::pin_and_resume_windows(cpu_id, child.id());
        }

        Ok(child)
    }
}
