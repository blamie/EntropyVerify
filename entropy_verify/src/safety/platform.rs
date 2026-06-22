/// Platform-specific volume and mount point detection.

use std::path::{Path, PathBuf};

/// Information about the volume hosting a given path.
#[derive(Debug, Clone)]
pub struct VolumeInfo {
    /// The mount point / root path of the volume.
    pub mount_point: PathBuf,
    /// Filesystem type (e.g., "NTFS", "ext4").
    pub fs_type: String,
    /// Volume label / disk name.
    pub label: String,
    /// Total capacity in bytes.
    pub total_bytes: u64,
    /// Available (free) space in bytes.
    pub available_bytes: u64,
}

/// Resolve the volume info for a given path using sysinfo.
pub fn get_volume_info(target: &Path) -> Option<VolumeInfo> {
    use sysinfo::Disks;

    let mut canonical = std::fs::canonicalize(target).ok()?;
    #[cfg(target_os = "windows")]
    {
        let path_str = canonical.to_string_lossy();
        if path_str.starts_with(r"\\?\") {
            canonical = std::path::PathBuf::from(&path_str[4..]);
        }
    }
    let disks = Disks::new_with_refreshed_list();

    // Find the disk with the longest matching mount point prefix.
    let mut best_disk: Option<&sysinfo::Disk> = None;
    let mut best_len: usize = 0;

    for disk in disks.list() {
        let mount = disk.mount_point();
        if canonical.starts_with(mount) {
            let len = mount.as_os_str().len();
            if len > best_len {
                best_disk = Some(disk);
                best_len = len;
            }
        }
    }

    best_disk.map(|d| VolumeInfo {
        mount_point: d.mount_point().to_path_buf(),
        fs_type: d.file_system().to_string_lossy().to_string(),
        label: d.name().to_string_lossy().to_string(),
        total_bytes: d.total_space(),
        available_bytes: d.available_space(),
    })
}

/// Check if a path is the operating system root partition (Windows).
#[cfg(target_os = "windows")]
pub fn is_system_root(path: &Path) -> bool {
    // Get the system drive from %SystemDrive% (typically "C:")
    if let Ok(sys_drive) = std::env::var("SystemDrive") {
        let sys_root = format!("{}\\", sys_drive);
        let path_str = path.to_string_lossy().to_uppercase();
        let sys_str = sys_root.to_uppercase();
        return path_str == sys_str || path_str == sys_str.trim_end_matches('\\');
    }
    // Fallback: assume C:\ is system root
    let path_upper = path.to_string_lossy().to_uppercase();
    path_upper == "C:\\" || path_upper == "C:"
}

/// Check if a path is the root filesystem (Linux).
#[cfg(target_os = "linux")]
pub fn is_system_root(path: &Path) -> bool {
    path == Path::new("/")
}

/// Fallback for unsupported platforms.
#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn is_system_root(path: &Path) -> bool {
    let _ = path;
    false
}

/// Critical mount points that must never be written to (Linux).
#[cfg(target_os = "linux")]
const CRITICAL_MOUNTS: &[&str] = &["/boot", "/etc", "/usr", "/var", "/proc", "/sys", "/dev"];

/// Check if a path resides under a critical mount point (Linux).
#[cfg(target_os = "linux")]
pub fn is_critical_mount(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    for critical in CRITICAL_MOUNTS {
        if path_str.starts_with(critical) {
            return true;
        }
    }
    // Check if the path is on a swap partition
    is_swap_partition(path)
}

#[cfg(target_os = "windows")]
pub fn is_critical_mount(_path: &Path) -> bool {
    false // On Windows, the system drive check is sufficient
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
pub fn is_critical_mount(_path: &Path) -> bool {
    false
}

/// Check if the path's mount point is used for swap (Linux).
#[cfg(target_os = "linux")]
fn is_swap_partition(path: &Path) -> bool {
    let canonical = match std::fs::canonicalize(path) {
        Ok(p) => p,
        Err(_) => return false,
    };

    if let Ok(swaps) = std::fs::read_to_string("/proc/swaps") {
        for line in swaps.lines().skip(1) {
            // Format: Filename Type Size Used Priority
            if let Some(swap_path) = line.split_whitespace().next() {
                if canonical.starts_with(swap_path) {
                    return true;
                }
            }
        }
    }
    false
}
