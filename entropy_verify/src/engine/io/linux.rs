/// Linux I/O Engine: io_uring with O_DIRECT, with synchronous pread/pwrite fallback.
///
/// The primary engine uses the `io_uring` crate for zero-copy, kernel-bypassing
/// async I/O. If io_uring initialization fails (e.g., kernel < 5.6 or seccomp
/// restrictions), the engine falls back to synchronous pread/pwrite with O_DIRECT.

use super::aligned::AlignedBuffer;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// Per-slot state: owns an aligned buffer and tracks in-flight status.
struct IoSlot {
    buffer: AlignedBuffer,
    block_index: u32,
    in_flight: bool,
}

/// Linux I/O engine with io_uring and synchronous fallback.
pub struct LinuxIoEngine {
    inner: EngineInner,
    slots: Vec<IoSlot>,
    free_list: Vec<usize>,
    file_fd: i32,
    #[allow(dead_code)]
    block_size: usize,
}

enum EngineInner {
    IoUring(io_uring::IoUring),
    Synchronous, // Fallback: pread/pwrite
}

impl LinuxIoEngine {
    /// Create a new Linux I/O engine. Tries io_uring first, falls back to sync.
    pub fn new(queue_depth: usize, block_size: usize) -> io::Result<Self> {
        let inner = match io_uring::IoUring::builder()
            .build(queue_depth as u32)
        {
            Ok(ring) => EngineInner::IoUring(ring),
            Err(e) => {
                eprintln!(
                    "Warning: io_uring init failed ({}), falling back to synchronous I/O",
                    e
                );
                EngineInner::Synchronous
            }
        };

        let mut slots = Vec::with_capacity(queue_depth);
        let mut free_list = Vec::with_capacity(queue_depth);

        for i in 0..queue_depth {
            slots.push(IoSlot {
                buffer: AlignedBuffer::new(block_size),
                block_index: 0,
                in_flight: false,
            });
            free_list.push(i);
        }

        Ok(LinuxIoEngine {
            inner,
            slots,
            free_list,
            file_fd: -1,
            block_size,
        })
    }

    /// Open a file with O_DIRECT.
    pub fn open_file(&mut self, path: &Path, write: bool) -> io::Result<()> {
        self.close_file()?;

        use std::fs::OpenOptions;
        use std::os::unix::fs::OpenOptionsExt;

        let file = if write {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .custom_flags(libc::O_DIRECT)
                .open(path)?
        } else {
            OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECT)
                .open(path)?
        };

