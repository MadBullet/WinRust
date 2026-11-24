//! Typed registry access (used by Explorer's address bar, Services' config
//! page and the system-control panel).

use crate::error::{Result, WinError};
use crate::wide::{from_wide, str_to_wide};
use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteKeyExW, RegDeleteValueW, RegEnumKeyExW, RegEnumValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE,
    KEY_READ, KEY_SET_VALUE, KEY_WOW64_64KEY, REG_BINARY, REG_DWORD, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};

/// Predefined root keys supported by the wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    CurrentUser,
    LocalMachine,
}

impl Root {
    fn key(self) -> HKEY {
        match self {
            Root::CurrentUser => HKEY_CURRENT_USER,
            Root::LocalMachine => HKEY_LOCAL_MACHINE,
        }
    }
}

/// An open registry key, closed with `RegCloseKey` on drop.
///
/// (`CloseHandle` happens to work on `HKEY`s as well, but `RegCloseKey` is the
/// documented API, so the key owns the handle directly instead of routing it
/// through [`crate::handle::Handle`].)
pub struct Key {
    hkey: HKEY,
}

impl Drop for Key {
    fn drop(&mut self) {
        if !self.hkey.is_null() {
            unsafe { RegCloseKey(self.hkey) };
            self.hkey = std::ptr::null_mut();
        }
    }
}

