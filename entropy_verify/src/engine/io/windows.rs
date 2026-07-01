/// Windows IOCP + Direct I/O Engine.
///
/// Uses Win32 Overlapped I/O with I/O Completion Ports for high queue-depth
/// asynchronous file operations. Files are opened with FILE_FLAG_NO_BUFFERING
/// and FILE_FLAG_WRITE_THROUGH for direct-to-silicon I/O.

use super::aligned::AlignedBuffer;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, BOOL, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, ReadFile, WriteFile,
    CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_NO_BUFFERING,
    FILE_FLAG_OVERLAPPED, FILE_FLAG_WRITE_THROUGH, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::{
    CreateIoCompletionPort, GetQueuedCompletionStatus, OVERLAPPED,
};

/// Generic access rights (defined as constants for stability across windows-sys versions).
const GENERIC_READ: u32 = 0x80000000;
const GENERIC_WRITE: u32 = 0x40000000;

/// Infinite timeout for IOCP waits.
const INFINITE: u32 = 0xFFFFFFFF;

/// I/O pending error code.
const ERROR_IO_PENDING: u32 = 997;

/// Completion key for file I/O operations.
const FILE_IO_KEY: usize = 0x5656; // "VV"

/// Extended OVERLAPPED structure with slot tracking.
/// MUST be `#[repr(C)]` with `overlapped` as the first field so we can
/// cast `*mut OVERLAPPED` back to `*mut OverlappedEx` on completion.
#[repr(C)]
struct OverlappedEx {
    overlapped: OVERLAPPED,
    slot_index: usize,
}

/// Per-slot state: owns an aligned buffer and an overlapped operation.
struct IoSlot {
    /// The I/O buffer — page-aligned for direct I/O.
    buffer: AlignedBuffer,
    /// The extended overlapped struct — boxed for pointer stability.
    op: Box<OverlappedEx>,
    /// Which block this slot is currently processing.
    block_index: u32,
    /// Whether an async I/O operation is in-flight on this slot.
    in_flight: bool,
}

/// I/O Completion Port engine for Windows.
///
/// Provides high queue-depth pipelined reads and writes with direct I/O.
/// Each engine instance manages one open file at a time.
pub struct IocpEngine {
    /// The I/O completion port handle.
    iocp: HANDLE,
    /// Currently open file handle (INVALID_HANDLE_VALUE if none).
    file_handle: HANDLE,
    /// Pre-allocated I/O slots (one per queue-depth entry).
    slots: Vec<IoSlot>,
    /// Indices of slots that are currently free.
    free_list: Vec<usize>,
    /// Block size in bytes.
    #[allow(dead_code)]
    block_size: usize,
}

impl IocpEngine {
    /// Create a new IOCP engine with the given queue depth and block size.
    pub fn new(queue_depth: usize, block_size: usize) -> io::Result<Self> {
        // Create the I/O completion port (not associated with any file yet).
        let iocp = unsafe {
            CreateIoCompletionPort(INVALID_HANDLE_VALUE, 0, 0, 1)
        };
        if iocp == 0 {
            return Err(io::Error::last_os_error());
        }

        // Pre-allocate slots with aligned buffers.
        let mut slots = Vec::with_capacity(queue_depth);
        let mut free_list = Vec::with_capacity(queue_depth);

        for i in 0..queue_depth {
            let buffer = AlignedBuffer::new(block_size);
            let op = Box::new(OverlappedEx {
                overlapped: unsafe { std::mem::zeroed() },
                slot_index: i,
            });
            slots.push(IoSlot {
                buffer,
                op,
                block_index: 0,
                in_flight: false,
            });
            free_list.push(i);
        }

        Ok(IocpEngine {
            iocp,
            file_handle: INVALID_HANDLE_VALUE,
            slots,
            free_list,
            block_size,
        })
    }

    /// Open a file with direct I/O flags.
    ///
    /// - `write=true`: CREATE_ALWAYS + GENERIC_WRITE + WRITE_THROUGH
    /// - `write=false`: OPEN_EXISTING + GENERIC_READ
    pub fn open_file(&mut self, path: &Path, write: bool) -> io::Result<()> {
        // Ensure any previous file is closed.
        self.close_file()?;

        let wide_path: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let access = if write { GENERIC_WRITE } else { GENERIC_READ };
        let creation = if write { CREATE_ALWAYS } else { OPEN_EXISTING };

        let mut flags = FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED | FILE_ATTRIBUTE_NORMAL;
        if write {
            flags |= FILE_FLAG_WRITE_THROUGH;
        }

        let handle = unsafe {
            CreateFileW(
                wide_path.as_ptr(),
                access,
                0, // Exclusive access
                std::ptr::null(),
                creation,
                flags,
                0,
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }

        // Associate the file handle with the IOCP.
        let result = unsafe {
            CreateIoCompletionPort(handle, self.iocp, FILE_IO_KEY, 0)
        };

        if result == 0 {
            unsafe { CloseHandle(handle); }
            return Err(io::Error::last_os_error());
        }

        self.file_handle = handle;
        Ok(())
    }

    /// Acquire a free slot index. Blocks if all slots are in-flight
    /// by waiting for at least one completion.
    pub fn acquire_slot(&mut self) -> io::Result<usize> {
        if self.free_list.is_empty() {
            let completed = self.wait_completion()?;
            self.free_list.push(completed);
        }
        Ok(self.free_list.pop().expect("free_list should not be empty after wait"))
    }

    /// Get a mutable reference to a slot's buffer for filling.
    pub fn slot_buffer_mut(&mut self, slot: usize) -> &mut [u8] {
        &mut self.slots[slot].buffer
    }

    /// Get a reference to a slot's buffer for reading.
    pub fn slot_buffer(&self, slot: usize) -> &[u8] {
        &self.slots[slot].buffer
    }

    /// Get the block index associated with a slot.
    pub fn slot_block_index(&self, slot: usize) -> u32 {
        self.slots[slot].block_index
    }

    /// Submit an asynchronous write from the given slot's buffer.
    pub fn submit_write(
        &mut self,
        slot: usize,
        offset: u64,
        block_index: u32,
    ) -> io::Result<()> {
        let s = &mut self.slots[slot];
        s.block_index = block_index;
        s.in_flight = true;

        // Set the file offset in the OVERLAPPED structure and submit.
        s.op.overlapped.Anonymous.Anonymous.Offset = offset as u32;
        s.op.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        s.op.overlapped.Internal = 0;
        s.op.overlapped.InternalHigh = 0;
        s.op.overlapped.hEvent = 0;

        let mut bytes_written: u32 = 0;
        let result: BOOL = unsafe {
            WriteFile(
                self.file_handle,
                s.buffer.as_ptr(),
                s.buffer.len() as u32,
                &mut bytes_written,
                &mut s.op.overlapped,
            )
        };

        if result == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                s.in_flight = false;
                return Err(io::Error::from_raw_os_error(err as i32));
            }
            // ERROR_IO_PENDING is expected — the operation is queued.
        }

