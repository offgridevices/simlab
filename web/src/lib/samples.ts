// Example parts made in code: a three-blade 5-inch propeller, a 5-inch quad
// frame and a small motor mount drawn in inches. Demo mode serves them as
// the engine would (surface buffers with face groups), and the home screen's
// "try an example" chips upload them as STL files to the real engine.
import { ExtrudeGeometry, Path, Shape, Vector2 } from "three";
import type { Model, MountGuess, Patch, PatchKind, PropGuess, Vec3 } from "./types";

export interface SampleMesh {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
  /** face group per triangle */
  patch: Uint32Array;
  /** face group per vertex (every vertex belongs to exactly one group here) */
  vertexPatch: Uint32Array;
  patches: Patch[];
  suggestions: NonNullable<Model["suggestions"]>;
  /** is a point (mm) inside the part? Rough, for the demo's flow slices */
  inside(p: Vec3): boolean;
}

type PatchInfo = { kind: PatchKind; label?: string; normal?: Vec3; axis?: Vec3; radius_mm?: number };

class Builder {
  pos: number[] = [];
  idx: number[] = [];
  tri: number[] = [];
  vpatch: number[] = [];
  infos: PatchInfo[] = [];
  addPatch(info: PatchInfo): number {
    this.infos.push(info);
    return this.infos.length - 1;
  }
  v(x: number, y: number, z: number, patch: number): number {
    this.pos.push(x, y, z);
    this.vpatch.push(patch);
    return this.pos.length / 3 - 1;
  }
  t(a: number, b: number, c: number, patch: number) {
    this.idx.push(a, b, c);
    this.tri.push(patch);
  }
  quad(a: number, b: number, c: number, d: number, patch: number) {
    this.t(a, b, c, patch);
    this.t(a, c, d, patch);
  }
  finish(suggestions: SampleMesh["suggestions"], inside: SampleMesh["inside"], scale = 1): SampleMesh {
    const positions = new Float32Array(this.pos.length);
    for (let i = 0; i < positions.length; i++) positions[i] = this.pos[i]! * scale;
    const indices = new Uint32Array(this.idx);
    const patch = new Uint32Array(this.tri);
    const normals = vertexNormals(positions, indices);
    const area = new Float64Array(this.infos.length);
    const cen = new Float64Array(this.infos.length * 3);
    for (let t = 0; t < patch.length; t++) {
      const [a, b, c] = [indices[t * 3]!, indices[t * 3 + 1]!, indices[t * 3 + 2]!];
      const A = triArea(positions, a, b, c);
      const p = patch[t]!;
      area[p] = area[p]! + A;
      for (let k = 0; k < 3; k++) cen[p * 3 + k] = cen[p * 3 + k]! + (A * (positions[a * 3 + k]! + positions[b * 3 + k]! + positions[c * 3 + k]!)) / 3;
    }
    const patches: Patch[] = this.infos.map((info, id) => {
      const A = area[id] || 1;
      return {
        id,
        kind: info.kind,
        area_mm2: round(area[id]!, 3),
        center_mm: [round(cen[id * 3]! / A, 4), round(cen[id * 3 + 1]! / A, 4), round(cen[id * 3 + 2]! / A, 4)],
        ...(info.normal ? { normal: info.normal } : {}),
        ...(info.axis ? { axis: info.axis } : {}),
        ...(info.radius_mm ? { radius_mm: info.radius_mm * scale } : {}),
        ...(info.label ? { label: info.label } : {}),
      };
    });
    return { positions, normals, indices, patch, vertexPatch: new Uint32Array(this.vpatch), patches, suggestions, inside };
  }
}

const round = (v: number, sig: number) => Number(v.toPrecision(sig));

function triArea(p: Float32Array, a: number, b: number, c: number): number {
  const ux = p[b * 3]! - p[a * 3]!, uy = p[b * 3 + 1]! - p[a * 3 + 1]!, uz = p[b * 3 + 2]! - p[a * 3 + 2]!;
  const vx = p[c * 3]! - p[a * 3]!, vy = p[c * 3 + 1]! - p[a * 3 + 1]!, vz = p[c * 3 + 2]! - p[a * 3 + 2]!;
  return 0.5 * Math.hypot(uy * vz - uz * vy, uz * vx - ux * vz, ux * vy - uy * vx);
}

