//! The voxel world: a sparse map of chunks plus derived 2D surface layers.

use std::collections::BTreeMap;

use glam::IVec3;
use serde::{Deserialize, Serialize};

use crate::voxel::{CHUNK_SIZE_I32, Chunk, ChunkCoord, Material, Voxel, local_of};

/// A single changed voxel, recorded so the server can stream edits as deltas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoxelEdit {
    pub pos: IVec3,
    pub voxel: Voxel,
}

/// Axis-aligned voxel-space box `[min, min + size)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extent {
    pub min: IVec3,
    pub size: IVec3,
}

impl Extent {
    #[inline]
    pub fn max(&self) -> IVec3 {
        self.min + self.size
    }

    #[inline]
    pub fn contains(&self, p: IVec3) -> bool {
        p.cmpge(self.min).all() && p.cmplt(self.max()).all()
    }
}

/// Derived per-column data: where the walkable surface is. Rebuilt from voxels, never saved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SurfaceLayer {
    /// Y of the first air voxel above the top solid voxel (i.e. the standing height).
    height: Vec<i32>,
    /// Material at the top of the column (solid or water).
    top: Vec<Material>,
}

/// The world. Authoritative state is `extent` + `chunks`; everything else is derived or transient.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct World {
    extent: Extent,
    /// Chunks that are stored. A missing chunk inside the extent is all air.
    chunks: BTreeMap<ChunkCoord, Chunk>,
    #[serde(skip)]
    surface: SurfaceLayer,
    /// Edits since the last [`World::drain_edits`]. Transient, not part of the state hash.
    #[serde(skip)]
    edits: Vec<VoxelEdit>,
}

impl PartialEq for World {
    fn eq(&self, other: &Self) -> bool {
        self.extent == other.extent && self.chunks == other.chunks
    }
}

impl World {
    /// An empty (all-air) world. `extent` must be chunk-aligned.
    pub fn new(extent: Extent) -> Self {
        assert!(
            extent.min % CHUNK_SIZE_I32 == IVec3::ZERO
                && extent.size % CHUNK_SIZE_I32 == IVec3::ZERO,
            "world extent must be chunk-aligned"
        );
        let mut w = World {
            extent,
            chunks: BTreeMap::new(),
            surface: Default::default(),
            edits: vec![],
        };
        w.rebuild_surface();
        w
    }

    pub fn extent(&self) -> Extent {
        self.extent
    }

    pub fn chunks(&self) -> impl Iterator<Item = (&ChunkCoord, &Chunk)> {
        self.chunks.iter()
    }

    pub fn chunk(&self, c: ChunkCoord) -> Option<&Chunk> {
        self.chunks.get(&c)
    }

    /// Inserts a whole chunk (used by scene construction / world generation).
    pub fn insert_chunk(&mut self, c: ChunkCoord, chunk: Chunk) {
        self.chunks.insert(c, chunk);
    }

    #[inline]
    pub fn get(&self, p: IVec3) -> Voxel {
        if !self.extent.contains(p) {
            return Voxel::AIR;
        }
        match self.chunks.get(&ChunkCoord::containing(p)) {
            Some(c) => c.get(local_of(p)),
            None => Voxel::AIR,
        }
    }

    /// Sets a voxel inside the extent; records an edit and refreshes the surface column.
    /// Returns whether anything changed.
    pub fn set(&mut self, p: IVec3, v: Voxel) -> bool {
        if !self.extent.contains(p) {
            return false;
        }
        let coord = ChunkCoord::containing(p);
        let chunk = self
            .chunks
            .entry(coord)
            .or_insert_with(|| Chunk::Uniform(Voxel::AIR));
        let changed = chunk.set(local_of(p), v);
        if changed {
            chunk.compact();
            if matches!(chunk, Chunk::Uniform(u) if *u == Voxel::AIR) {
                self.chunks.remove(&coord);
            }
            self.edits.push(VoxelEdit { pos: p, voxel: v });
            self.rebuild_surface_column(p.x, p.z);
        }
        changed
    }

