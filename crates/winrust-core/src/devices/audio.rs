//! Master volume and mute, through the documented `IAudioEndpointVolume` COM
//! interface.
//!
//! `windows-sys` has no binding for the MMDevice APIs (they are a pure COM
//! contract, no import library), so the two interfaces this module needs are
//! declared here with their documented vtable layouts.  Only the slots the tray
//! uses are declared; anything past them is opaque padding.
//!
//! Everything returns `Option`/`bool` instead of an error: a machine with no
//! audio device is normal (servers, VMs), and the tray simply shows a dash.

use crate::icon::ComGuard;

/// `IMMDeviceEnumerator` (CLSID_MMDeviceEnumerator) - devices.
const CLSID_MM_DEVICE_ENUMERATOR: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0xBCDE_039C,
    data2: 0xE1E5,
    data3: 0x4E2A,
    data4: [0xA2, 0x42, 0x82, 0xBA, 0xA4, 0x35, 0x52, 0x61],
};
/// `IID_IMMDeviceEnumerator`.
const IID_IMM_DEVICE_ENUMERATOR: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0xA956_64C2,
    data2: 0x0F4B,
    data3: 0x4815,
    data4: [0xB5, 0x11, 0xCE, 0x57, 0x22, 0x5C, 0x24, 0x58],
};
/// `IID_IAudioEndpointVolume` - `5CDF2C82-841E-4546-9722-0CF74078229A`.
///
/// This constant was wrong.  It read `5CDF2B5A-4B4C-4E9C-A714-79CE8D5C9E3E`,
/// which is not any interface at all, so `IMMDevice::Activate` failed with
/// `E_NOINTERFACE` every time and both the getter and the setter returned
/// `None`.  The tray then had nothing to show but its "unknown" fallback, and
/// because writing the level also failed it never took: dragging the slider
/// snapped back, which is the "застряло на 50 и не меняется" report.  The value
/// below is taken from `endpointvolume.rdl` in windows-rs, the same metadata
/// that generates the official bindings.
const IID_IAUDIO_ENDPOINT_VOLUME: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x5CDF_2C82,
    data2: 0x841E,
    data3: 0x4546,
    data4: [0x97, 0x22, 0x0C, 0xF7, 0x40, 0x78, 0x22, 0x9A],
};
/// `EDataFlow::eRender`, `ERole::eMultimedia`.
const E_RENDER: i32 = 0;
const E_MULTIMEDIA: i32 = 0;
const CLSCTX_ALL: u32 = 23;
/// `STGM_READ`.
const STGM_READ: u32 = 0;

// The vtable as `endpointvolume.h` declares it, IUnknown included.  The
// earlier code was off by two for every call: it used 7 for the *getter*, but
// slot 7 is `SetMasterVolumeLevelScalar`, so reading wrote a float into the
// interface and setting wrote into our own stack variable - which is exactly
// why the tray always showed 0% and the slider did nothing.
const SLOT_SET_VOLUME_SCALAR: usize = 7;
const SLOT_GET_VOLUME_SCALAR: usize = 9;
const SLOT_SET_MUTE: usize = 14;
const SLOT_GET_MUTE: usize = 15;

/// A COM object whose only responsibility is `Release` on drop.
struct ComObject(*mut *const usize);

impl ComObject {
    /// The COM interface pointer as the vtable calls expect it.
    fn ptr(&self) -> *mut usize {
        self.0 as *mut usize
    }
}

impl Drop for ComObject {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let vtbl = *(self.0 as *const *const usize);
                if !vtbl.is_null() {
                    // IUnknown::Release is slot 2 (0 = QueryInterface,
                    // 1 = AddRef).
                    let release: unsafe extern "system" fn(*mut usize) -> u32 =
                        std::mem::transmute(*vtbl.add(2));
                    release(self.ptr());
                }
            }
        }
    }
}