/** smooth normals from the triangles' winding, area-weighted */
export function vertexNormals(p: Float32Array, idx: Uint32Array): Float32Array {
  const n = new Float32Array(p.length);
  for (let t = 0; t < idx.length; t += 3) {
    const a = idx[t]! * 3, b = idx[t + 1]! * 3, c = idx[t + 2]! * 3;
    const ux = p[b]! - p[a]!, uy = p[b + 1]! - p[a + 1]!, uz = p[b + 2]! - p[a + 2]!;
    const vx = p[c]! - p[a]!, vy = p[c + 1]! - p[a + 1]!, vz = p[c + 2]! - p[a + 2]!;
    const nx = uy * vz - uz * vy, ny = uz * vx - ux * vz, nz = ux * vy - uy * vx;
    for (const v of [a, b, c]) {
      n[v] = n[v]! + nx;
      n[v + 1] = n[v + 1]! + ny;
      n[v + 2] = n[v + 2]! + nz;
    }
  }
  for (let i = 0; i < n.length; i += 3) {
    const l = Math.hypot(n[i]!, n[i + 1]!, n[i + 2]!) || 1;
    n[i] = n[i]! / l;
    n[i + 1] = n[i + 1]! / l;
    n[i + 2] = n[i + 2]! / l;
  }
  return n;
}

// ---------------------------------------------------------------- propeller

export interface PropOptions {
  blades?: number;
  /** tip radius, mm (5-inch: 63.5) */
  radius?: number;
  /** pitch, mm per turn (4.3 inch ≈ 109 mm) */
  pitch?: number;
  /** stations along each blade and points around each section: more is finer */
  stations?: number;
  around?: number;
  hubSegments?: number;
}