        Ok(())
    }

    /// Submit an asynchronous read into the given slot's buffer.
    pub fn submit_read(
        &mut self,
        slot: usize,
        offset: u64,
        block_index: u32,
    ) -> io::Result<()> {
        let s = &mut self.slots[slot];
        s.block_index = block_index;
        s.in_flight = true;

        s.op.overlapped.Anonymous.Anonymous.Offset = offset as u32;
        s.op.overlapped.Anonymous.Anonymous.OffsetHigh = (offset >> 32) as u32;
        s.op.overlapped.Internal = 0;
        s.op.overlapped.InternalHigh = 0;
        s.op.overlapped.hEvent = 0;

        let mut bytes_read: u32 = 0;
        let result: BOOL = unsafe {
            ReadFile(
                self.file_handle,
                s.buffer.as_mut_ptr() as *mut _,
                s.buffer.len() as u32,
                &mut bytes_read,
                &mut s.op.overlapped,
            )
        };

        if result == 0 {
            let err = unsafe { GetLastError() };
            if err != ERROR_IO_PENDING {
                s.in_flight = false;
                return Err(io::Error::from_raw_os_error(err as i32));
            }
        }

        Ok(())
    }

    /// Wait for one I/O completion. Returns the slot index that completed.
    pub fn wait_completion(&mut self) -> io::Result<usize> {
        let mut bytes_transferred: u32 = 0;
        let mut completion_key: usize = 0;
        let mut overlapped_ptr: *mut OVERLAPPED = std::ptr::null_mut();

        let result = unsafe {
            GetQueuedCompletionStatus(
                self.iocp,
                &mut bytes_transferred,
                &mut completion_key,
                &mut overlapped_ptr,
                INFINITE,
            )
        };

        if result == 0 {
            if overlapped_ptr.is_null() {
                return Err(io::Error::last_os_error());
            }
            let op = unsafe { &*(overlapped_ptr as *const OverlappedEx) };
            let slot_index = op.slot_index;
            self.slots[slot_index].in_flight = false;
            return Err(io::Error::last_os_error());
        }

        // Successful completion — map the overlapped pointer back to a slot.
        let op = unsafe { &*(overlapped_ptr as *const OverlappedEx) };
        let slot_index = op.slot_index;
        self.slots[slot_index].in_flight = false;

        Ok(slot_index)
    }

    /// Wait for all in-flight operations to complete.
    pub fn drain(&mut self) -> io::Result<()> {
        while self.has_inflight() {
            let slot = self.wait_completion()?;
            self.free_list.push(slot);
        }
        Ok(())
    }

    /// Check if any operations are in-flight.
    pub fn has_inflight(&self) -> bool {
        self.slots.iter().any(|s| s.in_flight)
    }

    /// Number of currently in-flight operations.
    #[allow(dead_code)]
    pub fn inflight_count(&self) -> usize {
        self.slots.iter().filter(|s| s.in_flight).count()
    }

    /// Flush and close the current file handle.
    pub fn close_file(&mut self) -> io::Result<()> {
        // Drain any remaining in-flight operations.
        self.drain()?;

        if self.file_handle != INVALID_HANDLE_VALUE {
            unsafe {
                FlushFileBuffers(self.file_handle);
                CloseHandle(self.file_handle);
            }
            self.file_handle = INVALID_HANDLE_VALUE;
        }

        // Reset the free list.
        self.free_list.clear();
        for i in 0..self.slots.len() {
            self.free_list.push(i);
        }

        Ok(())
    }

    /// Return the block size.
    #[allow(dead_code)]
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    /// Return the queue depth.
    pub fn queue_depth(&self) -> usize {
        self.slots.len()
    }
}

impl Drop for IocpEngine {
    fn drop(&mut self) {
        let _ = self.close_file();
        if self.iocp != 0 {
            unsafe {
                CloseHandle(self.iocp);
            }
        }
    }
}
