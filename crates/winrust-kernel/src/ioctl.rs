//! `DeviceIoControl` dispatch: every request becomes a read-only query.
//!
//! All IOCTLs are `METHOD_BUFFERED`, so the I/O manager has already copied the
//! caller's bytes into kernel pool and will copy our answer back.  This module
//! therefore never dereferences a user-mode pointer - the classic kernel attack
//! surface is gone by construction.
//!
//! Each request is completed **exactly once**, at the end of
//! [`evt_io_device_control`], with the number of bytes written.

use crate::sysinfo;
use alloc::vec::Vec;
use wdk::println;
use wdk_sys::{
    call_unsafe_wdf_function_binding, NTSTATUS, PVOID, STATUS_BUFFER_TOO_SMALL,
    STATUS_INVALID_DEVICE_REQUEST, STATUS_INVALID_PARAMETER, STATUS_SUCCESS, WDFQUEUE, WDFREQUEST,
};
use winrust_kernel_abi::{
    ModuleRecord, Performance, ProcessRecord, Query, Request, ResponseHeader,
    IOCTL_WINRUST_GET_VERSION, IOCTL_WINRUST_QUERY, MAX_RECORDS,
};

/// Driver protocol version reported to the client.
const PROTOCOL_VERSION: u32 = 1;

/// `EvtIoDeviceControl` - the only entry point user mode can reach.
///
/// The `unsafe` inside is the WDF call itself; every buffer access is bounded.
#[link_section = "PAGE"]
pub extern "C" fn evt_io_device_control(
    _queue: WDFQUEUE,
    request: WDFREQUEST,
    _output_length: usize,
    _input_length: usize,
    ioctl_code: u32,
) {
    let (status, written) = match dispatch(request, ioctl_code) {
        Ok(written) => (STATUS_SUCCESS, written),
        Err(status) => (status, 0),
    };
    unsafe {
        // CompleteWithInformation is the only completion: `dispatch` never
        // completes the request itself.
        call_unsafe_wdf_function_binding!(
            WdfRequestCompleteWithInformation,
            request,
            status,
            written as u64
        );
    }
}

/// Answer one IOCTL, returning the number of output bytes written.
fn dispatch(request: WDFREQUEST, ioctl_code: u32) -> Result<usize, NTSTATUS> {
    let (page_size, processors) = sysinfo::basic_info().unwrap_or((4096, 0));

    match ioctl_code {
        IOCTL_WINRUST_GET_VERSION => {
            let header = ResponseHeader {
                status: 0,
                query: Query::Ping as u32,
                count: 0,
                total: 0,
                version: PROTOCOL_VERSION,
                processors,
                page_size,
                reserved: 0,
            };
            write_response(request, &header, &[])
        }
        IOCTL_WINRUST_QUERY => {
            let query = read_request(request).ok_or(STATUS_INVALID_PARAMETER)?;
            let kind = Query::from_raw(query.query).ok_or(STATUS_INVALID_PARAMETER)?;
            answer(request, kind, &query, page_size, processors)
        }
        _ => Err(STATUS_INVALID_DEVICE_REQUEST),
    }
}

/// Read the caller's [`Request`] from the buffered input.
fn read_request(request: WDFREQUEST) -> Option<Request> {
    let mut input: PVOID = core::ptr::null_mut();
    let mut length: usize = 0;
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestRetrieveInputBuffer,
            request,
            core::mem::size_of::<Request>(),
            &raw mut input,
            &raw mut length
        )
    };
    if !wdk::nt_success(status) || input.is_null() || length < core::mem::size_of::<Request>() {
        return None;
    }
    // SAFETY: the I/O manager copied at least `size_of::<Request>()` bytes into
    // this kernel buffer (checked above); `Request` is `repr(C)` over integers,
    // so every bit pattern is a valid value.
    Some(unsafe { core::ptr::read_unaligned(input.cast::<Request>()) })
}

/// Copy `header` plus `payload` into the caller's output buffer.
fn write_response(
    request: WDFREQUEST,
    header: &ResponseHeader,
    payload: &[u8],
) -> Result<usize, NTSTATUS> {
    let mut output: PVOID = core::ptr::null_mut();
    let mut capacity: usize = 0;
    let status = unsafe {
        call_unsafe_wdf_function_binding!(
            WdfRequestRetrieveOutputBuffer,
            request,
            core::mem::size_of::<ResponseHeader>(),
            &raw mut output,
            &raw mut capacity
        )
    };
    if !wdk::nt_success(status) || output.is_null() {
        return Err(STATUS_BUFFER_TOO_SMALL);
    }
    let needed = core::mem::size_of::<ResponseHeader>() + payload.len();
    if capacity < needed {
        return Err(STATUS_BUFFER_TOO_SMALL);
    }
    // SAFETY: `output` is a kernel buffer of `capacity >= needed` bytes.
    unsafe {
        core::ptr::write_unaligned(output.cast::<ResponseHeader>(), *header);
        let body = output
            .cast::<u8>()
            .add(core::mem::size_of::<ResponseHeader>());
        core::ptr::copy_nonoverlapping(payload.as_ptr(), body, payload.len());
    }
    Ok(needed)
}