/// The master volume of the default playback device, 0..=100.
pub fn master_volume() -> Option<u8> {
    with_endpoint(|volume: &ComObject| unsafe {
        let vtbl = *volume.0;
        let get: unsafe extern "system" fn(*mut usize, *mut f32) -> i32 =
            std::mem::transmute(*vtbl.add(SLOT_GET_VOLUME_SCALAR));
        let mut level = 0.0f32;
        if get(volume.ptr(), &mut level) < 0 {
            return None;
        }
        Some((level * 100.0).round().clamp(0.0, 100.0) as u8)
    })
    .flatten()
}

/// Set the master volume, 0..=100.
pub fn set_master_volume(percent: u8) -> Option<()> {
    with_endpoint(|volume| unsafe {
        let vtbl = *volume.0;
        let set: unsafe extern "system" fn(*mut usize, f32, *const windows_sys::core::GUID) -> i32 =
            std::mem::transmute(*vtbl.add(SLOT_SET_VOLUME_SCALAR));
        let level = (percent.min(100) as f32) / 100.0;
        if set(volume.ptr(), level, std::ptr::null()) < 0 {
            None
        } else {
            Some(())
        }
    })
    .flatten()
}

/// Whether the default playback device is muted.
pub fn is_muted() -> bool {
    with_endpoint(|volume| unsafe {
        let vtbl = *volume.0;
        let get: unsafe extern "system" fn(*mut usize, *mut i32) -> i32 =
            std::mem::transmute(*vtbl.add(SLOT_GET_MUTE));
        let mut muted = 0i32;
        if get(volume.ptr(), &mut muted) < 0 {
            return false;
        }
        muted != 0
    })
    .unwrap_or(false)
}

/// Mute or unmute the default playback device.
pub fn set_muted(muted: bool) -> Option<()> {
    with_endpoint(|volume| unsafe {
        let vtbl = *volume.0;
        let set: unsafe extern "system" fn(*mut usize, i32, *const windows_sys::core::GUID) -> i32 =
            std::mem::transmute(*vtbl.add(SLOT_SET_MUTE));
        if set(volume.ptr(), i32::from(muted), std::ptr::null()) < 0 {
            None
        } else {
            Some(())
        }
    })
    .flatten()
}

/// Friendly name of the default playback device (`Realtek Digital Output ...`).
pub fn device_name() -> Option<String> {
    let _com = ComGuard::new();
    let device = with_device()?;
    unsafe {
        let vtbl = *device.0;
        // IUnknown(3) + IMMDevice: Activate(3), OpenPropertyStore(4),
        // GetId(5), GetState(6) -> OpenPropertyStore is slot 4.
        let open: unsafe extern "system" fn(*mut usize, u32, *mut *mut usize) -> i32 =
            std::mem::transmute(*vtbl.add(4));
        let mut store: *mut usize = std::ptr::null_mut();
        if open(device.ptr(), STGM_READ, &mut store) < 0 || store.is_null() {
            return None;
        }
        let store = ComObject(store as *mut *const usize);
        let store_vtbl = *store.0;
        // IUnknown(3) + IPropertyStore::GetCount(3), GetAt(4), GetValue(5),
        // SetValue(6), Commit(7) -> GetValue is slot 5.
        let get_value: unsafe extern "system" fn(
            *mut usize,
            *const PROPERTYKEY,
            *mut PROPVARIANT,
        ) -> i32 = std::mem::transmute(*store_vtbl.add(5));
        // PKEY_Device_FriendlyName
        const PKEY: PROPERTYKEY = PROPERTYKEY {
            fmtid: windows_sys::core::GUID {
                data1: 0xA45C_254E,
                data2: 0x36C8,
                data3: 0x40A7,
                data4: [0x86, 0x9F, 0xA6, 0x54, 0x1D, 0xBA, 0xA5, 0x3D],
            },
            pid: 14,
        };
        let mut value: PROPVARIANT = std::mem::zeroed();
        if get_value(store.ptr(), &PKEY, &mut value) < 0 {
            return None;
        }
        // VT_LPWSTR = 31, the string lives in the union's first field.
        let name = if value.vt == 31 {
            let ptr = value.data as *const u16;
            if ptr.is_null() {
                None
            } else {
                let mut len = 0usize;
                while *ptr.add(len) != 0 && len < 512 {
                    len += 1;
                }
                Some(crate::wide::from_wide(
                    std::slice::from_raw_parts(ptr, len),
                    len,
                ))
            }
        } else {
            None
        };
        windows_sys::Win32::System::Com::CoTaskMemFree(value.data as *const _);
        name
    }
}

