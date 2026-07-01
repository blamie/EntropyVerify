// Config parser & test planner

use clap::Parser;
use std::path::PathBuf;

pub const DEFAULT_BLOCK_SIZE: u64 = 2 * 1024 * 1024;

pub const DEFAULT_FILE_SIZE: u64 = 1024 * 1024 * 1024;

pub const DEFAULT_QUEUE_DEPTH: u32 = 32;

// safety buffer to prevent filling disk 100%
pub const SAFETY_MARGIN_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Parser, Debug, Clone)]
#[command(name = "entropy_verify", version, about, long_about = None)]
pub struct Config {
    /// Target directory on the volume to test (must not be on a system partition).
    #[arg(short = 't', long = "target-dir")]
    pub target_dir: PathBuf,

    /// Block size in bytes for each I/O operation.
    #[arg(long, default_value_t = DEFAULT_BLOCK_SIZE)]
    pub block_size: u64,

    /// File segment size in bytes. Each test file is this size (except possibly the last).
    #[arg(long, default_value_t = DEFAULT_FILE_SIZE)]
    pub file_size: u64,

    /// Async I/O queue depth per worker thread.
    #[arg(long, default_value_t = DEFAULT_QUEUE_DEPTH)]
    pub queue_depth: u32,

    /// Number of worker threads (default: number of physical CPU cores).
    #[arg(long)]
    pub threads: Option<usize>,

    /// Run only the write phase (skip verification).
    #[arg(long, default_value_t = false)]
    pub write_only: bool,

    /// Run only the verify phase (skip writing — requires existing test files).
    #[arg(long, default_value_t = false)]
    pub verify_only: bool,

    /// Deterministic AES-CTR seed as a hex string (e.g., "0xDEADBEEF...").
    /// If omitted, a random seed is generated.
    #[arg(long)]
    pub seed: Option<String>,

    /// Allow testing on the system drive partition for large tests (>= 100 GiB).
    #[arg(long, default_value_t = false)]
    pub allow_c_drive_testing: bool,
}

impl Config {
    
    pub fn num_threads(&self) -> usize {
        self.threads.unwrap_or_else(|| {
            let cpus = num_cpus::get_physical();
            // Reserve at least 1 core for the TUI; use at least 1 worker.
            cpus.saturating_sub(1).max(1)
        })
    }

    pub fn blocks_per_full_file(&self) -> u32 {
        (self.file_size / self.block_size) as u32
    }

    // parse hex or generate random
    pub fn resolve_seed(&self) -> anyhow::Result<[u8; 16]> {
        match &self.seed {
            Some(hex_str) => parse_hex_seed(hex_str),
            None => {
                let mut key = [0u8; 16];
                use rand::RngCore;
                rand::thread_rng().fill_bytes(&mut key);
                Ok(key)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct TestPlan {
    /// Total number of files to write.
    pub total_files: u32,
    /// Number of 2-MiB blocks per full file.
    pub blocks_per_full_file: u32,
    /// Number of blocks in the final (possibly partial) file. 0 if last file is full.
    pub blocks_in_last_file: u32,
    /// Total number of blocks across all files.
    pub total_blocks: u64,
    /// Total bytes to be written/verified.
    pub total_bytes: u64,
}

impl TestPlan {
    // calculate files & blocks
    pub fn compute(available_bytes: u64, config: &Config) -> anyhow::Result<Self> {
        let usable_bytes = available_bytes.saturating_sub(SAFETY_MARGIN_BYTES);

        if usable_bytes < config.block_size {
            anyhow::bail!(
                "Insufficient disk space: {} bytes available (need at least {} bytes + {} safety margin)",
                available_bytes,
                config.block_size,
                SAFETY_MARGIN_BYTES
            );
        }

        let blocks_per_full_file = config.blocks_per_full_file();
        let full_files = usable_bytes / config.file_size;
        let remaining_bytes = usable_bytes % config.file_size;
        let remaining_blocks = (remaining_bytes / config.block_size) as u32;

        let total_files = if remaining_blocks > 0 {
            full_files as u32 + 1
        } else {
            full_files as u32
        };

        let total_blocks =
            full_files * blocks_per_full_file as u64 + remaining_blocks as u64;
        let total_bytes = total_blocks * config.block_size;

        Ok(TestPlan {
            total_files,
            blocks_per_full_file,
            blocks_in_last_file: remaining_blocks,
            total_blocks,
            total_bytes,
        })
    }

    pub fn blocks_for_file(&self, file_index: u32) -> u32 {
        if self.blocks_in_last_file > 0 && file_index == self.total_files - 1 {
            self.blocks_in_last_file
        } else {
            self.blocks_per_full_file
        }
    }

    pub fn file_name(file_index: u32) -> String {
        format!("ev_chunk_{:06}.bin", file_index)
    }
}

fn parse_hex_seed(hex_str: &str) -> anyhow::Result<[u8; 16]> {
    let hex_str = hex_str
        .strip_prefix("0x")
        .or_else(|| hex_str.strip_prefix("0X"))
        .unwrap_or(hex_str);

    if hex_str.len() < 2 {
        anyhow::bail!("Seed hex string is too short (need at least 1 byte)");
    }

    let bytes: Vec<u8> = (0..hex_str.len())
        .step_by(2)
        .map(|i| {
            let end = (i + 2).min(hex_str.len());
            u8::from_str_radix(&hex_str[i..end], 16)
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("Invalid hex in seed: {}", e))?;

    let mut key = [0u8; 16];
    let copy_len = bytes.len().min(16);
    key[..copy_len].copy_from_slice(&bytes[..copy_len]);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_seed() {
        let key = parse_hex_seed("0xDEADBEEF01020304AABBCCDD11223344").unwrap();
        assert_eq!(key[0], 0xDE);
        assert_eq!(key[1], 0xAD);
    }

    #[test]
    fn test_parse_hex_seed_short() {
        let key = parse_hex_seed("ABCD").unwrap();
        assert_eq!(key[0], 0xAB);
        assert_eq!(key[1], 0xCD);
        assert_eq!(key[2], 0x00); // padded with zeros
    }

    #[test]
    fn test_plan_computation() {
        let config = Config {
            target_dir: PathBuf::from("/tmp"),
            block_size: DEFAULT_BLOCK_SIZE,
            file_size: DEFAULT_FILE_SIZE,
            queue_depth: DEFAULT_QUEUE_DEPTH,
            threads: None,
            write_only: false,
            verify_only: false,
            seed: None,
            allow_c_drive_testing: false,
        };

        // 2.5 GiB available (minus 100 MiB safety = ~2.4 GiB usable)
        let avail = 2_684_354_560u64; // 2.5 GiB
        let plan = TestPlan::compute(avail, &config).unwrap();

        assert_eq!(plan.blocks_per_full_file, 512);
        assert!(plan.total_files >= 2);
        assert!(plan.total_bytes > 0);
    }
}
