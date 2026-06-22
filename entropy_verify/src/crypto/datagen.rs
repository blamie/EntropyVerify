/// AES-128-CTR pseudo-random block data generator.
///
/// Uses hardware-accelerated AES-NI in Counter mode to produce
/// incompressible pseudo-random data at >7 GB/s per core. The nonce
/// for each block is deterministically derived from (file_index, block_index),
/// ensuring the same seed always produces identical data for verification.

use aes::Aes128;
use cipher::{KeyIvInit, StreamCipher};
use ctr::Ctr128BE;

/// AES-128-CTR based block data generator.
///
/// Each instance holds a master key. Block data is generated
/// deterministically from (file_index, block_index) so the verification
/// pass can regenerate the expected data without storing it.
#[derive(Clone)]
pub struct BlockGenerator {
    key: [u8; 16],
}

impl BlockGenerator {
    /// Create a new generator with the given 16-byte AES key.
    pub fn new(key: [u8; 16]) -> Self {
        Self { key }
    }

    /// Return the master key (for report serialization).
    #[allow(dead_code)]
    pub fn key(&self) -> &[u8; 16] {
        &self.key
    }

    /// Fill `buf` with deterministic pseudo-random bytes derived from
    /// the given file and block indices.
    ///
    /// The nonce is: `[file_index (4 bytes LE) | block_index (4 bytes LE) | 0x00 × 8]`
    /// This guarantees unique keystream for every block across all files.
    pub fn fill_block(&self, file_index: u32, block_index: u32, buf: &mut [u8]) {
        let nonce = derive_nonce(file_index, block_index);
        let mut cipher =
            Ctr128BE::<Aes128>::new(self.key.as_ref().into(), nonce.as_ref().into());

        // Zero-fill first, then apply keystream.
        // apply_keystream XORs the buffer with the keystream.
        // Since we fill with zeros first, the result is pure keystream.
        buf.fill(0);
        cipher.apply_keystream(buf);
    }

    /// Derive the 16-byte nonce for a given (file_index, block_index).
    /// Exposed for header serialization.
    pub fn derive_nonce(file_index: u32, block_index: u32) -> [u8; 16] {
        derive_nonce(file_index, block_index)
    }
}

/// Derive a 16-byte CTR nonce from file and block indices.
fn derive_nonce(file_index: u32, block_index: u32) -> [u8; 16] {
    let mut nonce = [0u8; 16];
    nonce[0..4].copy_from_slice(&file_index.to_le_bytes());
    nonce[4..8].copy_from_slice(&block_index.to_le_bytes());
    // bytes 8..16 are zero — unique counter space for CTR mode
    nonce
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_output() {
        let gen = BlockGenerator::new([0xAA; 16]);
        let mut buf1 = vec![0u8; 4096];
        let mut buf2 = vec![0u8; 4096];

        gen.fill_block(0, 0, &mut buf1);
        gen.fill_block(0, 0, &mut buf2);
        assert_eq!(buf1, buf2, "Same inputs must produce identical output");
    }

    #[test]
    fn test_different_blocks_differ() {
        let gen = BlockGenerator::new([0xBB; 16]);
        let mut buf1 = vec![0u8; 4096];
        let mut buf2 = vec![0u8; 4096];

        gen.fill_block(0, 0, &mut buf1);
        gen.fill_block(0, 1, &mut buf2);
        assert_ne!(buf1, buf2, "Different block indices must produce different data");
    }

    #[test]
    fn test_output_not_all_zeros() {
        let gen = BlockGenerator::new([0xCC; 16]);
        let mut buf = vec![0u8; 4096];
        gen.fill_block(0, 0, &mut buf);
        assert!(buf.iter().any(|&b| b != 0), "Output must not be all zeros");
    }

    #[test]
    fn test_incompressible() {
        // A simple entropy check: count unique byte values in 4K of output.
        // Good pseudo-random data should use most of the 256 possible byte values.
        let gen = BlockGenerator::new([0xDD; 16]);
        let mut buf = vec![0u8; 4096];
        gen.fill_block(0, 0, &mut buf);

        let mut seen = [false; 256];
        for &b in &buf {
            seen[b as usize] = true;
        }
        let unique = seen.iter().filter(|&&v| v).count();
        assert!(
            unique > 200,
            "Expected >200 unique byte values in 4K, got {}",
            unique
        );
    }
}
