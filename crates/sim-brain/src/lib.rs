//! Formica brains: NEAT genomes, mutation, crossover, speciation and batched evaluation.
//!
//! Phase 0 only establishes the crate and its identifiers. The NEAT implementation arrives in
//! Phase 3 (see `docs/RESEARCH.md`, "Brain design and evolution").

use serde::{Deserialize, Serialize};

/// Global innovation number for a structural mutation (NEAT historical marking).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Innovation(pub u32);

/// Identifies a genome shared by many ants (one genome per colony lineage).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct GenomeId(pub u32);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn innovations_order_numerically() {
        assert!(Innovation(3) < Innovation(10));
    }
}
