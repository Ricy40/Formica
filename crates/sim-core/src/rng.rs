//! Seeded, deterministic random number streams.
//!
//! Two kinds of stream exist:
//! - **Per-system streams** ([`system_rng`]) carried in the serialized state, for systems that run
//!   serially (world generation, spawning).
//! - **Counter-based per-entity generators** ([`entity_rng`]): a fresh generator derived from
//!   `(world seed, system, tick, entity id)`. Parallel systems use these so the numbers an entity
//!   sees never depend on thread count or iteration order.

use rand_core::Rng;
use rand_pcg::Pcg32;
use serde::{Deserialize, Serialize};

/// Identifies the system consuming randomness. Values are part of the determinism contract:
/// never renumber an existing variant, only append.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[repr(u32)]
pub enum SystemId {
    Worldgen = 1,
    AgentSpawn = 2,
    AgentMove = 3,
}

/// SplitMix64 finaliser: a cheap, well-mixed bijection on `u64`.
#[inline]
pub const fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Folds any number of words into a seed. Order matters.
#[inline]
pub fn derive_seed(world_seed: u64, parts: &[u64]) -> u64 {
    let mut h = splitmix64(world_seed);
    for &p in parts {
        h = splitmix64(h ^ p);
    }
    h
}

/// Thin wrapper so the rest of the code base is insulated from `rand_core` API churn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimRng(Pcg32);

impl SimRng {
    #[inline]
    pub fn new(seed: u64, stream: u64) -> Self {
        Self(Pcg32::new(seed, stream))
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        self.0.next_u32()
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.0.next_u64()
    }

    /// Uniform in `[0, 1)` with 24 bits of precision (exactly representable in `f32`).
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }

    /// Uniform in `[lo, hi)`.
    #[inline]
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.next_f32()
    }

    /// Uniform integer in `[0, n)`; `n` must be non-zero. Lemire multiply-shift (bias < 2^-32,
    /// irrelevant at our scale and fully deterministic).
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        debug_assert!(n > 0);
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }

    /// Approximately normal with mean 0 and sd 1 (Irwin-Hall, sum of 4 uniforms). Avoids
    /// transcendentals so it is cheap and bit-identical on every target.
    #[inline]
    pub fn approx_normal(&mut self) -> f32 {
        let s = self.next_f32() + self.next_f32() + self.next_f32() + self.next_f32();
        (s - 2.0) * 1.732_050_8 // sqrt(12 / 4)
    }
}

/// A persistent, serialized stream for a serial system.
pub fn system_rng(world_seed: u64, system: SystemId) -> SimRng {
    SimRng::new(derive_seed(world_seed, &[system as u64]), system as u64)
}

/// A fresh generator for one entity in one tick of one system.
#[inline]
pub fn entity_rng(world_seed: u64, system: SystemId, tick: u64, entity: u64) -> SimRng {
    SimRng::new(
        derive_seed(world_seed, &[system as u64, tick, entity]),
        system as u64,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_same_stream() {
        let mut a = entity_rng(7, SystemId::AgentMove, 10, 3);
        let mut b = entity_rng(7, SystemId::AgentMove, 10, 3);
        for _ in 0..100 {
            assert_eq!(a.next_u32(), b.next_u32());
        }
    }

    #[test]
    fn different_inputs_differ() {
        let base = entity_rng(7, SystemId::AgentMove, 10, 3).next_u64();
        assert_ne!(base, entity_rng(8, SystemId::AgentMove, 10, 3).next_u64());
        assert_ne!(base, entity_rng(7, SystemId::AgentSpawn, 10, 3).next_u64());
        assert_ne!(base, entity_rng(7, SystemId::AgentMove, 11, 3).next_u64());
        assert_ne!(base, entity_rng(7, SystemId::AgentMove, 10, 4).next_u64());
    }

    #[test]
    fn known_answer() {
        // Pins the exact streams: if these change, every saved world and replay changes too.
        assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
        let mut r = SimRng::new(42, 54);
        let got: Vec<u32> = (0..3).map(|_| r.next_u32()).collect();
        // pcg32-demo reference output for pcg32_srandom_r(42, 54).
        assert_eq!(got, [0xa15c_02b7, 0x7b47_f409, 0xba1d_3330]);
    }

    #[test]
    fn f32_in_unit_interval_and_roughly_uniform() {
        let mut r = system_rng(1, SystemId::Worldgen);
        let mut sum = 0.0f64;
        for _ in 0..100_000 {
            let x = r.next_f32();
            assert!((0.0..1.0).contains(&x));
            sum += x as f64;
        }
        let mean = sum / 100_000.0;
        assert!((mean - 0.5).abs() < 0.01, "mean {mean}");
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = system_rng(1, SystemId::Worldgen);
        let mut seen = [false; 5];
        for _ in 0..1000 {
            let v = r.below(5);
            assert!(v < 5);
            seen[v as usize] = true;
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn approx_normal_moments() {
        let mut r = system_rng(3, SystemId::Worldgen);
        let n = 100_000;
        let (mut s, mut s2) = (0.0f64, 0.0f64);
        for _ in 0..n {
            let x = r.approx_normal() as f64;
            s += x;
            s2 += x * x;
        }
        let mean = s / n as f64;
        let var = s2 / n as f64 - mean * mean;
        assert!(mean.abs() < 0.02, "mean {mean}");
        assert!((var - 1.0).abs() < 0.03, "var {var}");
    }
}
