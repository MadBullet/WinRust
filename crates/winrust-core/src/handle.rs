//! RAII ownership for Win32 kernel handles.

use crate::error::{WinError, Result};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

/// An owned kernel handle.  Dropping it calls `CloseHandle` exactly once.
///
/// `HANDLE` is a raw pointer, so the wrapper is deliberately `!Send`/`!Sync`
/// only in the sense that it is not automatically — we do allow sending it
/// between threads because kernel handles are process-wide.
#[derive(Debug)]
pub struct Handle(HANDLE);

// A HANDLE is just an opaque kernel object reference; using it from another
// thread is legal and close-once semantics are preserved by the move.
unsafe impl Send for Handle {}

impl Handle {
    /// Wrap a handle returned by a Win32 call, validating it.
    ///
    /// Returns `None` for NULL and `INVALID_HANDLE_VALUE`.
    pub fn new(h: HANDLE) -> Option<Self> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(h))
        }
    }

    /// Wrap a handle and treat NULL as failure, attaching the last error.
    pub fn from_call(h: HANDLE, context: &'static str) -> Result<Self> {
        Self::new(h).ok_or_else(|| WinError::from_last_error(context))
    }

    /// Raw handle for passing to further Win32 calls.
    pub fn raw(&self) -> HANDLE {
        self.0
    }

    /// Give up ownership; the caller becomes responsible for closing.
    pub fn into_raw(self) -> HANDLE {
        let h = self.0;
        std::mem::forget(self);
        h
    }

    /// Close eagerly.  Dropping afterwards is a no-op.
    pub fn close(mut self) {
        self.close_inner();
    }

    fn close_inner(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
            self.0 = std::ptr::null_mut();
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        self.close_inner();
    }
}

/// Run `f` with a temporary owned handle, closing it even on panic.
pub fn with_handle<T>(h: HANDLE, context: &'static str, f: impl FnOnce(HANDLE) -> T) -> Result<T> {
    let owned = Handle::from_call(h, context)?;
    Ok(f(owned.raw()))
}

/// A handle that is deliberately never closed (e.g. `GetDC(NULL)` pairs).
#[derive(Debug, Clone, Copy)]
pub struct BorrowedHandle(pub HANDLE);

impl BorrowedHandle {
    pub fn raw(self) -> HANDLE {
        self.0
    }
}
