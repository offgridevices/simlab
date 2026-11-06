// Demo mode's stand-in for the solvers: hand-made formulas (beam bending,
// spinning-disc stress, a propeller's thrust coefficient, a wake behind a
// body) that give plausible numbers and fields in a few milliseconds. Not a
// simulation; it only has to look and behave like one.
import type { Accuracy, FieldInfo, Load, Material, Model, PropConfig, PropSummary, Region, Slice, StrengthConfig, StrengthSummary, Vec3, WindConfig, WindSummary } from "../lib/types";
import { regionCentre } from "../lib/regions";
import { SEA_LEVEL_AIR } from "../lib/types";
import type { SampleMesh } from "../lib/samples";
import { n as fmt } from "../lib/format";

const G = 9.80665;
const dot = (a: Vec3, b: Vec3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const norm = (a: Vec3): Vec3 => {
  const l = Math.hypot(...a) || 1;
  return [a[0] / l, a[1] / l, a[2] / l];
};

export interface FieldSet {
  infos: FieldInfo[];
  values: Record<string, Float32Array>;
}

function range(a: Float32Array): [number, number] {
  let lo = Infinity, hi = -Infinity;
  for (const v of a) {
    if (Number.isNaN(v)) continue;
    if (v < lo) lo = v;
    if (v > hi) hi = v;
  }
  return [lo, hi];
}
function info(name: string, label: string, unit: string, a: Float32Array, extra: Partial<FieldInfo> = {}): FieldInfo {
  const [min, max] = range(a);
  return { name, label, unit, min, max, scale: "linear", ...extra };
}

/** volume of a closed surface, mm³ */
export function volume(m: Pick<SampleMesh, "positions" | "indices">): number {
  let v = 0;
  const p = m.positions;
  for (let t = 0; t < m.indices.length; t += 3) {
    const a = m.indices[t]! * 3, b = m.indices[t + 1]! * 3, c = m.indices[t + 2]! * 3;
    v += p[a]! * (p[b + 1]! * p[c + 2]! - p[b + 2]! * p[c + 1]!) - p[a + 1]! * (p[b]! * p[c + 2]! - p[b + 2]! * p[c]!) + p[a + 2]! * (p[b]! * p[c + 1]! - p[b + 1]! * p[c]!);
  }
  return Math.abs(v / 6);
}

/** strength across a direction: weakest straight across the layers, strongest along them */
function strengthAlong(mat: Material, dir: Vec3, up: Vec3): number {
  const c2 = dot(norm(dir), norm(up)) ** 2;
  return 1 / Math.sqrt((1 - c2) / mat.uts_xy_mpa ** 2 + c2 / mat.uts_z_mpa ** 2);
}

const QUALITY_ELEMENTS = { draft: 60_000, normal: 180_000, fine: 520_000, custom: 250_000 } as const;

export function strength(mesh: SampleMesh, model: Model, cfg: StrengthConfig, mat: Material, unit = 1): { fields: FieldSet; summary: StrengthSummary } {
  const nv = mesh.positions.length / 3;
  const P = mesh.positions;
  const N = mesh.normals;
  const vm = new Float32Array(nv);
  const S = new Float32Array(nv); // strength in the stress direction at each point
  const uz = new Float32Array(nv);
  const ux = new Float32Array(nv);
  const uy = new Float32Array(nv);
  const up = cfg.print_up;
  const accel = cfg.loads.filter((l): l is Extract<Load, { type: "acceleration" }> => l.type === "acceleration").reduce<Vec3>((s, l) => [s[0] + l.g[0], s[1] + l.g[1], s[2] + l.g[2]], [0, 0, 0]);
  const motors = cfg.loads.filter((l): l is Extract<Load, { type: "motor" }> => l.type === "motor");
  const spinRpm = Math.max(0, ...cfg.loads.map((l) => (l.type === "spin" || l.type === "prop_aero" ? l.rpm : 0)));
  const aeroRpm = Math.max(0, ...cfg.loads.map((l) => (l.type === "prop_aero" ? l.rpm : 0)));
  const looks = model.suggestions?.looks_like ?? "part";
  let where = "near where it is held";
  let rpmLoad = 0;

  if (looks === "frame") {
    const L = 110, w = 14, r0 = 20, t = 6;
    const Z = (w * t * t) / 6;
    const I = (w * t ** 3) / 12;
    const arms = [45, 135, 225, 315].map((d) => (d * Math.PI) / 180);
    // the force at each motor pad, out of the plane (thrust up, plus the motor's own weight under the acceleration)
    const padForce = arms.map((a) => {
      const pad: Vec3 = [L * Math.cos(a), L * Math.sin(a), 0];
      let f = 0;
      for (const mo of motors) {
        const centre = patchCentre(mesh, mo.patches, cfg.regions);
        const near = centre ? Math.hypot(centre[0] - pad[0], centre[1] - pad[1]) < 30 : true;
        if (!near) continue;
        const share = centre ? 1 : 1 / 4;
        f += share * (mo.thrust_n * dot(norm(mo.axis), [0, 0, 1]) + ((mo.motor_mass_g ?? 0) / 1000) * G * accel[2]);
      }
      // the arm's own mass (about 9 g) under the acceleration
      f += 0.009 * G * accel[2] * 0.5;
      const inPlane = Math.hypot(accel[0], accel[1]) * (0.009 * G + motors.reduce((s, m) => s + (m.motor_mass_g ?? 0) / 4000, 0) * G);
      return { f, inPlane };
    });
    for (let i = 0; i < nv; i++) {
      const x = P[i * 3]!, y = P[i * 3 + 1]!, z = P[i * 3 + 2]!;
      let best = 0, bestDot = -Infinity;
      arms.forEach((a, k) => {
        const d = x * Math.cos(a) + y * Math.sin(a);
        if (d > bestDot) {
          bestDot = d;
          best = k;
        }
      });
      const a = bestDot;
      const dir: Vec3 = [Math.cos(arms[best]!), Math.sin(arms[best]!), 0];
      const { f, inPlane } = padForce[best]!;
      const lever = Math.max(0, L - Math.max(a, r0));
      const M = Math.abs(f) * lever;
      const kt = 1 + 0.6 * Math.exp(-Math.max(0, a - r0) / 5);
      const across = Math.min(1, Math.abs(z) / (t / 2));
      let s = (M / Z) * kt * (0.25 + 0.75 * across) + ((inPlane * lever) / ((t * w * w) / 6)) * 0.7;
      if (a < r0) s *= 0.25 + 0.75 * Math.max(0, (a - r0 + 12) / 12);
      const vp = mesh.vertexPatch[i]!;
      // bolt holes in the motor pads concentrate stress; the stack holes sit in the quiet middle
      if (mesh.patches[vp]?.kind === "cylinder" && a > L - 20) s *= 1.7;
      vm[i] = s + 0.4;
      S[i] = strengthAlong(mat, dir, up);
      const ap = Math.max(0, a - r0), Lp = L - r0;
      const defl = a > r0 ? (f * ap * ap * (3 * Lp - Math.min(ap, Lp))) / (6 * mat.e_xy_mpa * I) : 0;
      uz[i] = defl;
    }
    where = "at the arm root";
  } else if (looks === "propeller") {
    const R = (model.suggestions?.prop?.radius_mm ?? 63.5) / unit;
    const hubR = 6.5, r0 = hubR - 1.2;
    const blades = model.suggestions?.prop?.blades ?? 3;
    const omega = (spinRpm * 2 * Math.PI) / 60;
    const nps = aeroRpm / 60;
    const thrust = 0.11 * 1.204 * nps * nps * (2 * R / 1000) ** 4; // N, whole prop
    const Tb = thrust / blades;
    rpmLoad = spinRpm;
    for (let i = 0; i < nv; i++) {
      const x = P[i * 3]!, y = P[i * 3 + 1]!;
      const r = Math.hypot(x, y);
      const s = Math.min(1, Math.max(0, (r - r0) / (R - r0)));
      const c = 9 + 9 * Math.sin(Math.PI * (0.15 + 0.8 * s)) - 4 * s;
      const tc = (0.13 - 0.07 * s) * c;
      const sc = (mat.density * omega * omega * ((R / 1000) ** 2 - (r / 1000) ** 2)) / 2 / 1e6 * 0.55;
      const M = Tb * Math.max(0, 0.7 * R - r);
      const sb = M / ((c * tc * tc) / 6);
      const face = Math.sqrt(Math.abs(N[i * 3 + 2]!));
      let sv = sc + sb * face;
      if (r < hubR) sv = 0.12 * (sc + Tb * 0.7 * R / ((14 * 2.4 * 2.4) / 6)) + 0.3;
      vm[i] = sv + 0.2;
      S[i] = strengthAlong(mat, [x / (r || 1), y / (r || 1), 0], up);
      uz[i] = r > hubR ? ((Tb * 1e3 * s * s * (R / 63.5) ** 3) / mat.e_xy_mpa) * 0.12 : 0;
    }
    where = "at the blade root";
  } else {
    // anything else: stress spreads out from where the loads meet the holds
    const held = cfg.support.type === "fixed" ? patchCentre(mesh, cfg.support.patches, cfg.regions) : null;
    const total = motors.reduce((s, m) => s + m.thrust_n, 0) + cfg.loads.reduce((s, l) => s + (l.type === "force" ? Math.hypot(...l.force_n) : 0), 0) + Math.hypot(...accel) * 0.05;
    const size = model.size_mm ? Math.max(...model.size_mm) : 30;
    const centre = held ?? model.bbox_mm?.min ?? [0, 0, 0];
    for (let i = 0; i < nv; i++) {
      const d = Math.hypot(P[i * 3]! * unit - centre[0], P[i * 3 + 1]! * unit - centre[1], P[i * 3 + 2]! * unit - centre[2]) / size;
      vm[i] = (total / (size * 0.3)) * 4 * Math.exp(-d * 3) + 0.2;
      S[i] = strengthAlong(mat, [1, 0, 0], up);
      uz[i] = (total / mat.e_xy_mpa) * d * d * size * 0.05;
    }
  }

  // the biggest pull (+) or push (−): bending pulls one side of the part and pushes the other
  const mp = new Float32Array(nv);
  for (let i = 0; i < nv; i++) mp[i] = vm[i]! * (N[i * 3 + 2]! < 0 ? 1.04 : -0.6);
  const sf = new Float32Array(nv);
  const fi = new Float32Array(nv);
  let crit = 0;
  for (let i = 0; i < nv; i++) {
    sf[i] = Math.min(100, S[i]! / Math.max(vm[i]!, 1e-6));
    fi[i] = (vm[i]! / S[i]!) ** 2;
    if (sf[i]! < sf[crit]!) crit = i;
  }
  const disp = new Float32Array(nv);
  for (let i = 0; i < nv; i++) disp[i] = Math.hypot(ux[i]!, uy[i]!, uz[i]!);
  const minSf = sf[crit]!;
  const verdict = minSf >= 2 ? "safe" : minSf >= 1 ? "marginal" : "breaks";
  const xf = minSf.toFixed(1);
  const critPoint: Vec3 = [P[crit * 3]! * unit, P[crit * 3 + 1]! * unit, P[crit * 3 + 2]! * unit];
  const breaksRpm = rpmLoad ? rpmLoad * Math.sqrt(minSf) : null;
  const breaksText = breaksRpm ? `at about ${fmt(Math.round(breaksRpm / 100) * 100)} rpm` : `at about ${Math.round(minSf * 100)}% of this load`;
  const headline =
    verdict === "safe"
      ? `Safe: ${xf}× stronger than this load needs`
      : verdict === "marginal"
        ? `Marginal: only ${xf}× stronger than this load needs`
        : `Breaks ${breaksText}, ${where}`;
  const elements = QUALITY_ELEMENTS[cfg.quality.type];
  const nodes = Math.round(elements * 1.55);
  const summary: StrengthSummary = {
    kind: "strength",
    headline,
    verdict,
    safety_factor: minSf,
    max_von_mises_mpa: range(vm)[1],
    max_principal_mpa: range(vm)[1] * 1.04,
    max_displacement_mm: range(disp)[1],
    critical_point_mm: critPoint,
    critical_patch: mesh.vertexPatch[crit] ?? null,
    breaks_at: { load_factor: minSf, rpm: breaksRpm, text: `Breaks ${breaksText}` },
    mass_g: (volume(mesh) * unit ** 3 * 1e-9 * mat.density * 1000),
    element_mm: { draft: 3, normal: 1.6, fine: 0.8, custom: 1.2 }[cfg.quality.type] * (looks === "propeller" ? 0.5 : 1),
    accuracy: STRENGTH_ACCURACY,
    elements,
    nodes,
    dofs: nodes * 3,
    solver: "Direct (sparse Cholesky), 12 threads",
    solve_seconds: elements / 40_000,
    mesh_seconds: elements / 90_000,
    reactions_n: cfg.support.type === "fixed" ? [0, 0, -motors.reduce((s, m) => s + m.thrust_n, 0)] : null,
    warnings: mat.process === "fdm" && Math.abs(dot(norm(up), [0, 0, 1])) < 0.9 ? ["Printed on its side, the layers carry this load across their weakest direction."] : [],
  };
  const fields: FieldSet = {
    values: { safety_factor: sf, von_mises: vm, max_principal: mp, displacement: disp, failure_index: fi, ux, uy, uz },
    // the same names, labels and units as crates/fea surface_fields
    infos: [
      info("von_mises", "Stress (von Mises)", "MPa", vm),
      info("max_principal", "Largest pull (max principal stress)", "MPa", mp, { diverging: true }),
      info("failure_index", "Failure index (1 = breaks)", "", fi),
      info("safety_factor", "Safety factor", "×", sf),
      info("displacement", "Movement", "mm", disp),
      info("ux", "Movement x", "mm", ux, { diverging: true }),
      info("uy", "Movement y", "mm", uy, { diverging: true }),
      info("uz", "Movement z", "mm", uz, { diverging: true }),
    ],
  };
  return { fields, summary };
}

function patchCentre(mesh: SampleMesh, ids: number[], regions: Region[] = []): Vec3 | null {
  // areas picked by shape count by their middle
  const fromRegions = ids.flatMap((i) => {
    const r = regions.find((x) => x.id === i);
    return r ? [regionCentre(r.shape)] : [];
  });
  const ps = ids.map((i) => mesh.patches[i]).filter((p) => !!p);
  if (!ps.length && fromRegions.length) return [0, 1, 2].map((k) => fromRegions.reduce((s, c) => s + c[k]!, 0) / fromRegions.length) as Vec3;
  if (!ps.length) return null;
  const a = ps.reduce((s, p) => s + p.area_mm2, 0) || 1;
  return [0, 1, 2].map((k) => ps.reduce((s, p) => s + p.center_mm[k]! * p.area_mm2, 0) / a) as Vec3;
}

// ---------------------------------------------------------------- wind tunnel

export interface SliceData {
  slice: Slice;
  values: Record<string, Float32Array>;
}

export function wind(mesh: SampleMesh, model: Model, cfg: WindConfig): { fields: FieldSet; summary: WindSummary; slices: SliceData[] } {
  const air = cfg.air ?? SEA_LEVEL_AIR;
  const U = cfg.speed_m_s;
  const w = norm(cfg.wind_dir);
  const nv = mesh.positions.length / 3;
  const N = mesh.normals;
  const spin = cfg.spin && cfg.spin.rpm > 0 ? cfg.spin : null;
  const R = (model.suggestions?.prop?.radius_mm ?? 63.5) / 1000;
  const induced = spin ? Math.sqrt((0.11 * air.density * (spin.rpm / 60) ** 2 * (2 * R) ** 4) / (2 * air.density * Math.PI * R * R)) : 0;
  const Ueff = Math.max(U, induced, 0.5);
  const q = 0.5 * air.density * Ueff * Ueff;
  const p = new Float32Array(nv);
  for (let i = 0; i < nv; i++) {
    const nd = -(N[i * 3]! * w[0] + N[i * 3 + 1]! * w[1] + N[i * 3 + 2]! * w[2]);
    const cp = nd > 0 ? nd * nd : -0.25 - 0.55 * (1 - Math.abs(nd));
    p[i] = q * cp;
  }
  const size = model.size_mm ?? [100, 100, 10];
  // frontal area: the box seen from the wind, about 40% filled
  const sx = size[0] / 1000, sy = size[1] / 1000, sz = size[2] / 1000;
  const frontal = (Math.abs(w[0]) * sy * sz + Math.abs(w[1]) * sx * sz + Math.abs(w[2]) * sx * sy) * 0.42;
  const cd = 1.15;
  const drag = U > 0 ? 0.5 * air.density * U * U * cd * frontal : 0;
  const lift = drag * 0.08;
  const thrust = spin ? 0.11 * air.density * (spin.rpm / 60) ** 2 * (2 * R) ** 4 * (1 - Math.min(0.9, U / Math.max(1, (spin.rpm / 60) * 2 * R * 0.9))) : null;
  const power = spin ? 0.052 * air.density * (spin.rpm / 60) ** 3 * (2 * R) ** 5 : null;
  const torque = spin && power ? power / ((spin.rpm * 2 * Math.PI) / 60) : null;
  const cells = { draft: 2_000_000, normal: 8_000_000, fine: 24_000_000, custom: 12_000_000 }[cfg.quality.type];
  const L = Math.max(...size) / 1000;
  const summary: WindSummary = {
    kind: "wind",
    headline: spin
      ? `Pushes ${fmt(((thrust ?? 0) / G) * 1000)} g at ${fmt(spin.rpm)} rpm, using ${fmt(power ?? 0)} W`
      : `Drag ${fmt(drag, 2)} N at ${fmt(U)} m/s, about ${fmt((drag / G) * 1000)} g of push`,
    force_n: [w[0] * drag, w[1] * drag, w[2] * drag + (thrust ?? 0)],
    moment_nm: [0, 0, torque ?? 0],
    drag_n: drag,
    lift_n: lift,
    side_n: drag * 0.02,
    cd: U > 0 ? cd : null,
    ref_area_mm2: frontal * 1e6,
    thrust_n: thrust,
    torque_nm: torque,
    power_w: power,
    reynolds: (Ueff * L) / air.kinematic_viscosity,
    cells,
    steps: 12_000,
    mlups: 2400,
    seconds: (cells * 12_000) / 2.4e9,
    converged: true,
    warnings: U > 40 ? ["Above 40 m/s the air starts to squeeze; results get less exact."] : [],
  };
  const fields: FieldSet = { values: { pressure: p }, infos: [info("pressure", "Surface pressure", "Pa", p, { diverging: true })] };
  return { fields, summary, slices: windSlices(mesh, model, w, U, induced, air.density, spin ? R * 1000 : 0) };
}

function windSlices(mesh: SampleMesh, model: Model, w: Vec3, U: number, induced: number, rho: number, diskR: number): SliceData[] {
  const bb = model.bbox_mm ?? { min: [-50, -50, -5] as Vec3, max: [50, 50, 5] as Vec3 };
  const c: Vec3 = [0, 1, 2].map((k) => (bb.min[k]! + bb.max[k]!) / 2) as Vec3;
  const ext = Math.max(...[0, 1, 2].map((k) => bb.max[k]! - bb.min[k]!)) * 1.6;
  const out: SliceData[] = [];
  const planes: { name: string; label: string; u: Vec3; v: Vec3 }[] = [
    { name: "mid_xy", label: "Flat slice through the middle", u: [1, 0, 0], v: [0, 1, 0] },
    { name: "mid_xz", label: "Upright slice, front to back", u: [1, 0, 0], v: [0, 0, 1] },
  ];
  for (const pl of planes) {
    const nu = 160, nv = 160;
    const origin: Vec3 = [0, 1, 2].map((k) => c[k]! - (pl.u[k]! + pl.v[k]!) * ext / 2) as Vec3;
    const inside = new Uint8Array(nu * nv);
    for (let j = 0; j < nv; j++)
      for (let i = 0; i < nu; i++) {
        const fu = (i + 0.5) / nu, fv = (j + 0.5) / nv;
        const pt: Vec3 = [0, 1, 2].map((k) => origin[k]! + (pl.u[k]! * fu + pl.v[k]! * fv) * ext) as Vec3;
        inside[j * nu + i] = mesh.inside(pt) ? 1 : 0;
      }
    // the wake: sweep downstream along the grid direction closest to the wind
    const wu = dot(w, pl.u), wv = dot(w, pl.v);
    const deficit = new Float32Array(nu * nv);
    const alongU = Math.abs(wu) >= Math.abs(wv);
    const s = alongU ? Math.sign(wu) || 1 : Math.sign(wv) || 1;
    const A = alongU ? nu : nv, B = alongU ? nv : nu;
    const at = (a: number, b: number) => (alongU ? b * nu + a : a * nu + b);
    for (let k = 1; k < A; k++) {
      const a = s > 0 ? k : A - 1 - k;
      const prev = s > 0 ? a - 1 : a + 1;
      for (let b = 0; b < B; b++) {
        if (inside[at(prev, b)]) {
          deficit[at(a, b)] = 1;
          continue;
        }
        const m = deficit[at(prev, b)]! * 0.5 + (deficit[at(prev, Math.max(0, b - 1))]! + deficit[at(prev, Math.min(B - 1, b + 1))]!) * 0.25;
        deficit[at(a, b)] = m * 0.985;
      }
    }
    const speed = new Float32Array(nu * nv);
    const pres = new Float32Array(nu * nv);
    const inPlaneWind = Math.hypot(wu, wv);
    for (let j = 0; j < nv; j++)
      for (let i = 0; i < nu; i++) {
        const k = j * nu + i;
        if (inside[k]) {
          speed[k] = NaN;
          pres[k] = NaN;
          continue;
        }
        // speed-up beside the part: near a solid cell, not in its wake
        let near = 0;
        for (let dj = -3; dj <= 3; dj++)
          for (let di = -3; di <= 3; di++) {
            const ii = i + di, jj = j + dj;
            if (ii >= 0 && jj >= 0 && ii < nu && jj < nv && inside[jj * nu + ii]) near = Math.max(near, 1 - Math.hypot(di, dj) / 4.5);
          }
        let v = U * (1 - 0.85 * deficit[k]!) * (1 + 0.3 * near * (1 - deficit[k]!)) * (0.25 + 0.75 * inPlaneWind);
        if (diskR > 0) {
          // a propeller's downwash: a jet below the disc, gentle inflow above
          const fu = (i + 0.5) / nu, fv = (j + 0.5) / nv;
          const pt: Vec3 = [0, 1, 2].map((kk) => origin[kk]! + (pl.u[kk]! * fu + pl.v[kk]! * fv) * ext) as Vec3;
          const rr = Math.hypot(pt[0], pt[1]);
          const below = -pt[2];
          if (pl.v[2] !== 0) {
            const contract = below > 0 ? 1 - 0.29 * (1 - Math.exp(-below / diskR)) : 1;
            const inJet = rr < diskR * contract ? Math.exp(-(((rr / (diskR * contract)) ** 6))) : 0;
            const gain = below > 0 ? 1 + (1 - Math.exp(-below / (diskR * 0.6))) : 0.5 * Math.exp(below / diskR);
            v += induced * inJet * gain * (rr > 6.5 ? 1 : 0.4);
          } else if (rr < diskR) v += induced * (rr > 6.5 ? 1 : 0.3);
        }
        speed[k] = v;
        pres[k] = 0.5 * rho * (U * U - v * v);
      }
    const fieldInfo = (name: string, label: string, unit: string, a: Float32Array) => {
      const [min, max] = range(a);
      return { name, label, unit, min, max };
    };
    out.push({
      slice: {
        name: pl.name,
        label: pl.label,
        origin_mm: origin,
        u_mm: pl.u.map((x) => x * ext) as Vec3,
        v_mm: pl.v.map((x) => x * ext) as Vec3,
        nu,
        nv,
        fields: [fieldInfo("speed", "Air speed", "m/s", speed), fieldInfo("pressure", "Air pressure", "Pa", pres)],
      },
      values: { speed, pressure: pres },
    });
  }
  return out;
}

// ---------------------------------------------------------------- propeller

export function prop(model: Model, cfg: PropConfig): PropSummary {
  const air = cfg.air ?? SEA_LEVEL_AIR;
  const R = (model.suggestions?.prop?.radius_mm ?? 63.5) / 1000;
  const D = 2 * R;
  const blades = cfg.blades ?? model.suggestions?.prop?.blades ?? 3;
  const steps = cfg.rpm_steps ?? 12;
  const speeds = cfg.airspeeds_m_s?.length ? cfg.airspeeds_m_s : [0];
  const points: PropSummary["points"] = [];
  const k = blades / 3;
  for (const V of speeds)
    for (let i = 0; i < steps; i++) {
      const rpm = cfg.rpm_min + ((cfg.rpm_max - cfg.rpm_min) * i) / Math.max(1, steps - 1);
      const nps = rpm / 60;
      const J = nps > 0 ? V / (nps * D) : 0;
      const ct = Math.max(0, 0.115 * k ** 0.6 * (1 - J / 0.95));
      const cp = Math.max(0.004, 0.055 * k ** 0.8 * (1 - 0.35 * J - 0.4 * J * J));
      const T = ct * air.density * nps * nps * D ** 4;
      const Pw = cp * air.density * nps ** 3 * D ** 5;
      points.push({
        rpm,
        airspeed_m_s: V,
        thrust_n: T,
        thrust_g: (T / G) * 1000,
        torque_nm: nps > 0 ? Pw / (2 * Math.PI * nps) : 0,
        power_w: Pw,
        ct,
        cp,
        efficiency: V > 0 && Pw > 0 ? Math.max(0, (T * V) / Pw) : null,
      });
    }
  const hover = points.filter((p) => p.airspeed_m_s === 0);
  const top = (hover.length ? hover : points).at(-1)!;
  const geometry: PropSummary["geometry"] = [];
  const r0 = 5.3;
  const Rm = R * 1000;
  for (let i = 0; i <= 10; i++) {
    const s = i / 10;
    const r = r0 + (Rm - r0) * s;
    let c = 9 + 9 * Math.sin(Math.PI * (0.15 + 0.8 * s)) - 4 * s;
    if (s > 0.88) c *= Math.sqrt(Math.max(0.04, 1 - ((s - 0.88) / 0.12) ** 2));
    geometry.push({ r_mm: r, chord_mm: c, pitch_deg: (Math.atan(109 / (2 * Math.PI * Math.max(r, 8))) * 180) / Math.PI });
  }
  return {
    kind: "prop",
    headline: `Pushes ${fmt(top.thrust_g)} g at ${fmt(top.rpm)} rpm${top.airspeed_m_s ? ` and ${fmt(top.airspeed_m_s)} m/s` : " hovering"}, using ${fmt(top.power_w)} W`,
    blades,
    radius_mm: Rm,
    hub_radius_mm: 6.5,
    rotation: "ccw",
    geometry,
    points,
    warnings: [],
    accuracy: PROP_ACCURACY,
  };
}

// the engine's own words (crates/engine solve.rs accuracy())
const STRENGTH_ACCURACY: Accuracy = {
  method: "Finite elements (quadratic tetrahedra) with print-direction-aware material and Tsai-Hill failure check",
  checked_against: "CalculiX 2.21 on the same mesh; textbook and NAFEMS benchmarks",
  typical_error:
    "Matches CalculiX within 0.004%; benchmarks within 0.03–0.8%. The real-world limit is how strong your print really is (layer bonding, infill, moisture), so treat a safety factor under 2 as risky.",
};
const PROP_ACCURACY: Accuracy = {
  method: "Blade element momentum theory with low-Reynolds airfoil data",
  checked_against: "University of Illinois measured propeller data",
  typical_error: "Thrust within about 10% on low-pitch props like E-v5 and on measured props with a known blade section, but it reads high on steeper blades: 15–19% on P-v2 and 23–35% on A-v2 and P-v1 in your OpenFOAM study. Torque within about 12% (power read 24% low on one measured prop). Use the wind tunnel for the precise number.",
};
