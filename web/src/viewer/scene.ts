// The 3D view, imperative three.js: one mesh drawn from typed arrays, face
// groups coloured from a small texture (so selecting or hovering a face
// never touches the geometry), result fields coloured in the shader from a
// colour-scale texture, deformation as a vertex offset, and picking by a GPU
// pass that draws face-group numbers into a single pixel under the pointer.
// Frames are drawn on demand: nothing renders while nothing moves.
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { lut, type ColorScale } from "../lib/colormaps";
import type { Slice, Vec3 } from "../lib/types";
import type { Prepared } from "./prepare";

export type ViewPreset = "iso" | "front" | "back" | "left" | "right" | "top" | "bottom";
export const PRESETS: Record<ViewPreset, Vec3> = {
  iso: [0.85, -1.15, 0.8],
  front: [0, -1, 0.0001],
  back: [0, 1, 0.0001],
  left: [-1, 0, 0.0001],
  right: [1, 0, 0.0001],
  top: [0, -0.0001, 1],
  bottom: [0, -0.0001, -1],
};

export interface Mark {
  ids: number[];
  /** CSS colour */
  color: string;
  alpha?: number;
}
export interface Arrow {
  origin: Vec3;
  dir: Vec3;
  length?: number;
  color: string;
}
export interface SliceShow {
  key: string;
  slice: Slice;
  rgba: Uint8Array;
}
export interface SpinShow {
  center: Vec3;
  axis: Vec3;
  radius: number;
  color: string;
}

const reduceMotion = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

function color(css: string): THREE.Color {
  const c = new THREE.Color();
  try {
    c.setStyle(css || "#999", THREE.SRGBColorSpace);
  } catch {
    c.set("#999999");
  }
  return c;
}

export class SceneView {
  readonly renderer: THREE.WebGLRenderer;
  private scene = new THREE.Scene();
  private camera = new THREE.PerspectiveCamera(35, 1, 0.1, 10000);
  private controls: OrbitControls;
  private hemi = new THREE.HemisphereLight(0xffffff, 0x444444, 1.4);
  private sun = new THREE.DirectionalLight(0xffffff, 1.6);
  private fill = new THREE.DirectionalLight(0xffffff, 0.5);
  private material: THREE.MeshStandardMaterial;
  private uniforms = {
    uPatchTex: { value: null as THREE.DataTexture | null },
    uPatchW: { value: 1 },
    uPatchH: { value: 1 },
    uHover: { value: -1 },
    uHoverColor: { value: new THREE.Color(0xff6a00) },
    uLut: { value: null as THREE.DataTexture | null },
    uMin: { value: 0 },
    uMax: { value: 1 },
    uLog: { value: 0 },
    uNan: { value: new THREE.Color(0x777777) },
    uDeform: { value: 0 },
  };
  private mesh: THREE.Mesh | null = null;
  private geometry: THREE.BufferGeometry | null = null;
  private prepared: Prepared | null = null;
  private pickScene = new THREE.Scene();
  private pickMat: THREE.ShaderMaterial;
  private pickTarget = new THREE.WebGLRenderTarget(1, 1);
  private pickBuf = new Uint8Array(4);
  private overlays = new THREE.Group();
  private slices = new THREE.Group();
  private sliceCache = new Map<string, THREE.Mesh>();
  private center = new THREE.Vector3();
  private radius = 50;
  private frame = 0;
  private disposed = false;
  private touched = false;
  private tween: { from: [THREE.Vector3, THREE.Vector3]; to: [THREE.Vector3, THREE.Vector3]; t0: number } | null = null;
  private ro: ResizeObserver;
  private down: { x: number; y: number } | null = null;
  private hoverQueued: { x: number; y: number } | null = null;
  private hoverId = -1;
  private patchCount = 0;
  private patchData: Uint8Array | null = null;
  private critical: THREE.Vector3 | null = null;
  pickable = false;
  hoverable = false;
  onHover: (id: number | null) => void = () => {};
  onPick: (id: number) => void = () => {};
  onFrame: (camera: THREE.Camera) => void = () => {};
  stats = { frames: 0, renderMs: 0, triangles: 0, picks: 0, pickMs: 0 };

