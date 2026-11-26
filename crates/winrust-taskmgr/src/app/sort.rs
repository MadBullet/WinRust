//! The order and the filter of the process list: the sortable columns and the
//! stock "apps / background" split.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Cpu,
    Memory,
    Disk,
    Network,
    Gpu,
    /// "GPU engine" is text (`3D`, `Copy`, ...): compare it as a string.
    GpuEngine,
    /// Suspended / normal / not responding.
    Status,
    /// Base priority class.
    Priority,
    Pid,
    /// Details tab only: the owning account.
    UserName,
    /// Details tab only: the image path.
    Path,
}

/// Which rows a view shows: the stock "Apps" / "Background" split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowFilter {
    All,
    Apps,
    Background,
}

/// The Task Manager's "update speed".
///
/// Windows offers exactly these in the right-click menu on the process list.
/// `Instant` is the "промедленно/сразу" one: every sample the sampler produces
/// is shown as soon as it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateInterval {
    Instant,
    OneSecond,
    ThreeSeconds,
    FiveSeconds,
}

impl UpdateInterval {
    /// The entries of the context menu, in order.
    pub const ALL: [UpdateInterval; 4] = [
        UpdateInterval::OneSecond,
        UpdateInterval::ThreeSeconds,
        UpdateInterval::FiveSeconds,
        UpdateInterval::Instant,
    ];

    /// The menu label, already translated.
    pub fn label(self) -> &'static str {
        match self {
            UpdateInterval::Instant => winrust_core::i18n::ts("High"),
            UpdateInterval::OneSecond => winrust_core::i18n::ts("Normal (1 s)"),
            UpdateInterval::ThreeSeconds => winrust_core::i18n::ts("Low (3 s)"),
            UpdateInterval::FiveSeconds => winrust_core::i18n::ts("Paused (5 s)"),
        }
    }

    /// How long a sample must wait before it is shown.
    pub fn hold(self) -> std::time::Duration {
        match self {
            UpdateInterval::Instant => std::time::Duration::ZERO,
            UpdateInterval::OneSecond => std::time::Duration::from_secs(1),
            UpdateInterval::ThreeSeconds => std::time::Duration::from_secs(3),
            UpdateInterval::FiveSeconds => std::time::Duration::from_secs(5),
        }
    }
}
