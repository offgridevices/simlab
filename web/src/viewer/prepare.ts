// Getting a surface ready for the GPU. A model's surface gives a face group
// (patch) per triangle; the shader needs it per vertex, so vertices shared by
// two groups (along their border) are copied, one per group. Only border
// vertices are copied: a 2-million-triangle part grows by a few percent.
// All typed arrays, no per-vertex objects.
import type { Patch, SurfaceBuffers } from "../lib/types";

export interface Prepared {
  positions: Float32Array;
  normals: Float32Array;
  indices: Uint32Array;
  /** face group per vertex, as floats for the shader (exact below 2^24) */
  vertexPatch: Float32Array | null;
  /** for each vertex here, the vertex it was copied from (fields are per original vertex) */
  source: Uint32Array | null;
  /** round faces that are holes (they face their own axis) */
  holes: Map<number, boolean>;
  triangles: number;
}

export function prepare(s: SurfaceBuffers, patches?: Patch[]): Prepared {
  const nv = s.positions.length / 3;
  const tris = s.indices.length / 3;
  if (!s.patch) return { positions: s.positions, normals: s.normals, indices: s.indices, vertexPatch: null, source: null, holes: new Map(), triangles: tris };

  const owner = new Int32Array(nv).fill(-1);
  const copies = new Map<number, number>(); // vertex * 2^21 + patch -> new vertex
  const extra: number[] = []; // original vertex of each copy
  const extraPatch: number[] = [];
  const indices = new Uint32Array(s.indices.length);
  const P = 2 ** 21;
  for (let t = 0; t < tris; t++) {
    const p = s.patch[t]!;
    for (let k = 0; k < 3; k++) {
      const v = s.indices[t * 3 + k]!;
      const o = owner[v]!;
      if (o === -1) {
        owner[v] = p;
        indices[t * 3 + k] = v;
      } else if (o === p) indices[t * 3 + k] = v;
      else {
        const key = v * P + p;
        let nvx = copies.get(key);
        if (nvx === undefined) {
          nvx = nv + extra.length;
          copies.set(key, nvx);
          extra.push(v);
          extraPatch.push(p);
        }
        indices[t * 3 + k] = nvx;
      }
    }
  }
  const total = nv + extra.length;
  let positions = s.positions;
  let normals = s.normals;
  let source: Uint32Array | null = null;
  if (extra.length) {
    positions = new Float32Array(total * 3);
    normals = new Float32Array(total * 3);
    positions.set(s.positions);
    normals.set(s.normals);
    source = new Uint32Array(total);
    for (let i = 0; i < nv; i++) source[i] = i;
    for (let i = 0; i < extra.length; i++) {
      const from = extra[i]!;
      const to = nv + i;
      source[to] = from;
      for (let k = 0; k < 3; k++) {
        positions[to * 3 + k] = s.positions[from * 3 + k]!;
        normals[to * 3 + k] = s.normals[from * 3 + k]!;
      }
    }
  }
  const vertexPatch = new Float32Array(total);
  for (let i = 0; i < nv; i++) vertexPatch[i] = Math.max(0, owner[i]!);
  for (let i = 0; i < extraPatch.length; i++) vertexPatch[nv + i] = extraPatch[i]!;

  return { positions, normals, indices, vertexPatch, source, holes: findHoles(positions, normals, vertexPatch, patches), triangles: tris };
}

/** a round face is a hole when its surface faces its own axis */
export function findHoles(pos: Float32Array, nrm: Float32Array, vpatch: Float32Array, patches?: Patch[]): Map<number, boolean> {
  const out = new Map<number, boolean>();
  if (!patches) return out;
  const cyl = new Map<number, Patch>();
  for (const p of patches) if ((p.kind === "cylinder" || p.kind === "cone") && p.axis) cyl.set(p.id, p);
  if (!cyl.size) return out;
  const sum = new Map<number, number>();
  for (let i = 0; i < vpatch.length; i++) {
    const p = cyl.get(vpatch[i]!);
    if (!p) continue;
    const [ax, ay, az] = p.axis!;
    const dx = pos[i * 3]! - p.center_mm[0], dy = pos[i * 3 + 1]! - p.center_mm[1], dz = pos[i * 3 + 2]! - p.center_mm[2];
    const along = dx * ax + dy * ay + dz * az;
    const rx = dx - ax * along, ry = dy - ay * along, rz = dz - az * along;
    const d = rx * nrm[i * 3]! + ry * nrm[i * 3 + 1]! + rz * nrm[i * 3 + 2]!;
    sum.set(p.id, (sum.get(p.id) ?? 0) + d);
  }
  for (const [id, s] of sum) out.set(id, s < 0);
  return out;
}

/** per-vertex values of the original surface, laid out for the prepared one */
export function remap(values: Float32Array, source: Uint32Array | null, stride = 1): Float32Array {
  if (!source) return values;
  const out = new Float32Array(source.length * stride);
  for (let i = 0; i < source.length; i++) for (let k = 0; k < stride; k++) out[i * stride + k] = values[source[i]! * stride + k]!;
  return out;
}

/** the box around the points */
export function bounds(pos: Float32Array): { min: [number, number, number]; max: [number, number, number] } {
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < pos.length; i += 3)
    for (let k = 0; k < 3; k++) {
      const v = pos[i + k]!;
      if (v < min[k]!) min[k] = v;
      if (v > max[k]!) max[k] = v;
    }
  return { min, max };
}
