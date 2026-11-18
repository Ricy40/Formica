// Instanced rendering of agents with interpolation between the last two server frames.
// Pure presentation: positions come from the server; the viewer only lerps between them.

import * as THREE from "three/webgpu";
import { mergeGeometries } from "three/addons/utils/BufferGeometryUtils.js";
import type { AgentFrame } from "../proto/messages";

/** Interpolated pose of one agent. */
export interface Pose {
  x: number;
  y: number;
  z: number;
  heading: number;
}

function antGeometry(): THREE.BufferGeometry {
  // ~1.9 voxels long (≈ 3.8 mm, a Lasius worker), oriented along +X, standing on y = 0.
  const part = (sx: number, sy: number, sz: number, x: number, y: number) => {
    const g = new THREE.SphereGeometry(1, 10, 8);
    g.scale(sx, sy, sz);
    g.translate(x, y, 0);
    return g;
  };
  const merged = mergeGeometries([
    part(0.5, 0.36, 0.38, -0.45, 0.42), // gaster
    part(0.32, 0.2, 0.18, 0.12, 0.38), // mesosoma
    part(0.24, 0.22, 0.22, 0.55, 0.42), // head
  ]);
  if (!merged) throw new Error("failed to build ant geometry");
  return merged;
}

const lerpAngle = (a: number, b: number, t: number) => {
  let d = b - a;
  if (d > Math.PI) d -= 2 * Math.PI;
  if (d < -Math.PI) d += 2 * Math.PI;
  return a + d * t;
};

export class AgentLayer {
  readonly material = new THREE.MeshLambertMaterial({ color: 0xffffff });
  mesh: THREE.InstancedMesh;
  private geometry = antGeometry();
  private prev: AgentFrame | null = null;
  private curr: AgentFrame | null = null;
  /** index in `prev` for each agent index in `curr` (-1 = new agent). */
  private prevIndex = new Int32Array(0);
  private currTime = 0;
  private interval = 1000 / 30;
  private dummy = new THREE.Object3D();
  private baseColor = new THREE.Color(0x3a2a20);
  private selectedColor = new THREE.Color(0xffb300);
  selectedId: number | null = null;

  /** `scene` is the parent object; main.ts passes a ClippingGroup so the cutaway can clip agents. */
  constructor(private scene: THREE.Object3D) {
    this.mesh = this.makeMesh(1024);
  }

  private makeMesh(capacity: number): THREE.InstancedMesh {
    const mesh = new THREE.InstancedMesh(this.geometry, this.material, capacity);
    mesh.instanceMatrix.setUsage(THREE.DynamicDrawUsage);
    mesh.count = 0;
    mesh.frustumCulled = false;
    for (let i = 0; i < capacity; i++) mesh.setColorAt(i, this.baseColor);
    this.scene.add(mesh);
    return mesh;
  }

  setFrame(frame: AgentFrame, now: number) {
    if (this.curr) {
      const dt = now - this.currTime;
      // Smooth the inter-frame interval estimate (server paces at dt / speed).
      if (dt > 0 && dt < 1000) this.interval = this.interval * 0.8 + dt * 0.2;
    }
    this.prev = this.curr;
    this.curr = frame;
    this.currTime = now;

    if (frame.count > this.mesh.instanceMatrix.count) {
      this.scene.remove(this.mesh);
      this.mesh.dispose();
      let cap = this.mesh.instanceMatrix.count;
      while (cap < frame.count) cap *= 2;
      this.mesh = this.makeMesh(cap);
    }

    // Match agents to the previous frame by id. Ids are ascending (append-only), so a merge walk
    // is linear.
    this.prevIndex = new Int32Array(frame.count).fill(-1);
    if (this.prev) {
      const a = this.prev.ids;
      const b = frame.ids;
      let i = 0;
      for (let j = 0; j < b.length; j++) {
        while (i < a.length && a[i] < b[j]) i++;
        if (i < a.length && a[i] === b[j]) this.prevIndex[j] = i;
      }
    }
    this.refreshColors();
  }

  private refreshColors() {
    if (!this.curr) return;
    for (let j = 0; j < this.curr.count; j++) {
      this.mesh.setColorAt(j, this.curr.ids[j] === this.selectedId ? this.selectedColor : this.baseColor);
    }
    if (this.mesh.instanceColor) this.mesh.instanceColor.needsUpdate = true;
  }

  select(id: number | null) {
    this.selectedId = id;
    this.refreshColors();
  }

  private alpha(now: number): number {
    return Math.min(1, Math.max(0, (now - this.currTime) / this.interval));
  }

  /** Interpolated pose for agent index `j` of the current frame. */
  private poseAt(j: number, t: number, out: Pose): Pose {
    const c = this.curr!.data;
    const k = j * 4;
    const pi = this.prevIndex[j];
    if (pi < 0 || !this.prev) {
      out.x = c[k]; out.y = c[k + 1]; out.z = c[k + 2]; out.heading = c[k + 3];
      return out;
    }
    const p = this.prev.data;
    const q = pi * 4;
    // Big jumps (respawn, wrap) are not interpolated.
    const jump = Math.abs(c[k] - p[q]) + Math.abs(c[k + 2] - p[q + 2]) > 4;
    const tt = jump ? 1 : t;
    out.x = p[q] + (c[k] - p[q]) * tt;
    out.y = p[q + 1] + (c[k + 1] - p[q + 1]) * tt;
    out.z = p[q + 2] + (c[k + 2] - p[q + 2]) * tt;
    out.heading = lerpAngle(p[q + 3], c[k + 3], tt);
    return out;
  }

  update(now: number) {
    if (!this.curr) return;
    const t = this.alpha(now);
    const pose: Pose = { x: 0, y: 0, z: 0, heading: 0 };
    for (let j = 0; j < this.curr.count; j++) {
      this.poseAt(j, t, pose);
      this.dummy.position.set(pose.x, pose.y, pose.z);
      // Heading is measured from +X towards +Z; three's +Y rotation turns +X towards -Z.
      this.dummy.rotation.set(0, -pose.heading, 0);
      this.dummy.updateMatrix();
      this.mesh.setMatrixAt(j, this.dummy.matrix);
    }
    this.mesh.count = this.curr.count;
    this.mesh.instanceMatrix.needsUpdate = true;
  }

  /** Current interpolated pose of an agent id, if present. */
  poseOf(id: number, now: number): Pose | null {
    if (!this.curr) return null;
    const j = this.curr.ids.indexOf(id);
    if (j < 0) return null;
    return this.poseAt(j, this.alpha(now), { x: 0, y: 0, z: 0, heading: 0 });
  }

  idAtInstance(instanceId: number): number | null {
    if (!this.curr || instanceId >= this.curr.count) return null;
    return this.curr.ids[instanceId];
  }

  randomId(): number | null {
    if (!this.curr || this.curr.count === 0) return null;
    return this.curr.ids[Math.floor(Math.random() * this.curr.count)];
  }

  get count(): number {
    return this.curr?.count ?? 0;
  }
}
