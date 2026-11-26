//! Who this process is running as, and what it may do.
//!
//! One question - "may WinRust touch the machine?" - used to be answered in
//! four places, each with its own copy of the token inspection: the launcher
//! read `TokenElevation`, the desktop treated a failed query as "elevated ==
//! false", `storage::restore` asked `user::current_user()` again, and the
//! install page gated on that same field.  They disagreed: a UAC-limited
//! administrator was "standard user" everywhere, and an elevated process
//! whose token query failed was reported as one too.
//!
//! [`Privilege`] is the single answer.  It is produced once (by
//! [`Privilege::current`]), it says what the account *is* as well as what the
//! current token may do, and the UI never asks Windows again - it asks this
//! value.  `user::current_user()` keeps its old shape by embedding it, so no
//! call site had to move.

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::{
    CheckTokenMembership, CreateWellKnownSid, GetTokenInformation, TokenElevation,
    TokenElevationType, TokenElevationTypeFull, TokenElevationTypeLimited,
    WinBuiltinAdministratorsSid, TOKEN_ELEVATION, TOKEN_ELEVATION_TYPE, TOKEN_QUERY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::error::{Result, WinError};
use crate::handle::Handle;

/// What the current process may do, and what the account behind it is.
///
/// The flags are deliberately separate: Windows distinguishes *who the user
/// is* from *what this token can do*, and a shell that conflates them shows an
/// administrator a "standard user" banner (the 0.9.0 defect) or hides the
/// install button from an account that could elevate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Privilege {
    /// This token is the full administrator token: the process may write
    /// machine-wide state right now.
    pub elevated: bool,
    /// The account belongs to the local Administrators group, whether or not
    /// this process was started with the full token.
    pub administrator: bool,
    /// `administrator && !elevated` under UAC: the process can be *relaunched*
    /// with the full token.  This is the flag that decides whether the UI
    /// offers "restart as administrator".
    pub can_elevate: bool,
}

impl Privilege {
    /// Ask Windows once.
    ///
    /// Every step is best-effort, but the fallback is conservative: a token
    /// that cannot be inspected yields all-`false`, which the UI renders as
    /// "privileges could not be read" rather than as a policy decision.
    pub fn current() -> Self {
        let Ok(token) = ProcessToken::open() else {
            return Self::default();
        };
        let elevated = token.elevated().unwrap_or(false);
        let administrator = token.in_administrators().unwrap_or(false);
        let limited = token.elevation_kind() == Some(TokenElevationKind::Limited);
        Self {
            elevated,
            administrator,
            can_elevate: administrator && !elevated && limited,
        }
    }

    /// May this process perform a machine-wide change right now?
    pub fn may_modify_system(self) -> bool {
        self.elevated
    }

    /// A one-line label for the shell's status area.
    pub fn label(self) -> &'static str {
        if self.elevated {
            "Administrator"
        } else if self.administrator {
            "Administrator (UAC limited)"
        } else {
            "Standard user"
        }
    }
}

/// `TokenElevationType` in the terms the shell cares about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenElevationKind {
    /// The token cannot be split: a standard user, or the built-in
    /// Administrator running with UAC disabled.
    Default,
    /// The full administrator token: every group is enabled.
    Full,
    /// The filtered token a UAC-limited administrator runs under.
    Limited,
}

/// The process token, owned for the duration of one inspection.
struct ProcessToken(Handle);

impl ProcessToken {
    fn open() -> Result<Self> {
        let mut raw: HANDLE = std::ptr::null_mut();
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) };
        if ok == 0 {
            return Err(WinError::from_last_error("OpenProcessToken"));
        }
        Handle::new(raw)
            .map(Self)
            .ok_or_else(|| WinError::Other("null process token".into()))
    }

    fn raw(&self) -> HANDLE {
        self.0.raw()
    }

    /// `TOKEN_ELEVATION.TokenIsElevated`.
    fn elevated(&self) -> Result<bool> {
        let e = self.query::<TOKEN_ELEVATION>(TokenElevation)?;
        Ok(e.TokenIsElevated != 0)
    }

    /// `TokenElevationType`, or `None` when the class is unavailable.
    fn elevation_kind(&self) -> Option<TokenElevationKind> {
        let kind = self
            .query::<TOKEN_ELEVATION_TYPE>(TokenElevationType)
            .ok()?;
        Some(if kind == TokenElevationTypeFull {
            TokenElevationKind::Full
        } else if kind == TokenElevationTypeLimited {
            TokenElevationKind::Limited
        } else {
            TokenElevationKind::Default
        })
    }

    /// Is the Administrators group present *and enabled* in this token?
    ///
    /// This - not `TokenElevationType` - is the question Windows answers for
    /// the shield icon.  The filtered token of a UAC-limited administrator
    /// still lists the group, but deny-only, so `CheckTokenMembership`
    /// reports it absent; the full token reports it present.  Combined with
    /// [`Self::elevation_kind`] this distinguishes "not an administrator"
    /// from "an administrator who can elevate by relaunching".
    fn in_administrators(&self) -> Result<bool> {
        // SECURITY_MAX_SID_SIZE is 68 bytes; CreateWellKnownSid fills in the
        // real length.
        let mut sid = [0u8; 68];
        let mut size = sid.len() as u32;
        let ok = unsafe {
            CreateWellKnownSid(
                WinBuiltinAdministratorsSid,
                std::ptr::null_mut(),
                sid.as_mut_ptr() as *mut core::ffi::c_void,
                &mut size,
            )
        };
        if ok == 0 {
            return Err(WinError::from_last_error("CreateWellKnownSid"));
        }
        let mut member = 0;
        let ok = unsafe {
            CheckTokenMembership(self.raw(), sid.as_mut_ptr() as *mut core::ffi::c_void, &mut member)
        };
        if ok == 0 {
            return Err(WinError::from_last_error("CheckTokenMembership"));
        }
        Ok(member != 0)
    }

    /// Generic `GetTokenInformation` with the grow-once dance that the
    /// variable-length classes need.
    fn query<T: Copy>(&self, class: i32) -> Result<T> {
        let mut buf = vec![0u8; std::mem::size_of::<T>() + 64];
        let mut needed = 0u32;
        let ok = unsafe {
            GetTokenInformation(
                self.raw(),
                class,
                buf.as_mut_ptr() as *mut _,
                buf.len() as u32,
                &mut needed,
            )
        };
        if ok == 0 {
            let mut big = vec![0u8; needed.max(1024) as usize];
            let ok = unsafe {
                GetTokenInformation(
                    self.raw(),
                    class,
                    big.as_mut_ptr() as *mut _,
                    big.len() as u32,
                    &mut needed,
                )
            };
            if ok == 0 {
                return Err(WinError::from_last_error("GetTokenInformation"));
            }
            return Ok(unsafe { std::ptr::read_unaligned(big.as_ptr() as *const T) });
        }
        Ok(unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const T) })
    }
}

/// Ask Windows for the UAC consent prompt: relaunch this executable elevated.
///
/// Returns the new process id on success.  This is the only correct fix for a
/// UAC-limited administrator - the token of a running process cannot be
/// upgraded in place - so the launcher puts it on a button instead of asking
/// the user to find the exe and right-click it.
pub fn relaunch_elevated(args: &str) -> Result<Option<u32>> {
    let exe = std::env::current_exe().map_err(|e| WinError::Other(format!("{e}")))?;
    crate::shell::execute_elevated(&exe, args)
}
