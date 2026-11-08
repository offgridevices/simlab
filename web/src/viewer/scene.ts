// The 3D view, imperative three.js: one mesh drawn from typed arrays, face
// groups coloured from a small texture (so selecting or hovering a face
// never touches the geometry), result fields coloured in the shader from a
// colour-scale texture, deformation as a vertex offset, and picking by a GPU
// pass that draws face-group numbers into a single pixel under the pointer.
// Frames are drawn on demand: nothing renders while nothing moves.
import * as THREE from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import { lut, paletteStamp, type ColorScale } from "../lib/colormaps";
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
/** a quaternion [x, y, z, w]: how the part is turned to sit on the printer's bed */
export type Quat = [number, number, number, number];
/** a test shown as it runs: the part spinning, air streaming past or through it, loads pulsing */
export interface LiveShow {
  /** the part turns about this axis (shown slowly; the real speed would only flicker) */
  spin?: { center: Vec3; axis: Vec3 } | null;
  /** air: "around" a part in a stream, or "through" a propeller's disc (dir = the way the air moves) */
  flow?: { kind: "around" | "through"; dir: Vec3; center: Vec3; radius: number; color: string } | null;
  /** the load arrows breathe, so it reads as pushing */
  pulse?: boolean;
}

/** where the stream runs along its way, in part radii from the middle: from, to */
const FLOW_SPAN = { through: [-1.1, 1.7], around: [-1.6, 2] } as const;
const coarsePointer = () => typeof matchMedia !== "undefined" && matchMedia("(pointer: coarse)").matches;
const reduceMotion = () => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches;

/** a CSS colour (from the tokens) as a three.js colour; mid grey if it cannot be read */
function color(css: string): THREE.Color {
  const c = new THREE.Color().setRGB(0.6, 0.6, 0.6, THREE.SRGBColorSpace);
  if (!css) return c;
  try {
    c.setStyle(css, THREE.SRGBColorSpace);
  } catch {
    /* keep the grey */
  }
  return c;
}

/** a design token's value now */
function tok(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}
const tokNum = (name: string, fallback: number) => {
  const v = parseFloat(tok(name));
  return Number.isFinite(v) ? v : fallback;
};

/** a round grid step (1, 2 or 5 × 10ⁿ mm) giving about `cells` squares across `span` */
export function gridStep(span: number, cells = 12): number {
  const raw = span / cells;
  const p = 10 ** Math.floor(Math.log10(raw || 1));
  return [1, 2, 5, 10].map((m) => m * p).find((x) => x >= raw) ?? 10 * p;
}

export interface ProbeHit {
  value: number;
  pos: Vec3;
}

