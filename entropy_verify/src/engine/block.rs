/// Block header layout and serialization.
///
/// Each 2 MiB block has a 64-byte structured header followed by the AES-CTR payload.
/// The header is `#[repr(C)]` for zero-copy serialization via `bytemuck`.

/// Size of the block header in bytes.
pub const HEADER_SIZE: usize = 64;

/// Magic bytes identifying an Entropy Verify block.
pub const MAGIC: [u8; 4] = *b"EVFY";

/// Current block format version.
pub const FORMAT_VERSION: u8 = 1;

/// Block header — 64 bytes, zero-copy serializable.
///
/// Layout:
/// ```text
/// Offset  Size  Field
/// 0       4     magic ("EVFY")
/// 4       1     version (1)
/// 5       1     _pad
/// 6       2     thread_id
/// 8       4     file_index
/// 12      4     block_index
/// 16      16    aes_nonce (CTR nonce used to generate this block's payload)
/// 32      32    blake3_hash (BLAKE3 digest of the payload only)
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BlockHeader {
    pub magic: [u8; 4],
    pub version: u8,
    pub _pad: u8,
    pub thread_id: u16,
    pub file_index: u32,
    pub block_index: u32,
    pub aes_nonce: [u8; 16],
    pub blake3_hash: [u8; 32],
}

// Compile-time size assertion.
const _: () = assert!(std::mem::size_of::<BlockHeader>() == HEADER_SIZE);

impl BlockHeader {
    /// Create a new block header with the given metadata.
    pub fn new(
        file_index: u32,
        block_index: u32,
        thread_id: u16,
        aes_nonce: [u8; 16],
        blake3_hash: [u8; 32],
    ) -> Self {
        Self {
            magic: MAGIC,
            version: FORMAT_VERSION,
            _pad: 0,
            thread_id,
            file_index,
            block_index,
            aes_nonce,
            blake3_hash,
        }
    }

    /// Validate the magic and version fields.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.magic != MAGIC {
            return Err(BlockError::InvalidMagic(self.magic));
        }
        if self.version != FORMAT_VERSION {
            return Err(BlockError::UnsupportedVersion(self.version));
        }
        Ok(())
    }

    /// Serialize this header into a byte slice (zero-copy).
    pub fn as_bytes(&self) -> &[u8; HEADER_SIZE] {
        bytemuck::bytes_of(self)
            .try_into()
            .expect("BlockHeader is exactly HEADER_SIZE bytes")
    }

    /// Deserialize a header from a byte slice (zero-copy).
    pub fn from_bytes(bytes: &[u8; HEADER_SIZE]) -> &Self {
        bytemuck::from_bytes(bytes)
    }
}

/// Errors encountered when parsing block headers.
#[derive(Debug)]
pub enum BlockError {
    InvalidMagic([u8; 4]),
    UnsupportedVersion(u8),
}

impl std::fmt::Display for BlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlockError::InvalidMagic(m) => write!(f, "Invalid magic: {:?}", m),
            BlockError::UnsupportedVersion(v) => {
                write!(f, "Unsupported block version: {}", v)
            }
        }
    }
}

impl std::error::Error for BlockError {}

/// Stamp a block header into the first `HEADER_SIZE` bytes of a buffer,
/// and fill the payload region with AES-CTR data, then compute the BLAKE3 hash.
///
/// Returns the completed header for reporting purposes.
pub fn prepare_block(
    buf: &mut [u8],
    file_index: u32,
    block_index: u32,
    thread_id: u16,
    generator: &crate::crypto::datagen::BlockGenerator,
) -> BlockHeader {
    let payload = &mut buf[HEADER_SIZE..];

    // 1. Generate AES-CTR pseudo-random payload
    generator.fill_block(file_index, block_index, payload);

    // 2. Compute BLAKE3 hash of the payload
    let hash = crate::crypto::hasher::hash_payload(payload);

    // 3. Derive the nonce (for header metadata)
    let nonce = crate::crypto::datagen::BlockGenerator::derive_nonce(file_index, block_index);

    // 4. Build and stamp the header
    let header = BlockHeader::new(file_index, block_index, thread_id, nonce, hash);
    buf[..HEADER_SIZE].copy_from_slice(header.as_bytes());

    header
}

