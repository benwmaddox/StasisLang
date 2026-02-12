#![forbid(unsafe_code)]

/// Placeholder JIT/AOT substrate crate for Rewrite V1.
pub fn crate_ready() -> bool {
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_ready() {
        assert!(super::crate_ready());
    }
}
