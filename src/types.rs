use std::fmt;

/// The type of a CPU core in a hybrid architecture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreType {
    /// A performance core (P-core). Higher clock speeds, more power consumption.
    Performance,
    /// An efficiency core (E-core). Lower clock speeds, less power consumption.
    Efficiency,
    /// Core type could not be determined (homogeneous system or unsupported detection).
    Unknown,
}

impl fmt::Display for CoreType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CoreType::Performance => write!(f, "Performance"),
            CoreType::Efficiency => write!(f, "Efficiency"),
            CoreType::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Information about a physical CPU core.
#[derive(Debug, Clone)]
pub struct CpuInfo {
    /// Sequential index for this physical core (0-based, assigned by this crate).
    /// This is NOT a hardware ID — it is a stable enumeration index derived from
    /// the OS-reported topology. Cores are ordered by (package, core_id) on Linux,
    /// by performance level on macOS, and by API enumeration order on Windows.
    pub id: usize,
    /// The core type (Performance, Efficiency, or Unknown).
    pub core_type: CoreType,
    /// Maximum frequency in MHz, if available.
    pub max_frequency_mhz: Option<u64>,
    /// Logical CPU IDs (hardware threads) belonging to this physical core.
    pub logical_cpus: Vec<usize>,
}

/// The CPU topology of the system.
#[derive(Debug, Clone)]
pub struct CpuTopology {
    /// Whether the system has a hybrid architecture (mix of P-cores and E-cores).
    pub is_hybrid: bool,
    /// All physical cores in the system.
    pub cores: Vec<CpuInfo>,
}

impl CpuTopology {
    /// Get only the performance cores.
    pub fn performance_cores(&self) -> Vec<&CpuInfo> {
        self.cores
            .iter()
            .filter(|c| c.core_type == CoreType::Performance)
            .collect()
    }

    /// Get only the efficiency cores.
    pub fn efficiency_cores(&self) -> Vec<&CpuInfo> {
        self.cores
            .iter()
            .filter(|c| c.core_type == CoreType::Efficiency)
            .collect()
    }

    /// Get the best cores for compute workloads.
    ///
    /// On hybrid systems, returns the performance cores.
    /// On homogeneous systems, returns all cores.
    pub fn best_cores(&self) -> Vec<&CpuInfo> {
        if self.is_hybrid {
            self.performance_cores()
        } else {
            self.cores.iter().collect()
        }
    }
}

/// Errors that can occur during CPU detection or pinning.
#[derive(Debug)]
pub enum Error {
    /// The requested CPU ID does not exist.
    InvalidCpuId(usize),
    /// Thread pinning is not supported on this platform (e.g., macOS).
    PinningNotSupported,
    /// An OS-level error occurred.
    Os(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidCpuId(id) => write!(f, "invalid CPU ID: {id}"),
            Error::PinningNotSupported => {
                write!(f, "thread pinning is not supported on this platform")
            }
            Error::Os(e) => write!(f, "OS error: {e}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Os(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Os(e)
    }
}