export function makePropeller(o: PropOptions = {}): SampleMesh {
  const B = o.blades ?? 3;
  const R = o.radius ?? 63.5;
  const P = o.pitch ?? 109;
  const NS = o.stations ?? 48;
  const NP = o.around ?? 36;
  const HS = o.hubSegments ?? 64;
  const hubR = 6.5;
  const hubH = 7;
  const bore = 2.5;
  const z0 = -hubH / 2;
  const z1 = hubH / 2;
  const b = new Builder();

  // hub: outside, top, bottom, the shaft hole
  const outer = b.addPatch({ kind: "cylinder", axis: [0, 0, 1], radius_mm: hubR, label: "Hub, outside" });
  const top = b.addPatch({ kind: "plane", normal: [0, 0, 1], label: "Hub, top face" });
  const bottom = b.addPatch({ kind: "plane", normal: [0, 0, -1], label: "Hub, bottom face (sits on the motor)" });
  const hole = b.addPatch({ kind: "cylinder", axis: [0, 0, 1], radius_mm: bore, label: `Shaft hole, ${bore * 2} mm across` });
  ring(b, hubR, z0, z1, HS, outer, false);
  ring(b, bore, z0, z1, HS, hole, true);
  annulus(b, bore, hubR, z1, HS, top, true);
  annulus(b, bore, hubR, z0, HS, bottom, false);

  const r0 = hubR - 1.2;
  for (let k = 0; k < B; k++) {
    const pid = b.addPatch({ kind: "other", label: `Blade ${k + 1}` });
    const phi = (k / B) * Math.PI * 2;
    const er: Vec3 = [Math.cos(phi), Math.sin(phi), 0];
    const et: Vec3 = [-Math.sin(phi), Math.cos(phi), 0];
    const rows: number[][] = [];
    for (let i = 0; i < NS; i++) {
      const s = i / (NS - 1);
      const r = r0 + (R - r0) * s;
      let c = 9 + 9 * Math.sin(Math.PI * (0.15 + 0.8 * s)) - 4 * s;
      if (s > 0.88) c *= Math.sqrt(Math.max(0.04, 1 - ((s - 0.88) / 0.12) ** 2));
      const tc = (0.13 - 0.07 * s) * c;
      const beta = Math.atan(P / (2 * Math.PI * Math.max(r, 8)));
      const d: Vec3 = [-Math.cos(beta) * et[0], -Math.cos(beta) * et[1], -Math.sin(beta)];
      const nrm: Vec3 = [-Math.sin(beta) * et[0], -Math.sin(beta) * et[1], Math.cos(beta)];
      const row: number[] = [];
      for (let j = 0; j < NP; j++) {
        const u = (j / NP) * Math.PI * 2;
        const x = 0.5 * (1 + Math.cos(u)); // 1 trailing edge → 0 leading edge → 1
        const yt = 5 * tc * (0.2969 * Math.sqrt(x) - 0.126 * x - 0.3516 * x * x + 0.2843 * x ** 3 - 0.1036 * x ** 4);
        const camber = 0.05 * c * 4 * x * (1 - x);
        const eta = camber + (u < Math.PI ? yt : -yt);
        const xi = (x - 0.35) * c;
        row.push(b.v(r * er[0] + xi * d[0] + eta * nrm[0], r * er[1] + xi * d[1] + eta * nrm[1], xi * d[2] + eta * nrm[2], pid));
      }
      rows.push(row);
    }
    for (let i = 0; i + 1 < NS; i++)
      for (let j = 0; j < NP; j++) {
        const j2 = (j + 1) % NP;
        b.quad(rows[i]![j]!, rows[i + 1]![j]!, rows[i + 1]![j2]!, rows[i]![j2]!, pid);
      }
    // close the tip
    const last = rows[NS - 1]!;
    let cx = 0, cy = 0, cz = 0;
    for (const v of last) {
      cx += b.pos[v * 3]!;
      cy += b.pos[v * 3 + 1]!;
      cz += b.pos[v * 3 + 2]!;
    }
    const tip = b.v(cx / NP, cy / NP, cz / NP, pid);
    for (let j = 0; j < NP; j++) b.t(last[j]!, tip, last[(j + 1) % NP]!, pid);
  }

  const prop: PropGuess = { axis: [0, 0, 1], center_mm: [0, 0, 0], radius_mm: R, blades: B };
  const inside = ([x, y, z]: Vec3) => {
    const rr = Math.hypot(x, y);
    if (rr < hubR && rr > bore && Math.abs(z) < hubH / 2) return true;
    return false;
  };
  return b.finish({ looks_like: "propeller", prop }, inside);
}

function ring(b: Builder, r: number, z0: number, z1: number, n: number, pid: number, inward: boolean) {
  const lo: number[] = [];
  const hi: number[] = [];
  for (let i = 0; i < n; i++) {
    const a = (i / n) * Math.PI * 2;
    lo.push(b.v(r * Math.cos(a), r * Math.sin(a), z0, pid));
    hi.push(b.v(r * Math.cos(a), r * Math.sin(a), z1, pid));
  }
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n;
    if (inward) b.quad(lo[i]!, hi[i]!, hi[j]!, lo[j]!, pid);
    else b.quad(lo[i]!, lo[j]!, hi[j]!, hi[i]!, pid);
  }
}

function annulus(b: Builder, r0: number, r1: number, z: number, n: number, pid: number, up: boolean) {
  const inn: number[] = [];
  const out: number[] = [];
  for (let i = 0; i < n; i++) {
    const a = (i / n) * Math.PI * 2;
    inn.push(b.v(r0 * Math.cos(a), r0 * Math.sin(a), z, pid));
    out.push(b.v(r1 * Math.cos(a), r1 * Math.sin(a), z, pid));
  }
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n;
    if (up) b.quad(inn[i]!, out[i]!, out[j]!, inn[j]!, pid);
    else b.quad(inn[i]!, inn[j]!, out[j]!, out[i]!, pid);
  }
}

