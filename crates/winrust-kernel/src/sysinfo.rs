//! Structured enumeration through `ZwQuerySystemInformation`.
//!
//! The driver deliberately avoids hard-coded `EPROCESS` offsets: they differ
//! between Windows builds and guessing one is a one-way ticket to a bugcheck.
//! Everything the driver reports comes from `SYSTEM_INFORMATION_CLASS` queries
//! that the kernel itself populates:
//!
//! | Query | Gives |
//! |---|---|
//! | `SystemProcessInformation` (5) | processes: name, PID, PPID, threads, handles, working set, session, create time, CPU times |
//! | `SystemModuleInformation` (11) | loaded drivers: base address, size, image name |
//! | `SystemBasicInformation` (0) | page size, CPU count |
//! | `SystemPerformanceInformation` (2) | aggregate CPU and pool counters |
//!
//! All four are read-only and work on every Windows build.

use wdk_sys::{
    ntddk::{ExAllocatePool2, ExFreePool},
    NTSTATUS, POOL_FLAG_NON_PAGED, STATUS_INFO_LENGTH_MISMATCH, STATUS_SUCCESS,
};

// `ZwQuerySystemInformation` is present in ntddk.h but bindgen's default
// configuration for wdk-sys does not export it (it is not in the API subset the
// crate allowlists).  Declaring it here is the documented, supported way to
// reach an unbound ntoskrnl export from a Rust driver.
extern "system" {
    fn ZwQuerySystemInformation(
        system_information_class: u32,
        system_information: *mut core::ffi::c_void,
        system_information_length: u32,
        return_length: *mut u32,
    ) -> NTSTATUS;
}

/// `SystemProcessInformation`.
const CLASS_PROCESS: u32 = 5;
/// `SystemModuleInformation`.
const CLASS_MODULE: u32 = 11;
/// `SystemBasicInformation`.
const CLASS_BASIC: u32 = 0;
/// `SystemPerformanceInformation`.  Declared for completeness/documentation;
/// the aggregate counters are summed from the process table instead, because
/// that avoids depending on the fixed layout of `SYSTEM_PERFORMANCE_INFORMATION`.
#[allow(dead_code)]
const CLASS_PERFORMANCE: u32 = 2;

/// First guess for every query; covers realistic process/module tables.
const INITIAL_BYTES: usize = 96 * 1024;
/// Refuse to grow past this: a pathological system must not make us loop.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// A non-paged buffer owned by the driver and freed on drop.
struct QueryBuffer {
    ptr: *mut u8,
    len: usize,
}

impl QueryBuffer {
    fn allocate(len: usize) -> Option<Self> {
        // `ExAllocatePool2` replaces the deprecated `ExFreePoolWithTag` pair:
        // it zeroes the allocation (POOL_FLAG_NON_PAGED does not, so we also
        // zero defensively below) and returns null on failure.
        let ptr = unsafe {
            ExAllocatePool2(
                POOL_FLAG_NON_PAGED,
                len as u64,
                u32::from_le_bytes(*b"RnuW"),
            )
        }
        .cast::<u8>();
        if ptr.is_null() {
            None
        } else {
            unsafe { core::ptr::write_bytes(ptr, 0, len) };
            Some(Self { ptr, len })
        }
    }
}

impl Drop for QueryBuffer {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { ExFreePool(self.ptr.cast()) };
        }
    }
}

/// Run one information class and pass the raw bytes to `f`.
///
/// The buffer is allocated, grown on `STATUS_INFO_LENGTH_MISMATCH`, and freed
/// exactly once, whichever path `f` takes.
fn query_with<T>(class: u32, mut f: impl FnMut(&[u8]) -> T) -> Option<T> {
    let mut size = INITIAL_BYTES;
    loop {
        let buffer = QueryBuffer::allocate(size)?;
        let mut returned: u32 = 0;
        let status: NTSTATUS = unsafe {
            ZwQuerySystemInformation(
                class,
                buffer.ptr.cast(),
                buffer.len as u32,
                &raw mut returned,
            )
        };
        if status == STATUS_SUCCESS {
            // Some classes leave `returned` at zero while filling the buffer;
            // entry lists are self-terminating, so the whole buffer is safe.
            let used = (returned as usize).min(buffer.len);
            let len = if used == 0 { buffer.len } else { used };
            return Some(f(unsafe { core::slice::from_raw_parts(buffer.ptr, len) }));
        }
        if status != STATUS_INFO_LENGTH_MISMATCH {
            return None;
        }
        let needed = match (returned as usize).checked_add(4096) {
            Some(n) if n > size => n,
            _ => size * 5 / 4,
        };
        if needed > MAX_BYTES {
            return None;
        }
        size = needed;
    }
}