/// Verify a block by regenerating the expected payload and comparing BLAKE3 hashes.
///
/// Returns `Ok(())` if the block is valid, or `Err(...)` with details if corrupted.
pub fn verify_block(
    buf: &[u8],
    generator: &crate::crypto::datagen::BlockGenerator,
) -> Result<BlockHeader, VerifyError> {
    if buf.len() < HEADER_SIZE {
        return Err(VerifyError::BufferTooSmall(buf.len()));
    }

    // 1. Parse header
    let header_bytes: &[u8; HEADER_SIZE] = buf[..HEADER_SIZE]
        .try_into()
        .map_err(|_| VerifyError::BufferTooSmall(buf.len()))?;
    let header = *BlockHeader::from_bytes(header_bytes);

    // 2. Validate magic/version
    header
        .validate()
        .map_err(|e| VerifyError::HeaderInvalid(e.to_string()))?;

    // 3. Verify BLAKE3 hash of the stored payload
    let payload = &buf[HEADER_SIZE..];
    let actual_hash = crate::crypto::hasher::hash_payload(payload);

    if actual_hash != header.blake3_hash {
        return Err(VerifyError::HashMismatch {
            file_index: header.file_index,
            block_index: header.block_index,
            expected: header.blake3_hash,
            actual: actual_hash,
        });
    }

    // 4. Regenerate the expected payload and compare
    let mut expected_payload = vec![0u8; payload.len()];
    generator.fill_block(header.file_index, header.block_index, &mut expected_payload);
    let expected_hash = crate::crypto::hasher::hash_payload(&expected_payload);

    if expected_hash != actual_hash {
        return Err(VerifyError::DataCorrupted {
            file_index: header.file_index,
            block_index: header.block_index,
            expected_hash,
            actual_hash: actual_hash,
        });
    }

    Ok(header)
}

/// Errors encountered during block verification.
#[derive(Debug, Clone)]
pub enum VerifyError {
    BufferTooSmall(usize),
    HeaderInvalid(String),
    HashMismatch {
        file_index: u32,
        block_index: u32,
        expected: [u8; 32],
        actual: [u8; 32],
    },
    DataCorrupted {
        file_index: u32,
        block_index: u32,
        expected_hash: [u8; 32],
        actual_hash: [u8; 32],
    },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::BufferTooSmall(sz) => {
                write!(f, "Buffer too small: {} bytes (need >= {})", sz, HEADER_SIZE)
            }
            VerifyError::HeaderInvalid(msg) => write!(f, "Invalid header: {}", msg),
            VerifyError::HashMismatch {
                file_index,
                block_index,
                ..
            } => write!(
                f,
                "BLAKE3 hash mismatch at file {} block {}",
                file_index, block_index
            ),
            VerifyError::DataCorrupted {
                file_index,
                block_index,
                ..
            } => write!(
                f,
                "Data corruption detected at file {} block {}",
                file_index, block_index
            ),
        }
    }
}

impl std::error::Error for VerifyError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::datagen::BlockGenerator;

    #[test]
    fn test_header_size() {
        assert_eq!(std::mem::size_of::<BlockHeader>(), 64);
    }

    #[test]
    fn test_round_trip() {
        let header = BlockHeader::new(42, 7, 3, [0xAA; 16], [0xBB; 32]);
        let bytes = header.as_bytes();
        let parsed = BlockHeader::from_bytes(bytes);
        assert_eq!(parsed.file_index, 42);
        assert_eq!(parsed.block_index, 7);
        assert_eq!(parsed.thread_id, 3);
    }

    #[test]
    fn test_prepare_and_verify() {
        let gen = BlockGenerator::new([0x42; 16]);
        let block_size = 4096 + HEADER_SIZE; // Small block for testing
        let mut buf = vec![0u8; block_size];

        prepare_block(&mut buf, 0, 0, 1, &gen);

        let result = verify_block(&buf, &gen);
        assert!(result.is_ok(), "Block should verify: {:?}", result);
    }

    #[test]
    fn test_corruption_detected() {
        let gen = BlockGenerator::new([0x42; 16]);
        let block_size = 4096 + HEADER_SIZE;
        let mut buf = vec![0u8; block_size];

        prepare_block(&mut buf, 0, 0, 1, &gen);

        // Corrupt one byte in the payload
        buf[HEADER_SIZE + 10] ^= 0xFF;

        let result = verify_block(&buf, &gen);
        assert!(result.is_err(), "Corrupted block should fail verification");
    }
}