// ---------------------------------------------------------------- flat parts (frame, mount)

interface Hole {
  x: number;
  y: number;
  r: number;
  label: string;
}
interface Region {
  x: number;
  y: number;
  r: number;
  label: string;
}

/** an outline with holes, extruded to a thickness, split into the faces a person would click */
function flatPart(outline: [number, number][], holes: Hole[], thickness: number, regions: Region[], bodyLabel: string, maxEdge = 6): { b: Builder; holeIds: number[]; regionIds: { top: number; bottom: number }[] } {
  const shape = new Shape(outline.map(([x, y]) => new Vector2(x, y)));
  for (const h of holes) {
    const p = new Path();
    p.absarc(h.x, h.y, h.r, 0, Math.PI * 2, true);
    shape.holes.push(p);
  }
  const geo = new ExtrudeGeometry(shape, { depth: thickness, bevelEnabled: false, curveSegments: 20 });
  const pos = geo.getAttribute("position");
  const b = new Builder();
  const bodyTop = b.addPatch({ kind: "plane", normal: [0, 0, 1], label: `${bodyLabel}, top` });
  const bodyBottom = b.addPatch({ kind: "plane", normal: [0, 0, -1], label: `${bodyLabel}, bottom` });
  const edge = b.addPatch({ kind: "other", label: "Outside edge" });
  const regionIds = regions.map((r) => ({
    top: b.addPatch({ kind: "plane", normal: [0, 0, 1], label: `${r.label}, top` }),
    bottom: b.addPatch({ kind: "plane", normal: [0, 0, -1], label: `${r.label}, underside` }),
  }));
  const holeIds = holes.map((h) => b.addPatch({ kind: "cylinder", axis: [0, 0, 1], radius_mm: h.r, label: h.label }));
  const zMid = thickness / 2;
  for (let t = 0; t < pos.count; t += 3) {
    const p = [0, 1, 2].map((k) => [pos.getX(t + k), pos.getY(t + k), pos.getZ(t + k) - zMid] as Vec3);
    const [a, bb, c] = p as [Vec3, Vec3, Vec3];
    const cx = (a[0] + bb[0] + c[0]) / 3;
    const cy = (a[1] + bb[1] + c[1]) / 3;
    const flat = Math.abs(a[2] - bb[2]) < 1e-6 && Math.abs(a[2] - c[2]) < 1e-6;
    let pid: number;
    if (flat) {
      const up = a[2] > 0;
      const ri = regions.findIndex((r) => Math.hypot(cx - r.x, cy - r.y) < r.r);
      pid = ri >= 0 ? (up ? regionIds[ri]!.top : regionIds[ri]!.bottom) : up ? bodyTop : bodyBottom;
    } else {
      const hi = holes.findIndex((h) => Math.abs(Math.hypot(cx - h.x, cy - h.y) - h.r) < 0.25);
      pid = hi >= 0 ? holeIds[hi]! : edge;
    }
    // flat faces come out of the triangulator as long slivers; split them so
    // a result field has points to vary across (the engine's meshes are finer still)
    if (flat) split(b, a, bb, c, pid, maxEdge);
    else {
      const v = p.map((q) => b.v(q[0], q[1], q[2], pid));
      b.t(v[0]!, v[1]!, v[2]!, pid);
    }
  }
  geo.dispose();
  return { b, holeIds, regionIds };
}

