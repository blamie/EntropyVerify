/// Platform-dispatched I/O engine with aligned buffer support.
///
/// Compile-time dispatch selects the optimal engine for each OS:
/// - Windows: IOCP + Direct I/O (FILE_FLAG_NO_BUFFERING)
/// - Linux: io_uring + O_DIRECT (with pread/pwrite fallback)

pub mod aligned;

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "linux")]
pub mod linux;

/// Re-export the platform-appropriate engine type.
#[cfg(target_os = "windows")]
pub type PlatformEngine = windows::IocpEngine;

#[cfg(target_os = "linux")]
pub type PlatformEngine = linux::LinuxIoEngine;


