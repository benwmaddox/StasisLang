// Global paths and string literals share this stable FNV-1a identity contract
// across native and Web emitters.
pub(crate) fn hash_global_path(path: &str) -> i32 {
    let mut hash: u32 = 2_166_136_261;
    for byte in path.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as i32
}

pub(crate) fn hash_string_literal(value: &str) -> i32 {
    hash_global_path(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_hashes_remain_stable() {
        assert_eq!(hash_global_path("score"), -768_634_731);
        assert_eq!(hash_string_literal("fixed browser sample"), -1_269_689_559);
    }
}