/** a triangle halved across its longest edge until no edge is longer than `max` */
function split(b: Builder, a: Vec3, c1: Vec3, c2: Vec3, pid: number, max: number, depth = 0) {
  const len = (p: Vec3, q: Vec3) => Math.hypot(p[0] - q[0], p[1] - q[1], p[2] - q[2]);
  const e = [len(a, c1), len(c1, c2), len(c2, a)];
  const m = Math.max(...e);
  if (depth >= 16 || m <= max) {
    b.t(b.v(...a, pid), b.v(...c1, pid), b.v(...c2, pid), pid);
    return;
  }
  const mid = (p: Vec3, q: Vec3): Vec3 => [(p[0] + q[0]) / 2, (p[1] + q[1]) / 2, (p[2] + q[2]) / 2];
  // keep the winding: each half lists its corners in the same order as the whole
  if (m === e[0]) {
    const h = mid(a, c1);
    split(b, a, h, c2, pid, max, depth + 1);
    split(b, h, c1, c2, pid, max, depth + 1);
  } else if (m === e[1]) {
    const h = mid(c1, c2);
    split(b, a, c1, h, pid, max, depth + 1);
    split(b, a, h, c2, pid, max, depth + 1);
  } else {
    const h = mid(c2, a);
    split(b, a, c1, h, pid, max, depth + 1);
    split(b, h, c1, c2, pid, max, depth + 1);
  }
}

