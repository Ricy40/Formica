// Four camera modes sharing one scene:
//  - iso:     orthographic, locked isometric elevation, smooth orbit (drag or Q/E 90° steps) + zoom
//  - top:     orthographic straight down, pan + zoom
//  - cutaway: orthographic side view along an axis; the scene is sliced at a depth (ant-farm view)
//  - free:    perspective orbit + WASD fly, with follow-cam on a selected agent

import * as THREE from "three/webgpu";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";

export type CameraMode = "iso" | "top" | "cutaway" | "free";
export type CutAxis = "x" | "y" | "z";

const ISO_POLAR = Math.acos(1 / Math.sqrt(3)); // camera along (1, 1, 1): 54.7356°

export const MODE_HELP: Record<CameraMode, string> = {
  iso: "Drag: orbit · Q/E: rotate 90° · Wheel: zoom · Right-drag: pan",
  top: "Drag: pan · Wheel: zoom",
  cutaway: "Drag: pan · Wheel: zoom · Axis: X/Z side slice, Y horizontal slice · Slider: cut position",
  free: "Drag: orbit · WASD/RF: move · Wheel: dolly · Click ant: follow · Esc: stop following",
};

export class CameraRig {
  readonly ortho: THREE.OrthographicCamera;
  readonly persp: THREE.PerspectiveCamera;
  readonly controls: OrbitControls;
  mode: CameraMode = "iso";
  cutAxis: CutAxis = "z";

  private azimuthTween: { from: number; to: number; t: number } | null = null;
  private keys = new Set<string>();
  private followTarget: THREE.Vector3 | null = null;
  private aspect = 1;
  /** World units visible vertically in the orthographic views at zoom 1. */
  private viewHeight = 60;