  constructor(
    private host: HTMLElement,
    private marker: HTMLElement | null,
  ) {
    const coarse = matchMedia("(pointer: coarse)").matches;
    this.renderer = new THREE.WebGLRenderer({ antialias: !coarse, powerPreference: "high-performance", alpha: true });
    this.renderer.setPixelRatio(Math.min(devicePixelRatio, coarse ? 1.5 : 2));
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.renderer.setClearColor(0x000000, 0);
    const el = this.renderer.domElement;
    el.className = "absolute inset-0 size-full outline-none";
    el.setAttribute("data-testid", "viewer-canvas");
    host.prepend(el);

    this.camera.up.set(0, 0, 1);
    this.controls = new OrbitControls(this.camera, el);
    this.controls.enableDamping = !reduceMotion();
    this.controls.dampingFactor = 0.12;
    this.controls.addEventListener("change", () => this.request());
    this.controls.addEventListener("start", () => {
      this.touched = true;
      this.tween = null;
    });

    this.scene.add(this.hemi, this.overlays, this.slices);
    // the lights move with the camera, so the side you look at is always lit
    this.camera.add(this.sun, this.fill);
    this.sun.position.set(0.6, 1, 1.4);
    this.fill.position.set(-1, -0.4, 0.4);
    this.scene.add(this.camera);

    this.material = new THREE.MeshStandardMaterial({ color: 0xb9b0a0, roughness: 0.62, metalness: 0.05, side: THREE.DoubleSide });
    this.material.onBeforeCompile = (shader) => {
      Object.assign(shader.uniforms, this.uniforms);
      shader.vertexShader = shader.vertexShader
        .replace(
          "#include <common>",
          `#include <common>
          #ifdef USE_PATCH
            attribute float patchId; varying float vPatch;
          #endif
          #ifdef USE_FIELD
            attribute float fval; varying float vF;
          #endif
          #ifdef USE_DISP
            attribute vec3 disp; uniform float uDeform;
          #endif`,
        )
        .replace(
          "#include <begin_vertex>",
          `#include <begin_vertex>
          #ifdef USE_PATCH
            vPatch = patchId;
          #endif
          #ifdef USE_FIELD
            vF = fval;
          #endif
          #ifdef USE_DISP
            transformed += disp * uDeform;
          #endif`,
        );
      shader.fragmentShader = shader.fragmentShader
        .replace(
          "#include <common>",
          `#include <common>
          #ifdef USE_PATCH
            varying float vPatch; uniform sampler2D uPatchTex; uniform float uPatchW; uniform float uPatchH; uniform float uHover; uniform vec3 uHoverColor;
          #endif
          #ifdef USE_FIELD
            varying float vF; uniform sampler2D uLut; uniform float uMin; uniform float uMax; uniform float uLog; uniform vec3 uNan;
          #endif`,
        )
        .replace(
          "vec4 diffuseColor = vec4( diffuse, opacity );",
          `vec3 base = diffuse;
          #ifdef USE_FIELD
            if (vF != vF) base = uNan;
            else {
              float t = uLog > 0.5 ? (log(max(vF, uMin)) - log(uMin)) / max(log(uMax) - log(uMin), 1e-6) : (vF - uMin) / max(uMax - uMin, 1e-12);
              base = texture2D(uLut, vec2(clamp(t, 0.0, 1.0) * (255.0 / 256.0) + 0.5 / 256.0, 0.5)).rgb;
            }
          #endif
          #ifdef USE_PATCH
            float pid = floor(vPatch + 0.5);
            vec4 st = texture2D(uPatchTex, vec2((mod(pid, uPatchW) + 0.5) / uPatchW, (floor(pid / uPatchW) + 0.5) / uPatchH));
            base = mix(base, st.rgb, st.a);
            if (abs(pid - uHover) < 0.5) base = mix(base, uHoverColor, 0.5);
          #endif
          vec4 diffuseColor = vec4( base, opacity );`,
        );
    };
    this.material.customProgramCacheKey = () => JSON.stringify(this.material.defines ?? {});

    this.pickMat = new THREE.ShaderMaterial({
      side: THREE.DoubleSide,
      uniforms: { uDeform: this.uniforms.uDeform },
      vertexShader: `
        attribute float patchId; varying float vP;
        #ifdef USE_DISP
          attribute vec3 disp; uniform float uDeform;
        #endif
        void main() {
          vP = patchId;
          vec3 p = position;
          #ifdef USE_DISP
            p += disp * uDeform;
          #endif
          gl_Position = projectionMatrix * modelViewMatrix * vec4(p, 1.0);
        }`,
      fragmentShader: `
        varying float vP;
        void main() {
          float id = floor(vP + 0.5) + 1.0;
          gl_FragColor = vec4(mod(id, 256.0) / 255.0, mod(floor(id / 256.0), 256.0) / 255.0, floor(id / 65536.0) / 255.0, 1.0);
        }`,
    });

    this.ro = new ResizeObserver(() => this.resize());
    this.ro.observe(host);
    el.addEventListener("pointerdown", this.pDown);
    el.addEventListener("pointermove", this.pMove);
    el.addEventListener("pointerup", this.pUp);
    el.addEventListener("pointerleave", this.pLeave);
    this.resize();
  }

