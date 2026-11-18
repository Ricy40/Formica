// Decodes the Rust-generated golden fixtures (crates/sim-proto/fixtures) and re-encodes client
// messages byte-for-byte. Expected values mirror `sim_proto::fixtures::all()`.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  CHUNK_VOLUME,
  Material,
  PROTOCOL_VERSION,
  decodeServerMsg,
  encodeClientMsg,
  expandChunk,
  type ClientMsg,
} from "./messages";
import { Reader, Writer } from "./postcard";

const fixture = (name: string) =>
  new Uint8Array(
    readFileSync(fileURLToPath(new URL(`../../../crates/sim-proto/fixtures/${name}.bin`, import.meta.url))),
  );

describe("postcard primitives", () => {
  it("round-trips varints and zigzag", () => {
    for (const v of [0, 1, 127, 128, 300, 16384, 2 ** 31, 2 ** 32 - 1]) {
      expect(new Reader(new Writer().u32(v).finish()).u32()).toBe(v);
    }
    for (const v of [0, -1, 1, -64, 64, 2 ** 31 - 1, -(2 ** 31)]) {
      expect(new Reader(new Writer().i32(v).finish()).i32()).toBe(v);
    }
  });
});

describe("server fixtures", () => {
  it("hello", () => {
    const m = decodeServerMsg(fixture("server_hello"));
    expect(m).toEqual({
      type: "hello",
      hello: {
        protocolVersion: PROTOCOL_VERSION,
        seed: 42n,
        tick: 300,
        dt: Math.fround(1 / 30),
        worldMin: [0, 0, 0],
        worldSize: [32, 32, 32],
      },
    });
  });

  it("uniform chunk", () => {
    const m = decodeServerMsg(fixture("server_chunk_uniform"));
    if (m.type !== "chunk") throw new Error(m.type);
    expect(m.chunk.coord).toEqual([-1, 0, 2]);
    const mats = expandChunk(m.chunk.data);
    expect(mats.length).toBe(CHUNK_VOLUME);
    expect(mats.every((x) => x === Material.Clay)).toBe(true);
  });

  it("rle chunk", () => {
    const m = decodeServerMsg(fixture("server_chunk_rle"));
    if (m.type !== "chunk") throw new Error(m.type);
    const mats = expandChunk(m.chunk.data);
    expect(mats[0]).toBe(Material.Rock);
    expect(mats[1]).toBe(Material.Topsoil);
    expect(mats[CHUNK_VOLUME - 2]).toBe(Material.Topsoil);
    expect(mats[CHUNK_VOLUME - 1]).toBe(Material.Air);
  });

  it("agent frame", () => {
    const m = decodeServerMsg(fixture("server_agent_frame"));
    if (m.type !== "agentFrame") throw new Error(m.type);
    expect(m.frame.tick).toBe(7);
    expect(m.frame.count).toBe(2);
    expect(Array.from(m.frame.ids)).toEqual([0, 1]);
    expect(Array.from(m.frame.data)).toEqual([1.5, 20, -3.25, 0.5, 31, 0, 16, 6]);
  });

  it("voxel edits", () => {
    const m = decodeServerMsg(fixture("server_voxel_edits"));
    expect(m).toEqual({
      type: "voxelEdits",
      edits: { tick: 9, edits: [{ pos: [3, -4, 200], material: Material.Air }] },
    });
  });

  it("stats", () => {
    const m = decodeServerMsg(fixture("server_stats"));
    expect(m).toEqual({
      type: "stats",
      stats: {
        tick: 123456,
        tps: 30,
        stepMs: 0.25,
        agents: 300,
        stateHash: 0xdeadbeefcafef00dn,
        paused: false,
        speed: 1,
      },
    });
  });

  it("notice", () => {
    expect(decodeServerMsg(fixture("server_notice"))).toEqual({ type: "notice", text: "saved" });
  });

  it("rejects trailing bytes and unknown variants", () => {
    const good = fixture("server_notice");
    const longer = new Uint8Array(good.length + 1);
    longer.set(good);
    expect(() => decodeServerMsg(longer)).toThrow(/trailing/);
    expect(() => decodeServerMsg(Uint8Array.of(99))).toThrow(/unknown/);
  });
});

describe("client fixtures encode byte-identically", () => {
  const cases: [string, ClientMsg][] = [
    ["client_hello", { type: "hello", protocolVersion: PROTOCOL_VERSION }],
    ["client_spawn", { type: "command", command: { type: "spawnAgents", count: 100 } }],
    [
      "client_set_voxel",
      { type: "command", command: { type: "setVoxel", pos: [10, -1, 300], material: Material.Rock } },
    ],
    ["client_speed", { type: "control", control: { type: "setSpeed", speed: 2.5 } }],
    ["client_save", { type: "control", control: { type: "saveSnapshot" } }],
  ];
  for (const [name, msg] of cases) {
    it(name, () => {
      expect(Array.from(encodeClientMsg(msg))).toEqual(Array.from(fixture(name)));
    });
  }
});
