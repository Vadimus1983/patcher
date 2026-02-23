/// Adler32-style rolling hash for block matching.
///
/// Uses two 16-bit sums (a, b) combined into a 32-bit hash.
/// Supports O(1) sliding window updates: remove oldest byte, add new byte.
const MOD_ADLER: u32 = 65521;

pub struct RollingHash {
    a: u32,
    b: u32,
    window_size: u32,
}

impl RollingHash {
    pub fn new() -> Self {
        Self {
            a: 1,
            b: 0,
            window_size: 0,
        }
    }

    /// Compute hash over an initial block of data.
    pub fn init(&mut self, data: &[u8]) {
        self.window_size = data.len() as u32;
        // Accumulate in u64 to defer all modular reductions to a single pair of operations
        // at the end, rather than reducing on every byte.
        let mut a: u64 = 1;
        let mut b: u64 = 0;
        for &byte in data {
            a += byte as u64;
            b += a;
        }
        self.a = (a % MOD_ADLER as u64) as u32;
        self.b = (b % MOD_ADLER as u64) as u32;
    }

    /// Slide the window: remove `old_byte` from front, add `new_byte` at back.
    pub fn rotate(&mut self, old_byte: u8, new_byte: u8) {
        let old = old_byte as u32;
        let new = new_byte as u32;

        self.a = (self.a + MOD_ADLER - old + new) % MOD_ADLER;
        self.b = (self.b + MOD_ADLER - 1 + self.a
            - (old * self.window_size) % MOD_ADLER)
            % MOD_ADLER;
    }

    pub fn digest(&self) -> u32 {
        (self.b << 16) | self.a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_init_deterministic() {
        let data = b"Hello, World!";
        let mut h1 = RollingHash::new();
        h1.init(data);
        let mut h2 = RollingHash::new();
        h2.init(data);
        assert_eq!(h1.digest(), h2.digest());
    }

    #[test]
    fn test_different_data_different_hash() {
        let mut h1 = RollingHash::new();
        h1.init(b"Hello");
        let mut h2 = RollingHash::new();
        h2.init(b"World");
        assert_ne!(h1.digest(), h2.digest());
    }

    #[test]
    fn test_rotate_equals_fresh_init() {
        let data = b"ABCDE";
        let mut rolling = RollingHash::new();
        rolling.init(&data[0..4]);
        rolling.rotate(data[0], data[4]);

        let mut fresh = RollingHash::new();
        fresh.init(&data[1..5]);

        assert_eq!(rolling.digest(), fresh.digest());
    }
}