/** a point inside a polygon (even-odd) */
function inPolygon(x: number, y: number, poly: [number, number][]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 1; i < poly.length; j = i++) {
    const [xi, yi] = poly[i]!;
    const [xj, yj] = poly[j]!;
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

export function makeFrame(): SampleMesh {
  const L = 110; // centre to motor
  const w = 14; // arm width
  const r0 = 20; // where the arms meet the body
  const R = 14; // motor pad radius
  const t = 6;
  const arms = [45, 135, 225, 315].map((d) => (d * Math.PI) / 180);
  const rot = (a: number, x: number, y: number): [number, number] => [x * Math.cos(a) - y * Math.sin(a), x * Math.sin(a) + y * Math.cos(a)];
  const clean = frameOutline(arms, L, w, r0, R, rot);
  const holes: Hole[] = [];
  const regions: Region[] = [];
  const mounts: { center: [number, number]; holes: number[] }[] = [];
  arms.forEach((a, k) => {
    const [cx, cy] = rot(a, L, 0);
    regions.push({ x: cx, y: cy, r: R - 0.01, label: `Motor pad ${k + 1}` });
    const ids: number[] = [];
    for (let h = 0; h < 4; h++) {
      const ang = a + Math.PI / 4 + (h * Math.PI) / 2;
      ids.push(holes.length);
      holes.push({ x: cx + 8 * Math.cos(ang), y: cy + 8 * Math.sin(ang), r: 1.6, label: `Motor ${k + 1} bolt hole, 3.2 mm across` });
    }
    holes.push({ x: cx, y: cy, r: 3, label: `Motor ${k + 1} shaft clearance, 6 mm across` });
    mounts.push({ center: [cx, cy], holes: ids });
  });
  for (const [sx, sy] of [
    [1, 1],
    [-1, 1],
    [-1, -1],
    [1, -1],
  ] as const)
    holes.push({ x: sx * 15.25, y: sy * 15.25, r: 1.6, label: "Flight-controller stack hole, 3.2 mm across" });
  const { b, holeIds } = flatPart(clean, holes, t, regions, "Body and arms");
  const motor_mounts: MountGuess[] = mounts.map((m) => ({
    center_mm: [round(m.center[0], 5), round(m.center[1], 5), t / 2],
    axis: [0, 0, 1],
    bolt_circle_mm: 16,
    patches: m.holes.map((i) => holeIds[i]!),
  }));
  const inside = ([x, y, z]: Vec3) => Math.abs(z) <= t / 2 && inPolygon(x, y, clean) && !holes.some((h) => Math.hypot(x - h.x, y - h.y) < h.r);
  return b.finish({ looks_like: "frame", motor_mounts }, inside);
}

function frameOutline(arms: number[], L: number, w: number, r0: number, R: number, rot: (a: number, x: number, y: number) => [number, number]): [number, number][] {
  const out: [number, number][] = [];
  const half = Math.asin(w / 2 / R);
  for (const a of arms) {
    out.push(rot(a, r0, -w / 2));
    // around the pad, counter-clockwise, from the arm's right edge to its left edge
    const start = -Math.PI + half;
    const end = Math.PI - half;
    const n = 32;
    for (let i = 0; i <= n; i++) {
      const th = start + ((end - start) * i) / n;
      out.push(rot(a, L + R * Math.cos(th), R * Math.sin(th)));
    }
    out.push(rot(a, r0, w / 2));
  }
  return out;
}

/** a 1.2 × 1.2 inch motor mount plate, drawn in inches (so it looks 1.2 mm across until the units are fixed) */
export function makeMount(): SampleMesh {
  const s = 0.6;
  const rc = 0.12;
  const outline: [number, number][] = [];
  for (const [cx, cy, a0] of [
    [s - rc, -s + rc, -Math.PI / 2],
    [s - rc, s - rc, 0],
    [-s + rc, s - rc, Math.PI / 2],
    [-s + rc, -s + rc, Math.PI],
  ] as const)
    for (let i = 0; i <= 8; i++) {
      const th = a0 + (i / 8) * (Math.PI / 2);
      outline.push([cx + rc * Math.cos(th), cy + rc * Math.sin(th)]);
    }
  const holes: Hole[] = [];
  for (let h = 0; h < 4; h++) {
    const ang = Math.PI / 4 + (h * Math.PI) / 2;
    holes.push({ x: 0.315 * Math.cos(ang), y: 0.315 * Math.sin(ang), r: 0.063, label: "" });
  }
  holes.push({ x: 0, y: 0, r: 0.12, label: "" });
  const { b, holeIds } = flatPart(outline, holes, 0.12, [], "Plate", 0.12);
  // labels come from the engine's own words once the size is known; leave them generic here
  for (const id of holeIds) b.infos[id]!.label = undefined;
  const inside = ([x, y, z]: Vec3) => Math.abs(z) <= 0.06 && inPolygon(x, y, outline);
  return b.finish(
    {
      looks_like: "part",
      motor_mounts: [{ center_mm: [0, 0, 0.06], axis: [0, 0, 1], bolt_circle_mm: 0.63, patches: holeIds.slice(0, 4) }],
    },
    inside,
  );
}

/** a mesh as a binary STL file */
export function toStl(m: Pick<SampleMesh, "positions" | "indices">, name = "simlab example"): Blob {
  const tris = m.indices.length / 3;
  const buf = new ArrayBuffer(84 + tris * 50);
  const dv = new DataView(buf);
  const head = new TextEncoder().encode(name.slice(0, 79));
  new Uint8Array(buf, 0, head.length).set(head);
  dv.setUint32(80, tris, true);
  let o = 84;
  const p = m.positions;
  for (let t = 0; t < tris; t++) {
    const a = m.indices[t * 3]! * 3, b = m.indices[t * 3 + 1]! * 3, c = m.indices[t * 3 + 2]! * 3;
    const ux = p[b]! - p[a]!, uy = p[b + 1]! - p[a + 1]!, uz = p[b + 2]! - p[a + 2]!;
    const vx = p[c]! - p[a]!, vy = p[c + 1]! - p[a + 1]!, vz = p[c + 2]! - p[a + 2]!;
    let nx = uy * vz - uz * vy, ny = uz * vx - ux * vz, nz = ux * vy - uy * vx;
    const l = Math.hypot(nx, ny, nz) || 1;
    nx /= l;
    ny /= l;
    nz /= l;
    for (const v of [nx, ny, nz]) {
      dv.setFloat32(o, v, true);
      o += 4;
    }
    for (const v of [a, b, c])
      for (let k = 0; k < 3; k++) {
        dv.setFloat32(o, p[v + k]!, true);
        o += 4;
      }
    o += 2;
  }
  return new Blob([buf], { type: "model/stl" });
}

export type SampleName = "propeller" | "frame" | "mount";
export const SAMPLES: Record<SampleName, { make: () => SampleMesh; file: string; project: string }> = {
  propeller: { make: () => makePropeller(), file: "example-propeller-5in.stl", project: "Example 5-inch propeller" },
  frame: { make: makeFrame, file: "example-quad-frame-5in.stl", project: "Example 5-inch quad frame" },
  mount: { make: makeMount, file: "example-motor-mount.stl", project: "Example motor mount" },
};
