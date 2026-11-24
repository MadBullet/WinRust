//! Process-creation guard.
//!
//! `PsSetCreateProcessNotifyRoutineEx` runs a callback *before* a new process
//! gets its first instruction; returning a failure status stops the image from
//! ever executing.  WinRust uses it for one narrow purpose: when the user turns
//! on "block untrusted images" in the launcher, an image the user-mode signature
//! check rejected can be refused here, where a user-mode tamper cannot bypass it.
//!
//! The list lives in RAM only.  Unloading the driver - or a reboot - removes it.
//! That is the strongest possible undo for a security feature: there is no
//! persisted state to clean up and a broken policy cannot brick the machine.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use wdk::println;
use wdk_sys::{
    ntddk::PsSetCreateProcessNotifyRoutineEx, PS_CREATE_NOTIFY_INFO, STATUS_ACCESS_DENIED,
};

/// Set while the callback is registered.
static REGISTERED: AtomicBool = AtomicBool::new(false);

/// Names (lowercase, bare file name) that must not start.
static mut BLOCKED: Option<Vec<String>> = None;

/// Minimal spin lock.  Held only for list reads/writes of a handful of short
/// strings and never across an allocation-free kernel call.
static LOCK: AtomicBool = AtomicBool::new(false);

fn with_lock<R>(f: impl FnOnce() -> R) -> R {
    while LOCK
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        core::hint::spin_loop();
    }
    let out = f();
    LOCK.store(false, Ordering::Release);
    out
}

fn blocked_mut<R>(f: impl FnOnce(&mut Vec<String>) -> R) -> R {
    with_lock(|| {
        // SAFETY: every access to `BLOCKED` happens inside `with_lock`, and the
        // static outlives the driver image, so the reference cannot dangle.
        let list = unsafe {
            let slot = core::ptr::addr_of_mut!(BLOCKED);
            if (*slot).is_none() {
                *slot = Some(Vec::new());
            }
            (*slot).as_mut().expect("initialised just above")
        };
        f(list)
    })
}

fn is_blocked(image: &str) -> bool {
    with_lock(|| {
        // SAFETY: see `blocked_mut`.
        let slot = unsafe { core::ptr::addr_of!(BLOCKED) };
        match unsafe { &*slot } {
            Some(list) => list.iter().any(|entry| entry.eq_ignore_ascii_case(image)),
            None => false,
        }
    })
}

/// Add a bare image name to the block list (idempotent).
pub fn block(image: &str) {
    blocked_mut(|list| {
        if !list.iter().any(|e| e.eq_ignore_ascii_case(image)) {
            list.push(String::from(image));
        }
    });
}

/// Remove a name from the block list.
pub fn allow(image: &str) {
    blocked_mut(|list| list.retain(|e| !e.eq_ignore_ascii_case(image)));
}

/// Number of blocked names.
#[must_use]
pub fn count() -> usize {
    blocked_mut(|list| list.len())
}

/// Is the guard registered?
#[must_use]
pub fn is_active() -> bool {
    REGISTERED.load(Ordering::Acquire)
}

/// Copy the bare file name out of the kernel's `UNICODE_STRING`.
///
/// # Safety
///
/// `buffer` must point to `length` bytes of readable UTF-16, exactly as the
/// kernel passes it in `PS_CREATE_NOTIFY_INFO`.
unsafe fn image_name(buffer: *const u16, length: usize) -> Option<String> {
    if buffer.is_null() || length == 0 || length > 0x1000 {
        return None;
    }
    let mut out = String::with_capacity(length / 2);
    for index in 0..length / 2 {
        let unit = core::ptr::read_unaligned(buffer.add(index));
        if unit == 0 {
            break;
        }
        // Only the last path component matters.
        if unit == u16::from(b'\\') || unit == u16::from(b'/') {
            out.clear();
            continue;
        }
        out.push(char::from_u32(u32::from(unit)).unwrap_or('\u{FFFD}'));
    }
    Some(out)
}

/// `PCREATE_PROCESS_NOTIFY_ROUTINE_EX`.
///
/// The signature is a private detail of the binding: the addresses are handed
/// to `PsSetCreateProcessNotifyRoutineEx` as a `*mut c_void`, which is exactly
/// what the WDK does in C (`(PVOID)callback`).
extern "C" fn notify(
    _parent: *mut core::ffi::c_void,
    _pid: *mut core::ffi::c_void,
    info: *mut PS_CREATE_NOTIFY_INFO,
) {
    // SAFETY: the kernel guarantees a valid `PS_CREATE_NOTIFY_INFO` while the
    // callback runs when `info` is non-null; process *exit* notifications pass
    // null, which is why the check comes first.
    let Some(info) = (unsafe { info.as_ref() }) else {
        return;
    };
    // `ImageFileName` is `PUNICODE_STRING` in the WDK, so dereference it once
    // and keep the reference for the two field reads.
    let Some(name) = (unsafe { info.ImageFileName.as_ref() }) else {
        return;
    };
    let Some(image) = (unsafe { image_name(name.Buffer, usize::from(name.Length)) }) else {
        return;
    };
    if is_blocked(&image) {
        // The calling `CreateProcess` then fails with "Access is denied", which
        // is what a policy block should look like to user mode.
        unsafe {
            core::ptr::write_unaligned(
                core::ptr::addr_of_mut!(info.CreationStatus),
                STATUS_ACCESS_DENIED,
            );
        }
        println!("WinRust: refused to start blocked image {image}");
    }
}

/// Register the callback once.
pub fn enable() -> bool {
    if REGISTERED.load(Ordering::Acquire) {
        return true;
    }
    let status = unsafe { PsSetCreateProcessNotifyRoutineEx(Some(notify), 0) };
    if !wdk::nt_success(status) {
        println!("WinRust: process notify registration failed {status:#010X}");
        return false;
    }
    REGISTERED.store(true, Ordering::Release);
    println!("WinRust: process guard active");
    true
}

/// Deregister the callback.  Must run before the image unloads, or the kernel
/// would call into freed memory.
pub fn disable() {
    if !REGISTERED.swap(false, Ordering::AcqRel) {
        return;
    }
    unsafe {
        PsSetCreateProcessNotifyRoutineEx(Some(notify), 1);
    }
    blocked_mut(Vec::clear);
}
