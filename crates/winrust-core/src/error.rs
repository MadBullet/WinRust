//! The single error type used by every WinRust component.

use std::fmt;

pub type Result<T> = std::result::Result<T, WinError>;

/// A Win32 failure (or a WinRust-level invariant violation).
#[derive(Debug, Clone)]
pub enum WinError {
    /// The last-error code returned by a Win32 call.
    Api {
        code: u32,
        message: String,
        context: &'static str,
    },
    /// Access denied — very common for services / other users' processes.
    AccessDenied { context: &'static str },
    /// The object does not exist (process exited, key deleted, ...).
    NotFound { context: &'static str },
    /// A buffer-size negotiation failed unexpectedly.
    Buffer { context: &'static str },
    /// The caller passed something structurally invalid.
    Invalid(String),
    /// Anything else, with a human-readable message.
    Other(String),
}

impl WinError {
    pub fn from_last_error(context: &'static str) -> Self {
        let code = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        Self::from_code(code, context)
    }

    pub fn from_code(code: u32, context: &'static str) -> Self {
        match code {
            windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED => Self::AccessDenied { context },
            windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND
            | windows_sys::Win32::Foundation::ERROR_PATH_NOT_FOUND
            | windows_sys::Win32::Foundation::ERROR_SERVICE_DOES_NOT_EXIST
            | windows_sys::Win32::Foundation::ERROR_INVALID_HANDLE => Self::NotFound { context },
            windows_sys::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER
            | windows_sys::Win32::Foundation::ERROR_MORE_DATA => Self::Buffer { context },
            _ => Self::Api {
                code,
                message: message_for(code),
                context,
            },
        }
    }

    /// True when retrying with more privileges (UAC elevation) may help.
    pub fn is_access_denied(&self) -> bool {
        matches!(self, Self::AccessDenied { .. })
    }

    pub fn code(&self) -> Option<u32> {
        match self {
            Self::Api { code, .. } => Some(*code),
            Self::AccessDenied { .. } => Some(5),
            Self::NotFound { .. } => Some(2),
            _ => None,
        }
    }
}

impl fmt::Display for WinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Api {
                code,
                message,
                context,
            } => {
                write!(f, "{context}: {message} (0x{code:08X})")
            }
            Self::AccessDenied { context } => {
                write!(f, "{context}: access denied — run WinRust elevated")
            }
            Self::NotFound { context } => write!(f, "{context}: object not found"),
            Self::Buffer { context } => write!(f, "{context}: buffer negotiation failed"),
            Self::Invalid(m) => write!(f, "invalid argument: {m}"),
            Self::Other(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for WinError {}

/// Turn a `BOOL` return value into a `Result`, capturing `GetLastError()`.
#[inline]
pub fn check(context: &'static str, ok: i32) -> Result<()> {
    if ok != 0 {
        Ok(())
    } else {
        Err(WinError::from_last_error(context))
    }
}

/// Human-readable message for a Win32 error code.
pub fn message_for(code: u32) -> String {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        FormatMessageW, FORMAT_MESSAGE_FROM_SYSTEM, FORMAT_MESSAGE_IGNORE_INSERTS,
    };
    let mut buf = [0u16; 1024];
    let n = unsafe {
        FormatMessageW(
            FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
            std::ptr::null(),
            code,
            0,
            buf.as_mut_ptr(),
            buf.len() as u32,
            std::ptr::null(),
        )
    };
    if n == 0 {
        return format!("unknown Win32 error {code}");
    }
    let s = crate::wide::from_wide(&buf, n as usize);
    s.trim().to_string()
}
