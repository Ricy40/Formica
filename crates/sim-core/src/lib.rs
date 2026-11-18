//! Formica simulation core.
//!
//! Pure, deterministic simulation: no I/O, no networking, no rendering. Everything that changes
//! state enters through [`sim::SimCommand`]s applied at tick boundaries, and every random number
//! comes from [`rng`]. See `CLAUDE.md` for the determinism rules.

pub mod agents;
pub mod rng;
pub mod scene;
pub mod sim;
pub mod snapshot;
pub mod voxel;
pub mod world;

pub use glam;
pub use sim::{CommandLog, LoggedCommand, Sim, SimCommand, SimConfig, SimState};
pub use voxel::{CHUNK_SIZE, CHUNK_VOLUME, Chunk, ChunkCoord, Material, Voxel};
pub use world::World;