  // ------------------------------------------------------------ drawing

  request = () => {
    if (!this.frame && !this.disposed) this.frame = requestAnimationFrame(this.draw);
  };

  private draw = () => {
    this.frame = 0;
    if (this.tween) {
      const k = Math.min(1, (performance.now() - this.tween.t0) / 320);
      const e = k < 0.5 ? 2 * k * k : 1 - (-2 * k + 2) ** 2 / 2;
      this.camera.position.lerpVectors(this.tween.from[0], this.tween.to[0], e);
      this.controls.target.lerpVectors(this.tween.from[1], this.tween.to[1], e);
      if (k >= 1) this.tween = null;
    }
    const moving = this.controls.update();
    if (this.hoverQueued) {
      const { x, y } = this.hoverQueued;
      this.hoverQueued = null;
      const id = this.pick(x, y);
      if (id !== this.hoverId) {
        this.hoverId = id;
        this.uniforms.uHover.value = id;
        this.onHover(id >= 0 ? id : null);
      }
    }
    const t = performance.now();
    this.renderer.render(this.scene, this.camera);
    this.stats.frames++;
    this.stats.renderMs += performance.now() - t;
    this.placeMarker();
    this.onFrame(this.camera);
    if (moving || this.tween) this.request();
  };

  private resize() {
    const r = this.host.getBoundingClientRect();
    if (!r.width || !r.height) return;
    this.renderer.setSize(r.width, r.height, false);
    this.camera.aspect = r.width / r.height;
    this.camera.updateProjectionMatrix();
    if (!this.touched && this.prepared) this.fit(false);
    this.request();
  }

  // ------------------------------------------------------------ the part

