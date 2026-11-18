// Formica viewer: renders what sim-server streams. No simulation logic lives here.

import * as THREE from "three/webgpu";
import { Connection, defaultServerUrl, type ConnState } from "./net";
import { CHUNK_SIZE, Material, expandChunk, type ServerMsg, type Stats, type Vec3i } from "./proto/messages";
import { AgentLayer } from "./render/agents";
import { CameraRig, MODE_HELP, type CameraMode, type CutAxis } from "./render/cameras";
import { meshChunk, type Cut, type MeshData } from "./render/chunkMesh";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

// ---------- renderer & scene ----------
const container = $("app");
const renderer = new THREE.WebGPURenderer({ antialias: true });
await renderer.init();
const backendName = (renderer.backend as { isWebGPUBackend?: boolean }).isWebGPUBackend ? "WebGPU" : "WebGL2";
renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2));
container.appendChild(renderer.domElement);

const scene = new THREE.Scene();
scene.background = new THREE.Color(0xcfd8dc);
scene.add(new THREE.HemisphereLight(0xe6eef5, 0x5a4a3a, 1.7));
const sun = new THREE.DirectionalLight(0xfff1dc, 1.9);
sun.position.set(40, 80, 25);
scene.add(sun);

const solidMaterial = new THREE.MeshLambertMaterial({ vertexColors: true });
const waterMaterial = new THREE.MeshLambertMaterial({
  vertexColors: true,
  transparent: true,
  opacity: 0.6,
  depthWrite: false,
});

const center = new THREE.Vector3(16, 18, 16);
const rig = new CameraRig(renderer.domElement, center);
const agents = new AgentLayer(scene);

function resize() {
  const w = container.clientWidth;
  const h = container.clientHeight;
  renderer.setSize(w, h, false);
  rig.resize(w, h);
}
window.addEventListener("resize", resize);
resize();
rig.setMode("iso");

// ---------- world (render-side copy of chunk materials) ----------
interface ChunkView {
  coord: Vec3i;
  mats: Uint8Array;
  solid: THREE.Mesh;
  water: THREE.Mesh;
}
const chunks = new Map<string, ChunkView>();
const key = (c: Vec3i) => c.join(",");
let worldMin: Vec3i = [0, 0, 0];
let worldSize: Vec3i = [32, 32, 32];

function geometryFrom(m: MeshData): THREE.BufferGeometry {
  const g = new THREE.BufferGeometry();
  g.setAttribute("position", new THREE.BufferAttribute(m.positions, 3));
  g.setAttribute("normal", new THREE.BufferAttribute(m.normals, 3));
  g.setAttribute("color", new THREE.BufferAttribute(m.colors, 3));
  g.setIndex(new THREE.BufferAttribute(m.indices, 1));
  g.computeBoundingSphere();
  return g;
}

function currentCut(): Cut | undefined {
  if (rig.mode !== "cutaway") return undefined;
  return { axis: cutAxis, depth: cutDepth };
}

function remesh(view: ChunkView) {
  const origin: Vec3i = [view.coord[0] * CHUNK_SIZE, view.coord[1] * CHUNK_SIZE, view.coord[2] * CHUNK_SIZE];
  const { solid, water } = meshChunk(view.mats, origin, currentCut());
  view.solid.geometry.dispose();
  view.water.geometry.dispose();
  view.solid.geometry = geometryFrom(solid);
  view.water.geometry = geometryFrom(water);
}

function remeshAll() {
  for (const v of chunks.values()) remesh(v);
}

function upsertChunk(coord: Vec3i, mats: Uint8Array) {
  let view = chunks.get(key(coord));
  if (!view) {
    view = {
      coord,
      mats,
      solid: new THREE.Mesh(new THREE.BufferGeometry(), solidMaterial),
      water: new THREE.Mesh(new THREE.BufferGeometry(), waterMaterial),
    };
    view.water.renderOrder = 1;
    scene.add(view.solid, view.water);
    chunks.set(key(coord), view);
  }
  view.mats = mats;
  remesh(view);
}

function clearWorld() {
  for (const v of chunks.values()) {
    scene.remove(v.solid, v.water);
    v.solid.geometry.dispose();
    v.water.geometry.dispose();
  }
  chunks.clear();
}

// ---------- cutaway ----------
let cutAxis: CutAxis = "z";
let cutDepth = 14;
const clipPlane = new THREE.Plane();