export class SceneView {
  readonly renderer: THREE.WebGLRenderer;
  private scene = new THREE.Scene();
  private camera = new THREE.PerspectiveCamera(35, 1, 0.1, 10000);
  private controls: OrbitControls;
  private hemi = new THREE.HemisphereLight();
  private sun = new THREE.DirectionalLight();
  private fill = new THREE.DirectionalLight();
  private material: THREE.MeshStandardMaterial;
  private uniforms = {
    uPatchTex: { value: null as THREE.DataTexture | null },
    uPatchW: { value: 1 },
    uPatchH: { value: 1 },
    uHover: { value: -1 },
    uHoverColor: { value: new THREE.Color() },
    uLut: { value: null as THREE.DataTexture | null },
    uMin: { value: 0 },
    uMax: { value: 1 },
    uLog: { value: 0 },
    uNan: { value: new THREE.Color() },
    uDeform: { value: 0 },
    uRCol: { value: Array.from({ length: 8 }, () => new THREE.Vector4()) },
  };
  private mesh: THREE.Mesh | null = null;
  private geometry: THREE.BufferGeometry | null = null;
  private prepared: Prepared | null = null;
  private pickScene = new THREE.Scene();
  private pickMat: THREE.ShaderMaterial;
  private pickTarget = new THREE.WebGLRenderTarget(1, 1);
  private pickBuf = new Uint8Array(4);
  /** drawn in the part's own frame (load arrows, spin ring), turned with it */
  private overlays = new THREE.Group();
  private slices = new THREE.Group();
  /** drawn in the printer's frame: the bed (its own group, so redrawing it never touches the air streaks) */
  private world = new THREE.Group();
  private bedGroup = new THREE.Group();
  /** the part's turn onto the bed (identity: as drawn), and the spin on top of it while a test runs */
  private pose = new THREE.Matrix4();
  private partM = new THREE.Matrix4();
  private partInv = new THREE.Matrix4();
  private live: LiveShow | null = null;
  private liveT0 = 0;
  private flowLines: THREE.LineSegments | null = null;
  private flowP: Float32Array | null = null;
  private flowAge: Float32Array | null = null;
  private lastLive = 0;
  /** the part's middle in its own frame (the bounding sphere's centre) */
  private modelCenter = new THREE.Vector3();
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
  private grid = new THREE.Group();
  private gridMats: THREE.LineBasicMaterial[] = [];
  private probeScene = new THREE.Scene();
  private probeMat: THREE.ShaderMaterial;
  private probeTarget: THREE.WebGLRenderTarget | null = null;
  private probeBuf = new Float32Array(4);
  /** reading values under the pointer needs float render targets (every Mac has them) */
  readonly canProbe: boolean;
  pickable = false;
  hoverable = false;
  /** clicking reports the 3D point under the pointer (the brush) */
  pointing = false;
  onPoint: (p: Vec3) => void = () => {};
  /** hovering reads the result's value (when a field is shown) instead of picking faces */
  probing = false;
  onHover: (id: number | null) => void = () => {};
  onPick: (id: number) => void = () => {};
  onProbe: (hit: (ProbeHit & { x: number; y: number }) | null) => void = () => {};
  onProbePin: (hit: ProbeHit) => void = () => {};
  /** the ground grid's square size, mm (0 = no grid) */
  onGrid: (stepMm: number) => void = () => {};
  onFrame: (camera: THREE.Camera) => void = () => {};
  stats = { frames: 0, renderMs: 0, triangles: 0, picks: 0, pickMs: 0 };