    /// Takes the edits recorded since the last call.
    pub fn drain_edits(&mut self) -> Vec<VoxelEdit> {
        std::mem::take(&mut self.edits)
    }

    fn column_index(&self, x: i32, z: i32) -> Option<usize> {
        let lx = x - self.extent.min.x;
        let lz = z - self.extent.min.z;
        if lx < 0 || lz < 0 || lx >= self.extent.size.x || lz >= self.extent.size.z {
            return None;
        }
        Some((lx + lz * self.extent.size.x) as usize)
    }

    /// Recomputes all derived surface data. Call after bulk edits or after loading.
    pub fn rebuild_surface(&mut self) {
        let n = (self.extent.size.x * self.extent.size.z) as usize;
        self.surface.height = vec![self.extent.min.y; n];
        self.surface.top = vec![Material::Air; n];
        for z in self.extent.min.z..self.extent.max().z {
            for x in self.extent.min.x..self.extent.max().x {
                self.rebuild_surface_column(x, z);
            }
        }
    }

    fn rebuild_surface_column(&mut self, x: i32, z: i32) {
        let Some(i) = self.column_index(x, z) else {
            return;
        };
        let (mut height, mut top) = (self.extent.min.y, Material::Air);
        for y in (self.extent.min.y..self.extent.max().y).rev() {
            let m = self.get(IVec3::new(x, y, z)).material;
            if m != Material::Air {
                height = y + 1;
                top = m;
                break;
            }
        }
        self.surface.height[i] = height;
        self.surface.top[i] = top;
    }

    /// Standing height and top material of the column at integer `(x, z)`.
    #[inline]
    pub fn surface_at(&self, x: i32, z: i32) -> Option<(i32, Material)> {
        let i = self.column_index(x, z)?;
        Some((self.surface.height[i], self.surface.top[i]))
    }

    /// Number of chunks stored dense vs uniform (for stats and tests).
    pub fn chunk_counts(&self) -> (usize, usize) {
        let dense = self.chunks.values().filter(|c| c.is_dense()).count();
        (dense, self.chunks.len() - dense)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one_chunk_world() -> World {
        World::new(Extent {
            min: IVec3::ZERO,
            size: IVec3::splat(32),
        })
    }

    #[test]
    fn empty_world_is_air() {
        let w = one_chunk_world();
        assert_eq!(w.get(IVec3::new(3, 3, 3)), Voxel::AIR);
        assert_eq!(w.surface_at(3, 3), Some((0, Material::Air)));
        assert_eq!(w.surface_at(40, 3), None);
    }

    #[test]
    fn set_updates_surface_and_records_edit() {
        let mut w = one_chunk_world();
        assert!(w.set(IVec3::new(2, 5, 2), Voxel::of(Material::Rock)));
        assert_eq!(w.surface_at(2, 2), Some((6, Material::Rock)));
        assert_eq!(w.drain_edits().len(), 1);
        assert!(w.drain_edits().is_empty());
        assert!(
            !w.set(IVec3::new(2, 5, 2), Voxel::of(Material::Rock)),
            "no-op"
        );
        assert!(
            !w.set(IVec3::new(99, 5, 2), Voxel::of(Material::Rock)),
            "outside extent"
        );
    }

    #[test]
    fn clearing_last_voxel_drops_chunk() {
        let mut w = one_chunk_world();
        w.set(IVec3::new(1, 1, 1), Voxel::of(Material::Clay));
        assert_eq!(w.chunk_counts(), (1, 0));
        w.set(IVec3::new(1, 1, 1), Voxel::AIR);
        assert_eq!(w.chunk_counts(), (0, 0));
        assert_eq!(w.surface_at(1, 1), Some((0, Material::Air)));
    }

    #[test]
    #[should_panic(expected = "chunk-aligned")]
    fn rejects_unaligned_extent() {
        World::new(Extent {
            min: IVec3::ZERO,
            size: IVec3::new(33, 32, 32),
        });
    }
}