function applyCut() {
  if (rig.mode === "cutaway") {
    // Keep coordinate < depth along the axis (the camera looks from the +axis side).
    clipPlane.set(cutAxis === "z" ? new THREE.Vector3(0, 0, -1) : new THREE.Vector3(-1, 0, 0), cutDepth);
    agents.material.clippingPlanes = [clipPlane];
  } else {
    agents.material.clippingPlanes = [];
  }
  agents.material.needsUpdate = true;
  remeshAll();
}

// ---------- networking ----------
let lastStats: Stats | null = null;
let editsReceived = 0;
let haveHello = false;
const connEl = $("conn");

function onConnState(s: ConnState) {
  connEl.textContent = s;
  connEl.className = "pill " + (s === "open" ? "ok" : s === "closed" ? "bad" : "");
}

function onMessage(msg: ServerMsg) {
  switch (msg.type) {
    case "hello": {
      clearWorld();
      worldMin = msg.hello.worldMin;
      worldSize = msg.hello.worldSize;
      const depth = $<HTMLInputElement>("cut-depth");
      depth.min = String(worldMin[2]);
      depth.max = String(worldMin[2] + worldSize[2]);
      if (!haveHello) {
        center.set(worldMin[0] + worldSize[0] / 2, worldMin[1] + worldSize[1] * 0.45, worldMin[2] + worldSize[2] / 2);
        rig.fitWorld(Math.max(...worldSize));
        rig.resetTarget();
        haveHello = true;
      }
      break;
    }
    case "chunk":
      upsertChunk(msg.chunk.coord, expandChunk(msg.chunk.data));
      break;
    case "agentFrame":
      agents.setFrame(msg.frame, performance.now());
      break;
    case "voxelEdits": {
      const dirty = new Set<ChunkView>();
      for (const e of msg.edits.edits) {
        const c: Vec3i = [
          Math.floor(e.pos[0] / CHUNK_SIZE),
          Math.floor(e.pos[1] / CHUNK_SIZE),
          Math.floor(e.pos[2] / CHUNK_SIZE),
        ];
        let view = chunks.get(key(c));
        if (!view) {
          upsertChunk(c, new Uint8Array(CHUNK_SIZE ** 3));
          view = chunks.get(key(c))!;
        }
        const l = e.pos.map((p) => ((p % CHUNK_SIZE) + CHUNK_SIZE) % CHUNK_SIZE);
        view.mats[l[0] + l[2] * CHUNK_SIZE + l[1] * CHUNK_SIZE * CHUNK_SIZE] = e.material;
        dirty.add(view);
      }
      for (const v of dirty) remesh(v);
      editsReceived += msg.edits.edits.length;
      break;
    }
    case "stats":
      lastStats = msg.stats;
      $("pause").textContent = msg.stats.paused ? "Resume" : "Pause";
      break;
    case "notice":
      $("notice").textContent = msg.text;
      break;
  }
}

const conn = new Connection(defaultServerUrl(), onMessage, onConnState);

// ---------- UI ----------
const modeButtons = document.querySelectorAll<HTMLButtonElement>("[data-mode]");
function setMode(mode: CameraMode) {
  rig.setMode(mode);
  modeButtons.forEach((b) => b.classList.toggle("active", b.dataset.mode === mode));
  $("cutaway-controls").hidden = mode !== "cutaway";
  $("mode-help").textContent = MODE_HELP[mode];
  applyCut();
}
modeButtons.forEach((b) => b.addEventListener("click", () => setMode(b.dataset.mode as CameraMode)));
window.addEventListener("keydown", (e) => {
  if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
  const modes: Record<string, CameraMode> = { "1": "iso", "2": "top", "3": "cutaway", "4": "free" };
  if (modes[e.key]) setMode(modes[e.key]);
  if (e.key === "Escape") selectAgent(null);
});
$("mode-help").textContent = MODE_HELP.iso;

$<HTMLSelectElement>("cut-axis").addEventListener("change", (e) => {
  cutAxis = (e.target as HTMLSelectElement).value as CutAxis;
  rig.setCutAxis(cutAxis);
  applyCut();
});
$<HTMLInputElement>("cut-depth").addEventListener("input", (e) => {
  cutDepth = Number((e.target as HTMLInputElement).value);
  $("cut-depth-out").textContent = String(cutDepth);
  applyCut();
});