  setMesh(p: Prepared | null) {
    if (p === this.prepared) return;
    if (this.mesh) {
      this.scene.remove(this.mesh);
      this.pickScene.clear();
      this.geometry?.dispose();
      this.mesh = null;
      this.geometry = null;
    }
    this.prepared = p;
    this.stats.triangles = 0;
    if (!p) {
      this.request();
      return;
    }
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", new THREE.BufferAttribute(p.positions, 3));
    g.setAttribute("normal", new THREE.BufferAttribute(p.normals, 3));
    g.setIndex(new THREE.BufferAttribute(p.indices, 1));
    if (p.vertexPatch) g.setAttribute("patchId", new THREE.BufferAttribute(p.vertexPatch, 1));
    g.computeBoundingSphere();
    this.geometry = g;
    this.setDefine("USE_PATCH", !!p.vertexPatch);
    this.setDefine("USE_FIELD", false);
    this.setDefine("USE_DISP", false);
    this.mesh = new THREE.Mesh(g, this.material);
    this.mesh.matrixAutoUpdate = false;
    this.scene.add(this.mesh);
    if (p.vertexPatch) {
      const pm = new THREE.Mesh(g, this.pickMat);
      pm.matrixAutoUpdate = false;
      this.pickScene.add(pm);
      let max = 0;
      for (let i = 0; i < p.vertexPatch.length; i++) if (p.vertexPatch[i]! > max) max = p.vertexPatch[i]!;
      this.allocPatches(max + 1);
    }
    this.stats.triangles = p.triangles;
    const s = g.boundingSphere!;
    this.center.copy(s.center);
    this.radius = s.radius || 1;
    this.camera.near = this.radius / 200;
    this.camera.far = this.radius * 200;
    this.camera.updateProjectionMatrix();
    this.controls.minDistance = this.radius * 0.05;
    this.controls.maxDistance = this.radius * 40;
    if (!this.touched) this.fit(false);
    this.request();
  }

  get size() {
    return { center: this.center.toArray() as Vec3, radius: this.radius };
  }

  private setDefine(name: string, on: boolean) {
    const d = (this.material.defines ??= {});
    const was = name in d;
    if (on) d[name] = "";
    else delete d[name];
    if (was !== on) this.material.needsUpdate = true;
    const pd = (this.pickMat.defines ??= {});
    if (name === "USE_DISP") {
      if (on) pd[name] = "";
      else delete pd[name];
      if (was !== on) this.pickMat.needsUpdate = true;
    }
  }

  private allocPatches(n: number) {
    this.patchCount = n;
    const w = Math.min(4096, Math.max(1, n));
    const h = Math.max(1, Math.ceil(n / w));
    this.patchData = new Uint8Array(w * h * 4);
    this.uniforms.uPatchTex.value?.dispose();
    const tex = new THREE.DataTexture(this.patchData, w, h, THREE.RGBAFormat, THREE.UnsignedByteType);
    tex.colorSpace = THREE.SRGBColorSpace;
    tex.magFilter = THREE.NearestFilter;
    tex.minFilter = THREE.NearestFilter;
    tex.needsUpdate = true;
    this.uniforms.uPatchTex.value = tex;
    this.uniforms.uPatchW.value = w;
    this.uniforms.uPatchH.value = h;
  }

  /** colour groups of faces (held, loaded, selected); later marks win */
  setMarks(marks: Mark[]) {
    const d = this.patchData;
    if (!d) return;
    d.fill(0);
    for (const m of marks) {
      const c = { r: 0, g: 0, b: 0 };
      color(m.color).getRGB(c, THREE.SRGBColorSpace);
      const a = Math.round((m.alpha ?? 0.85) * 255);
      for (const id of m.ids) {
        if (id < 0 || id >= this.patchCount) continue;
        d[id * 4] = Math.round(c.r * 255);
        d[id * 4 + 1] = Math.round(c.g * 255);
        d[id * 4 + 2] = Math.round(c.b * 255);
        d[id * 4 + 3] = a;
      }
    }
    this.uniforms.uPatchTex.value!.needsUpdate = true;
    this.request();
  }

  setPartColor(css: string, hover: string) {
    this.material.color.copy(color(css));
    this.uniforms.uHoverColor.value.copy(color(hover));
    this.request();
  }

