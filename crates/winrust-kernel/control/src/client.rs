//! User-mode side of the `\\.\WinRust` protocol.
//!
//! Opens the control device, sends one METHOD_BUFFERED `DeviceIoControl` per
//! query and decodes the reply.  Nothing here is privileged: the driver's device
//! ACL decides who may open it, and every operation is a read.

use std::fmt;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use winrust_core::wide::to_wide;
use winrust_kernel_abi::{
    ModuleRecord, Performance, ProcessRecord, Query, Request, ResponseHeader, IOCTL_WINRUST_QUERY,
    MAX_RECORDS,
};

/// `GENERIC_READ` - the device only needs read access for queries.
const GENERIC_READ: u32 = 0x8000_0000;

/// A failed driver interaction.
#[derive(Debug)]
pub enum DriverError {
    /// Could not open `\\.\WinRust` - the driver is not loaded, or access denied.
    NotLoaded { code: u32 },
    /// The driver answered with an error status.
    Status { status: i32 },
    /// The reply was shorter or larger than the protocol allows.
    Protocol(String),
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotLoaded { code } => write!(
                f,
                "the WinRust driver is not loaded (CreateFile failed with {code}). \
                 Install it with `winrust-driver install` from an elevated prompt."
            ),
            Self::Status { status } => {
                write!(f, "driver returned NTSTATUS {status:#010X}")
            }
            Self::Protocol(msg) => write!(f, "protocol mismatch: {msg}"),
        }
    }
}

impl std::error::Error for DriverError {}

/// An open handle to the control device, closed on drop.
pub struct Driver {
    handle: HANDLE,
}

impl Driver {
    /// Open `\\.\WinRust`.
    pub fn connect() -> Result<Self, DriverError> {
        let path = to_wide(r"\\.\WinRust");
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(DriverError::NotLoaded {
                code: std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
            });
        }
        Ok(Self { handle })
    }

    /// Send one query and return the raw response bytes.
    fn call(&self, request: &Request, payload: &mut [u8]) -> Result<ResponseHeader, DriverError> {
        let mut returned: u32 = 0;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_WINRUST_QUERY,
                (request as *const Request).cast(),
                std::mem::size_of::<Request>() as u32,
                payload.as_mut_ptr().cast(),
                payload.len() as u32,
                &raw mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(DriverError::Status {
                status: std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            });
        }
        if (returned as usize) < std::mem::size_of::<ResponseHeader>() {
            return Err(DriverError::Protocol(format!(
                "only {returned} bytes returned"
            )));
        }
        // SAFETY: the driver wrote a `ResponseHeader` at the start of `payload`
        // and reported at least that many bytes.
        let header = unsafe { std::ptr::read_unaligned(payload.as_ptr().cast::<ResponseHeader>()) };
        if header.status != 0 {
            return Err(DriverError::Status {
                status: header.status,
            });
        }
        if header.version == 0 {
            return Err(DriverError::Protocol("driver reports version 0".into()));
        }
        Ok(header)
    }

    /// `Ping`: returns `(protocol version, processors, page size)`.
    pub fn ping(&self) -> Result<(u32, u32, u32), DriverError> {
        let mut buffer = vec![0u8; winrust_kernel_abi::RESPONSE_SIZE];
        let request = Request {
            query: Query::Ping as u32,
            offset: 0,
            limit: 1,
            reserved: 0,
        };
        let header = self.call(&request, &mut buffer)?;
        Ok((header.version, header.processors, header.page_size))
    }

    /// Every process, paging through the table `MAX_RECORDS` at a time.
    pub fn processes(&self) -> Result<Vec<ProcessRecord>, DriverError> {
        self.paged(Query::Processes, |bytes| from_bytes::<ProcessRecord>(bytes))
    }

    /// Every loaded kernel module.
    pub fn modules(&self) -> Result<Vec<ModuleRecord>, DriverError> {
        self.paged(Query::Modules, |bytes| from_bytes::<ModuleRecord>(bytes))
    }

    /// Aggregate counters.
    pub fn performance(&self) -> Result<Performance, DriverError> {
        let mut buffer = vec![0u8; winrust_kernel_abi::RESPONSE_SIZE];
        let request = Request {
            query: Query::Performance as u32,
            offset: 0,
            limit: 1,
            reserved: 0,
        };
        let header = self.call(&request, &mut buffer)?;
        if header.count == 0 {
            return Err(DriverError::Protocol("no performance record".into()));
        }
        Ok(from_bytes::<Performance>(&buffer[header_bytes()..])
            .first()
            .copied()
            .unwrap_or_default())
    }

    fn paged<T: Copy>(
        &self,
        query: Query,
        decode: impl Fn(&[u8]) -> Vec<T>,
    ) -> Result<Vec<T>, DriverError> {
        let mut out = Vec::new();
        let mut offset = 0u32;
        loop {
            let mut buffer = vec![0u8; winrust_kernel_abi::RESPONSE_SIZE];
            let request = Request {
                query: query as u32,
                offset,
                limit: MAX_RECORDS as u32,
                reserved: 0,
            };
            let header = self.call(&request, &mut buffer)?;
            let records = decode(&buffer[header_bytes()..]);
            let got = records.len().min(header.count as usize);
            out.extend_from_slice(&records[..got]);
            offset = offset.saturating_add(got as u32);
            if got == 0 || offset >= header.total {
                break;
            }
        }
        Ok(out)
    }
}

fn header_bytes() -> usize {
    std::mem::size_of::<ResponseHeader>()
}

/// Reinterpret the payload as a slice of `T`, ignoring any trailing padding.
fn from_bytes<T: Copy>(bytes: &[u8]) -> Vec<T> {
    let size = std::mem::size_of::<T>();
    if size == 0 || bytes.len() < size {
        return Vec::new();
    }
    let count = bytes.len() / size;
    let mut out = Vec::with_capacity(count);
    for index in 0..count {
        let start = index * size;
        // SAFETY: `start + size <= bytes.len()` by construction; `T` is `Copy`
        // and `repr(C)` over integers, so the read cannot produce an invalid
        // value.
        out.push(unsafe { std::ptr::read_unaligned(bytes.as_ptr().add(start).cast::<T>()) });
    }
    out
}

impl Drop for Driver {
    fn drop(&mut self) {
        if self.handle != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.handle) };
        }
    }
}
