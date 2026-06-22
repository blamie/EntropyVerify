/// BLAKE3 block checksum utilities.
///
/// Uses the BLAKE3 hashing algorithm for block verification.
/// BLAKE3 auto-vectorizes with AVX2/SSE4.1, achieving ~2 bytes/cycle —
/// exceeding 6 GB/s single-threaded on modern CPUs.

/// Compute the BLAKE3 hash of a payload, returning a 32-byte digest.
#[inline]
pub fn hash_payload(payload: &[u8]) -> [u8; 32] {
    *blake3::hash(payload).as_bytes()
}

/// Verify that a payload matches an expected BLAKE3 hash.
#[inline]
#[allow(dead_code)]
pub fn verify_hash(payload: &[u8], expected: &[u8; 32]) -> bool {
    let actual = hash_payload(payload);
    // Constant-time comparison isn't needed here (not a security context),
    // but standard PartialEq on arrays is fine.
    actual == *expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hash_deterministic() {
        let data = b"EntropyVerify test data";
        let h1 = hash_payload(data);
        let h2 = hash_payload(data);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_differs() {
        let h1 = hash_payload(b"data_a");
        let h2 = hash_payload(b"data_b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_verify() {
        let data = b"verification payload";
        let hash = hash_payload(data);
        assert!(verify_hash(data, &hash));
        assert!(!verify_hash(b"wrong data", &hash));
    }
}
