//! # WinRust driver wire protocol
//!
//! One `#[repr(C)]` request type, one response type, and two IOCTLs - shared
//! verbatim by the kernel driver and the user-mode client so the two can never
//! drift apart.  This crate has **no dependencies** because it is compiled into
//! the `no_std` driver as well.
//!
//! ```text
//! user mode                          kernel (winrust-kernel)
//! ----------                        -------------------------
//! DeviceIoControl(\\.\WinRust,   -->  EvtIoDeviceControl
//!   IOCTL_WINRUST_QUERY,               ioctl::handle_query
//!   &Request, sizeof,                  fills Response
//!   &Response, sizeof, ...)       <--
//! ```
//!
//! Everything is `METHOD_BUFFERED`: the I/O manager copies the request in and
//! the response out, so the driver never touches a user-mode address and a
//! malicious caller cannot make it dereference a bogus pointer.

#![no_std]
#![deny(clippy::all)]

/// Device name the client opens: `\\.\WinRust`, i.e. `\Device\WinRust`.
pub const DEVICE_NAME: &str = "\\Device\\WinRust";
/// Symbolic link that makes the device visible to user mode.
pub const SYMBOLIC_LINK: &str = "\\DosDevices\\WinRust";
/// `FILE_DEVICE_UNKNOWN`, the documented choice for a custom control device.
pub const DEVICE_TYPE: u32 = 0x22;

/// `CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_READ_DATA|FILE_WRITE_DATA)`
pub const IOCTL_WINRUST_QUERY: u32 = ctl_code(0x800, 0, 3);
/// `CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_WRITE_DATA)`
pub const IOCTL_WINRUST_SET_GUARD: u32 = ctl_code(0x801, 0, 2);
/// `CTL_CODE(FILE_DEVICE_UNKNOWN, 0x802, METHOD_BUFFERED, FILE_READ_DATA)`
pub const IOCTL_WINRUST_GET_VERSION: u32 = ctl_code(0x802, 0, 1);

/// The NT `CTL_CODE` macro, in `const fn` form.
#[must_use]
pub const fn ctl_code(function: u32, method: u32, access: u32) -> u32 {
    (DEVICE_TYPE << 16) | (access << 14) | (function << 2) | method
}

/// Which piece of information the client wants.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Query {
    /// The driver is loaded and can talk: version, CPU count, page size.
    Ping = 0,
    /// Every loaded kernel module with base address and size.
    Modules = 1,
    /// Every process with kernel-side CPU times, threads, handles, session.
    Processes = 2,
    /// Aggregate counters (CPU time, pool usage, process/thread counts).
    Performance = 3,
}

impl Query {
    #[must_use]
    pub const fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Ping),
            1 => Some(Self::Modules),
            2 => Some(Self::Processes),
            3 => Some(Self::Performance),
            _ => None,
        }
    }
}

/// Fixed-size record describing one process.
///
/// `#[repr(C)]` with only integer fields, so kernel and user mode agree byte
/// for byte.  First field doubles as the record count marker in the response.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessRecord {
    pub pid: u32,
    pub parent_pid: u32,
    pub threads: u32,
    pub handles: u32,
    pub session_id: u32,
    pub base_priority: i32,
    /// `100ns` units, matching `GetProcessTimes`.
    pub create_time: i64,
    pub user_time: i64,
    pub kernel_time: i64,
    pub working_set: u64,
    pub virtual_size: u64,
    pub pagefile_usage: u64,
    /// UTF-16 image name, zero padded.
    pub name: [u16; Self::NAME_CHARS],
}

impl ProcessRecord {
    pub const NAME_CHARS: usize = 64;
}

/// Fixed-size record describing one loaded kernel module.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ModuleRecord {
    pub base_address: u64,
    pub image_size: u32,
    pub flags: u32,
    /// UTF-16 full path, zero padded.
    pub name: [u16; Self::NAME_CHARS],
}

impl ModuleRecord {
    pub const NAME_CHARS: usize = 128;
}

/// How many records fit in one response.
///
/// `METHOD_BUFFERED` copies the response from non-paged pool, so the packet is
/// kept well under the 64 KiB `IOCTL_BUFFERED_MAX` and callers page through
/// the table with `offset`.
pub const MAX_RECORDS: usize = 96;

/// Request header, always the first `size_of::<Request>()` bytes of the input.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Request {
    /// One of [`Query`].
    pub query: u32,
    /// Index of the first record to return (paging).
    pub offset: u32,
    /// Upper bound on records to return, clamped to [`MAX_RECORDS`].
    pub limit: u32,
    pub reserved: u32,
}

/// Response header.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ResponseHeader {
    /// `NTSTATUS` of the operation; 0 means success.
    pub status: i32,
    /// `Query` that was answered.
    pub query: u32,
    /// Records written into the payload.
    pub count: u32,
    /// Records that exist in total (for paging).
    pub total: u32,
    /// Driver protocol version.
    pub version: u32,
    /// CPU count from `SystemBasicInformation`.
    pub processors: u32,
    /// Page size from `SystemBasicInformation`.
    pub page_size: u32,
    pub reserved: u32,
}

/// Aggregate counters answer for [`Query::Performance`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Performance {
    pub total_user_time: i64,
    pub total_kernel_time: i64,
    pub total_idle_time: i64,
    pub process_count: u32,
    pub thread_count: u32,
    pub handle_count: u32,
    pub committed_bytes: u64,
    pub paged_pool_bytes: u64,
    pub nonpaged_pool_bytes: u64,
}

/// Size of the response buffer the client must provide.
///
/// Always `size_of::<ResponseHeader>() + MAX_RECORDS * max(record size)`, so a
/// single allocation covers every query.
pub const RESPONSE_SIZE: usize = core::mem::size_of::<ResponseHeader>()
    + MAX_RECORDS * (core::mem::size_of::<ProcessRecord>());