  constructor(
    private host: HTMLElement,
    /** elements with data-anchor="x,y,z" are kept over that point of the part */
    private anchors: HTMLElement | null,
  ) {
    const coarse = matchMedia("(pointer: coarse)").matches;
    this.renderer = new THREE.WebGLRenderer({ antialias: !coarse, powerPreference: "high-performance", alpha: true });
    this.renderer.setPixelRatio(Math.min(devicePixelRatio, coarse ? 1.5 : 2));
    this.renderer.outputColorSpace = THREE.SRGBColorSpace;
    this.renderer.setClearColor(new THREE.Color(), 0);
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

    this.scene.add(this.hemi, this.grid, this.overlays, this.slices, this.world, this.bedGroup);
    for (const g of [this.overlays, this.slices]) g.matrixAutoUpdate = false;
    // the lights move with the camera, so the side you look at is always lit
    this.camera.add(this.sun, this.fill);
    this.sun.position.set(0.6, 1, 1.4);
    this.fill.position.set(-1, -0.4, 0.4);
    this.scene.add(this.camera);

    this.material = new THREE.MeshStandardMaterial({ roughness: 0.62, metalness: 0.05, side: THREE.DoubleSide });
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
          #endif
          #ifdef USE_RMARK
            attribute float rmark; varying float vR;
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
          #endif
          #ifdef USE_RMARK
            vR = rmark;
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
          #endif
          #ifdef USE_RMARK
            varying float vR; uniform vec4 uRCol[8];
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
          #ifdef USE_RMARK
            // areas picked by shape; a soft edge where a triangle is only partly in
            if (vR > 0.05) {
              int k = int(mod(floor(vR + 0.5) - 1.0, 8.0));
              vec4 rc = uRCol[0];
              for (int i = 1; i < 8; i++) if (i == k) rc = uRCol[i];
              base = mix(base, rc.rgb, rc.a * clamp(vR / max(floor(vR + 0.5), 1.0), 0.0, 1.0));
            }
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

    this.probeMat = new THREE.ShaderMaterial({
      side: THREE.DoubleSide,
      uniforms: { uDeform: this.uniforms.uDeform, uMode: { value: 0 } },
      vertexShader: `
        attribute float fval; varying float vF; varying vec3 vW;
        #ifdef USE_DISP
          attribute vec3 disp; uniform float uDeform;
        #endif
        void main() {
          vF = fval;
          vec3 p = position;
          #ifdef USE_DISP
            p += disp * uDeform;
          #endif
          vec4 w = modelMatrix * vec4(p, 1.0);
          vW = w.xyz;
          gl_Position = projectionMatrix * viewMatrix * w;
        }`,
      fragmentShader: `
        uniform float uMode; varying float vF; varying vec3 vW;
        void main() {
          gl_FragColor = uMode < 0.5 ? vec4(vF, 1.0, 0.0, 1.0) : vec4(vW, 1.0);
        }`,
    });
    this.canProbe = this.renderer.capabilities.isWebGL2 && this.renderer.extensions.has("EXT_color_buffer_float");
    if (this.canProbe) this.probeTarget = new THREE.WebGLRenderTarget(1, 1, { type: THREE.FloatType, format: THREE.RGBAFormat });

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
    const animating = this.stepLive();
    if (this.hoverQueued) {
      const { x, y } = this.hoverQueued;
      this.hoverQueued = null;
      if (this.probing) {
        const hit = this.probe(x, y);
        this.onProbe(hit ? { ...hit, x, y } : null);
      } else {
        const id = this.pick(x, y);
        if (id !== this.hoverId) {
          this.hoverId = id;
          this.uniforms.uHover.value = id;
          this.onHover(id >= 0 ? id : null);
        }
      }
    }
    const t = performance.now();
    this.renderer.render(this.scene, this.camera);
    this.stats.frames++;
    this.stats.renderMs += performance.now() - t;
    this.placeAnchors();
    this.onFrame(this.camera);
    if (moving || this.tween || animating) this.request();
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
      this.probeScene.clear();
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
    this.setDefine("USE_RMARK", false);
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
    const pm2 = new THREE.Mesh(g, this.probeMat);
    pm2.matrixAutoUpdate = false;
    this.probeScene.add(pm2);
    this.stats.triangles = p.triangles;
    const s = g.boundingSphere!;
    this.modelCenter.copy(s.center);
    this.center.copy(s.center).applyMatrix4(this.pose);
    this.radius = s.radius || 1;
    this.camera.near = this.radius / 200;
    this.camera.far = this.radius * 200;
    this.camera.updateProjectionMatrix();
    this.controls.minDistance = this.radius * 0.05;
    this.controls.maxDistance = this.radius * 40;
    this.applyPart();
    this.buildGrid(this.posedBox());
    if (!this.touched) this.fit(false);
    this.request();
  }

  /** the part's box in the printer's frame (as it sits now) */
  private posedBox(): THREE.Box3 {
    const box = new THREE.Box3();
    const p = this.prepared?.positions;
    if (!p) return box.setFromCenterAndSize(new THREE.Vector3(), new THREE.Vector3(10, 10, 10));
    const v = new THREE.Vector3();
    const stride = 3 * Math.max(1, Math.floor(p.length / 3 / 200000));
    for (let i = 0; i < p.length; i += stride) box.expandByPoint(v.set(p[i]!, p[i + 1]!, p[i + 2]!).applyMatrix4(this.pose));
    return box;
  }

  /** put the part (and everything drawn in its frame) where the pose and the spin say */
  private applyPart(spin?: THREE.Matrix4) {
    this.partM.copy(this.pose);
    if (spin) this.partM.multiply(spin);
    this.partInv.copy(this.partM).invert();
    for (const m of [this.mesh, ...this.pickScene.children, ...this.probeScene.children]) {
      if (!m) continue;
      m.matrix.copy(this.partM);
      m.matrixWorldNeedsUpdate = true;
    }
    for (const g of [this.overlays, this.slices]) {
      g.matrix.copy(this.pose);
      g.matrixWorldNeedsUpdate = true;
    }
  }