// ---------------------------------------------------------------------------
// SystemProcessInformation
// ---------------------------------------------------------------------------

/// One process, decoded into plain values so no pointer outlives the buffer.
#[derive(Clone, Copy, Default)]
pub struct ProcessRow {
    /// Byte length of the image name; zero for Idle and for names stored
    /// outside the query buffer.
    pub name_len: usize,
    /// Offset of the image name inside the buffer (valid when `name_len > 0`).
    pub name_offset: usize,
    pub pid: u32,
    pub parent_pid: u32,
    pub threads: u32,
    pub handles: u32,
    pub session_id: u32,
    pub base_priority: i32,
    pub create_time: i64,
    pub user_time: i64,
    pub kernel_time: i64,
    pub working_set: usize,
    pub virtual_size: usize,
    pub pagefile_usage: usize,
}

/// Fixed part of `SYSTEM_PROCESS_INFORMATION` on x64, matching the Windows SDK
/// layout.  The leading timings are read out of `reserved1` by offset because
/// they sit inside the C `Reserved1[48]` array.
#[repr(C)]
#[derive(Clone, Copy)]
struct SystemProcessInformation {
    next_entry_offset: u32,
    number_of_threads: u32,
    reserved1: [u8; 48],
    /// `UNICODE_STRING`: `{ Length: u16, MaximumLength: u16, Buffer: *mut u16 }`
    name_length: u16,
    name_max_length: u16,
    _pad: u32,
    name_buffer: usize,
    base_priority: i32,
    _pad2: u32,
    unique_process_id: usize,
    inherited_from_unique_process_id: usize,
    handle_count: u32,
    session_id: u32,
    _unique_process_key: usize,
    _peak_virtual_size: usize,
    virtual_size: usize,
    _page_fault_count: u32,
    _pad3: u32,
    _peak_working_set: usize,
    working_set: usize,
    _quota_peak_paged: usize,
    _quota_paged: usize,
    _quota_peak_non_paged: usize,
    _quota_non_paged: usize,
    pagefile_usage: usize,
    _peak_pagefile_usage: usize,
    _private_page_count: usize,
    _read_operation_count: i64,
    _write_operation_count: i64,
    _other_operation_count: i64,
    _read_transfer_count: i64,
    _write_transfer_count: i64,
    _other_transfer_count: i64,
}

const PROC_HEADER_SIZE: usize = core::mem::size_of::<SystemProcessInformation>();

/// Offsets inside `reserved1` of `CreateTime`, `UserTime`, `KernelTime`
/// (`LARGE_INTEGER` each).
const OFF_CREATE_TIME: usize = 24;
const OFF_USER_TIME: usize = 32;
const OFF_KERNEL_TIME: usize = 40;

fn read_i64(bytes: &[u8], offset: usize) -> i64 {
    let mut buf = [0u8; 8];
    if offset + 8 <= bytes.len() {
        buf.copy_from_slice(&bytes[offset..offset + 8]);
    }
    i64::from_le_bytes(buf)
}

/// Call `f` for every process in the system.  Returns false if the query failed.
pub fn for_each_process(mut f: impl FnMut(ProcessRow, &[u8])) -> bool {
    query_with(CLASS_PROCESS, |bytes| {
        let base = bytes.as_ptr() as usize;
        let mut offset = 0usize;
        while offset + PROC_HEADER_SIZE <= bytes.len() {
            let entry = unsafe {
                core::ptr::read_unaligned(
                    bytes
                        .as_ptr()
                        .add(offset)
                        .cast::<SystemProcessInformation>(),
                )
            };
            // The kernel usually points `Buffer` inside the same allocation,
            // after the array; only trust it when it really is in range.
            let relative = entry
                .name_buffer
                .checked_sub(base)
                .filter(|rel| *rel + entry.name_length as usize <= bytes.len());

            f(
                ProcessRow {
                    name_len: if relative.is_some() {
                        entry.name_length as usize
                    } else {
                        0
                    },
                    name_offset: relative.unwrap_or(0),
                    pid: entry.unique_process_id as u32,
                    parent_pid: entry.inherited_from_unique_process_id as u32,
                    threads: entry.number_of_threads,
                    handles: entry.handle_count,
                    session_id: entry.session_id,
                    base_priority: entry.base_priority,
                    create_time: read_i64(&entry.reserved1, OFF_CREATE_TIME),
                    user_time: read_i64(&entry.reserved1, OFF_USER_TIME),
                    kernel_time: read_i64(&entry.reserved1, OFF_KERNEL_TIME),
                    working_set: entry.working_set,
                    virtual_size: entry.virtual_size,
                    pagefile_usage: entry.pagefile_usage,
                },
                bytes,
            );

            if entry.next_entry_offset == 0 {
                break;
            }
            offset += entry.next_entry_offset as usize;
        }
    })
    .is_some()
}