  setLights(dark: boolean) {
    this.hemi.color.set(dark ? 0xf1ece0 : 0xffffff);
    this.hemi.groundColor.set(dark ? 0x2a241d : 0x8a8478);
    this.hemi.intensity = dark ? 1.25 : 1.5;
    this.sun.intensity = dark ? 1.6 : 1.4;
    this.request();
  }

  /** colour the part by a field: values per vertex of the prepared mesh, or null for the plain part */
  setField(values: Float32Array | null, scale: ColorScale | null) {
    const g = this.geometry;
    if (!g) return;
    if (!values || !scale) {
      // the attribute stays (switching fields refills it); the shader just stops reading it
      this.setDefine("USE_FIELD", false);
      this.request();
      return;
    }
    this.fillAttr("fval", values, 1);
    const old = this.uniforms.uLut.value;
    if (!old || old.userData.map !== scale.map) {
      old?.dispose();
      const tex = new THREE.DataTexture(lut(scale.map, 256), 256, 1, THREE.RGBAFormat, THREE.UnsignedByteType);
      tex.colorSpace = THREE.SRGBColorSpace;
      tex.magFilter = THREE.LinearFilter;
      tex.minFilter = THREE.LinearFilter;
      tex.userData.map = scale.map;
      tex.needsUpdate = true;
      this.uniforms.uLut.value = tex;
    }
    this.uniforms.uMin.value = scale.min;
    this.uniforms.uMax.value = scale.max;
    this.uniforms.uLog.value = scale.log ? 1 : 0;
    this.setDefine("USE_FIELD", true);
    this.request();
  }

  /** show the part bent: xyz movement per vertex (mm), times `k` */
  setDeform(disp: Float32Array | null, k: number) {
    const g = this.geometry;
    if (!g) return;
    if (!disp) this.setDefine("USE_DISP", false);
    else {
      this.fillAttr("disp", disp, 3);
      this.setDefine("USE_DISP", true);
    }
    this.uniforms.uDeform.value = disp ? k : 0;
    this.request();
  }

  /**
   * Put per-vertex values into one attribute owned by the mesh, copying into
   * it when it exists. Replacing the attribute instead would leave the old GPU
   * buffer behind (three.js frees only what is attached when the geometry goes).
   */
  private fillAttr(name: "fval" | "disp", values: Float32Array, size: number) {
    const g = this.geometry!;
    const cur = g.getAttribute(name) as THREE.BufferAttribute | undefined;
    if (cur?.array === values) return;
    if (cur && cur.array.length === values.length) {
      (cur.array as Float32Array).set(values);
      cur.needsUpdate = true;
      return;
    }
    // the first time (or a size that does not match, which a well-formed result never has)
    g.setAttribute(name, new THREE.BufferAttribute(new Float32Array(values), size));
  }

  // ------------------------------------------------------------ extras drawn around the part

