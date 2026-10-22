//! Small deterministic random number generator (SplitMix64).
//!
//! Every view and resource gets its own stream derived from the corpus seed
//! and a stable index, so output does not depend on generation order or
//! thread count.

#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed ^ 0x9E37_79B9_7F4A_7C15)
    }

    /// An independent stream for `(seed, stream, index)`.
    pub fn derive(seed: u64, stream: &str, index: u64) -> Rng {
        let mut h = seed ^ 0xCBF2_9CE4_8422_2325;
        for b in stream.bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01B3);
        }
        let mut r = Rng::new(h ^ index.wrapping_mul(0xD6E8_FEB8_6659_FD93));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[0, n)`; `n` must be positive.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform in `[lo, hi]`.
    pub fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    pub fn weighted<T: Copy>(&mut self, items: &[(T, u32)]) -> T {
        let total: u32 = items.iter().map(|(_, w)| w).sum();
        let mut x = (self.next_u64() % u64::from(total)) as u32;
        for (item, w) in items {
            if x < *w {
                return *item;
            }
            x -= w;
        }
        items[items.len() - 1].0
    }

    /// Log-normal sample, rounded, clamped to `[lo, hi]`.
    pub fn lognormal(&mut self, mu: f64, sigma: f64, lo: usize, hi: usize) -> usize {
        let u1 = self.f64().max(1e-12);
        let u2 = self.f64();
        let z = (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos();
        ((mu + sigma * z).exp().round() as usize).clamp(lo, hi)
    }
}

#[cfg(test)]
mod tests {
    use super::Rng;

    #[test]
    fn deterministic_streams() {
        let a: Vec<u64> = (0..4).map(|_| Rng::derive(7, "view", 3).next_u64()).collect();
        assert!(a.windows(2).all(|w| w[0] == w[1]));
        assert_ne!(
            Rng::derive(7, "view", 3).next_u64(),
            Rng::derive(7, "view", 4).next_u64()
        );
    }

    #[test]
    fn lognormal_is_clamped() {
        let mut r = Rng::new(1);
        for _ in 0..1000 {
            let x = r.lognormal(2.0, 2.0, 1, 50);
            assert!((1..=50).contains(&x));
        }
    }
}
