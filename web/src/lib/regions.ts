// Areas picked by shape: the same rule as the engine (crates/core config.rs
// RegionShape::contains): a surface triangle belongs to a region when its
// centre lies inside the shape.
import { REGION_ID_BASE, type Region, type RegionShape, type Vec3 } from "./types";

export const isRegion = (id: number) => id >= REGION_ID_BASE;

export function contains(s: RegionShape, p: Vec3): boolean {
  if (s.type === "sphere") return Math.hypot(p[0] - s.center_mm[0], p[1] - s.center_mm[1], p[2] - s.center_mm[2]) <= s.radius_mm;
  if (s.type === "box") return [0, 1, 2].every((k) => p[k]! >= s.min_mm[k]! && p[k]! <= s.max_mm[k]!);
  const l = Math.hypot(...s.axis) || 1;
  const a: Vec3 = [s.axis[0] / l, s.axis[1] / l, s.axis[2] / l];
  const d: Vec3 = [p[0] - s.center_mm[0], p[1] - s.center_mm[1], p[2] - s.center_mm[2]];
  const along = d[0] * a[0] + d[1] * a[1] + d[2] * a[2];
  const radial = Math.hypot(d[0] - a[0] * along, d[1] - a[1] * along, d[2] - a[2] * along);
  return radial <= s.radius_mm && (s.half_length_mm == null || Math.abs(along) <= s.half_length_mm);
}

/**
 * For each vertex, the index+1 of the last region (in `shapes` order) whose
 * shape holds the centre of a triangle using that vertex; 0 for none. Typed
 * arrays only: a 2-million-triangle part takes a few tens of milliseconds.
 */
export function regionMask(positions: Float32Array, indices: Uint32Array, shapes: RegionShape[]): Float32Array {
  const mask = new Float32Array(positions.length / 3);
  if (!shapes.length) return mask;
  const c: Vec3 = [0, 0, 0];
  for (let t = 0; t < indices.length; t += 3) {
    const a = indices[t]! * 3, b = indices[t + 1]! * 3, d = indices[t + 2]! * 3;
    c[0] = (positions[a]! + positions[b]! + positions[d]!) / 3;
    c[1] = (positions[a + 1]! + positions[b + 1]! + positions[d + 1]!) / 3;
    c[2] = (positions[a + 2]! + positions[b + 2]! + positions[d + 2]!) / 3;
    for (let k = shapes.length - 1; k >= 0; k--)
      if (contains(shapes[k]!, c)) {
        mask[indices[t]!] = k + 1;
        mask[indices[t + 1]!] = k + 1;
        mask[indices[t + 2]!] = k + 1;
        break;
      }
  }
  return mask;
}

/** how many triangles a shape covers (to warn about an empty brush) */
export function coverage(positions: Float32Array, indices: Uint32Array, s: RegionShape): number {
  let n = 0;
  const c: Vec3 = [0, 0, 0];
  for (let t = 0; t < indices.length; t += 3) {
    const a = indices[t]! * 3, b = indices[t + 1]! * 3, d = indices[t + 2]! * 3;
    for (let k = 0; k < 3; k++) c[k] = (positions[a + k]! + positions[b + k]! + positions[d + k]!) / 3;
    if (contains(s, c)) n++;
  }
  return n;
}

/** the next free region id */
export function nextRegionId(regions: Region[] | undefined): number {
  return Math.max(REGION_ID_BASE - 1, ...(regions ?? []).map((r) => r.id)) + 1;
}

/** the middle of a region's shape */
export function regionCentre(s: RegionShape): Vec3 {
  if (s.type === "box") return [0, 1, 2].map((k) => (s.min_mm[k]! + s.max_mm[k]!) / 2) as Vec3;
  return s.center_mm;
}

/** "Area around a click, 6 mm across" */
export function regionLabel(r: Region): string {
  if (r.label) return r.label;
  const s = r.shape;
  if (s.type === "sphere") return `Area around a point, ${Math.round(s.radius_mm * 20) / 10} mm across`;
  if (s.type === "cylinder") return `Round area, ${Math.round(s.radius_mm * 20) / 10} mm across`;
  return "Box-shaped area";
}
