//! Voxels and chunks.
//!
//! 1 voxel = 2 mm (about the width of a small worker's tunnel). Chunks are 32³ voxels (6.4 cm a
//! side). A chunk nobody has touched is stored as a single [`Voxel`] value; it is promoted to a
//! dense array on the first write and demoted again by [`Chunk::compact`] once it is uniform.

use glam::{IVec3, UVec3};
use serde::{Deserialize, Serialize};

pub const CHUNK_SIZE: usize = 32;
pub const CHUNK_SIZE_I32: i32 = CHUNK_SIZE as i32;
pub const CHUNK_VOLUME: usize = CHUNK_SIZE * CHUNK_SIZE * CHUNK_SIZE;

/// Voxel materials (research doc: soil, clay, sand, loam, gravel, rock, wood, leaf litter, water,
/// air; soil split into topsoil and subsoil for strata). Discriminants are part of the save and
/// wire format: only append.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum Material {
    #[default]
    Air = 0,
    Water = 1,
    Topsoil = 2,
    Subsoil = 3,
    Clay = 4,
    Sand = 5,
    Loam = 6,
    Gravel = 7,
    Rock = 8,
    Wood = 9,
    LeafLitter = 10,
}

impl Material {
    pub const ALL: [Material; 11] = [
        Material::Air,
        Material::Water,
        Material::Topsoil,
        Material::Subsoil,
        Material::Clay,
        Material::Sand,
        Material::Loam,
        Material::Gravel,
        Material::Rock,
        Material::Wood,
        Material::LeafLitter,
    ];

    /// Can an ant stand on it / does it block movement?
    #[inline]
    pub const fn is_solid(self) -> bool {
        !matches!(self, Material::Air | Material::Water)
    }

    #[inline]
    pub const fn from_u8(v: u8) -> Option<Material> {
        if (v as usize) < Self::ALL.len() {
            Some(Self::ALL[v as usize])
        } else {
            None
        }
    }
}

/// One voxel, 4 bytes. The `moisture` and `temperature` encodings are fixed in Phase 1 (climate);
/// `flags` bits are the `FLAG_*` constants.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Voxel {
    pub material: Material,
    pub moisture: u8,
    pub temperature: u8,
    pub flags: u8,
}

impl Voxel {
    pub const AIR: Voxel = Voxel::of(Material::Air);
    /// Voxel was excavated by an ant (as opposed to natural air).
    pub const FLAG_DUG: u8 = 1 << 0;
    pub const FLAG_PHEROMONE: u8 = 1 << 1;
    pub const FLAG_ROOT: u8 = 1 << 2;

    #[inline]
    pub const fn of(material: Material) -> Voxel {
        Voxel {
            material,
            moisture: 0,
            temperature: 0,
            flags: 0,
        }
    }
}

/// Chunk coordinate (in chunks, not voxels). `Ord` so it can key a `BTreeMap` deterministically.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ChunkCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl ChunkCoord {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    /// Chunk containing a world voxel position.
    #[inline]
    pub fn containing(p: IVec3) -> Self {
        Self {
            x: p.x.div_euclid(CHUNK_SIZE_I32),
            y: p.y.div_euclid(CHUNK_SIZE_I32),
            z: p.z.div_euclid(CHUNK_SIZE_I32),
        }
    }

    /// World voxel position of this chunk's (0,0,0) corner.
    #[inline]
    pub fn origin(self) -> IVec3 {
        IVec3::new(self.x, self.y, self.z) * CHUNK_SIZE_I32
    }
}

/// Position of a world voxel inside its chunk.
#[inline]
pub fn local_of(p: IVec3) -> UVec3 {
    p.rem_euclid(IVec3::splat(CHUNK_SIZE_I32)).as_uvec3()
}