  setOverlays(o: { arrows?: Arrow[]; plate?: { up: Vec3; color: string } | null; spin?: SpinShow | null }) {
    for (const c of [...this.overlays.children]) {
      this.overlays.remove(c);
      c.traverse((x) => {
        const m = x as THREE.Mesh;
        m.geometry?.dispose();
        (m.material as THREE.Material | undefined)?.dispose?.();
      });
    }
    const R = this.radius;
    for (const a of o.arrows ?? []) {
      const dir = new THREE.Vector3(...a.dir).normalize();
      const len = a.length ?? R * 0.35;
      const tip = new THREE.Vector3(...a.origin);
      const start = tip.clone().addScaledVector(dir, -len);
      const h = new THREE.ArrowHelper(dir, start, len, color(a.color).getHex(), len * 0.28, len * 0.16);
      h.traverse((x) => {
        const m = x as THREE.Mesh;
        if (m.material) {
          const mat = m.material as THREE.MeshBasicMaterial;
          mat.depthTest = false;
          mat.transparent = true;
        }
        x.renderOrder = 10;
      });
      this.overlays.add(h);
    }
    if (o.plate && this.prepared) {
      const up = new THREE.Vector3(...o.plate.up).normalize();
      // the lowest point along "up": where the bed is
      const p = this.prepared.positions;
      let lo = Infinity;
      for (let i = 0; i < p.length; i += 3 * Math.max(1, Math.floor(p.length / 3 / 200000))) lo = Math.min(lo, p[i]! * up.x + p[i + 1]! * up.y + p[i + 2]! * up.z);
      const c = this.center.clone();
      const onBed = c.clone().addScaledVector(up, lo - c.dot(up) - R * 0.01);
      const size = R * 2.3;
      const q = new THREE.Quaternion().setFromUnitVectors(new THREE.Vector3(0, 0, 1), up);
      const plate = new THREE.Mesh(
        new THREE.PlaneGeometry(size, size),
        new THREE.MeshBasicMaterial({ color: color(o.plate.color), transparent: true, opacity: 0.16, side: THREE.DoubleSide, depthWrite: false }),
      );
      plate.position.copy(onBed);
      plate.quaternion.copy(q);
      const grid = new THREE.GridHelper(size, 12, color(o.plate.color).getHex(), color(o.plate.color).getHex());
      (grid.material as THREE.LineBasicMaterial).transparent = true;
      (grid.material as THREE.LineBasicMaterial).opacity = 0.35;
      grid.quaternion.copy(q).multiply(new THREE.Quaternion().setFromAxisAngle(new THREE.Vector3(1, 0, 0), Math.PI / 2));
      grid.position.copy(onBed);
      const arrowStart = onBed.clone().addScaledVector(new THREE.Vector3(1, 0, 0).applyQuaternion(q), size * 0.42).addScaledVector(new THREE.Vector3(0, 1, 0).applyQuaternion(q), -size * 0.42);
      const arrow = new THREE.ArrowHelper(up, arrowStart, R * 0.6, color(o.plate.color).getHex(), R * 0.14, R * 0.08);
      this.overlays.add(plate, grid, arrow);
    }
    if (o.spin) {
      const axis = new THREE.Vector3(...o.spin.axis).normalize();
      const q = new THREE.Quaternion().setFromUnitVectors(new THREE.Vector3(0, 0, 1), axis);
      const ring = new THREE.Mesh(
        new THREE.TorusGeometry(o.spin.radius, R * 0.008, 8, 96, Math.PI * 1.7),
        new THREE.MeshBasicMaterial({ color: color(o.spin.color), transparent: true, opacity: 0.9, depthTest: false }),
      );
      ring.quaternion.copy(q);
      ring.position.set(...o.spin.center);
      ring.renderOrder = 10;
      const ax = new THREE.ArrowHelper(axis, new THREE.Vector3(...o.spin.center).addScaledVector(axis, -R * 0.2), R * 0.7, color(o.spin.color).getHex(), R * 0.1, R * 0.06);
      this.overlays.add(ring, ax);
    }
    this.request();
  }

  setSlices(list: SliceShow[]) {
    const keep = new Set(list.map((s) => s.key));
    for (const [k, m] of this.sliceCache)
      if (!keep.has(k)) {
        this.slices.remove(m);
        m.geometry.dispose();
        const mat = m.material as THREE.MeshBasicMaterial;
        mat.map?.dispose();
        mat.dispose();
        this.sliceCache.delete(k);
      }
    for (const s of list) {
      if (this.sliceCache.has(s.key)) continue;
      const { origin_mm: o, u_mm: u, v_mm: v, nu, nv } = s.slice;
      const g = new THREE.BufferGeometry();
      const pts = [o, [o[0] + u[0], o[1] + u[1], o[2] + u[2]], [o[0] + u[0] + v[0], o[1] + u[1] + v[1], o[2] + u[2] + v[2]], [o[0] + v[0], o[1] + v[1], o[2] + v[2]]].flat();
      g.setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
      g.setAttribute("uv", new THREE.Float32BufferAttribute([0, 0, 1, 0, 1, 1, 0, 1], 2));
      g.setIndex([0, 1, 2, 0, 2, 3]);
      const tex = new THREE.DataTexture(s.rgba, nu, nv, THREE.RGBAFormat, THREE.UnsignedByteType);
      tex.colorSpace = THREE.SRGBColorSpace;
      tex.magFilter = THREE.LinearFilter;
      tex.minFilter = THREE.LinearFilter;
      tex.needsUpdate = true;
      const m = new THREE.Mesh(g, new THREE.MeshBasicMaterial({ map: tex, transparent: true, side: THREE.DoubleSide, depthWrite: false, opacity: 0.92 }));
      this.slices.add(m);
      this.sliceCache.set(s.key, m);
    }
    this.request();
  }