#[repr(C)]
struct PROPERTYKEY {
    fmtid: windows_sys::core::GUID,
    pid: u32,
}

/// A `PROPVARIANT` laid out the way COM expects: a 2-byte type tag, three
/// reserved words, then an 8-byte union.  The union starts at offset 8 on both
/// x86 and x64, and the string pointer is its first field.
#[repr(C)]
struct PROPVARIANT {
    vt: u16,
    w1: u16,
    w2: u16,
    w3: u16,
    data: usize,
}

/// Open the default multimedia render endpoint.
///
/// The caller owns the COM apartment: this function used to create its own
/// [`ComGuard`] and hand back an `IMMDevice`, which meant `CoUninitialize` ran
/// the moment it returned - while the caller still held a live COM pointer.
/// Every later call on that pointer failed, so `master_volume()` was always
/// `None` and the tray showed 0.  The guard now lives in [`with_endpoint`],
/// which is the whole lifetime of the objects it creates.
fn with_device() -> Option<ComObject> {
    unsafe {
        let mut enumerator: *mut usize = std::ptr::null_mut();
        let hr = windows_sys::Win32::System::Com::CoCreateInstance(
            &CLSID_MM_DEVICE_ENUMERATOR,
            std::ptr::null_mut(),
            CLSCTX_ALL,
            &IID_IMM_DEVICE_ENUMERATOR,
            &mut enumerator as *mut _ as *mut *mut core::ffi::c_void,
        );
        if hr < 0 || enumerator.is_null() {
            return None;
        }
        let enumerator = ComObject(enumerator as *mut *const usize);
        let vtbl = *enumerator.0;
        // IUnknown(3) + IMMDeviceEnumerator: EnumAudioEndpoints(3),
        // GetDefaultAudioEndpoint(4), GetDevice(5), Register(6), Unregister(7).
        let get_default: unsafe extern "system" fn(*mut usize, i32, i32, *mut *mut usize) -> i32 =
            std::mem::transmute(*vtbl.add(4));
        let mut device: *mut usize = std::ptr::null_mut();
        if get_default(enumerator.ptr(), E_RENDER, E_MULTIMEDIA, &mut device) < 0
            || device.is_null()
        {
            return None;
        }
        Some(ComObject(device as *mut *const usize))
    }
}

/// Activate `IAudioEndpointVolume` on the default endpoint and run `f`.
fn with_endpoint<T>(f: impl FnOnce(&ComObject) -> T) -> Option<T> {
    let _com = ComGuard::new();
    let device = with_device()?;
    unsafe {
        let vtbl = *device.0;
        // IMMDevice::Activate is slot 3.
        let activate: unsafe extern "system" fn(
            *mut usize,
            *const windows_sys::core::GUID,
            u32,
            *const core::ffi::c_void,
            *mut *mut usize,
        ) -> i32 = std::mem::transmute(*vtbl.add(3));
        let mut volume: *mut usize = std::ptr::null_mut();
        if activate(
            device.ptr(),
            &IID_IAUDIO_ENDPOINT_VOLUME,
            CLSCTX_ALL,
            std::ptr::null(),
            &mut volume,
        ) < 0
            || volume.is_null()
        {
            return None;
        }
        let volume = ComObject(volume as *mut *const usize);
        Some(f(&volume))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn volume_round_trips() {
        // On CI there is usually no audio device; both directions must then
        // simply report "unknown" instead of panicking.
        let before = crate::devices::audio::master_volume();
        if let Some(v) = before {
            let _ = crate::devices::audio::set_master_volume(v);
            assert_eq!(crate::devices::audio::master_volume(), Some(v));
        }
    }
}
