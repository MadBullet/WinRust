//! Control device creation and the I/O queue that serves it.
//!
//! The device is a *software* device, so no hardware is involved and the driver
//! installs on any machine.  It exposes `\Device\WinRust` plus the
//! `\DosDevices\WinRust` link that user mode opens as `\\.\WinRust`.

use crate::ioctl;
use wdk::println;
use wdk_sys::{
    call_unsafe_wdf_function_binding, _WDF_EXECUTION_LEVEL, _WDF_IO_QUEUE_DISPATCH_TYPE,
    _WDF_SYNCHRONIZATION_SCOPE, _WDF_TRI_STATE, NTSTATUS, PWDFDEVICE_INIT, STATUS_SUCCESS,
    UNICODE_STRING, WDFDEVICE, WDFDRIVER, WDFQUEUE, WDF_IO_QUEUE_CONFIG, WDF_NO_HANDLE,
    WDF_OBJECT_ATTRIBUTES,
};

/// `FILE_DEVICE_UNKNOWN` - the documented device type for a custom control
/// device.  Must match `DEVICE_TYPE` in `winrust-kernel-abi`, or `CTL_CODE`
/// on the client side would compute a different IOCTL number.
const FILE_DEVICE_UNKNOWN: u32 = 0x22;

#[allow(clippy::cast_possible_truncation)]
const WDF_IO_QUEUE_CONFIG_SIZE: u32 = {
    const S: usize = core::mem::size_of::<WDF_IO_QUEUE_CONFIG>();
    const {
        assert!(
            S <= u32::MAX as usize,
            "WDF_IO_QUEUE_CONFIG must fit in a ULONG"
        )
    };
    S as u32
};

#[allow(clippy::cast_possible_truncation)]
const WDF_OBJECT_ATTRIBUTES_SIZE: u32 = {
    const S: usize = core::mem::size_of::<WDF_OBJECT_ATTRIBUTES>();
    const {
        assert!(
            S <= u32::MAX as usize,
            "WDF_OBJECT_ATTRIBUTES must fit in a ULONG"
        )
    };
    S as u32
};

/// Build a `UNICODE_STRING` over one of our static wide literals.
///
/// The framework copies the string into its own allocation during
/// `WdfDeviceInitAssignName`, so the `'static` borrow is sufficient.
fn unicode(s: &'static [u16]) -> UNICODE_STRING {
    UNICODE_STRING {
        Length: (s.len() * 2) as u16,
        MaximumLength: (s.len() * 2) as u16,
        Buffer: s.as_ptr() as *mut u16,
    }
}

/// `EvtDriverDeviceAdd` - called by the framework when the driver loads.
///
/// `device_init` is owned by the framework for the duration of the call and is
/// freed once `WdfDeviceCreate` succeeds, so it must not escape.
#[link_section = "PAGE"]
pub extern "C" fn evt_device_add(driver: WDFDRIVER, device_init: PWDFDEVICE_INIT) -> NTSTATUS {
    let _ = driver;

    // A software device: no hardware ids, no resources.
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceInitSetDeviceType,
            device_init,
            FILE_DEVICE_UNKNOWN
        )
    };
    if !wdk::nt_success(status) {
        println!("WinRust: SetDeviceType failed {status:#010X}");
        return status;
    }

    let mut device_name = unicode(crate::ABI_DEVICE_NAME);
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceInitAssignName,
            device_init,
            &raw mut device_name
        )
    };
    if !wdk::nt_success(status) {
        println!("WinRust: AssignName failed {status:#010X}");
        return status;
    }

    let mut attributes = WDF_OBJECT_ATTRIBUTES {
        Size: WDF_OBJECT_ATTRIBUTES_SIZE,
        ExecutionLevel: _WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope: _WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        ..WDF_OBJECT_ATTRIBUTES::default()
    };
    let mut device: WDFDEVICE = WDF_NO_HANDLE.cast();
    // `WdfDeviceCreate` takes `*mut PWDFDEVICE_INIT`, i.e. the *address* of our
    // pointer variable: the framework nulls it once it consumes the init struct.
    let mut init_slot = device_init;
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfDeviceCreate,
            &raw mut init_slot,
            &raw mut attributes,
            &raw mut device,
        )
    };
    if !wdk::nt_success(status) {
        println!("WinRust: WdfDeviceCreate failed {status:#010X}");
        return status;
    }

    let mut link = unicode(crate::ABI_SYMBOLIC_LINK);
    let status = unsafe {
        call_unsafe_wdf_function_binding!(WdfDeviceCreateSymbolicLink, device, &raw mut link)
    };
    if !wdk::nt_success(status) {
        println!("WinRust: symbolic link failed {status:#010X}");
        return status;
    }

    // One sequential queue for DeviceIoControl.  Sequential = one request at a
    // time, which keeps the driver lock-free.
    let mut queue_config = WDF_IO_QUEUE_CONFIG {
        Size: WDF_IO_QUEUE_CONFIG_SIZE,
        PowerManaged: _WDF_TRI_STATE::WdfUseDefault,
        DispatchType: _WDF_IO_QUEUE_DISPATCH_TYPE::WdfIoQueueDispatchSequential,
        EvtIoDeviceControl: Some(ioctl::evt_io_device_control),
        ..WDF_IO_QUEUE_CONFIG::default()
    };
    let mut queue: WDFQUEUE = WDF_NO_HANDLE.cast();
    let mut queue_attributes = WDF_OBJECT_ATTRIBUTES {
        Size: WDF_OBJECT_ATTRIBUTES_SIZE,
        ExecutionLevel: _WDF_EXECUTION_LEVEL::WdfExecutionLevelInheritFromParent,
        SynchronizationScope: _WDF_SYNCHRONIZATION_SCOPE::WdfSynchronizationScopeInheritFromParent,
        ..WDF_OBJECT_ATTRIBUTES::default()
    };
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfIoQueueCreate,
            device,
            &raw mut queue_config,
            &raw mut queue_attributes,
            &raw mut queue,
        )
    };
    if !wdk::nt_success(status) {
        println!("WinRust: WdfIoQueueCreate failed {status:#010X}");
        return status;
    }

    // Publish the device handle so the unload callback can delete the link.
    crate::DEVICE.store(device as usize, core::sync::atomic::Ordering::Release);

    println!("WinRust: control device \\Device\\WinRust ready");
    STATUS_SUCCESS
}