        self.file_fd = file.as_raw_fd();
        std::mem::forget(file); // We manage the fd ourselves
        Ok(())
    }

    /// Acquire a free slot.
    pub fn acquire_slot(&mut self) -> io::Result<usize> {
        if self.free_list.is_empty() {
            let completed = self.wait_completion()?;
            self.free_list.push(completed);
        }
        Ok(self.free_list.pop().unwrap())
    }

    pub fn slot_buffer_mut(&mut self, slot: usize) -> &mut [u8] {
        &mut self.slots[slot].buffer
    }

    pub fn slot_buffer(&self, slot: usize) -> &[u8] {
        &self.slots[slot].buffer
    }

    pub fn slot_block_index(&self, slot: usize) -> u32 {
        self.slots[slot].block_index
    }

    /// Submit a write operation.
    pub fn submit_write(
        &mut self,
        slot: usize,
        offset: u64,
        block_index: u32,
    ) -> io::Result<()> {
        self.slots[slot].block_index = block_index;
        self.slots[slot].in_flight = true;

        match &mut self.inner {
            EngineInner::IoUring(ring) => {
                let s = &self.slots[slot];
                let write_e = io_uring::opcode::Write::new(
                    io_uring::types::Fd(self.file_fd),
                    s.buffer.as_ptr(),
                    s.buffer.len() as u32,
                )
                .offset(offset as _)
                .build()
                .user_data(slot as u64);

                unsafe {
                    ring.submission()
                        .push(&write_e)
                        .map_err(|_| io::Error::new(io::ErrorKind::Other, "io_uring SQ full"))?;
                }
                ring.submit()?;
                Ok(())
            }
            EngineInner::Synchronous => {
                // Synchronous pwrite — blocks until complete
                let s = &self.slots[slot];
                let ret = unsafe {
                    libc::pwrite(
                        self.file_fd,
                        s.buffer.as_ptr() as *const libc::c_void,
                        s.buffer.len(),
                        offset as libc::off_t,
                    )
                };
                self.slots[slot].in_flight = false;
                if ret < 0 {
                    return Err(io::Error::last_os_error());
                }
                self.free_list.push(slot);
                Ok(())
            }
        }
    }

    /// Submit a read operation.
    pub fn submit_read(
        &mut self,
        slot: usize,
        offset: u64,
        block_index: u32,
    ) -> io::Result<()> {
        self.slots[slot].block_index = block_index;
        self.slots[slot].in_flight = true;

        match &mut self.inner {
            EngineInner::IoUring(ring) => {
                let s = &mut self.slots[slot];
                let read_e = io_uring::opcode::Read::new(
                    io_uring::types::Fd(self.file_fd),
                    s.buffer.as_mut_ptr(),
                    s.buffer.len() as u32,
                )
                .offset(offset as _)
                .build()
                .user_data(slot as u64);

                unsafe {
                    ring.submission()
                        .push(&read_e)
                        .map_err(|_| io::Error::new(io::ErrorKind::Other, "io_uring SQ full"))?;
                }
                ring.submit()?;
                Ok(())
            }
            EngineInner::Synchronous => {
                let s = &mut self.slots[slot];
                let ret = unsafe {
                    libc::pread(
                        self.file_fd,
                        s.buffer.as_mut_ptr() as *mut libc::c_void,
                        s.buffer.len(),
                        offset as libc::off_t,
                    )
                };
                self.slots[slot].in_flight = false;
                if ret < 0 {
                    return Err(io::Error::last_os_error());
                }
                self.free_list.push(slot);
                Ok(())
            }
        }
    }

    /// Wait for one I/O completion. Returns the slot index.
    pub fn wait_completion(&mut self) -> io::Result<usize> {
        match &mut self.inner {
            EngineInner::IoUring(ring) => {
                ring.submit_and_wait(1)?;

                let cqe = ring
                    .completion()
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "No CQE available"))?;

                let slot_index = cqe.user_data() as usize;
                let result = cqe.result();

                self.slots[slot_index].in_flight = false;

                if result < 0 {
                    return Err(io::Error::from_raw_os_error(-result));
                }

                Ok(slot_index)
            }
            EngineInner::Synchronous => {
                // In sync mode, operations complete immediately during submit.
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "No in-flight operations in synchronous mode",
                ))
            }
        }
    }

    /// Drain all in-flight operations.
    pub fn drain(&mut self) -> io::Result<()> {
        while self.has_inflight() {
            let slot = self.wait_completion()?;
            self.free_list.push(slot);
        }
        Ok(())
    }

    pub fn has_inflight(&self) -> bool {
        self.slots.iter().any(|s| s.in_flight)
    }

    #[allow(dead_code)]
    pub fn inflight_count(&self) -> usize {
        self.slots.iter().filter(|s| s.in_flight).count()
    }

    /// Close the current file.
    pub fn close_file(&mut self) -> io::Result<()> {
        self.drain()?;

        if self.file_fd >= 0 {
            unsafe {
                libc::fdatasync(self.file_fd);
                libc::close(self.file_fd);
            }
            self.file_fd = -1;
        }

        self.free_list.clear();
        for i in 0..self.slots.len() {
            self.free_list.push(i);
        }

        Ok(())
    }

    #[allow(dead_code)]
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn queue_depth(&self) -> usize {
        self.slots.len()
    }
}

impl Drop for LinuxIoEngine {
    fn drop(&mut self) {
        let _ = self.close_file();
    }
}
