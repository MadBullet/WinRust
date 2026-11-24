//! # WinRust kernel driver (KMDF)
//!
//! The privileged half of WinRust.  It answers read-only queries that user mode
//! cannot perform without a handle (and therefore cannot perform at all for
//! protected processes), and it can refuse to start a blocked image before the
//! image runs a single instruction.
//!
//! ## What it does
//!
//! * `\\.\WinRust` control device with three IOCTLs (see `winrust-kernel-abi`).
//! * Process table straight from `ZwQuerySystemInformation(SystemProcessInformation)`:
//!   name, PID, PPID, thread/handle counts, session, create time and CPU times -
//!   including PPL processes that `OpenProcess` refuses.
//! * Loaded-module table from `SystemModuleInformation`: every driver with base
//!   address and size, which is what a rootkit check compares against.
//! * Optional `PsSetCreateProcessNotifyRoutineEx` guard with an in-RAM block
//!   list, cleared on unload.
//!
//! ## What it deliberately does not do
//!
//! No `EPROCESS` offset arithmetic, no undocumented structure writes, no
//! arbitrary kernel memory mapping, no persistence.  Every query is a documented
//! read-only API, so an unknown Windows build cannot turn into a bugcheck.
//!
//! ## Layout
//!
//! ```text
//! lib.rs       DriverEntry, device-global state
//! device.rs    control device + I/O queue (WDF)
//! ioctl.rs     request decoding, paging, response serialisation
//! sysinfo.rs   ZwQuerySystemInformation wrappers
//! guard.rs     process-creation block list
//! ```
//!
//! The user-mode client is `control/`; the wire protocol is
//! `crates/winrust-kernel-abi`.

#![no_std]
#![deny(clippy::all)]
#![allow(clippy::missing_safety_doc)]

extern crate alloc;

mod device;
mod guard;
mod ioctl;
mod sysinfo;

#[cfg(not(test))]
extern crate wdk_panic;

#[cfg(not(test))]
use wdk_alloc::WdkAllocator;

#[cfg(not(test))]
#[global_allocator]
static GLOBAL_ALLOCATOR: WdkAllocator = WdkAllocator;

use core::sync::atomic::AtomicUsize;
use wdk::println;
use wdk_sys::{
    call_unsafe_wdf_function_binding, DRIVER_OBJECT, NTSTATUS, PCUNICODE_STRING, PDRIVER_OBJECT,
    STATUS_SUCCESS, WDFDRIVER, WDF_DRIVER_CONFIG, WDF_NO_HANDLE, WDF_NO_OBJECT_ATTRIBUTES,
};

/// `\Device\WinRust`, as UTF-16 without the terminating NUL.
pub(crate) const ABI_DEVICE_NAME: &[u16] = &[
    b'\\' as u16, b'D' as u16, b'e' as u16, b'v' as u16, b'i' as u16, b'c' as u16, b'e' as u16,
    b'\\' as u16, b'W' as u16, b'i' as u16, b'n' as u16, b'R' as u16, b'u' as u16, b's' as u16,
    b't' as u16,
];

/// `\DosDevices\WinRust`, the link user mode opens as `\\.\WinRust`.
pub(crate) const ABI_SYMBOLIC_LINK: &[u16] = &[
    b'\\' as u16, b'D' as u16, b'o' as u16, b's' as u16, b'D' as u16, b'e' as u16, b'v' as u16,
    b'i' as u16, b'c' as u16, b'e' as u16, b's' as u16, b'\\' as u16, b'W' as u16, b'i' as u16,
    b'n' as u16, b'R' as u16, b'u' as u16, b's' as u16, b't' as u16,
];

/// The WDF device handle, stored so the unload path can remove the link.
pub(crate) static DEVICE: AtomicUsize = AtomicUsize::new(0);

/// `size_of` as the `ULONG` WDF's `Size` fields expect.
#[allow(clippy::cast_possible_truncation)]
const WDF_DRIVER_CONFIG_SIZE: u32 = {
    const S: usize = core::mem::size_of::<WDF_DRIVER_CONFIG>();
    const {
        assert!(
            S <= u32::MAX as usize,
            "WDF_DRIVER_CONFIG must fit in a ULONG"
        )
    };
    S as u32
};

/// Runs when the driver unloads.  Deregisters the process guard first, because
/// a notify callback that outlives the image would be called into freed memory.
#[link_section = "PAGE"]
extern "C" fn evt_driver_unload(_driver: WDFDRIVER) {
    guard::disable();
    println!("WinRust: driver unloading");
}

/// `DriverEntry` - first routine the memory manager calls after loading.
#[link_section = "INIT"]
#[export_name = "DriverEntry"]
extern "system" fn driver_entry(
    driver: &mut DRIVER_OBJECT,
    registry_path: PCUNICODE_STRING,
) -> NTSTATUS {
    // The export name must be exactly `DriverEntry`: `FxDriverEntry` resolves it
    // by name at load time, and build.rs adds `/IGNORE:4216` for the LNK warning
    // that exporting an entry point normally produces.
    let mut config = WDF_DRIVER_CONFIG {
        Size: WDF_DRIVER_CONFIG_SIZE,
        EvtDriverDeviceAdd: Some(device::evt_device_add),
        EvtDriverUnload: Some(evt_driver_unload),
        ..WDF_DRIVER_CONFIG::default()
    };

    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDriverCreate,
            driver as PDRIVER_OBJECT,
            registry_path,
            WDF_NO_OBJECT_ATTRIBUTES,
            &raw mut config,
            WDF_NO_HANDLE.cast::<WDFDRIVER>(),
        )
    };

    if !wdk::nt_success(status) {
        println!("WinRust: WdfDriverCreate failed {status:#010X}");
        return status;
    }

    // The guard is optional: a machine where process notifications are already
    // exhausted (they are a scarce resource) must still get the diagnostics.
    if !guard::enable() {
        println!("WinRust: running without the process guard");
    }

    println!("WinRust kernel driver loaded");
    STATUS_SUCCESS
}