$("pause").addEventListener("click", () =>
  conn.send({ type: "control", control: { type: lastStats?.paused ? "resume" : "pause" } }),
);
$<HTMLSelectElement>("speed").addEventListener("change", (e) =>
  conn.send({ type: "control", control: { type: "setSpeed", speed: Number((e.target as HTMLSelectElement).value) } }),
);
$("save").addEventListener("click", () => conn.send({ type: "control", control: { type: "saveSnapshot" } }));
$("spawn").addEventListener("click", () => conn.send({ type: "command", command: { type: "spawnAgents", count: 100 } }));
$("clear").addEventListener("click", () => conn.send({ type: "command", command: { type: "clearAgents" } }));

// ---------- picking: select/follow an ant, dig/fill voxels ----------
const raycaster = new THREE.Raycaster();
let downAt: { x: number; y: number } | null = null;
let followId: number | null = null;

function selectAgent(id: number | null) {
  followId = id;
  agents.select(id);
  if (id === null) rig.follow(null);
  $("follow-row").textContent = id === null ? "Click an ant to follow it." : `Following ant #${id} (Esc to stop)`;
}
selectAgent(null);

renderer.domElement.addEventListener("pointerdown", (e) => (downAt = { x: e.clientX, y: e.clientY }));
renderer.domElement.addEventListener("pointerup", (e) => {
  if (!downAt || Math.hypot(e.clientX - downAt.x, e.clientY - downAt.y) > 4) return;
  const rect = renderer.domElement.getBoundingClientRect();
  const ndc = new THREE.Vector2(((e.clientX - rect.left) / rect.width) * 2 - 1, -((e.clientY - rect.top) / rect.height) * 2 + 1);
  raycaster.setFromCamera(ndc, rig.camera);
  if (rig.mode === "cutaway") raycaster.near = 0;

  if (e.shiftKey || e.ctrlKey) {
    const hits = raycaster.intersectObjects([...chunks.values()].map((v) => v.solid), false);
    const hit = hits[0];
    if (!hit?.face) return;
    const n = hit.face.normal;
    const s = e.shiftKey ? -0.5 : 0.5;
    const p = hit.point.clone().addScaledVector(n, s).floor();
    conn.send({
      type: "command",
      command: { type: "setVoxel", pos: [p.x, p.y, p.z], material: e.shiftKey ? Material.Air : Material.Topsoil },
    });
    return;
  }

  agents.mesh.computeBoundingSphere();
  const hit = raycaster.intersectObject(agents.mesh, false)[0];
  if (hit?.instanceId !== undefined) selectAgent(agents.idAtInstance(hit.instanceId));
});

// ---------- stats ----------
const statsEl = $("stats");
let frames = 0;
let fps = 0;
let fpsWindowStart = performance.now();
// Debug/inspection hook (read-only), used for automated checks of the running viewer.
(window as unknown as { __formica: object }).__formica = {
  get fps() { return fps; },
  get editsReceived() { return editsReceived; },
  backend: backendName,
};

function renderStats() {
  const s = lastStats;
  const rows: [string, string][] = [
    ["FPS", fps.toFixed(0)],
    ["Renderer", backendName],
    ["Tick", s ? String(s.tick) : "–"],
    ["Sim tick/s", s ? s.tps.toFixed(1) : "–"],
    ["Step", s ? `${s.stepMs.toFixed(2)} ms` : "–"],
    ["Agents", s ? String(s.agents) : String(agents.count)],
    ["Speed", s ? `${s.speed}×${s.paused ? " (paused)" : ""}` : "–"],
    ["State hash", s ? s.stateHash.toString(16).padStart(16, "0") : "–"],
  ];
  statsEl.replaceChildren(
    ...rows.flatMap(([k, v]) => {
      const dt = document.createElement("dt");
      dt.textContent = k;
      const dd = document.createElement("dd");
      dd.textContent = v;
      return [dt, dd];
    }),
  );
}

// ---------- frame loop ----------
let last = performance.now();
renderer.setAnimationLoop(() => {
  const now = performance.now();
  const dt = Math.min(0.1, (now - last) / 1000);
  last = now;

  agents.update(now);
  if (followId !== null) {
    const pose = agents.poseOf(followId, now);
    if (pose) rig.follow(new THREE.Vector3(pose.x, pose.y + 0.5, pose.z));
    else selectAgent(null);
  }
  rig.update(dt);
  renderer.render(scene, rig.camera);

  frames++;
  if (now - fpsWindowStart >= 1000) {
    fps = (frames * 1000) / (now - fpsWindowStart);
    frames = 0;
    fpsWindowStart = now;
    renderStats();
  }
});
