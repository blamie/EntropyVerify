/// Safety guardrails: validate that the target directory is safe to write to.

use super::platform::{self, VolumeInfo};
use std::path::Path;

/// Errors returned by safety validation.
#[derive(Debug)]
pub enum SafetyError {
    /// The target directory does not exist.
    DirectoryNotFound(String),
    /// The target path is not a directory.
    NotADirectory(String),
    /// Failed to resolve the volume for the target path.
    VolumeResolutionFailed(String),
    /// The target resides on a system/boot partition.
    SystemPartition(String),
    /// The target resides under a critical mount point.
    CriticalMountPoint(String),
    /// The target directory is not writable.
    NotWritable(String),
}

impl std::fmt::Display for SafetyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SafetyError::DirectoryNotFound(p) => {
                write!(f, "Target directory not found: {}", p)
            }
            SafetyError::NotADirectory(p) => {
                write!(f, "Target path is not a directory: {}", p)
            }
            SafetyError::VolumeResolutionFailed(p) => {
                write!(f, "Could not determine the volume for path: {}", p)
            }
            SafetyError::SystemPartition(p) => {
                write!(
                    f,
                    "SAFETY BLOCK: Target '{}' resides on a system partition. \
                     Entropy Verify refuses to write to OS root partitions.",
                    p
                )
            }
            SafetyError::CriticalMountPoint(p) => {
                write!(
                    f,
                    "SAFETY BLOCK: Target '{}' resides under a critical mount point. \
                     Entropy Verify refuses to write to /boot, /etc, /usr, /var, or swap partitions.",
                    p
                )
            }
            SafetyError::NotWritable(p) => {
                write!(f, "Target directory is not writable: {}", p)
            }
        }
    }
}

impl std::error::Error for SafetyError {}

/// Validate that the target directory is safe to use for testing.
///
/// Returns the `VolumeInfo` on success so callers can use it for disk metrics.
pub fn validate_target(target: &Path) -> Result<VolumeInfo, SafetyError> {
    // 1. Check the directory exists
    if !target.exists() {
        // Try to create it
        if let Err(_) = std::fs::create_dir_all(target) {
            return Err(SafetyError::DirectoryNotFound(
                target.display().to_string(),
            ));
        }
    }

    if !target.is_dir() {
        return Err(SafetyError::NotADirectory(target.display().to_string()));
    }

    // 2. Resolve the volume
    let volume = platform::get_volume_info(target).ok_or_else(|| {
        SafetyError::VolumeResolutionFailed(target.display().to_string())
    })?;

    // Resolve target's canonical path and strip UNC prefix if on Windows
    let canonical = std::fs::canonicalize(target).map_err(|_| {
        SafetyError::VolumeResolutionFailed(target.display().to_string())
    })?;
    let mut clean_canonical = canonical;
    #[cfg(target_os = "windows")]
    {
        let path_str = clean_canonical.to_string_lossy();
        if path_str.starts_with(r"\\?\") {
            clean_canonical = std::path::PathBuf::from(&path_str[4..]);
        }
    }

    // 3. System partition check (fails only if targeting the OS root directly)
    if platform::is_system_root(&clean_canonical) {
        return Err(SafetyError::SystemPartition(
            clean_canonical.display().to_string(),
        ));
    }

    // 4. Critical mount point check
    if platform::is_critical_mount(&clean_canonical) {
        return Err(SafetyError::CriticalMountPoint(
            clean_canonical.display().to_string(),
        ));
    }

    // 5. Writability check — create and remove a sentinel file
    let sentinel = target.join(".entropy_verify_sentinel");
    match std::fs::write(&sentinel, b"EV") {
        Ok(_) => {
            let _ = std::fs::remove_file(&sentinel);
        }
        Err(_) => {
            return Err(SafetyError::NotWritable(target.display().to_string()));
        }
    }

    Ok(volume)
}
