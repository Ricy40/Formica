import { describe, expect, it } from "vitest";
import { CHUNK_SIZE, CHUNK_VOLUME, Material } from "../proto/messages";
import { meshChunk, type MeshData } from "./chunkMesh";

const S = CHUNK_SIZE;
const idx = (x: number, y: number, z: number) => x + z * S + y * S * S;

function triangleNormalsMatch(m: MeshData): boolean {
  for (let t = 0; t < m.indices.length; t += 3) {
    const [a, b, c] = [m.indices[t], m.indices[t + 1], m.indices[t + 2]];
    const p = (i: number) => [m.positions[i * 3], m.positions[i * 3 + 1], m.positions[i * 3 + 2]];
    const [pa, pb, pc] = [p(a), p(b), p(c)];
    const u = pb.map((v, i) => v - pa[i]);
    const v = pc.map((w, i) => w - pa[i]);
    const n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    const want = [m.normals[a * 3], m.normals[a * 3 + 1], m.normals[a * 3 + 2]];
    if (n[0] * want[0] + n[1] * want[1] + n[2] * want[2] <= 0) return false;
  }
  return true;
}

describe("meshChunk", () => {
  it("meshes a lone voxel as 6 outward faces", () => {
    const mats = new Uint8Array(CHUNK_VOLUME);
    mats[idx(5, 5, 5)] = Material.Rock;
    const { solid, water } = meshChunk(mats, [0, 0, 0]);
    expect(solid.positions.length / 3).toBe(24);
    expect(solid.indices.length).toBe(36);
    expect(water.indices.length).toBe(0);
    expect(triangleNormalsMatch(solid)).toBe(true);
  });

  it("culls faces shared by solid neighbours", () => {
    const mats = new Uint8Array(CHUNK_VOLUME);
    mats[idx(5, 5, 5)] = Material.Rock;
    mats[idx(6, 5, 5)] = Material.Clay;
    expect(meshChunk(mats, [0, 0, 0]).solid.indices.length).toBe(10 * 6);
  });

  it("shows solid faces under water but culls water-water faces", () => {
    const mats = new Uint8Array(CHUNK_VOLUME);
    mats[idx(5, 4, 5)] = Material.Sand;
    mats[idx(5, 5, 5)] = Material.Water;
    mats[idx(6, 5, 5)] = Material.Water;
    const { solid, water } = meshChunk(mats, [0, 0, 0]);
    expect(solid.indices.length).toBe(36);
    expect(water.indices.length).toBe((6 + 6 - 2 - 1) * 6); // shared face and the face on sand removed
    expect(triangleNormalsMatch(water)).toBe(true);
  });

  it("a cut removes voxels and caps the cut surface", () => {
    const mats = new Uint8Array(CHUNK_VOLUME).fill(Material.Subsoil);
    const whole = meshChunk(mats, [0, 0, 0]).solid;
    expect(whole.indices.length).toBe(6 * S * S * 6);
    const cut = meshChunk(mats, [0, 0, 0], { axis: "z", depth: 10 }).solid;
    // Box of 32 x 32 x 10: two 32x32 faces + four 32x10 faces.
    expect(cut.indices.length).toBe((2 * S * S + 4 * S * 10) * 6);
    let maxZ = 0;
    for (let i = 2; i < cut.positions.length; i += 3) maxZ = Math.max(maxZ, cut.positions[i]);
    expect(maxZ).toBe(10);
    expect(triangleNormalsMatch(cut)).toBe(true);
  });
});
