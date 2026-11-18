// Hand-written mirror of crates/sim-proto/src/lib.rs. Variant order = wire discriminant.
// On any change: bump PROTOCOL_VERSION in both places and re-bless the fixtures.

import { ProtoError, Reader, Writer } from "./postcard";

export const PROTOCOL_VERSION = 1;
export const AGENT_STRIDE = 16; // x, y, z, heading: f32 LE
export const CHUNK_SIZE = 32;
export const CHUNK_VOLUME = CHUNK_SIZE ** 3;

/** sim_core::Material discriminants. */
export enum Material {
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
export const MATERIAL_COUNT = 11;

export type Vec3i = [number, number, number];

export interface Hello {
  protocolVersion: number;
  seed: bigint;
  tick: number;
  dt: number;
  worldMin: Vec3i;
  worldSize: Vec3i;
}

export type ChunkData =
  | { kind: "uniform"; material: number }
  | { kind: "rle"; runs: { len: number; material: number }[] };

export interface ChunkMsg {
  coord: Vec3i;
  data: ChunkData;
}

export interface AgentFrame {
  tick: number;
  count: number;
  /** Agent ids (copied into an aligned buffer). */
  ids: Uint32Array;
  /** x, y, z, heading per agent. */
  data: Float32Array;
}

export interface VoxelEdits {
  tick: number;
  edits: { pos: Vec3i; material: number }[];
}

export interface Stats {
  tick: number;
  tps: number;
  stepMs: number;
  agents: number;
  stateHash: bigint;
  paused: boolean;
  speed: number;
}

export type ServerMsg =
  | { type: "hello"; hello: Hello }
  | { type: "chunk"; chunk: ChunkMsg }
  | { type: "agentFrame"; frame: AgentFrame }
  | { type: "voxelEdits"; edits: VoxelEdits }
  | { type: "stats"; stats: Stats }
  | { type: "notice"; text: string };

/** Copies a byte view into a fresh, 4-byte-aligned buffer so typed arrays can map it. */
function aligned(bytes: Uint8Array): ArrayBuffer {
  const out = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(out).set(bytes);
  return out;
}

export function decodeServerMsg(bytes: Uint8Array): ServerMsg {
  const r = new Reader(bytes);
  const variant = r.u32();
  let msg: ServerMsg;
  switch (variant) {
    case 0:
      msg = {
        type: "hello",
        hello: {
          protocolVersion: r.u32(),
          seed: r.u64(),
          tick: r.u64n(),
          dt: r.f32(),
          worldMin: r.i32x3(),
          worldSize: r.i32x3(),
        },
      };
      break;
    case 1: {
      const coord = r.i32x3();
      const kind = r.u32();
      let data: ChunkData;
      if (kind === 0) data = { kind: "uniform", material: r.u8() };
      else if (kind === 1) data = { kind: "rle", runs: r.vec((rr) => ({ len: rr.u16(), material: rr.u8() })) };
      else throw new ProtoError(`unknown ChunkData variant ${kind}`);
      msg = { type: "chunk", chunk: { coord, data } };
      break;
    }
    case 2: {
      const tick = r.u64n();
      const ids = r.bytesView();
      const data = r.bytesView();
      if (ids.length % 4 !== 0 || data.length !== (ids.length / 4) * AGENT_STRIDE) {
        throw new ProtoError("agent frame arrays have inconsistent lengths");
      }
      msg = {
        type: "agentFrame",
        frame: {
          tick,
          count: ids.length / 4,
          ids: new Uint32Array(aligned(ids)),
          data: new Float32Array(aligned(data)),
        },
      };
      break;
    }
    case 3:
      msg = {
        type: "voxelEdits",
        edits: { tick: r.u64n(), edits: r.vec((rr) => ({ pos: rr.i32x3(), material: rr.u8() })) },
      };
      break;
    case 4:
      msg = {
        type: "stats",
        stats: {
          tick: r.u64n(),
          tps: r.f32(),
          stepMs: r.f32(),
          agents: r.u32(),
          stateHash: r.u64(),
          paused: r.bool(),
          speed: r.f32(),
        },
      };
      break;
    case 5:
      msg = { type: "notice", text: r.string() };
      break;
    default:
      throw new ProtoError(`unknown ServerMsg variant ${variant}`);
  }
  if (r.remaining !== 0) throw new ProtoError(`${r.remaining} trailing bytes after ${msg.type}`);
  return msg;
}

/** Expands chunk data to one material per voxel, in y-major order (x + z*32 + y*1024). */
export function expandChunk(data: ChunkData): Uint8Array {
  const out = new Uint8Array(CHUNK_VOLUME);
  if (data.kind === "uniform") {
    out.fill(data.material);
    return out;
  }
  let i = 0;
  for (const run of data.runs) {
    if (i + run.len > CHUNK_VOLUME) throw new ProtoError("chunk RLE overruns chunk");
    out.fill(run.material, i, i + run.len);
    i += run.len;
  }
  if (i !== CHUNK_VOLUME) throw new ProtoError(`chunk RLE covers ${i} voxels`);
  return out;
}

export type SimCommand =
  | { type: "spawnAgents"; count: number }
  | { type: "clearAgents" }
  | { type: "setVoxel"; pos: Vec3i; material: Material };

export type Control =
  | { type: "pause" }
  | { type: "resume" }
  | { type: "setSpeed"; speed: number }
  | { type: "saveSnapshot" };

export type ClientMsg =
  | { type: "hello"; protocolVersion: number }
  | { type: "command"; command: SimCommand }
  | { type: "control"; control: Control };

export function encodeClientMsg(msg: ClientMsg): Uint8Array {
  const w = new Writer();
  switch (msg.type) {
    case "hello":
      w.u32(0).u32(msg.protocolVersion);
      break;
    case "command": {
      w.u32(1);
      const c = msg.command;
      if (c.type === "spawnAgents") w.u32(0).u32(c.count);
      else if (c.type === "clearAgents") w.u32(1);
      else w.u32(2).i32(c.pos[0]).i32(c.pos[1]).i32(c.pos[2]).u32(c.material);
      break;
    }
    case "control": {
      w.u32(2);
      const c = msg.control;
      if (c.type === "pause") w.u32(0);
      else if (c.type === "resume") w.u32(1);
      else if (c.type === "setSpeed") w.u32(2).f32(c.speed);
      else w.u32(3);
      break;
    }
  }
  return w.finish();
}
