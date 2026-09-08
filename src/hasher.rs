// Shaicoin proof of work, post 2026-09-01:
//     H = SHA256d(header)          header is 112 bytes
//     R = RandomX(seed, H)
//     valid iff R <= target
use primitive_types::U256;
use sha2::{Digest, Sha256};
use crate::randomx::RxVm;

// Serialized CBlockHeader (src/primitives/block.h):
//    0 nVersion 4 | 4 hashPrevBlock 32 | 36 hashMerkleRoot 32
//   68 nTime 4    | 72 nBits 4         | 76 nNonce 4
//   80 hashExtCommitment 32           = 112 bytes
// The nonce is NOT the trailing field the way Bitcoin's is: the extension
// commitment follows it. Splice at byte 76, never append.
pub const HEADER_BYTES: usize = 112;
pub const HEADER_HEX_LEN: usize = HEADER_BYTES * 2; // 224
pub const NONCE_HEX_START: usize = 76 * 2;          // 152
pub const NONCE_HEX_LEN: usize = 8;

/// Replace nNonce in a serialized header. `nonce_hex` is the four nonce bytes
/// in header (little-endian) order and is sent to the pool verbatim, so there
/// is no endianness question on the wire.
pub fn splice_nonce(header_hex: &str, nonce_hex: &str) -> Option<String> {
    if header_hex.len() < HEADER_HEX_LEN || nonce_hex.len() != NONCE_HEX_LEN { return None; }
    let mut s = String::with_capacity(header_hex.len());
    s.push_str(&header_hex[..NONCE_HEX_START]);
    s.push_str(nonce_hex);
    s.push_str(&header_hex[NONCE_HEX_START + NONCE_HEX_LEN..]);
    Some(s)
}

#[inline]
pub fn sha256d(bytes: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(bytes);
    let second = Sha256::digest(first);
    let mut out = [0u8; 32];
    out.copy_from_slice(&second);
    out
}

/// The consensus hash for a candidate header. Returns the raw 32 RandomX bytes.
#[inline]
pub fn pow_hash(vm: &mut RxVm, header_bytes: &[u8]) -> [u8; 32] {
    vm.hash(&sha256d(header_bytes))
}

/// The node stores the RandomX output into a uint256 (little-endian internally)
/// and compares with UintToArith256, so the raw bytes are read little-endian.
/// The hex the pool logs is the reverse of these bytes.
#[inline]
pub fn hash_to_u256(raw: &[u8; 32]) -> U256 { U256::from_little_endian(raw) }

/// Big-endian display form - what the pool and node print.
pub fn hash_to_hex(raw: &[u8; 32]) -> String {
    raw.iter().rev().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn splice_lands_at_byte_76() {
        let h = "ab".repeat(112);
        let s = splice_nonce(&h, "deadbeef").unwrap();
        assert_eq!(s.len(), 224);
        assert_eq!(&s[152..160], "deadbeef");
        assert_eq!(&s[..152], &h[..152]);
        assert_eq!(&s[160..], &h[160..], "the 32-byte ext commitment must survive");
    }
    #[test]
    fn rejects_bad_input() {
        assert!(splice_nonce(&"ab".repeat(112), "dead").is_none());
        assert!(splice_nonce("abcd", "deadbeef").is_none());
    }
    #[test]
    fn endianness_matches_the_node() {
        // raw little-endian 0x01 in the low byte -> displayed hex ends ...01
        let mut raw = [0u8; 32]; raw[0] = 1;
        assert_eq!(hash_to_u256(&raw), U256::one());
        assert!(hash_to_hex(&raw).ends_with("01"));
    }
}
