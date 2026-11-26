//! UTF-16 helpers.
//!
//! Win32's `W` APIs take NUL-terminated UTF-16 strings.  Rust strings are
//! UTF-8, so every FFI call crosses this boundary.  The helpers here keep the
//! conversion in one place and make the "temporary buffer" lifetime explicit.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

/// Convert an `OsStr` (lossless on Windows) into a NUL-terminated UTF-16 buffer.
pub fn to_wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    let mut v: Vec<u16> = s.as_ref().encode_wide().collect();
    v.push(0);
    v
}

/// Convert a `&str` into a NUL-terminated UTF-16 buffer.
pub fn str_to_wide(s: &str) -> Vec<u16> {
    to_wide(OsStr::new(s))
}

/// Read a NUL-terminated UTF-16 buffer into a `String`, up to `len` code units.
///
/// Invalid sequences are replaced, mirroring how Windows itself tends to
/// handle broken file names rather than failing the whole enumeration.
pub fn from_wide(buf: &[u16], len: usize) -> String {
    let end = len.min(buf.len());
    let slice = &buf[..end];
    let stop = slice.iter().position(|&c| c == 0).unwrap_or(slice.len());
    String::from_utf16_lossy(&slice[..stop])
}

/// Read a fixed NUL-terminated UTF-16 array (e.g. `[u16; 260]`) into a `String`.
pub fn from_wide_fixed(buf: &[u16]) -> String {
    from_wide(buf, buf.len())
}

/// Read `len` UTF-16 code units, stopping at any interior NUL.
pub fn from_wide_len(buf: &[u16], len: usize) -> String {
    from_wide(buf, len)
}

/// Read a NUL-terminated UTF-16 string from a raw pointer (PDH instance names,
/// registry buffers).  Returns an empty string for a null pointer.
///
/// # Safety
/// `p` must point to a valid NUL-terminated UTF-16 buffer.
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    while *p.add(len) != 0 {
        len += 1;
        if len > 4096 {
            break;
        }
    }
    if len == 0 {
        return String::new();
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_ascii() {
        let w = str_to_wide("C:\\Windows");
        assert_eq!(w.last(), Some(&0));
        assert_eq!(from_wide(&w, w.len()), "C:\\Windows");
    }

    #[test]
    fn roundtrip_cyrillic() {
        let w = str_to_wide("Проводник");
        assert_eq!(from_wide(&w, w.len()), "Проводник");
    }

    #[test]
    fn stops_at_nul() {
        let mut w = str_to_wide("abc");
        w.extend_from_slice(&str_to_wide("def"));
        assert_eq!(from_wide(&w, w.len()), "abc");
    }
}