impl Key {
    pub fn open(root: Root, path: &str) -> Result<Self> {
        let wpath = str_to_wide(path);
        let mut hkey: HKEY = std::ptr::null_mut();
        let rc = unsafe {
            RegOpenKeyExW(
                root.key(),
                wpath.as_ptr(),
                0,
                KEY_READ | KEY_WOW64_64KEY,
                &mut hkey,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(WinError::from_code(rc, "RegOpenKeyExW"));
        }
        if hkey.is_null() {
            return Err(WinError::Other("null HKEY".into()));
        }
        Ok(Self { hkey })
    }

    pub fn raw(&self) -> HKEY {
        self.hkey
    }

    /// Sub-key names of this key.
    pub fn subkeys(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut index = 0u32;
        loop {
            let mut name = [0u16; 512];
            let mut len = name.len() as u32;
            let rc = unsafe {
                RegEnumKeyExW(
                    self.raw(),
                    index,
                    name.as_mut_ptr(),
                    &mut len,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            if rc != ERROR_SUCCESS {
                break;
            }
            out.push(from_wide(&name, len as usize));
            index += 1;
        }
        out
    }

    /// Raw value bytes plus the registry type.
    pub fn value_raw(&self, name: Option<&str>) -> Result<(u32, Vec<u8>)> {
        let wname = name.map(str_to_wide);
        let name_ptr = wname
            .as_ref()
            .map(|w| w.as_ptr())
            .unwrap_or(std::ptr::null());

        // First call sizes the buffer.
        let mut ty = 0u32;
        let mut size = 0u32;
        let rc = unsafe {
            RegQueryValueExW(
                self.raw(),
                name_ptr,
                std::ptr::null(),
                &mut ty,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(WinError::from_code(rc, "RegQueryValueExW(size)"));
        }
        let mut data = vec![0u8; size.max(1) as usize];
        let rc = unsafe {
            RegQueryValueExW(
                self.raw(),
                name_ptr,
                std::ptr::null(),
                &mut ty,
                data.as_mut_ptr(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS {
            return Err(WinError::from_code(rc, "RegQueryValueExW"));
        }
        data.truncate(size as usize);
        Ok((ty, data))
    }

    pub fn string(&self, name: &str) -> Option<String> {
        let (ty, data) = self.value_raw(Some(name)).ok()?;
        match ty {
            REG_SZ | 2 => {
                let units: Vec<u16> = data
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                Some(from_wide(&units, units.len()))
            }
            _ => None,
        }
    }

    pub fn u32(&self, name: &str) -> Option<u32> {
        let (ty, data) = self.value_raw(Some(name)).ok()?;
        if ty == REG_DWORD && data.len() >= 4 {
            Some(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
        } else {
            None
        }
    }

    /// Multi-line values (used for service descriptions with newlines, and
    /// Explorer's folder view settings).
    pub fn set_string(&self, name: &str, value: &str) -> Result<()> {
        let wide = str_to_wide(value);
        let bytes: Vec<u8> = wide.iter().flat_map(|w| w.to_le_bytes()).collect();
        let wname = str_to_wide(name);
        let rc = unsafe {
            RegSetValueExW(
                self.raw(),
                wname.as_ptr(),
                0,
                REG_SZ,
                bytes.as_ptr(),
                bytes.len() as u32,
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(WinError::from_code(rc, "RegSetValueExW"))
        }
    }

    /// Arbitrary binary value (`REG_BINARY`), used for the `StartupApproved`
    /// enable/disable blobs that Task Manager writes.
    pub fn set_binary(&self, name: &str, data: &[u8]) -> Result<()> {
        let wname = str_to_wide(name);
        let rc = unsafe {
            RegSetValueExW(
                self.raw(),
                wname.as_ptr(),
                0,
                REG_BINARY,
                data.as_ptr(),
                data.len() as u32,
            )
        };
        if rc == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(WinError::from_code(rc, "RegSetValueExW(binary)"))
        }
    }
}

/// Read a string from `HKLM` directly (convenience used by `system.rs`).
pub fn read_machine_string(path: &str, name: &str) -> Option<String> {
    Key::open(Root::LocalMachine, path).ok()?.string(name)
}

pub fn read_machine_u32(path: &str, name: &str) -> Option<u32> {
    Key::open(Root::LocalMachine, path).ok()?.u32(name)
}

pub fn read_user_string(path: &str, name: &str) -> Option<String> {
    Key::open(Root::CurrentUser, path).ok()?.string(name)
}

/// Write an expandable user environment variable (`HKCU\Environment`).
pub fn set_user_env(name: &str, value: &str) -> Result<()> {
    let key = open_for_write(Root::CurrentUser, "Environment")?;
    key.set_string(name, value)?;
    Ok(())
}

pub fn open_for_write(root: Root, path: &str) -> Result<Key> {
    let wpath = str_to_wide(path);
    let mut hkey: HKEY = std::ptr::null_mut();
    let mut disposition = 0u32;
    let rc = unsafe {
        RegCreateKeyExW(
            root.key(),
            wpath.as_ptr(),
            0,
            std::ptr::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE | KEY_READ | KEY_WOW64_64KEY,
            std::ptr::null(),
            &mut hkey,
            &mut disposition,
        )
    };
    if rc != ERROR_SUCCESS {
        return Err(WinError::from_code(rc, "RegCreateKeyExW"));
    }
    if hkey.is_null() {
        return Err(WinError::Other("null HKEY".into()));
    }
    Ok(Key { hkey })
}

/// Enumerate values as `(name, type, data)`.
pub fn enum_values(path: &str, root: Root) -> Vec<(String, u32, Vec<u8>)> {
    let Ok(key) = Key::open(root, path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut index = 0u32;
    loop {
        let mut name = [0u16; 512];
        let mut name_len = name.len() as u32;
        let mut ty = 0u32;
        let mut size = 0u32;
        // size query
        let rc = unsafe {
            RegEnumValueW(
                key.raw(),
                index,
                name.as_mut_ptr(),
                &mut name_len,
                std::ptr::null(),
                &mut ty,
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS {
            break;
        }
        let mut data = vec![0u8; size.max(1) as usize];
        let mut name2 = [0u16; 512];
        let mut name_len2 = name2.len() as u32;
        let rc = unsafe {
            RegEnumValueW(
                key.raw(),
                index,
                name2.as_mut_ptr(),
                &mut name_len2,
                std::ptr::null(),
                &mut ty,
                data.as_mut_ptr(),
                &mut size,
            )
        };
        if rc != ERROR_SUCCESS {
            break;
        }
        data.truncate(size as usize);
        out.push((from_wide(&name2, name_len2 as usize), ty, data));
        index += 1;
    }
    out
}

/// Delete a value (used by "Reset" buttons).
pub fn delete_value(root: Root, path: &str, name: &str) -> Result<()> {
    let key = open_for_write(root, path)?;
    let wname = str_to_wide(name);
    let rc = unsafe { RegDeleteValueW(key.raw(), wname.as_ptr()) };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(WinError::from_code(rc, "RegDeleteValueW"))
    }
}

/// Delete a sub-key tree node.
pub fn delete_key(root: Root, path: &str, sub: &str) -> Result<()> {
    let key = open_for_write(root, path)?;
    let wsub = str_to_wide(sub);
    let rc = unsafe { RegDeleteKeyExW(key.raw(), wsub.as_ptr(), KEY_WOW64_64KEY, 0) };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(WinError::from_code(rc, "RegDeleteKeyExW"))
    }
}
