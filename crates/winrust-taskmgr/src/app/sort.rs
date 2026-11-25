//! The order and the filter of the process list: the sortable columns and the
//! stock "apps / background" split.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SortBy {
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
pub(super) enum RowFilter {
    All,
    Apps,
    Background,
}
