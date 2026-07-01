// blake3 utilities

#[inline]
pub fn hash_payload(payload: &[u8]) -> [u8; 32] {
    *blake3::hash(payload).as_bytes()
}

#[inline]
#[allow(dead_code)]
pub fn verify_hash(payload: &[u8], expected: &[u8; 32]) -> bool {
    let actual = hash_payload(payload);
    
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
