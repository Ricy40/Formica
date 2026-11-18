//! Hand-made scenes. Phase 0 has one: a single 32³ chunk with soil strata, a stone, a leaf-litter
//! patch, a puddle and a founding-style burrow (entrance shaft down to a chamber). Procedural world
//! generation replaces this in Phase 1.

use glam::IVec3;
use serde::{Deserialize, Serialize};

use crate::voxel::{CHUNK_SIZE_I32, ChunkCoord, Material, Voxel};
use crate::world::{Extent, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SceneConfig {
    /// One hand-made chunk with `agents` dummy walkers.
    TestChunk { agents: u32 },
}

/// Ground height (standing height) of the test chunk's undisturbed surface at a column.
fn ground_height(x: i32, z: i32) -> i32 {
    let fx = x as f32;
    let fz = z as f32;
    let h = 21.0 + 1.6 * libm::sinf(fx / 5.0) * libm::cosf(fz / 6.5) + 0.8 * libm::sinf(fz / 3.1);
    libm::floorf(h) as i32
}

fn strata(x: i32, y: i32, z: i32, surface: i32) -> Material {
    let depth = surface - 1 - y;
    let rock_top = 3 + ((x + 2 * z) % 3).abs() / 2;
    if y < rock_top {
        Material::Rock
    } else if depth < 5 {
        Material::Topsoil
    } else if (x - 22).pow(2) + (y - 9).pow(2) * 3 + (z - 8).pow(2) < 30 {
        Material::Clay
    } else if (x - 6).pow(2) + (y - 7).pow(2) * 4 + (z - 24).pow(2) < 24 {
        Material::Sand
    } else {
        Material::Subsoil
    }
}

pub fn build_test_chunk() -> World {
    let s = CHUNK_SIZE_I32;
    let mut world = World::new(Extent {
        min: IVec3::ZERO,
        size: IVec3::splat(s),
    });
    world.insert_chunk(
        ChunkCoord::new(0, 0, 0),
        crate::voxel::Chunk::uniform(Material::Air),
    );

    // Soil strata below an undulating surface.
    for z in 0..s {
        for x in 0..s {
            let h = ground_height(x, z);
            for y in 0..h {
                world.set(IVec3::new(x, y, z), Voxel::of(strata(x, y, z, h)));
            }
        }
    }

    // A flat stone lying on the surface (prime Lasius nest site).
    for z in 18..25 {
        for x in 19..28 {
            let base = ground_height(x, z);
            for y in base..base + 2 {
                world.set(IVec3::new(x, y, z), Voxel::of(Material::Rock));
            }
        }
    }

    // Leaf-litter patch.
    for z in 2..9 {
        for x in 20..29 {
            if (x * 7 + z * 3) % 4 != 0 {
                let h = ground_height(x, z);
                world.set(IVec3::new(x, h, z), Voxel::of(Material::LeafLitter));
            }
        }
    }

    // Puddle in a shallow scrape.
    for z in 25..30 {
        for x in 3..9 {
            let h = ground_height(x, z);
            world.set(IVec3::new(x, h - 1, z), Voxel::of(Material::Water));
        }
    }

    // Founding burrow: a sloping shaft from an entrance next to the stone down to a chamber.
    let dug = Voxel {
        flags: Voxel::FLAG_DUG,
        ..Voxel::AIR
    };
    let entrance = IVec3::new(17, ground_height(17, 21), 21);
    let chamber = IVec3::new(13, 10, 14);
    let steps = 40;
    for i in 0..=steps {
        let t = i as f32 / steps as f32;
        let p = entrance.as_vec3().lerp(chamber.as_vec3(), t);
        let c = IVec3::new(p.x.round() as i32, p.y.round() as i32, p.z.round() as i32);
        for d in [
            IVec3::ZERO,
            IVec3::X,
            IVec3::Z,
            IVec3::X + IVec3::Z,
            IVec3::Y,
            IVec3::Y + IVec3::X,
        ] {
            world.set(c + d, dug);
        }
    }
    for z in -5..=5 {
        for y in -2..=2 {
            for x in -6..=6 {
                let e = (x * x) as f32 / 36.0 + (y * y) as f32 / 6.0 + (z * z) as f32 / 25.0;
                if e <= 1.0 {
                    world.set(chamber + IVec3::new(x, y, z), dug);
                }
            }
        }
    }

    world.drain_edits();
    world.rebuild_surface();
    world
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_has_expected_features() {
        let w = build_test_chunk();
        assert_eq!(w.chunk_counts(), (1, 0));
        // Bedrock at the bottom, air at the top.
        assert_eq!(w.get(IVec3::new(5, 0, 5)).material, Material::Rock);
        assert_eq!(w.get(IVec3::new(5, 31, 5)).material, Material::Air);
        // Chamber is dug out underground.
        let c = w.get(IVec3::new(13, 10, 14));
        assert_eq!(c.material, Material::Air);
        assert_ne!(c.flags & Voxel::FLAG_DUG, 0);
        // Stone, litter, water present on the surface.
        assert_eq!(w.surface_at(23, 21).unwrap().1, Material::Rock);
        assert_eq!(w.surface_at(5, 27).unwrap().1, Material::Water);
    }

    #[test]
    fn test_chunk_is_deterministic() {
        assert_eq!(build_test_chunk(), build_test_chunk());
    }
}