/// Linear index inside a chunk. Layout is y-major (`x + z*32 + y*32*32`) so horizontal layers are
/// contiguous, which suits strata and the cutaway view. The wire RLE uses the same order.
#[inline]
pub const fn index(x: usize, y: usize, z: usize) -> usize {
    x + z * CHUNK_SIZE + y * CHUNK_SIZE * CHUNK_SIZE
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Chunk {
    Uniform(Voxel),
    Dense(Box<[Voxel]>),
}

impl Chunk {
    pub fn uniform(material: Material) -> Self {
        Chunk::Uniform(Voxel::of(material))
    }

    #[inline]
    pub fn get(&self, l: UVec3) -> Voxel {
        match self {
            Chunk::Uniform(v) => *v,
            Chunk::Dense(d) => d[index(l.x as usize, l.y as usize, l.z as usize)],
        }
    }

    /// Writes a voxel, promoting a uniform chunk to dense if needed. Returns whether it changed.
    pub fn set(&mut self, l: UVec3, v: Voxel) -> bool {
        if let Chunk::Uniform(u) = self {
            if *u == v {
                return false;
            }
            *self = Chunk::Dense(vec![*u; CHUNK_VOLUME].into_boxed_slice());
        }
        let Chunk::Dense(d) = self else {
            unreachable!()
        };
        let slot = &mut d[index(l.x as usize, l.y as usize, l.z as usize)];
        let changed = *slot != v;
        *slot = v;
        changed
    }

    /// Demotes a dense chunk whose voxels are all equal back to `Uniform`.
    pub fn compact(&mut self) {
        if let Chunk::Dense(d) = self {
            let first = d[0];
            if d.iter().all(|v| *v == first) {
                *self = Chunk::Uniform(first);
            }
        }
    }

    pub fn is_dense(&self) -> bool {
        matches!(self, Chunk::Dense(_))
    }

    /// Materials in index order (see [`index`]).
    pub fn materials(&self) -> impl Iterator<Item = Material> + '_ {
        (0..CHUNK_VOLUME).map(move |i| match self {
            Chunk::Uniform(v) => v.material,
            Chunk::Dense(d) => d[i].material,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_roundtrip_u8() {
        for m in Material::ALL {
            assert_eq!(Material::from_u8(m as u8), Some(m));
        }
        assert_eq!(Material::from_u8(200), None);
    }

    #[test]
    fn voxel_is_four_bytes() {
        assert_eq!(std::mem::size_of::<Voxel>(), 4);
    }

    #[test]
    fn uniform_promotes_and_compacts() {
        let mut c = Chunk::uniform(Material::Topsoil);
        assert!(!c.is_dense());
        assert!(!c.set(UVec3::new(1, 2, 3), Voxel::of(Material::Topsoil)));
        assert!(!c.is_dense(), "no-op write must not promote");
        assert!(c.set(UVec3::new(1, 2, 3), Voxel::AIR));
        assert!(c.is_dense());
        assert_eq!(c.get(UVec3::new(1, 2, 3)), Voxel::AIR);
        assert_eq!(c.get(UVec3::new(0, 0, 0)).material, Material::Topsoil);
        c.compact();
        assert!(c.is_dense());
        c.set(UVec3::new(1, 2, 3), Voxel::of(Material::Topsoil));
        c.compact();
        assert_eq!(c, Chunk::uniform(Material::Topsoil));
    }

    #[test]
    fn coords_handle_negatives() {
        assert_eq!(
            ChunkCoord::containing(IVec3::new(-1, 0, 32)),
            ChunkCoord::new(-1, 0, 1)
        );
        assert_eq!(local_of(IVec3::new(-1, 0, 33)), UVec3::new(31, 0, 1));
        assert_eq!(ChunkCoord::new(-1, 2, 0).origin(), IVec3::new(-32, 64, 0));
    }

    #[test]
    fn index_is_y_major() {
        assert_eq!(index(1, 0, 0), 1);
        assert_eq!(index(0, 0, 1), 32);
        assert_eq!(index(0, 1, 0), 1024);
        assert_eq!(index(31, 31, 31), CHUNK_VOLUME - 1);
    }
}
