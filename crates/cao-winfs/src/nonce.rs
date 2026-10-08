//! Staging nonces (#463).

use std::io;

/// Returns 16 unpredictable bytes from the OS (`ProcessPrng` on Windows 10
/// and later), replacing C++'s `std::random_device`.
///
/// # Errors
///
/// Returns the OS error when the random source is unavailable.
pub fn random_nonce() -> io::Result<[u8; 16]> {
    let mut nonce = [0; 16];
    getrandom::fill(&mut nonce)?;
    Ok(nonce)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nonces_differ() {
        let first = random_nonce().unwrap();
        let second = random_nonce().unwrap();
        // 2^-128 odds of a false failure.
        assert_ne!(first, second);
    }
}
