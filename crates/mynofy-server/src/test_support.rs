//! Reproducible bounded generators for functional tests; never used for cryptography.
use std::env;
pub(crate) const DEFAULT_SEED: u64 = 0x6d79_6e6f_6679_0001;
pub(crate) fn setting(name: &str, default: usize, maximum: usize) -> usize {
    env::var(name).map_or(default, |value| {
        let value: usize = value
            .parse()
            .expect("Test count must be a positive integer");
        assert!(
            (1..=maximum).contains(&value),
            "{name} must be in 1..={maximum}"
        );
        value
    })
}
pub(crate) fn seed() -> u64 {
    env::var("MYNOFY_TEST_SEED").map_or(DEFAULT_SEED, |value| {
        if let Some(hex) = value.strip_prefix("0x") {
            u64::from_str_radix(hex, 16)
        } else {
            value.parse()
        }
        .expect("MYNOFY_TEST_SEED must be decimal or 0x-prefixed hexadecimal")
    })
}
pub(crate) fn fuzz_cases() -> usize {
    setting("MYNOFY_FUZZ_CASES", 2048, 1_000_000)
}
pub(crate) struct Rng(pub(crate) u64);
impl Rng {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }
    pub(crate) fn pick(&mut self, length: usize) -> usize {
        assert!(length > 0);
        (self.next() % length as u64) as usize
    }
    /// Bounded byte mutation, truncation, insertion, duplication or random replacement.
    pub(crate) fn mutate(&mut self, original: &[u8], maximum: usize) -> Vec<u8> {
        assert!(original.len() <= maximum);
        let mut bytes = original.to_vec();
        match self.pick(6) {
            0 | 1 if !bytes.is_empty() => {
                for _ in 0..1 + self.pick(8) {
                    let position = self.pick(bytes.len());
                    bytes[position] ^= 1 << self.pick(8);
                }
            }
            2 => {
                let end = self.pick(bytes.len() + 1);
                bytes.truncate(end);
            }
            3 => {
                let count = (1 + self.pick(32)).min(maximum - bytes.len());
                let position = self.pick(bytes.len() + 1);
                let extra: Vec<_> = (0..count).map(|_| self.next() as u8).collect();
                bytes.splice(position..position, extra);
            }
            4 if !bytes.is_empty() => {
                let start = self.pick(bytes.len());
                let length = (1 + self.pick(bytes.len() - start)).min(maximum - bytes.len());
                let extra = bytes[start..start + length].to_vec();
                let position = self.pick(bytes.len() + 1);
                bytes.splice(position..position, extra);
            }
            _ => {
                bytes = (0..self.pick(maximum.min(512) + 1))
                    .map(|_| self.next() as u8)
                    .collect();
            }
        }
        bytes
    }
}
