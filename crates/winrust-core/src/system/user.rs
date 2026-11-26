//! Token information: who is running WinRust, and are we elevated?

use crate::error::{check, Result, WinError};
use crate::handle::Handle;
use crate::system::privilege::Privilege;
use crate::wide::from_wide;
use windows_sys::Win32::Security::{
    GetTokenInformation, LookupAccountSidW, TokenIntegrityLevel, TokenUser, SID_AND_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[derive(Debug, Clone, Default)]
pub struct UserInfo {
    pub account: String,
    pub domain: String,
    pub sid: String,
    /// Kept for the call sites that only need the boolean; it is
    /// `privilege.elevated`.
    pub elevated: bool,
    /// Who the account is and what this token may do - the single answer the
    /// UI is allowed to make permission decisions on.
    pub privilege: Privilege,
    /// `Untrusted` / `Low` / `Medium` / `High` / `System`.
    pub integrity: &'static str,
}

/// Token of the current process.
pub struct Token(Handle);

impl Token {
    pub fn current() -> Result<Self> {
        let mut h = std::ptr::null_mut();
        check("OpenProcessToken", unsafe {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut h)
        })?;
        Handle::new(h)
            .map(Token)
            .ok_or_else(|| WinError::Other("null token".into()))
    }

    /// Copy of the user SID, owned by the caller.
    fn user_sid(&self) -> Option<OwnedSid> {
        let mut needed = 0u32;
        unsafe {
            GetTokenInformation(
                self.0.raw(),
                TokenUser,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
        }
        if needed == 0 {
            return None;
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = unsafe {
            GetTokenInformation(
                self.0.raw(),
                TokenUser,
                buf.as_mut_ptr() as *mut _,
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return None;
        }
        let tu = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
        let sid_len = unsafe { windows_sys::Win32::Security::GetLengthSid(tu.User.Sid) } as usize;
        let mut sid = vec![0u8; sid_len];
        unsafe {
            std::ptr::copy_nonoverlapping(tu.User.Sid as *const u8, sid.as_mut_ptr(), sid_len);
        }
        Some(OwnedSid(sid))
    }

    /// Account + domain via `LookupAccountSidW`.
    pub fn account(&self) -> Option<(String, String)> {
        let sid = self.user_sid()?;
        let sid_ptr = sid.as_ptr();
        let mut name = [0u16; 256];
        let mut name_len = name.len() as u32;
        let mut domain = [0u16; 256];
        let mut domain_len = domain.len() as u32;
        let mut use_kind = 0i32;
        let ok = unsafe {
            LookupAccountSidW(
                std::ptr::null(),
                sid_ptr,
                name.as_mut_ptr(),
                &mut name_len,
                domain.as_mut_ptr(),
                &mut domain_len,
                &mut use_kind,
            )
        };
        if ok == 0 {
            return None;
        }
        Some((
            from_wide(&name, name_len as usize),
            from_wide(&domain, domain_len as usize),
        ))
    }

    /// String SID (`S-1-5-21-...`).
    pub fn sid_string(&self) -> Option<String> {
        let sid = self.user_sid()?;
        let mut out: *mut u16 = std::ptr::null_mut();
        let ok = unsafe {
            windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW(
                sid.as_ptr(),
                &mut out,
            )
        };
        if ok == 0 || out.is_null() {
            return None;
        }
        let mut len = 0usize;
        while unsafe { *out.add(len) } != 0 {
            len += 1;
        }
        let s = from_wide(unsafe { std::slice::from_raw_parts(out, len) }, len);
        unsafe { windows_sys::Win32::Foundation::LocalFree(out as *mut _) };
        Some(s)
    }

    /// Integrity level, derived from the SID's last RID.
    pub fn integrity(&self) -> &'static str {
        let mut needed = 0u32;
        unsafe {
            GetTokenInformation(
                self.0.raw(),
                TokenIntegrityLevel,
                std::ptr::null_mut(),
                0,
                &mut needed,
            );
        }
        if needed == 0 {
            return "Unknown";
        }
        let mut buf = vec![0u8; needed as usize];
        let ok = unsafe {
            GetTokenInformation(
                self.0.raw(),
                TokenIntegrityLevel,
                buf.as_mut_ptr() as *mut _,
                needed,
                &mut needed,
            )
        };
        if ok == 0 {
            return "Unknown";
        }
        let label = unsafe {
            &*(buf.as_ptr() as *const windows_sys::Win32::Security::TOKEN_MANDATORY_LABEL)
        };
        let sid = label.Label.Sid;
        let count = unsafe { *windows_sys::Win32::Security::GetSidSubAuthorityCount(sid) };
        if count == 0 {
            return "Unknown";
        }
        let rid =
            unsafe { *windows_sys::Win32::Security::GetSidSubAuthority(sid, (count - 1) as u32) };
        match rid {
            0x0000 => "Untrusted",
            0x1000 => "Low",
            0x2000 => "Medium",
            0x2100 => "Medium Plus",
            0x3000 => "High",
            0x4000 => "System",
            _ => "Unknown",
        }
    }
}

/// A SID byte copy that frees itself on drop.
struct OwnedSid(Vec<u8>);

impl OwnedSid {
    fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.0.as_ptr() as *mut std::ffi::c_void
    }
}

/// One call that gathers everything the title bar needs.
pub fn current_user() -> UserInfo {
    let mut info = UserInfo::default();
    // One token inspection decides every permission question in the shell;
    // the `elevated` field is a copy so old call sites keep compiling.
    info.privilege = Privilege::current();
    info.elevated = info.privilege.elevated;
    if let Ok(token) = Token::current() {
        info.integrity = token.integrity();
        if let Some((account, domain)) = token.account() {
            info.account = account;
            info.domain = domain;
        }
        if let Some(sid) = token.sid_string() {
            info.sid = sid;
        }
    }
    info
}

/// Is the current process running elevated?  (Single-call helper.)
pub fn is_elevated() -> bool {
    Privilege::current().elevated
}

#[allow(dead_code)]
fn _keep(_: SID_AND_ATTRIBUTES) {}
