/// FNV-1a 64-bit hash.
///
/// Canonical implementation used across the workspace for source_path
/// hashing and any other non-cryptographic hashing needs.
pub fn fnv1a_u64(data: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 14695981039346656037;
    const PRIME: u64 = 1099511628211;
    let mut hash = OFFSET_BASIS;
    for byte in data {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_u64_stability() {
        let h1 = fnv1a_u64(b"context-tree/k8s.md");
        let h2 = fnv1a_u64(b"context-tree/k8s.md");
        assert_eq!(h1, h2, "same input must produce same hash");

        let h3 = fnv1a_u64(b"context-tree/redis.md");
        assert_ne!(h1, h3, "different inputs should produce different hashes");
    }

    #[test]
    fn test_fnv1a_u64_matches_legacy() {
        // Ensure the canonical implementation produces the same output as
        // the legacy fnv1a_64 in temporal.rs (which now delegates here).
        let input = b"some/path.md";
        let expected: u64 = {
            const OFFSET_BASIS: u64 = 14695981039346656037;
            const PRIME: u64 = 1099511628211;
            let mut hash = OFFSET_BASIS;
            for byte in input.iter() {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(PRIME);
            }
            hash
        };
        assert_eq!(fnv1a_u64(input), expected);
    }
}
