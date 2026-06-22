/// Page-aligned buffer allocator for direct I/O.
///
/// Direct I/O (FILE_FLAG_NO_BUFFERING on Windows, O_DIRECT on Linux) requires
/// that I/O buffers are aligned to the volume's sector size (typically 4096 bytes).
///
/// - **Windows**: Uses `VirtualAlloc` for guaranteed page-aligned memory.
/// - **Linux**: Uses `posix_memalign` for explicit 4096-byte alignment.

use std::ops::{Deref, DerefMut};

/// A heap-allocated buffer guaranteed to be aligned to 4096 bytes.
/// Implements `Deref<Target=[u8]>` and `DerefMut` for ergonomic slice access.
pub struct AlignedBuffer {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: The buffer is a raw heap allocation with exclusive ownership.
// No references are shared across threads without external synchronization.
unsafe impl Send for AlignedBuffer {}
unsafe impl Sync for AlignedBuffer {}

impl AlignedBuffer {
    /// Allocate a new page-aligned buffer of the given size (zeroed).
    #[cfg(target_os = "windows")]
    pub fn new(size: usize) -> Self {
        use windows_sys::Win32::System::Memory::{
            VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE,
        };

        let ptr = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                size,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            ) as *mut u8
        };

        if ptr.is_null() {
            panic!(
                "VirtualAlloc({} bytes) failed: {}",
                size,
                std::io::Error::last_os_error()
            );
        }

        // VirtualAlloc returns zeroed memory for MEM_COMMIT pages.
        AlignedBuffer { ptr, len: size }
    }

    /// Allocate a new page-aligned buffer of the given size (zeroed).
    #[cfg(target_os = "linux")]
    pub fn new(size: usize) -> Self {
        let mut ptr: *mut libc::c_void = std::ptr::null_mut();
        let ret = unsafe { libc::posix_memalign(&mut ptr, 4096, size) };
        if ret != 0 {
            panic!("posix_memalign({} bytes, align=4096) failed: error {}", size, ret);
        }
        // Zero-initialize (posix_memalign does not guarantee zeroed memory)
        unsafe {
            std::ptr::write_bytes(ptr as *mut u8, 0, size);
        }
        AlignedBuffer {
            ptr: ptr as *mut u8,
            len: size,
        }
    }

    /// Fallback for unsupported platforms — uses a Vec with manual alignment check.
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    pub fn new(size: usize) -> Self {
        // Over-allocate and align manually. Not ideal but functional.
        let layout = std::alloc::Layout::from_size_align(size, 4096)
            .expect("Invalid layout");
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        if ptr.is_null() {
            panic!("alloc_zeroed({} bytes, align=4096) failed", size);
        }
        AlignedBuffer { ptr, len: size }
    }

    /// Return the raw pointer to the buffer.
    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Return a mutable raw pointer to the buffer.
    #[inline]
    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.ptr
    }

    /// Return the buffer length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Check if the buffer is empty.
    #[inline]
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

impl Drop for AlignedBuffer {
    fn drop(&mut self) {
        if self.ptr.is_null() {
            return;
        }

        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::System::Memory::{VirtualFree, MEM_RELEASE};
            unsafe {
                VirtualFree(self.ptr as *mut std::ffi::c_void, 0, MEM_RELEASE);
            }
        }

        #[cfg(target_os = "linux")]
        {
            unsafe {
                libc::free(self.ptr as *mut libc::c_void);
            }
        }

        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let layout = std::alloc::Layout::from_size_align(self.len, 4096)
                .expect("Invalid layout");
            unsafe {
                std::alloc::dealloc(self.ptr, layout);
            }
        }
    }
}

impl Deref for AlignedBuffer {
    type Target = [u8];

    #[inline]
    fn deref(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl DerefMut for AlignedBuffer {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_allocation_and_alignment() {
        let buf = AlignedBuffer::new(4096);
        assert_eq!(buf.len(), 4096);
        assert_eq!(buf.as_ptr() as usize % 4096, 0, "Buffer must be 4096-aligned");
    }

    #[test]
    fn test_zeroed() {
        let buf = AlignedBuffer::new(8192);
        assert!(buf.iter().all(|&b| b == 0), "Buffer must be zero-initialized");
    }

    #[test]
    fn test_read_write() {
        let mut buf = AlignedBuffer::new(4096);
        buf[0] = 0xAA;
        buf[4095] = 0xBB;
        assert_eq!(buf[0], 0xAA);
        assert_eq!(buf[4095], 0xBB);
    }

    #[test]
    fn test_large_allocation() {
        // 2 MiB — our standard block size
        let buf = AlignedBuffer::new(2 * 1024 * 1024);
        assert_eq!(buf.len(), 2 * 1024 * 1024);
        assert_eq!(buf.as_ptr() as usize % 4096, 0);
    }
}