// ---------------------------------------------------------------------------
// SystemModuleInformation
// ---------------------------------------------------------------------------

/// `SYSTEM_MODULE_ENTRY` as it appears in `RTL_PROCESS_MODULES`.
///
/// The image name is an inline ANSI array whose length varies by `FullPathName`
/// on modern builds, so the entry is walked by offset rather than by `repr(C)`
/// (the struct has a trailing variable-length string).
#[derive(Clone, Copy, Default)]
pub struct ModuleRow {
    pub base_address: usize,
    pub image_size: u32,
    /// Offset of the ANSI name inside the buffer, and its byte length.
    pub name_offset: usize,
    pub name_len: usize,
    pub load_order_index: u16,
}

/// Layout constants of `RTL_PROCESS_MODULE_INFORMATION` on x64.
const MOD_OFF_BASE: usize = 0;
const MOD_OFF_SIZE: usize = 0x14;
const MOD_OFF_NAME: usize = 0x20;
const MOD_NAME_FIELD: usize = 256;
const MOD_ENTRY_SIZE: usize = 0x28 + MOD_NAME_FIELD;

/// `RTL_PROCESS_MODULES` header: `ULONG NumberOfModules;` then a 4-byte pad.
const MODULES_HEADER: usize = 8;

/// Call `f` for every loaded kernel module (drivers included).
///
/// The callback receives the row plus the buffer the name points into, so no
/// raw kernel pointer ever leaves this function.
pub fn for_each_module(mut f: impl FnMut(ModuleRow, &[u8])) -> bool {
    query_with(CLASS_MODULE, |bytes| {
        if bytes.len() < MODULES_HEADER {
            return;
        }
        let count = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        for index in 0..count {
            let start = MODULES_HEADER + index * MOD_ENTRY_SIZE;
            if start + MOD_ENTRY_SIZE > bytes.len() {
                break;
            }
            let entry = &bytes[start..start + MOD_ENTRY_SIZE];
            let base = usize::from_le_bytes(
                entry[MOD_OFF_BASE..MOD_OFF_BASE + 8]
                    .try_into()
                    .unwrap_or([0; 8]),
            );
            let size = u32::from_le_bytes(
                entry[MOD_OFF_SIZE..MOD_OFF_SIZE + 4]
                    .try_into()
                    .unwrap_or([0; 4]),
            );
            let name = &entry[MOD_OFF_NAME..MOD_OFF_NAME + MOD_NAME_FIELD];
            let name_len = name.iter().position(|b| *b == 0).unwrap_or(name.len());
            f(
                ModuleRow {
                    base_address: base,
                    image_size: size,
                    name_offset: start + MOD_OFF_NAME,
                    name_len,
                    load_order_index: index as u16,
                },
                bytes,
            );
        }
    })
    .is_some()
}

// ---------------------------------------------------------------------------
// SystemBasicInformation / SystemPerformanceInformation
// ---------------------------------------------------------------------------

/// System-wide constants: page size and processor count.
pub fn basic_info() -> Option<(u32, u32)> {
    query_with(CLASS_BASIC, |bytes| {
        if bytes.len() < 8 {
            return None;
        }
        let page_size = u32::from_le_bytes(bytes[4..8].try_into().unwrap_or([0; 4]));
        // `NumberOfProcessors` follows the page size on x64.
        let processors = if bytes.len() >= 0x24 {
            u32::from_le_bytes(bytes[0x20..0x24].try_into().unwrap_or([0; 4]))
        } else {
            0
        };
        Some((page_size, processors))
    })
    .flatten()
}