  /**
   * Turn the part onto the printer's bed (null: as drawn). The grid stays flat
   * under it and the compass keeps naming the printer's directions.
   */
  setPose(q: Quat | null) {
    const next = new THREE.Matrix4();
    if (q) {
      const qq = new THREE.Quaternion(q[0], q[1], q[2], q[3]).normalize();
      // turn about the part's middle, so it stays where it was
      const c = this.modelCenter;
      next.makeTranslation(c.x, c.y, c.z).multiply(new THREE.Matrix4().makeRotationFromQuaternion(qq)).multiply(new THREE.Matrix4().makeTranslation(-c.x, -c.y, -c.z));
    }
    if (next.equals(this.pose)) return;
    this.pose.copy(next);
    this.applyPart();
    if (!this.prepared) return;
    this.center.copy(this.modelCenter).applyMatrix4(this.pose);
    this.buildGrid(this.posedBox());
    this.drawBed();
    // keep looking from the same side, at the part where it now sits
    this.fit(true, this.camera.position.clone().sub(this.controls.target).normalize());
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
    if (name === "USE_DISP")
      for (const m of [this.pickMat, this.probeMat]) {
        const pd = (m.defines ??= {});
        const had = name in pd;
        if (on) pd[name] = "";
        else delete pd[name];
        if (had !== on) m.needsUpdate = true;
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

  /** read every colour the 3D view uses from the design tokens (call again when the theme changes) */
  applyTheme() {
    this.material.color.copy(color(tok("--part")));
    this.uniforms.uHoverColor.value.copy(color(tok("--part-hover")));
    this.uniforms.uNan.value.copy(color(tok("--nan")));
    this.hemi.color.copy(color(tok("--viewer-light-sky")));
    this.hemi.groundColor.copy(color(tok("--viewer-light-ground")));
    this.hemi.intensity = tokNum("--viewer-light-sky-intensity", 1.3);
    this.sun.color.copy(color(tok("--viewer-light-key")));
    this.sun.intensity = tokNum("--viewer-light-key-intensity", 1.5);
    this.fill.color.copy(color(tok("--viewer-light-key")));
    this.fill.intensity = 0.45;
    const op = tokNum("--viewer-grid-opacity", 0.6);
    const [minor, major] = this.gridMats;
    if (minor && major) {
      minor.color.copy(color(tok("--viewer-grid")));
      major.color.copy(color(tok("--viewer-grid-major")));
      minor.opacity = op * 0.6;
      major.opacity = op;
    }
    this.request();
  }

  /** a ground grid under the part, in round millimetre squares, every fifth line stronger */
  private buildGrid(box: THREE.Box3) {
    for (const c of [...this.grid.children]) {
      this.grid.remove(c);
      (c as THREE.LineSegments).geometry.dispose();
    }
    for (const m of this.gridMats) m.dispose();
    const size = box.getSize(new THREE.Vector3());
    const span = Math.max(size.x, size.y) * 1.5 || 10;
    const step = gridStep(span, 24);
    const half = Math.ceil(span / 2 / step) * step;
    const cx = Math.round((box.min.x + box.max.x) / 2 / step) * step;
    const cy = Math.round((box.min.y + box.max.y) / 2 / step) * step;
    const z = box.min.z - Math.max(size.z, span) * 0.002;
    const minor: number[] = [];
    const major: number[] = [];
    const n = Math.round(half / step);
    for (let i = -n; i <= n; i++) {
      const list = i % 5 === 0 ? major : minor;
      const v = i * step;
      list.push(cx + v, cy - half, z, cx + v, cy + half, z);
      list.push(cx - half, cy + v, z, cx + half, cy + v, z);
    }
    this.gridMats = [minor, major].map(() => new THREE.LineBasicMaterial({ transparent: true, depthWrite: false }));
    [minor, major].forEach((pts, k) => {
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", new THREE.Float32BufferAttribute(pts, 3));
      const l = new THREE.LineSegments(g, this.gridMats[k]);
      l.renderOrder = -1;
      this.grid.add(l);
    });
    this.applyTheme();
    this.onGrid(step);
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
    // the safety scale is made of theme colours: a new theme means a new table
    const key = `${scale.map}:${paletteStamp()}`;
    if (!old || old.userData.key !== key) {
      old?.dispose();
      const tex = new THREE.DataTexture(lut(scale.map, 256), 256, 1, THREE.RGBAFormat, THREE.UnsignedByteType);
      tex.colorSpace = THREE.SRGBColorSpace;
      tex.magFilter = THREE.LinearFilter;
      tex.minFilter = THREE.LinearFilter;
      tex.userData.key = key;
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
  private fillAttr(name: "fval" | "disp" | "rmark", values: Float32Array, size: number) {
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
      h.userData.arrow = { start: start.clone(), dir: dir.clone(), len };
      this.overlays.add(h);
    }
    this.bed = o.plate ? { color: o.plate.color } : null;
    this.drawBed();
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

  private bed: { color: string } | null = null;
  /** the printer's bed: a flat tinted sheet just under the part as it sits now */
  private drawBed() {
    for (const c of [...this.bedGroup.children]) {
      this.bedGroup.remove(c);
      c.traverse((x) => {
        const m = x as THREE.Mesh;
        m.geometry?.dispose();
        (m.material as THREE.Material | undefined)?.dispose?.();
      });
    }
    if (this.bed && this.prepared) {
      const box = this.posedBox();
      const size = box.getSize(new THREE.Vector3());
      const span = Math.max(size.x, size.y) * 1.4 || 10;
      const z = box.min.z - Math.max(size.z, span) * 0.003;
      const col = color(this.bed.color);
      const sheet = new THREE.Mesh(
        new THREE.PlaneGeometry(span, span),
        new THREE.MeshBasicMaterial({ color: col, transparent: true, opacity: 0.14, side: THREE.DoubleSide, depthWrite: false }),
      );
      sheet.position.set((box.min.x + box.max.x) / 2, (box.min.y + box.max.y) / 2, z);
      const edge = new THREE.LineSegments(new THREE.EdgesGeometry(sheet.geometry), new THREE.LineBasicMaterial({ color: col, transparent: true, opacity: 0.7 }));
      edge.position.copy(sheet.position);
      sheet.renderOrder = edge.renderOrder = -1;
      this.bedGroup.add(sheet, edge);
    }
    this.request();
  }

  /** show a test as it runs (null: still) */
  setLive(l: LiveShow | null) {
    this.live = l && (l.spin || l.flow || l.pulse) ? l : null;
    this.liveT0 = performance.now();
    this.lastLive = 0;
    if (this.flowLines) {
      this.world.remove(this.flowLines);
      this.flowLines.geometry.dispose();
      (this.flowLines.material as THREE.Material).dispose();
      this.flowLines = null;
    }
    const f = this.live?.flow;
    if (f) {
      const N = f.kind === "through" ? (coarsePointer() ? 140 : 260) : coarsePointer() ? 220 : 420;
      this.flowP = new Float32Array(N * 3);
      this.flowAge = new Float32Array(N);
      const g = new THREE.BufferGeometry();
      g.setAttribute("position", new THREE.BufferAttribute(new Float32Array(N * 6), 3));
      g.setAttribute("color", new THREE.BufferAttribute(new Float32Array(N * 8), 4));
      const mat = new THREE.LineBasicMaterial({ vertexColors: true, transparent: true, depthWrite: false });
      this.flowLines = new THREE.LineSegments(g, mat);
      this.flowLines.frustumCulled = false;
      this.flowLines.renderOrder = 5;
      this.flowLines.matrixAutoUpdate = false;
      this.world.add(this.flowLines);
      for (let i = 0; i < N; i++) this.spawn(i, true);
    }
    if (!this.live) {
      this.applyPart();
      this.pulseArrows(1);
    }
    this.request();
  }

  /** a stream particle back to the start (anywhere along the way the first time) */
  private spawn(i: number, anywhere: boolean) {
    const f = this.live?.flow;
    const P = this.flowP!;
    if (!f) return;
    const R = f.radius;
    const [x0, x1] = FLOW_SPAN[f.kind];
    const span = x1 - x0;
    const u = anywhere ? Math.random() : 0;
    if (f.kind === "through") {
      // a stream tube a bit wider than the disc upstream, as far as the disc downstream
      const r = R * Math.sqrt(Math.random());
      const a = Math.random() * Math.PI * 2;
      P[i * 3] = (x0 + u * span) * R;
      P[i * 3 + 1] = r * Math.cos(a);
      P[i * 3 + 2] = r * Math.sin(a);
    } else {
      P[i * 3] = (x0 + u * span) * R;
      P[i * 3 + 1] = (Math.random() * 2 - 1) * 1.3 * R;
      P[i * 3 + 2] = (Math.random() * 2 - 1) * 1.3 * R;
    }
    this.flowAge![i] = anywhere ? Math.random() : 0;
  }

  /** move what moves while a test runs; true while there is something to draw next frame */
  private stepLive(): boolean {
    const l = this.live;
    if (!l) return false;
    const now = performance.now();
    const t = (now - this.liveT0) / 1000;
    // reduced motion: one still picture of the test (streaks in place, part not turning), no frame loop
    const still = reduceMotion();
    const slow = still ? 0 : 1;
    if (l.spin) {
      const c = new THREE.Vector3(...l.spin.center);
      const ax = new THREE.Vector3(...l.spin.axis).normalize();
      const turn = new THREE.Matrix4().makeTranslation(c.x, c.y, c.z).multiply(new THREE.Matrix4().makeRotationAxis(ax, t * 2.4 * slow)).multiply(new THREE.Matrix4().makeTranslation(-c.x, -c.y, -c.z));
      this.applyPart(turn);
    }
    if (l.pulse) this.pulseArrows(1 + 0.12 * Math.sin(t * 5 * slow));
    if (l.flow && this.flowLines) this.stepFlow(l.flow, Math.min(0.05, (now - (this.lastLive || now)) / 1000) * slow);
    this.lastLive = now;
    return !still;
  }

  private pulseArrows(k: number) {
    for (const c of this.overlays.children) {
      const a = c.userData.arrow as { start: THREE.Vector3; dir: THREE.Vector3; len: number } | undefined;
      if (!a) continue;
      // grow from the tip backwards: the tip stays on the face it pushes
      c.position.copy(a.start).addScaledVector(a.dir, a.len * (1 - k));
      c.scale.setScalar(k);
    }
  }

  /** move the stream: potential flow round a ball, or a propeller's stream tube (contracting, speeding up, swirling) */
  private stepFlow(f: NonNullable<LiveShow["flow"]>, dt: number) {
    const P = this.flowP!, age = this.flowAge!;
    const N = age.length;
    const R = f.radius;
    // the stream's own frame: x along the flow
    const d = new THREE.Vector3(...f.dir).normalize();
    const a = Math.abs(d.z) < 0.9 ? new THREE.Vector3(0, 0, 1) : new THREE.Vector3(1, 0, 0);
    const e1 = new THREE.Vector3().crossVectors(d, a).normalize();
    const e2 = new THREE.Vector3().crossVectors(d, e1).normalize();
    const c = new THREE.Vector3(...f.center).applyMatrix4(this.pose);
    const dW = d.clone().transformDirection(this.pose);
    const e1W = e1.clone().transformDirection(this.pose);
    const e2W = e2.clone().transformDirection(this.pose);
    const pos = this.flowLines!.geometry.getAttribute("position") as THREE.BufferAttribute;
    const col = this.flowLines!.geometry.getAttribute("color") as THREE.BufferAttribute;
    const rgb = color(f.color);
    const U = R * 0.9; // shown speed: about a part's size each second
    const ball = R * 0.7;
    for (let i = 0; i < N; i++) {
      let x = P[i * 3]!, y = P[i * 3 + 1]!, z = P[i * 3 + 2]!;
      let vx: number, vy: number, vz: number;
      if (f.kind === "through") {
        // momentum theory: the air leaves at twice the speed it arrives, the tube narrows to 0.7 of
        // its width, and it swirls behind the disc. x runs the way the air moves.
        const w = 0.45 * R;
        const s0 = Math.tanh(x / w);
        const speed = U * (1.5 + 0.5 * s0);
        const r0 = Math.hypot(y, z) / (0.85 - 0.15 * s0);
        const ang = Math.atan2(z, y) + ((1 + s0) / 2) * 1.6 * dt;
        x += speed * dt;
        const s1 = Math.tanh(x / w);
        const r1 = r0 * (0.85 - 0.15 * s1);
        const ny = r1 * Math.cos(ang), nz = r1 * Math.sin(ang);
        vx = speed;
        vy = (ny - y) / (dt || 1);
        vz = (nz - z) / (dt || 1);
        y = ny;
        z = nz;
        if (x > FLOW_SPAN.through[1] * R) this.spawn(i, false), (x = P[i * 3]!), (y = P[i * 3 + 1]!), (z = P[i * 3 + 2]!);
      } else {
        // uniform stream past a ball: u = U d + (U a³ / 2)(d / r³ − 3 (d·r) r / r⁵)
        const r2 = x * x + y * y + z * z;
        const r = Math.sqrt(r2) || 1e-9;
        const k = (ball ** 3 / 2) / (r2 * r);
        const dot = x;
        vx = U * (1 + k * (1 - (3 * dot * x) / r2));
        vy = U * k * (-(3 * dot * y) / r2);
        vz = U * k * (-(3 * dot * z) / r2);
        x += vx * dt;
        y += vy * dt;
        z += vz * dt;
        if (x > FLOW_SPAN.around[1] * R || x * x + y * y + z * z < ball * ball * 0.95) this.spawn(i, false), (x = P[i * 3]!), (y = P[i * 3 + 1]!), (z = P[i * 3 + 2]!);
      }
      P[i * 3] = x;
      P[i * 3 + 1] = y;
      P[i * 3 + 2] = z;
      age[i] = Math.min(1, age[i]! + dt * 1.5);
      // a short streak behind each particle, fading in as it starts
      const sp = Math.hypot(vx, vy, vz) || 1;
      const L = R * (f.kind === "through" ? 0.1 : 0.16) * (sp / U);
      // fade in where the stream starts and out where it ends
      const [a0, a1] = FLOW_SPAN[f.kind];
      const at = (x / R - a0) / (a1 - a0);
      const fade = Math.min(age[i]!, Math.min(1, at / 0.2), Math.min(1, (1 - at) / 0.3));
      const tx = x - (vx / sp) * L, ty = y - (vy / sp) * L, tz = z - (vz / sp) * L;
      for (const [j, px, py, pz] of [[0, x, y, z], [1, tx, ty, tz]] as const) {
        pos.setXYZ(i * 2 + j, c.x + dW.x * px + e1W.x * py + e2W.x * pz, c.y + dW.y * px + e1W.y * py + e2W.y * pz, c.z + dW.z * px + e1W.z * py + e2W.z * pz);
        col.setXYZW(i * 2 + j, rgb.r, rgb.g, rgb.b, j === 0 ? 0.85 * Math.max(0, fade) : 0);
      }
    }
    pos.needsUpdate = true;
    col.needsUpdate = true;
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

  /** keep every [data-anchor="x,y,z"] element over its point (the weakest spot, pinned values) */
  private placeAnchors() {
    const box = this.anchors;
    if (!box) return;
    const r = this.renderer.domElement.getBoundingClientRect();
    const v = new THREE.Vector3();
    for (const el of box.querySelectorAll<HTMLElement>("[data-anchor]")) {
      const [x, y, z] = (el.dataset.anchor ?? "").split(",").map(Number);
      if (![x, y, z].every(Number.isFinite)) continue;
      v.set(x!, y!, z!).applyMatrix4(this.partM).project(this.camera);
      el.style.visibility = v.z > 1 || v.z < -1 ? "hidden" : "visible";
      el.style.transform = `translate(${((v.x + 1) / 2) * r.width}px, ${((1 - v.y) / 2) * r.height}px)`;
    }
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
    const target = new THREE.Vector3(...p).applyMatrix4(this.partM);
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
    this.renderer.setClearColor(new THREE.Color(), 0);
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

  /** the result's value and the 3D point under a pixel, or null off the part */
  probe(x: number, y: number): ProbeHit | null {
    const t = this.probeTarget;
    if (!t || !this.mesh || !this.geometry?.getAttribute("fval")) return null;
    const el = this.renderer.domElement;
    this.camera.setViewOffset(el.clientWidth, el.clientHeight, x, y, 1, 1);
    const prev = this.renderer.getRenderTarget();
    const read = (mode: number) => {
      this.probeMat.uniforms.uMode!.value = mode;
      this.renderer.setRenderTarget(t);
      this.renderer.setClearColor(new THREE.Color(), 0);
      this.renderer.clear();
      this.renderer.render(this.probeScene, this.camera);
      this.renderer.readRenderTargetPixels(t, 0, 0, 1, 1, this.probeBuf);
      return Array.from(this.probeBuf);
    };
    const [value, hit] = read(0);
    const pos = hit === 1 ? read(1) : null;
    this.renderer.setRenderTarget(prev);
    this.camera.clearViewOffset();
    if (!pos || value === undefined) return null;
    const m = new THREE.Vector3(pos[0]!, pos[1]!, pos[2]!).applyMatrix4(this.partInv);
    return { value, pos: [m.x, m.y, m.z] };
  }

  /** the 3D point (mm) on the part under a pixel, or null */
  pointAt(x: number, y: number): Vec3 | null {
    const t = this.probeTarget;
    if (!t || !this.mesh) return null;
    const el = this.renderer.domElement;
    this.camera.setViewOffset(el.clientWidth, el.clientHeight, x, y, 1, 1);
    const prev = this.renderer.getRenderTarget();
    this.probeMat.uniforms.uMode!.value = 1;
    this.renderer.setRenderTarget(t);
    this.renderer.setClearColor(new THREE.Color(), 0);
    this.renderer.clear();
    this.renderer.render(this.probeScene, this.camera);
    this.renderer.readRenderTargetPixels(t, 0, 0, 1, 1, this.probeBuf);
    this.renderer.setRenderTarget(prev);
    this.camera.clearViewOffset();
    const b = this.probeBuf;
    if (b[3] !== 1) return null;
    const m = new THREE.Vector3(b[0]!, b[1]!, b[2]!).applyMatrix4(this.partInv);
    return [m.x, m.y, m.z];
  }

  /** colour the surface covered by areas picked by shape (triangle centre inside the shape) */
  setRegions(list: { mask: Float32Array | null; colors: { color: string; alpha: number }[] }) {
    const g = this.geometry;
    if (!g) return;
    if (!list.mask) {
      this.setDefine("USE_RMARK", false);
      this.request();
      return;
    }
    this.fillAttr("rmark", list.mask, 1);
    list.colors.slice(0, 8).forEach((c, i) => {
      const col = color(c.color);
      this.uniforms.uRCol.value[i]!.set(col.r, col.g, col.b, c.alpha);
    });
    this.setDefine("USE_RMARK", true);
    this.request();
  }

  get surface() {
    return this.prepared;
  }

  private local(e: PointerEvent) {
    const r = this.renderer.domElement.getBoundingClientRect();
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  }
  private pDown = (e: PointerEvent) => {
    this.down = this.local(e);
  };
  private pMove = (e: PointerEvent) => {
    if (!(this.pickable || this.hoverable || this.probing) || this.pointing || e.buttons || e.pointerType === "touch") return;
    this.hoverQueued = this.local(e);
    this.request();
  };
  private pUp = (e: PointerEvent) => {
    const d = this.down;
    this.down = null;
    if (!d) return;
    const p = this.local(e);
    if (Math.hypot(p.x - d.x, p.y - d.y) > 5) return;
    if (this.pointing) {
      const at = this.pointAt(p.x, p.y);
      if (at) this.onPoint(at);
      return;
    }
    if (this.probing) {
      const hit = this.probe(p.x, p.y);
      if (hit) this.onProbePin(hit);
      return;
    }
    if (!this.pickable) return;
    const id = this.pick(p.x, p.y);
    if (id >= 0) this.onPick(id);
    this.request();
  };
  private pLeave = () => {
    this.hoverQueued = null;
    if (this.probing) this.onProbe(null);
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
    this.setLive(null);
    this.bed = null;
    this.drawBed();
    this.setSlices([]);
    this.geometry?.dispose();
    this.material.dispose();
    this.pickMat.dispose();
    this.pickTarget.dispose();
    this.probeMat.dispose();
    this.probeTarget?.dispose();
    for (const m of this.gridMats) m.dispose();
    this.uniforms.uPatchTex.value?.dispose();
    this.uniforms.uLut.value?.dispose();
    this.renderer.dispose();
    this.renderer.forceContextLoss();
    el.remove();
  }
}