fn answer(
    request: WDFREQUEST,
    kind: Query,
    query: &Request,
    page_size: u32,
    processors: u32,
) -> Result<usize, NTSTATUS> {
    let limit = (query.limit as usize).clamp(1, MAX_RECORDS);
    let start = query.offset as usize;

    match kind {
        Query::Ping => {
            let header = ResponseHeader {
                status: 0,
                query: kind as u32,
                count: 0,
                total: 0,
                version: PROTOCOL_VERSION,
                processors,
                page_size,
                reserved: 0,
            };
            write_response(request, &header, &[])
        }
        Query::Processes => {
            let mut records: Vec<ProcessRecord> = Vec::with_capacity(limit);
            let mut total = 0usize;
            sysinfo::for_each_process(|row, bytes| {
                total += 1;
                if total <= start || records.len() >= limit {
                    return;
                }
                let mut name = [0u16; ProcessRecord::NAME_CHARS];
                if row.name_len > 0 {
                    let chars = (row.name_len / 2).min(ProcessRecord::NAME_CHARS - 1);
                    let end = row.name_offset + chars * 2;
                    if end <= bytes.len() {
                        for (index, chunk) in
                            bytes[row.name_offset..end].chunks_exact(2).enumerate()
                        {
                            name[index] = u16::from_le_bytes([chunk[0], chunk[1]]);
                        }
                    }
                }
                records.push(ProcessRecord {
                    pid: row.pid,
                    parent_pid: row.parent_pid,
                    threads: row.threads,
                    handles: row.handles,
                    session_id: row.session_id,
                    base_priority: row.base_priority,
                    create_time: row.create_time,
                    user_time: row.user_time,
                    kernel_time: row.kernel_time,
                    working_set: row.working_set as u64,
                    virtual_size: row.virtual_size as u64,
                    pagefile_usage: row.pagefile_usage as u64,
                    name,
                });
            });
            let header = ResponseHeader {
                status: 0,
                query: kind as u32,
                count: records.len() as u32,
                total: total as u32,
                version: PROTOCOL_VERSION,
                processors,
                page_size,
                reserved: 0,
            };
            write_records(request, &header, &records)
        }
        Query::Modules => {
            let mut records: Vec<ModuleRecord> = Vec::with_capacity(limit);
            let mut total = 0usize;
            sysinfo::for_each_module(|module, bytes| {
                total += 1;
                if total <= start || records.len() >= limit {
                    return;
                }
                let mut name = [0u16; ModuleRecord::NAME_CHARS];
                let end = module.name_offset + module.name_len;
                if end <= bytes.len() {
                    for (index, byte) in bytes[module.name_offset..end]
                        .iter()
                        .take(ModuleRecord::NAME_CHARS - 1)
                        .enumerate()
                    {
                        // Module names are ANSI; widen so the wire format stays
                        // uniform UTF-16.
                        name[index] = u16::from(*byte);
                    }
                }
                records.push(ModuleRecord {
                    base_address: module.base_address as u64,
                    image_size: module.image_size,
                    flags: u32::from(module.load_order_index),
                    name,
                });
            });
            let header = ResponseHeader {
                status: 0,
                query: kind as u32,
                count: records.len() as u32,
                total: total as u32,
                version: PROTOCOL_VERSION,
                processors,
                page_size,
                reserved: 0,
            };
            write_records(request, &header, &records)
        }
        Query::Performance => {
            let mut processes = 0u32;
            let mut threads = 0u32;
            let mut handles = 0u32;
            let mut user_time = 0i64;
            let mut kernel_time = 0i64;
            sysinfo::for_each_process(|row, _| {
                processes = processes.saturating_add(1);
                threads = threads.saturating_add(row.threads);
                handles = handles.saturating_add(row.handles);
                user_time = user_time.saturating_add(row.user_time);
                kernel_time = kernel_time.saturating_add(row.kernel_time);
            });
            let perf = Performance {
                total_user_time: user_time,
                total_kernel_time: kernel_time,
                total_idle_time: 0,
                process_count: processes,
                thread_count: threads,
                handle_count: handles,
                committed_bytes: 0,
                paged_pool_bytes: 0,
                nonpaged_pool_bytes: 0,
            };
            let header = ResponseHeader {
                status: 0,
                query: kind as u32,
                count: 1,
                total: 1,
                version: PROTOCOL_VERSION,
                processors,
                page_size,
                reserved: 0,
            };
            write_records(request, &header, core::slice::from_ref(&perf))
        }
    }
}

/// Serialise a record slice as bytes and hand it to [`write_response`].
fn write_records<T: Copy>(
    request: WDFREQUEST,
    header: &ResponseHeader,
    records: &[T],
) -> Result<usize, NTSTATUS> {
    // SAFETY: `records` is a live slice of `Copy` values; the cast to `u8` is
    // valid for the duration of the borrow and `write_response` copies it out.
    let bytes = unsafe {
        core::slice::from_raw_parts(
            records.as_ptr().cast::<u8>(),
            core::mem::size_of_val(records),
        )
    };
    write_response(request, header, bytes)
}