  constructor(dom: HTMLElement, private center: THREE.Vector3) {
    this.ortho = new THREE.OrthographicCamera(-1, 1, 1, -1, -500, 500);
    this.persp = new THREE.PerspectiveCamera(50, 1, 0.1, 2000);
    this.controls = new OrbitControls(this.ortho, dom);
    this.controls.enableDamping = true;
    this.controls.dampingFactor = 0.12;
    this.controls.target.copy(center);

    window.addEventListener("keydown", (e) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLSelectElement) return;
      this.keys.add(e.key.toLowerCase());
      if (this.mode === "iso" && (e.key === "q" || e.key === "e")) this.rotateIso(e.key === "q" ? 1 : -1);
    });
    window.addEventListener("keyup", (e) => this.keys.delete(e.key.toLowerCase()));
    window.addEventListener("blur", () => this.keys.clear());
  }

  get camera(): THREE.Camera {
    return this.mode === "free" ? this.persp : this.ortho;
  }

  resize(width: number, height: number) {
    this.aspect = width / Math.max(1, height);
    this.applyProjection();
  }

  /** Fit the orthographic views to a world of the given largest dimension. */
  fitWorld(maxDim: number) {
    this.viewHeight = maxDim * 1.9;
    this.applyProjection();
  }

  private applyProjection() {
    // Keep the whole world visible on portrait screens too.
    const h = (this.viewHeight / 2) * Math.max(1, 1 / this.aspect);
    this.ortho.left = -h * this.aspect;
    this.ortho.right = h * this.aspect;
    this.ortho.top = h;
    this.ortho.bottom = -h;
    this.ortho.updateProjectionMatrix();
    this.persp.aspect = this.aspect;
    this.persp.updateProjectionMatrix();
  }

  setMode(mode: CameraMode) {
    this.mode = mode;
    this.azimuthTween = null;
    const c = this.controls;
    const target = c.target.clone();
    c.object = this.camera;
    c.target.copy(target);
    c.enableRotate = true;
    c.enablePan = true;
    c.screenSpacePanning = true;
    c.minPolarAngle = 0;
    c.maxPolarAngle = Math.PI;
    c.minAzimuthAngle = -Infinity;
    c.maxAzimuthAngle = Infinity;
    c.mouseButtons = { LEFT: THREE.MOUSE.ROTATE, MIDDLE: THREE.MOUSE.DOLLY, RIGHT: THREE.MOUSE.PAN };
    c.touches = { ONE: THREE.TOUCH.ROTATE, TWO: THREE.TOUCH.DOLLY_PAN };

    switch (mode) {
      case "iso":
        c.minPolarAngle = c.maxPolarAngle = ISO_POLAR;
        this.placeOrtho(ISO_POLAR, Math.PI / 4);
        break;
      case "top":
        c.enableRotate = false;
        c.mouseButtons = { LEFT: THREE.MOUSE.PAN, MIDDLE: THREE.MOUSE.DOLLY, RIGHT: THREE.MOUSE.PAN };
        c.touches = { ONE: THREE.TOUCH.PAN, TWO: THREE.TOUCH.DOLLY_PAN };
        c.minPolarAngle = c.maxPolarAngle = 0;
        this.placeOrtho(1e-4, 0);
        break;
      case "cutaway":
        c.enableRotate = false;
        c.mouseButtons = { LEFT: THREE.MOUSE.PAN, MIDDLE: THREE.MOUSE.DOLLY, RIGHT: THREE.MOUSE.PAN };
        c.touches = { ONE: THREE.TOUCH.PAN, TWO: THREE.TOUCH.DOLLY_PAN };
        if (this.cutAxis === "y") {
          // Horizontal slice: look straight down onto the cut (a plan view of the nest).
          c.minPolarAngle = c.maxPolarAngle = 0;
          this.placeOrtho(1e-4, 0);
        } else {
          c.minPolarAngle = c.maxPolarAngle = Math.PI / 2;
          this.placeOrtho(Math.PI / 2, this.cutAxis === "z" ? 0 : Math.PI / 2);
        }
        break;
      case "free": {
        c.minPolarAngle = 0.05;
        c.maxPolarAngle = Math.PI - 0.05;
        const offset = new THREE.Vector3(26, 22, 30);
        this.persp.position.copy(c.target).add(offset);
        break;
      }
    }
    c.update();
  }

  setCutAxis(axis: CutAxis) {
    this.cutAxis = axis;
    if (this.mode === "cutaway") this.setMode("cutaway");
  }

  private placeOrtho(polar: number, azimuth: number) {
    const r = 200;
    const s = new THREE.Spherical(r, polar, azimuth);
    this.ortho.position.copy(this.controls.target).add(new THREE.Vector3().setFromSpherical(s));
    this.ortho.up.set(0, 1, 0);
    // Straight down needs a defined "up" on screen.
    if (polar < 1e-3) this.ortho.up.set(0, 0, -1);
    this.ortho.lookAt(this.controls.target);
  }

  private rotateIso(dir: number) {
    const from = this.controls.getAzimuthalAngle();
    const base = this.azimuthTween ? this.azimuthTween.to : from;
    // Snap to the next quarter turn offset by 45° (the classic isometric diagonals).
    const step = Math.PI / 2;
    const snapped = Math.round((base - Math.PI / 4) / step) * step + Math.PI / 4;
    this.azimuthTween = { from, to: snapped + dir * step, t: 0 };
  }

  /** Follow a point (agent) smoothly, keeping the camera's offset from its target. */
  follow(p: THREE.Vector3 | null) {
    this.followTarget = p ? p.clone() : null;
  }

  resetTarget() {
    this.followTarget = null;
    this.controls.target.copy(this.center);
    this.setMode(this.mode);
  }

  update(dtSec: number) {
    const c = this.controls;
    const cam = this.camera;

    if (this.azimuthTween && this.mode === "iso") {
      const tw = this.azimuthTween;
      tw.t = Math.min(1, tw.t + dtSec / 0.45);
      const e = tw.t < 0.5 ? 2 * tw.t * tw.t : 1 - (-2 * tw.t + 2) ** 2 / 2;
      this.placeOrtho(ISO_POLAR, tw.from + (tw.to - tw.from) * e);
      if (tw.t >= 1) this.azimuthTween = null;
    }

    if (this.mode === "free" && this.keys.size > 0) {
      const speed = 25 * dtSec;
      const fwd = new THREE.Vector3();
      cam.getWorldDirection(fwd);
      fwd.y = 0;
      fwd.normalize();
      const right = new THREE.Vector3().crossVectors(fwd, new THREE.Vector3(0, 1, 0));
      const move = new THREE.Vector3();
      if (this.keys.has("w")) move.add(fwd);
      if (this.keys.has("s")) move.sub(fwd);
      if (this.keys.has("d")) move.add(right);
      if (this.keys.has("a")) move.sub(right);
      if (this.keys.has("r")) move.y += 1;
      if (this.keys.has("f")) move.y -= 1;
      if (move.lengthSq() > 0) {
        move.normalize().multiplyScalar(speed);
        cam.position.add(move);
        c.target.add(move);
      }
    }

    if (this.followTarget) {
      const delta = this.followTarget.clone().sub(c.target).multiplyScalar(Math.min(1, dtSec * 6));
      c.target.add(delta);
      cam.position.add(delta);
    }

    c.update();
  }
}
