// Face-culled voxel chunk mesher with per-vertex ambient occlusion. Pure function of the voxel
// data (rendering only; greedy meshing replaces this in Phase 1).
//
// The cutaway view meshes the chunk as if every voxel on the camera side of the cut were air, so
// the cut surface gets real, correctly coloured faces (a GPU clipping plane alone would leave the
// solid ground hollow, because interior faces are culled).

import { CHUNK_SIZE, Material, type Vec3i } from "../proto/messages";

export interface Cut {
  axis: "x" | "z";
  /** Voxels with coordinate >= depth along `axis` are removed. */
  depth: number;
}

export interface MeshData {
  positions: Float32Array;
  normals: Float32Array;
  colors: Float32Array;
  indices: Uint32Array;
}

const S = CHUNK_SIZE;

// Base colours (linear-ish sRGB, 0..1). Chosen to read as soil strata in the cutaway.
const MATERIAL_COLORS: Record<number, [number, number, number]> = {
  [Material.Water]: [0.24, 0.5, 0.72],
  [Material.Topsoil]: [0.36, 0.26, 0.19],
  [Material.Subsoil]: [0.5, 0.37, 0.25],
  [Material.Clay]: [0.63, 0.4, 0.29],
  [Material.Sand]: [0.82, 0.73, 0.54],
  [Material.Loam]: [0.43, 0.31, 0.21],
  [Material.Gravel]: [0.54, 0.52, 0.5],
  [Material.Rock]: [0.55, 0.56, 0.57],
  [Material.Wood]: [0.48, 0.33, 0.2],
  [Material.LeafLitter]: [0.54, 0.42, 0.17],
};

export function materialColor(m: number): [number, number, number] {
  return MATERIAL_COLORS[m] ?? [1, 0, 1];
}

// Face definitions: normal, and the 4 corners (unit cube offsets) in CCW order seen from outside.
const FACES: { n: Vec3i; corners: Vec3i[] }[] = [
  { n: [1, 0, 0], corners: [[1, 0, 0], [1, 1, 0], [1, 1, 1], [1, 0, 1]] },
  { n: [-1, 0, 0], corners: [[0, 0, 1], [0, 1, 1], [0, 1, 0], [0, 0, 0]] },
  { n: [0, 1, 0], corners: [[0, 1, 1], [1, 1, 1], [1, 1, 0], [0, 1, 0]] },
  { n: [0, -1, 0], corners: [[0, 0, 0], [1, 0, 0], [1, 0, 1], [0, 0, 1]] },
  { n: [0, 0, 1], corners: [[1, 0, 1], [1, 1, 1], [0, 1, 1], [0, 0, 1]] },
  { n: [0, 0, -1], corners: [[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]] },
];

const AO_LEVELS = [0.5, 0.68, 0.84, 1.0];

/** Cheap deterministic per-voxel brightness jitter so flat areas show texture. */
function jitter(x: number, y: number, z: number): number {
  let h = (x * 73856093) ^ (y * 19349663) ^ (z * 83492791);
  h = Math.imul(h ^ (h >>> 13), 0x5bd1e995);
  return 0.93 + ((h >>> 0) % 1000) / 1000 * 0.12;
}

class Builder {
  positions: number[] = [];
  normals: number[] = [];
  colors: number[] = [];
  indices: number[] = [];

  finish(): MeshData {
    return {
      positions: new Float32Array(this.positions),
      normals: new Float32Array(this.normals),
      colors: new Float32Array(this.colors),
      indices: new Uint32Array(this.indices),
    };
  }
}

export function meshChunk(mats: Uint8Array, origin: Vec3i, cut?: Cut): { solid: MeshData; water: MeshData } {
  const removed = (x: number, y: number, z: number) => {
    if (!cut) return false;
    const w = cut.axis === "x" ? origin[0] + x : origin[2] + z;
    void y;
    return w >= cut.depth;
  };
  const at = (x: number, y: number, z: number): number => {
    if (x < 0 || y < 0 || z < 0 || x >= S || y >= S || z >= S) return Material.Air;
    if (removed(x, y, z)) return Material.Air;
    return mats[x + z * S + y * S * S];
  };
  const isSolid = (m: number) => m !== Material.Air && m !== Material.Water;
  const occludes = (x: number, y: number, z: number) => (isSolid(at(x, y, z)) ? 1 : 0);

  const solid = new Builder();
  const water = new Builder();

  for (let y = 0; y < S; y++) {
    for (let z = 0; z < S; z++) {
      for (let x = 0; x < S; x++) {
        const m = at(x, y, z);
        if (m === Material.Air) continue;
        const isWater = m === Material.Water;
        const b = isWater ? water : solid;
        const [cr, cg, cb] = materialColor(m);
        const j = jitter(origin[0] + x, origin[1] + y, origin[2] + z);

        for (const face of FACES) {
          const [nx, ny, nz] = face.n;
          const nm = at(x + nx, y + ny, z + nz);
          const visible = isWater ? nm === Material.Air : !isSolid(nm);
          if (!visible) continue;

          const base = b.positions.length / 3;
          const ao: number[] = [];
          for (const [cx, cy, cz] of face.corners) {
            b.positions.push(origin[0] + x + cx, origin[1] + y + cy, origin[2] + z + cz);
            b.normals.push(nx, ny, nz);
            // AO: sample the 3 voxels touching this corner in the layer in front of the face.
            const dx = cx === 0 ? -1 : 1;
            const dy = cy === 0 ? -1 : 1;
            const dz = cz === 0 ? -1 : 1;
            const px = x + nx, py = y + ny, pz = z + nz;
            let s1: number, s2: number, c: number;
            if (nx !== 0) {
              s1 = occludes(px, py + dy, pz); s2 = occludes(px, py, pz + dz); c = occludes(px, py + dy, pz + dz);
            } else if (ny !== 0) {
              s1 = occludes(px + dx, py, pz); s2 = occludes(px, py, pz + dz); c = occludes(px + dx, py, pz + dz);
            } else {
              s1 = occludes(px + dx, py, pz); s2 = occludes(px, py + dy, pz); c = occludes(px + dx, py + dy, pz);
            }
            const level = isWater ? 3 : s1 && s2 ? 0 : 3 - (s1 + s2 + c);
            ao.push(level);
            const k = AO_LEVELS[level] * j;
            b.colors.push(cr * k, cg * k, cb * k);
          }
          // Flip the quad's diagonal to avoid AO interpolation artefacts.
          if (ao[0] + ao[2] > ao[1] + ao[3]) {
            b.indices.push(base, base + 1, base + 2, base, base + 2, base + 3);
          } else {
            b.indices.push(base + 1, base + 2, base + 3, base + 1, base + 3, base);
          }
        }
      }
    }
  }
  return { solid: solid.finish(), water: water.finish() };
}