  setCritical(p: Vec3 | null) {
    this.critical = p ? new THREE.Vector3(...p) : null;
    if (this.marker) this.marker.style.display = p ? "" : "none";
    this.request();
  }

  private placeMarker() {
    const m = this.marker;
    if (!m || !this.critical) return;
    const v = this.critical.clone().project(this.camera);
    const r = this.renderer.domElement.getBoundingClientRect();
    const hidden = v.z > 1 || v.z < -1;
    m.style.visibility = hidden ? "hidden" : "visible";
    m.style.transform = `translate(${((v.x + 1) / 2) * r.width}px, ${((1 - v.y) / 2) * r.height}px)`;
  }

  // ------------------------------------------------------------ camera

  private fit(animate: boolean, dir?: THREE.Vector3) {
    const d = dir ?? this.camera.position.clone().sub(this.controls.target).normalize();
    if (!dir && (!Number.isFinite(d.x) || d.lengthSq() < 0.5)) d.set(...PRESETS.iso).normalize();
    const v = THREE.MathUtils.degToRad(this.camera.fov / 2);
    const h = Math.atan(Math.tan(v) * this.camera.aspect);
    const dist = (this.radius / Math.sin(Math.min(v, h))) * 1.05;
    const pos = this.center.clone().addScaledVector(d, dist);
    if (animate && !reduceMotion()) this.tween = { from: [this.camera.position.clone(), this.controls.target.clone()], to: [pos, this.center.clone()], t0: performance.now() };
    else {
      this.tween = null;
      this.camera.position.copy(pos);
      this.controls.target.copy(this.center);
    }
    this.controls.update();
    this.request();
  }

  view(preset: ViewPreset) {
    this.fit(true, new THREE.Vector3(...PRESETS[preset]).normalize());
  }
  /** look at the part from a direction (the camera sits on that side) */
  lookFrom(dir: Vec3) {
    const d = new THREE.Vector3(...dir).normalize();
    if (Math.abs(d.z) > 0.999) d.y = -0.0001;
    this.fit(true, d.normalize());
  }
  reset() {
    this.touched = false;
    this.fit(true, new THREE.Vector3(...PRESETS.iso).normalize());
  }
  zoom(f: number) {
    const off = this.camera.position.clone().sub(this.controls.target).multiplyScalar(f);
    const len = THREE.MathUtils.clamp(off.length(), this.controls.minDistance, this.controls.maxDistance);
    off.setLength(len);
    this.touched = true;
    if (!reduceMotion()) this.tween = { from: [this.camera.position.clone(), this.controls.target.clone()], to: [this.controls.target.clone().add(off), this.controls.target.clone()], t0: performance.now() };
    else this.camera.position.copy(this.controls.target).add(off);
    this.request();
  }
  /** turn the view by small steps (keyboard) */
  orbit(dx: number, dy: number) {
    const off = this.camera.position.clone().sub(this.controls.target);
    const s = new THREE.Spherical().setFromVector3(new THREE.Vector3(off.x, off.z, -off.y));
    s.theta -= dx;
    s.phi = THREE.MathUtils.clamp(s.phi - dy, 0.05, Math.PI - 0.05);
    const p = new THREE.Vector3().setFromSpherical(s);
    this.camera.position.copy(this.controls.target).add(new THREE.Vector3(p.x, -p.z, p.y));
    this.touched = true;
    this.controls.update();
    this.request();
  }
  focus(p: Vec3) {
    const target = new THREE.Vector3(...p);
    const off = this.camera.position.clone().sub(this.controls.target).setLength(this.radius * 1.4);
    this.touched = true;
    const to: [THREE.Vector3, THREE.Vector3] = [target.clone().add(off), target];
    if (reduceMotion()) {
      this.camera.position.copy(to[0]);
      this.controls.target.copy(to[1]);
    } else this.tween = { from: [this.camera.position.clone(), this.controls.target.clone()], to, t0: performance.now() };
    this.request();
  }
  cameraQuaternion() {
    return this.camera.quaternion;
  }

