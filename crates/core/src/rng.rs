//! A tiny, fully specified PRNG.
//!
//! Hand-rolled rather than pulled from `rand` for one reason: reproducibility
//! is a contract here. `StartRunResponse` echoes the seed so a run can be
//! replayed exactly, which means the generator's output must not shift when a
//! dependency changes its default algorithm or its platform-specific paths.
//!
//! SplitMix64, as published by Vigna.

#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`. Returns 0 when `n == 0`.
    #[inline]
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_stream() {
        let a: Vec<u64> = (0..16).map(|_| Rng::new(42).next_u64()).collect();
        let mut r = Rng::new(42);
        let b: Vec<u64> = (0..16).map(|_| r.next_u64()).collect();
        assert_eq!(a[0], b[0]);

        let mut x = Rng::new(7);
        let mut y = Rng::new(7);
        for _ in 0..1000 {
            assert_eq!(x.next_u64(), y.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut x = Rng::new(1);
        let mut y = Rng::new(2);
        assert_ne!(x.next_u64(), y.next_u64());
    }
}