  // ------------------------------------------------------------ picking

  /** the face group under a point (CSS pixels in the canvas), or -1 */
  pick(x: number, y: number): number {
    if (!this.prepared?.vertexPatch || !this.mesh) return -1;
    const t = performance.now();
    const el = this.renderer.domElement;
    const w = el.clientWidth, h = el.clientHeight;
    this.camera.setViewOffset(w, h, x, y, 1, 1);
    const prevTarget = this.renderer.getRenderTarget();
    this.renderer.setRenderTarget(this.pickTarget);
    this.renderer.setClearColor(0x000000, 0);
    this.renderer.clear();
    this.renderer.render(this.pickScene, this.camera);
    this.renderer.readRenderTargetPixels(this.pickTarget, 0, 0, 1, 1, this.pickBuf);
    this.renderer.setRenderTarget(prevTarget);
    this.camera.clearViewOffset();
    this.stats.picks++;
    this.stats.pickMs += performance.now() - t;
    const b = this.pickBuf;
    return b[0]! + b[1]! * 256 + b[2]! * 65536 - 1;
  }

  private local(e: PointerEvent) {
    const r = this.renderer.domElement.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }
  private pDown = (e: PointerEvent) => {
    this.down = this.local(e);
  };
  private pMove = (e: PointerEvent) => {
    if (!(this.pickable || this.hoverable) || e.buttons || e.pointerType === "touch") return;
    this.hoverQueued = this.local(e);
    this.request();
  };
  private pUp = (e: PointerEvent) => {
    const d = this.down;
    this.down = null;
    if (!d || !this.pickable) return;
    const p = this.local(e);
    if (Math.hypot(p.x - d.x, p.y - d.y) > 5) return;
    const id = this.pick(p.x, p.y);
    if (id >= 0) this.onPick(id);
    this.request();
  };
  private pLeave = () => {
    this.hoverQueued = null;
    if (this.hoverId !== -1) {
      this.hoverId = -1;
      this.uniforms.uHover.value = -1;
      this.onHover(null);
      this.request();
    }
  };
  setHoverId(id: number | null) {
    this.hoverId = id ?? -1;
    this.uniforms.uHover.value = this.hoverId;
    this.request();
  }

  dispose() {
    this.disposed = true;
    cancelAnimationFrame(this.frame);
    this.ro.disconnect();
    this.controls.dispose();
    const el = this.renderer.domElement;
    el.removeEventListener("pointerdown", this.pDown);
    el.removeEventListener("pointermove", this.pMove);
    el.removeEventListener("pointerup", this.pUp);
    el.removeEventListener("pointerleave", this.pLeave);
    this.setOverlays({});
    this.setSlices([]);
    this.geometry?.dispose();
    this.material.dispose();
    this.pickMat.dispose();
    this.pickTarget.dispose();
    this.uniforms.uPatchTex.value?.dispose();
    this.uniforms.uLut.value?.dispose();
    this.renderer.dispose();
    this.renderer.forceContextLoss();
    el.remove();
  }
}
