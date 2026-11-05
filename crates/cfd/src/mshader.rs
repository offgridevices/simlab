//! WGSL for the multi-level D3Q27 solver, generated per grid level so sizes
//! and relaxation times are compile-time constants.
//!
//! Streaming: pull, with two population sets per level (A/B). A step reads
//! the post-collision populations of the neighbours from the source set,
//! applies the wall conditions, collides and writes the cell's own
//! post-collision populations to the destination set. Which set is the
//! source comes from the per-dispatch uniform (`st.src`).
//!
//! Collision: cumulant-type central-moment collision. The deviatoric
//! second-order central moments relax with the viscous rate; the trace and
//! every higher-order cumulant relax to equilibrium in one step (rate 1), so
//! the post-collision central moments are those of a Gaussian with the
//! relaxed second moments. This is the cumulant method of Geier et al.
//! (2015) with all higher-order rates set to one: unconditionally stable at
//! very low viscosity without a turbulence model, and Galilean invariant.
//!
//! Levels: a finer level (half the cell size, two steps per coarse step) is
//! a node-aligned box inside its parent. Its outer layer of nodes are ghosts
//! filled from the parent's moments on the box surface, interpolated in
//! space (linear) and time (linear); the parent's nodes deeper than two
//! cells inside the box are restricted from the coincident fine nodes. The
//! non-equilibrium stress is rescaled between levels (Dupuis & Chopard
//! 2003) so the physical stress is continuous.
//!
//! Walls: Bouzidi linear interpolated bounce-back; Galilean-invariant
//! momentum exchange for forces. Used for still parts (spinning parts stay
//! on the D3Q19 solver in `shader`).

use crate::d3q27::{Q, REST, c, opp, slot, weight};

pub const RS: u32 = 24;
pub const R_MASK: u32 = 0;
pub const R_CELL: u32 = 1;
pub const R_Q: u32 = 2;
pub const R_FT: u32 = 15;
pub const MW: u32 = 12; // floats per moments node
pub const WG: u32 = 64;
pub const RED_CHUNK: u32 = 4096;
pub const MAX_POP_BUFS: usize = 6;
pub const FP16_SCALE: f64 = 32768.0;

/// Cell types (low 3 bits of the flag word); records index in the rest.
pub const T_FLUID: u32 = 0;
pub const T_SOLID: u32 = 1;
pub const T_FACE: u32 = 2;
pub const T_GHOST: u32 = 3;
pub const T_RESTRICT: u32 = 4;

#[derive(Clone, Debug)]
pub struct SliceDef {
    pub origin: [i32; 3],
    pub u: [i32; 3],
    pub v: [i32; 3],
    pub nu: u32,
    pub nv: u32,
}

#[derive(Clone, Debug)]
pub struct LevelShader {
    pub level: usize,
    pub dims: [u32; 3],
    pub periodic: [bool; 3],
    pub fp16: bool,
    pub dirs_per_buf: u32,
    pub tau0: f64,
    /// optional Smagorinsky constant squared (0: none; the collision is stable without)
    pub smag2: f64,
    pub sponge_lo: [f64; 3],
    pub sponge_hi: [f64; 3],
    pub sponge_tau: f64,
    pub sigma: f64,
    pub absorb_velocity: bool,
    pub total_p: bool,
    pub u_inf: [f64; 3],
    pub u_init: [f64; 3],
    pub grav: [f64; 3],
    /// moment reference / spin centre, this level's lattice coordinates
    pub center: [f64; 3],
    pub nrec: u32,
    /// child box (node range lo .. lo + n - 1 in this level's coordinates)
    pub child: Option<([i32; 3], [u32; 3])>,
    pub has_parent: bool,
    pub nface: u32,
    pub nghost: u32,
    pub nrestrict: u32,
    pub nvert: u32,
    pub vertex_offset: f64,
    pub npix: u32,
    pub hist_off: u32,
}

fn fl(x: f64) -> String {
    format!("f32({:e})", x as f32)
}
fn v3(a: [f64; 3]) -> String {
    format!("vec3<f32>({}, {}, {})", fl(a[0]), fl(a[1]), fl(a[2]))
}
fn nb_expr(d: [i32; 3]) -> String {
    let ax = |c: i32, s: &str| match c {
        1 => format!("{s}p"),
        -1 => format!("{s}m"),
        _ => s.to_string(),
    };
    format!("(({} * NY + {}) * NX + {})", ax(d[2], "z"), ax(d[1], "y"), ax(d[0], "x"))
}
const PRELUDE_NB: &str = "  let x = xyz.x; let y = xyz.y; let z = xyz.z;\n\
  let xp = select(x + 1u, 0u, x == NX - 1u); let xm = select(x - 1u, NX - 1u, x == 0u);\n\
  let yp = select(y + 1u, 0u, y == NY - 1u); let ym = select(y - 1u, NY - 1u, y == 0u);\n\
  let zp = select(z + 1u, 0u, z == NZ - 1u); let zm = select(z - 1u, NZ - 1u, z == 0u);\n";

impl LevelShader {
    fn n(&self) -> u64 {
        self.dims.iter().map(|&d| d as u64).product()
    }
    pub fn n_pop_bufs(&self) -> usize {
        Q.div_ceil(self.dirs_per_buf as usize)
    }
    fn dib(&self, b: usize) -> u64 {
        let per = self.dirs_per_buf as usize;
        per.min(Q - b * per) as u64
    }
    fn slot_expr(&self, i: usize, cell: &str, dst: bool) -> String {
        let per = self.dirs_per_buf as usize;
        let b = i / per;
        let off = (i % per) as u64 * self.n();
        let setsz = self.dib(b) * self.n();
        let set = if dst { "(1u - st.src)" } else { "st.src" };
        format!("pop{b}[{set} * {setsz}u + {off}u + {cell}]")
    }
    /// shifted population (f - w) from the source set
    fn ld(&self, i: usize, cell: &str) -> String {
        let e = self.slot_expr(i, cell, false);
        if self.fp16 { format!("(f32({e}) * INV_S)") } else { e }
    }
    fn st(&self, i: usize, cell: &str, v: &str, dst: bool) -> String {
        let e = self.slot_expr(i, cell, dst);
        if self.fp16 {
            format!("{e} = f16(sround({v} * FS, {cell} * 27u + {i}u));")
        } else {
            format!("{e} = {v};")
        }
    }

    pub fn source(&self) -> String {
        let mut s = String::new();
        if self.fp16 {
            s += "enable f16;\n";
        }
        let [nx, ny, nz] = self.dims;
        let n = self.n();
        s += &format!("const NX: u32 = {nx}u;\nconst NY: u32 = {ny}u;\nconst NZ: u32 = {nz}u;\nconst N: u32 = {n}u;\nconst WG: u32 = {WG}u;\nconst RS: u32 = {RS}u;\nconst MW: u32 = {MW}u;\n");
        s += &format!("const TAU0: f32 = {};\nconst SMAG2: f32 = {};\nconst SPONGE_TAU: f32 = {};\nconst SIGMA: f32 = {};\n", fl(self.tau0), fl(self.smag2), fl(self.sponge_tau), fl(self.sigma));
        s += &format!("const SP_LO: vec3<f32> = {};\nconst SP_HI: vec3<f32> = {};\n", v3(self.sponge_lo), v3(self.sponge_hi));
        s += &format!("const ABS_U: bool = {};\nconst TOTAL_P: bool = {};\n", self.absorb_velocity, self.total_p);
        s += &format!("const U_INF: vec3<f32> = {};\nconst U_INIT: vec3<f32> = {};\nconst GRAV: vec3<f32> = {};\nconst CENTER: vec3<f32> = {};\n", v3(self.u_inf), v3(self.u_init), v3(self.grav), v3(self.center));
        s += &format!("const NREC: u32 = {}u;\nconst NPART: u32 = {}u;\n", self.nrec, self.nrec.max(1).div_ceil(RED_CHUNK));
        let (clo, cn) = self.child.unwrap_or(([0; 3], [1; 3]));
        s += &format!(
            "const HAS_CHILD: bool = {};\nconst CH_LO: vec3<i32> = vec3<i32>({}, {}, {});\nconst CH_N: vec3<u32> = vec3<u32>({}u, {}u, {}u);\nconst CH_NODES: u32 = {}u;\n",
            self.child.is_some(),
            clo[0],
            clo[1],
            clo[2],
            cn[0],
            cn[1],
            cn[2],
            cn.iter().map(|&v| v as u64).product::<u64>()
        );
        // this level's nodes seen from the parent (all-even local nodes)
        let pn = [(nx - 1) / 2 + 1, (ny - 1) / 2 + 1, (nz - 1) / 2 + 1];
        s += &format!(
            "const HAS_PARENT: bool = {};\nconst PN: vec3<u32> = vec3<u32>({}u, {}u, {}u);\nconst CH_NODES_P: u32 = {}u;\n",
            self.has_parent,
            pn[0],
            pn[1],
            pn[2],
            pn.iter().map(|&v| v as u64).product::<u64>()
        );
        s += &format!("const NFACE: u32 = {}u;\nconst NGHOST: u32 = {}u;\nconst NRESTRICT: u32 = {}u;\n", self.nface, self.nghost, self.nrestrict);
        s += &format!("const NVERT: u32 = {}u;\nconst VOFF: f32 = {};\nconst NPIX: u32 = {}u;\nconst HIST_OFF: u32 = {}u;\n", self.nvert, fl(self.vertex_offset), self.npix, self.hist_off);
        s += &format!("const FS: f32 = {};\nconst INV_S: f32 = {};\n", fl(FP16_SCALE), fl(1.0 / FP16_SCALE));
        s += &format!("const PER: vec3<bool> = vec3<bool>({}, {}, {});\n", self.periodic[0], self.periodic[1], self.periodic[2]);
        s += &format!("const N_SPONGE: bool = {};\n", self.sponge_lo.iter().chain(self.sponge_hi.iter()).any(|&v| v > 0.0));

        let elem = if self.fp16 { "f16" } else { "f32" };
        for b in 0..self.n_pop_bufs() {
            s += &format!("@group(0) @binding({b}) var<storage, read_write> pop{b}: array<{elem}>;\n");
        }
        s += BINDINGS;
        s += &self.gen_access();
        s += &self.gen_math();
        s += COMMON;
        s += &self.gen_step();
        s += &self.gen_level_kernels();
        s
    }

    fn gen_access(&self) -> String {
        let mut s = String::new();
        // own post-collision populations (source set)
        s += "fn own(n: u32) -> array<f32, 27> {\n  var f: array<f32, 27>;\n";
        for i in 0..Q {
            s += &format!("  f[{i}] = {};\n", self.ld(i, "n"));
        }
        s += "  return f;\n}\n";
        // write a full post-collision state to the source set (refills, ghosts, faces)
        s += "fn put_src(n: u32, f: array<f32, 27>) {\n";
        for i in 0..Q {
            s += &format!("  {}\n", self.st(i, "n", &format!("f[{i}]"), false));
        }
        s += "}\n";
        s += "fn put_dst(n: u32, f: array<f32, 27>) {\n";
        for i in 0..Q {
            s += &format!("  {}\n", self.st(i, "n", &format!("f[{i}]"), true));
        }
        s += "}\n";
        // pull: population moving along c_i comes from n - c_i
        s += "fn pull(n: u32, xyz: vec3<u32>) -> array<f32, 27> {\n  var f: array<f32, 27>;\n";
        s += PRELUDE_NB;
        for i in 0..Q {
            let cv = c(i);
            let e = if i == REST { self.ld(i, "n") } else { self.ld(i, &nb_expr([-cv[0], -cv[1], -cv[2]])) };
            s += &format!("  f[{i}] = {e};\n");
        }
        s += "  return f;\n}\n";
        s
    }

    fn gen_math(&self) -> String {
        let mut s = String::new();
        // weights and velocities as constant arrays for loops
        let ws: Vec<String> = (0..Q).map(|i| fl(weight(i))).collect();
        s += &format!("const W: array<f32, 27> = array<f32, 27>({});\n", ws.join(", "));
        let cs: Vec<String> = (0..Q).map(|i| { let v = c(i); format!("vec3<f32>({}.0, {}.0, {}.0)", v[0], v[1], v[2]) }).collect();
        s += &format!("const CV: array<vec3<f32>, 27> = array<vec3<f32>, 27>({});\n", cs.join(", "));
        // moments of a shifted population set
        let sum = |coef: &dyn Fn([i32; 3]) -> i32| -> String {
            let mut t = String::new();
            for i in 0..Q {
                let k = coef(c(i));
                if k == 1 {
                    t += &format!(" + f[{i}]");
                } else if k == -1 {
                    t += &format!(" - f[{i}]");
                }
            }
            if t.is_empty() { "0.0".into() } else { format!("0.0{t}") }
        };
        s += "struct Mom { rho1: f32, j: vec3<f32>, s: array<f32, 6> }\nfn moments(f: array<f32, 27>) -> Mom {\n  var m: Mom;\n";
        s += &format!("  m.rho1 = {};\n", sum(&|_v| 1));
        s += &format!("  m.j = vec3<f32>({}, {}, {});\n", sum(&|v| v[0]), sum(&|v| v[1]), sum(&|v| v[2]));
        for (k, (a, b)) in [(0, 0), (1, 1), (2, 2), (0, 1), (0, 2), (1, 2)].iter().enumerate() {
            s += &format!("  m.s[{k}] = {};\n", sum(&|v| v[*a] * v[*b]));
        }
        s += "  return m;\n}\n";
        // product-form equilibrium, shifted
        s += "fn feq(rho: f32, u: vec3<f32>) -> array<f32, 27> {\n  var f: array<f32, 27>;\n";
        s += "  let ax = array<f32, 3>(0.5 * (1.0 / 3.0 + u.x * u.x - u.x), 2.0 / 3.0 - u.x * u.x, 0.5 * (1.0 / 3.0 + u.x * u.x + u.x));\n";
        s += "  let ay = array<f32, 3>(0.5 * (1.0 / 3.0 + u.y * u.y - u.y), 2.0 / 3.0 - u.y * u.y, 0.5 * (1.0 / 3.0 + u.y * u.y + u.y));\n";
        s += "  let az = array<f32, 3>(0.5 * (1.0 / 3.0 + u.z * u.z - u.z), 2.0 / 3.0 - u.z * u.z, 0.5 * (1.0 / 3.0 + u.z * u.z + u.z));\n";
        for i in 0..Q {
            let v = c(i);
            s += &format!("  f[{i}] = rho * ax[{}] * ay[{}] * az[{}] - {};\n", v[0] + 1, v[1] + 1, v[2] + 1, fl(weight(i)));
        }
        s += "  return f;\n}\n";
        // second-order Hermite non-equilibrium for a stress p (xx yy zz xy xz yz)
        s += "fn fneq(p: array<f32, 6>) -> array<f32, 27> {\n  var f: array<f32, 27>;\n  let tr = (p[0] + p[1] + p[2]) * (1.0 / 3.0);\n";
        for i in 0..Q {
            let v = c(i);
            let mut t = String::from("-tr");
            for (a, b, k) in [(0, 0, 0), (1, 1, 1), (2, 2, 2), (0, 1, 3), (0, 2, 4), (1, 2, 5)] {
                let m = v[a] * v[b] * if a == b { 1 } else { 2 };
                if m != 0 {
                    t += &format!(" + {m}.0 * p[{k}]");
                }
            }
            s += &format!("  f[{i}] = {} * ({t});\n", fl(4.5 * weight(i)));
        }
        s += "  return f;\n}\n";
        // Gaussian central moments -> populations (backward chimera transform)
        s += &self.gen_gauss();
        s
    }

    /// Post-collision populations (shifted) for density `rho`, velocity `u`,
    /// covariance (central second moments / rho) `sg` = (xx yy zz xy xz yz)
    /// and first central moments `m1` (forcing), all higher cumulants zero.
    fn gen_gauss(&self) -> String {
        let mut s = String::from("fn gauss(rho: f32, u: vec3<f32>, sg: array<f32, 6>, m1: vec3<f32>) -> array<f32, 27> {\n");
        s += "  let a = sg[0]; let b = sg[1]; let cc = sg[2]; let d = sg[3]; let e = sg[4]; let g = sg[5];\n";
        // central moments k[i][j][k] (powers of x, y, z), times rho
        let km = |i: usize, j: usize, k: usize| -> String {
            match (i, j, k) {
                (0, 0, 0) => "1.0".into(),
                (1, 0, 0) => "m1.x".into(),
                (0, 1, 0) => "m1.y".into(),
                (0, 0, 1) => "m1.z".into(),
                (2, 0, 0) => "a".into(),
                (0, 2, 0) => "b".into(),
                (0, 0, 2) => "cc".into(),
                (1, 1, 0) => "d".into(),
                (1, 0, 1) => "e".into(),
                (0, 1, 1) => "g".into(),
                (2, 2, 0) => "(a * b + 2.0 * d * d)".into(),
                (2, 0, 2) => "(a * cc + 2.0 * e * e)".into(),
                (0, 2, 2) => "(b * cc + 2.0 * g * g)".into(),
                (2, 1, 1) => "(a * g + 2.0 * d * e)".into(),
                (1, 2, 1) => "(b * e + 2.0 * d * g)".into(),
                (1, 1, 2) => "(cc * d + 2.0 * e * g)".into(),
                (2, 2, 2) => "(a * b * cc + 2.0 * (a * g * g + b * e * e + cc * d * d) + 8.0 * d * e * g)".into(),
                _ => "0.0".into(),
            }
        };
        for i in 0..3 {
            for j in 0..3 {
                for k in 0..3 {
                    s += &format!("  let k{i}{j}{k} = rho * {};\n", km(i, j, k));
                }
            }
        }
        // 1D backward transform helper inline: (k0, k1, k2) about w -> (f-, f0, f+)
        let back = |out: &str, k0: &str, k1: &str, k2: &str, w: &str| -> String {
            format!(
                "  let {out}r1 = {k1} + {w} * {k0}; let {out}r2 = {k2} + 2.0 * {w} * {k1} + {w} * {w} * {k0};\n\
                 \x20 let {out}m = 0.5 * ({out}r2 - {out}r1); let {out}o = {k0} - {out}r2; let {out}p = 0.5 * ({out}r2 + {out}r1);\n"
            )
        };
        let sfx = ["m", "o", "p"];
        // along z: for each (i,j) -> t_{i}{j}{cz}
        for i in 0..3 {
            for j in 0..3 {
                s += &back(&format!("z{i}{j}"), &format!("k{i}{j}0"), &format!("k{i}{j}1"), &format!("k{i}{j}2"), "u.z");
            }
        }
        // along y: for each (i, cz) from z{i}{j}{cz}
        for i in 0..3 {
            for cz in 0..3 {
                s += &back(&format!("y{i}{}", sfx[cz]), &format!("z{i}0{}", sfx[cz]), &format!("z{i}1{}", sfx[cz]), &format!("z{i}2{}", sfx[cz]), "u.y");
            }
        }
        // along x: for each (cy, cz) from y{i}{cz}{cy}
        for cy in 0..3 {
            for cz in 0..3 {
                s += &back(
                    &format!("x{}{}", sfx[cy], sfx[cz]),
                    &format!("y0{}{}", sfx[cz], sfx[cy]),
                    &format!("y1{}{}", sfx[cz], sfx[cy]),
                    &format!("y2{}{}", sfx[cz], sfx[cy]),
                    "u.x",
                );
            }
        }
        s += "  var f: array<f32, 27>;\n";
        for idx in 0..Q {
            let v = c(idx);
            let (cx, cy, cz) = ((v[0] + 1) as usize, (v[1] + 1) as usize, (v[2] + 1) as usize);
            s += &format!("  f[{idx}] = x{}{}{} - {};\n", sfx[cy], sfx[cz], sfx[cx], fl(weight(idx)));
        }
        s += "  return f;\n}\n";
        s
    }

    fn gen_step(&self) -> String {
        let mut walls = String::new();
        for i in 0..Q {
            if i == REST {
                continue;
            }
            let o = opp(i);
            let cv = c(i);
            let w = fl(weight(i));
            let sl = slot(i);
            let (qw, half) = (R_Q as usize + sl / 2, ["x", "y"][sl % 2]);
            walls += &format!(
                "    if ((mask & {bit}u) != 0u) {{\n\
                 \x20     let q = max(unpack2x16float(recs[base + {qw}u]).{half}, 0.001);\n\
                 \x20     let fpi = {fpi} + {w};\n\
                 \x20     let fpo = {fpo} + {w};\n\
                 \x20     let ci = vec3<f32>({cx}.0, {cy}.0, {cz}.0);\n\
                 \x20     let xw = pc + q * ci;\n\
                 \x20     let uw = cross(st.omega.xyz, xw);\n\
                 \x20     let cu = dot(ci, uw);\n\
                 \x20     var fb: f32;\n\
                 \x20     if (q >= 0.5) {{ fb = (fpi + (2.0 * q - 1.0) * fpo - 6.0 * {w} * cu) / (2.0 * q); }}\n\
                 \x20     else if ((mask & {obit}u) == 0u) {{ fb = 2.0 * q * fpi + (1.0 - 2.0 * q) * (fin[{i}] + {w}) - 6.0 * {w} * cu; }}\n\
                 \x20     else {{ fb = fpi - 6.0 * {w} * cu; }}\n\
                 \x20     f[{o}] = fb - {w};\n\
                 \x20     let flk = (ci - uw) * fpi + (ci + uw) * fb;\n\
                 \x20     F += flk; T += cross(xw, flk);\n\
                 \x20   }}\n",
                bit = 1u32 << i,
                obit = 1u32 << o,
                fpi = self.ld(i, "n"),
                fpo = self.ld(o, "n"),
                cx = cv[0],
                cy = cv[1],
                cz = cv[2],
            );
        }
        let forcing = self.grav.iter().any(|g| *g != 0.0);
        STEP.replace("$WALLS$", &walls).replace("$FORCING$", if forcing { "true" } else { "false" })
    }

    fn gen_level_kernels(&self) -> String {
        LEVEL.to_string()
    }
}

const BINDINGS: &str = r#"
@group(0) @binding(8) var<storage, read_write> flags: array<u32>;
@group(0) @binding(9) var<storage, read_write> recs: array<u32>;
@group(0) @binding(10) var<storage, read_write> lists: array<u32>;
@group(0) @binding(11) var<storage, read_write> mom_child: array<f32>;
@group(0) @binding(12) var<storage, read_write> mom_parent: array<f32>;
@group(0) @binding(13) var<storage, read_write> mom_up: array<f32>;
@group(0) @binding(14) var<storage, read_write> mom_child_up: array<f32>;
@group(0) @binding(15) var<storage, read_write> red: array<f32>;
@group(0) @binding(19) var<storage, read_write> probes: array<f32>;
@group(0) @binding(20) var<storage, read_write> spix: array<u32>;
@group(0) @binding(21) var<storage, read_write> sacc: array<f32>;
@group(0) @binding(22) var<storage, read_write> dump: array<vec4<f32>>;

struct Step {
  r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>,
  q0: vec4<f32>, q1: vec4<f32>, q2: vec4<f32>,
  omega: vec4<f32>,
  src: u32, hslot: u32, mw: u32, mold: u32,
  mnew: u32, seed: u32, blend: f32, p3: u32,
}
@group(1) @binding(0) var<uniform> st: Step;
"#;

const COMMON: &str = r#"
fn lin_id(gid: vec3<u32>, nwg: vec3<u32>) -> u32 { return gid.x + gid.y * (nwg.x * WG); }
fn coords(n: u32) -> vec3<u32> {
  let z = n / (NX * NY);
  let r = n - z * (NX * NY);
  let y = r / NX;
  return vec3<u32>(r - y * NX, y, z);
}
fn cidx(c: vec3<u32>) -> u32 { return (c.z * NY + c.y) * NX + c.x; }

// FP16 storage: stochastic rounding, so changes smaller than half a unit in
// the last place still accumulate on average
fn sround(x: f32, key: u32) -> f32 {
  var h = (key * 0x9E3779B1u) ^ (st.seed * 0x85EBCA77u + 0x632BE5ABu);
  h ^= h >> 16u; h *= 0x7feb352du; h ^= h >> 15u; h *= 0x846ca68bu; h ^= h >> 16u;
  let r = f32(h >> 8u) * (1.0 / 16777216.0) - 0.5;
  let e = i32((bitcast<u32>(x) >> 23u) & 0xffu) - 127;
  let ulp = exp2(f32(max(e, -14) - 10));
  return clamp(x + r * ulp, -65000.0, 65000.0);
}

fn from_body(b: vec3<f32>) -> vec3<f32> { return st.r0.xyz * b.x + st.r1.xyz * b.y + st.r2.xyz * b.z; }

fn sponge(xyz: vec3<u32>) -> f32 {
  let p = vec3<f32>(xyz);
  let hi = vec3<f32>(f32(NX - 1u), f32(NY - 1u), f32(NZ - 1u)) - p;
  let lo = SP_LO; let hw = SP_HI;
  var s = 0.0;
  for (var k = 0u; k < 3u; k++) {
    if (lo[k] > 0.0) { s = max(s, 1.0 - p[k] / lo[k]); }
    if (hw[k] > 0.0) { s = max(s, 1.0 - hi[k] / hw[k]); }
  }
  return clamp(s, 0.0, 1.0);
}

// non-equilibrium stress of a (post- or pre-collision) state
fn stress(mo: Mom, rho: f32, u: vec3<f32>) -> array<f32, 6> {
  var p: array<f32, 6>;
  p[0] = mo.s[0] - rho * u.x * u.x - mo.rho1 / 3.0;
  p[1] = mo.s[1] - rho * u.y * u.y - mo.rho1 / 3.0;
  p[2] = mo.s[2] - rho * u.z * u.z - mo.rho1 / 3.0;
  p[3] = mo.s[3] - rho * u.x * u.y;
  p[4] = mo.s[4] - rho * u.x * u.z;
  p[5] = mo.s[5] - rho * u.y * u.z;
  return p;
}
fn deviator(p: array<f32, 6>) -> array<f32, 6> {
  var d = p;
  let tr = (p[0] + p[1] + p[2]) / 3.0;
  d[0] -= tr; d[1] -= tr; d[2] -= tr;
  return d;
}
// a post-collision state from macroscopic data (ghosts, restriction, faces, refill)
fn rebuild(rho: f32, u: vec3<f32>, p_post: array<f32, 6>) -> array<f32, 27> {
  let fe = feq(rho, u);
  let fn_ = fneq(deviator(p_post));
  var f: array<f32, 27>;
  for (var i = 0u; i < 27u; i++) { f[i] = fe[i] + fn_[i]; }
  return f;
}

struct Macro { rho: f32, u: vec3<f32>, ok: bool }
fn cell_macro(m: u32) -> Macro {
  var r: Macro;
  let ty = flags[m] & 7u;
  if (ty == 1u) { r.ok = false; return r; }
  r.ok = true;
  let mo = moments(own(m));
  r.rho = 1.0 + mo.rho1;
  r.u = mo.j / r.rho - 0.5 * GRAV;
  return r;
}

@compute @workgroup_size(64)
fn init(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let n = lin_id(gid, nwg);
  if (n >= N) { return; }
  let f = feq(1.0, U_INIT);
  put_src(n, f);
  put_dst(n, f);
}

var<workgroup> shF: array<vec3<f32>, 256>;
var<workgroup> shT: array<vec3<f32>, 256>;

@compute @workgroup_size(256)
fn reduce1(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
  var F = vec3<f32>(0.0); var T = vec3<f32>(0.0);
  let start = wid.x * 4096u;
  let end = min(start + 4096u, NREC);
  for (var r = start + lid.x; r < end; r += 256u) {
    let b = r * RS + 15u;
    F += vec3<f32>(bitcast<f32>(recs[b]), bitcast<f32>(recs[b + 1u]), bitcast<f32>(recs[b + 2u]));
    T += vec3<f32>(bitcast<f32>(recs[b + 3u]), bitcast<f32>(recs[b + 4u]), bitcast<f32>(recs[b + 5u]));
  }
  shF[lid.x] = F; shT[lid.x] = T;
  workgroupBarrier();
  for (var s = 128u; s > 0u; s >>= 1u) {
    if (lid.x < s) { shF[lid.x] += shF[lid.x + s]; shT[lid.x] += shT[lid.x + s]; }
    workgroupBarrier();
  }
  if (lid.x == 0u) {
    let o = wid.x * 8u;
    red[o] = shF[0].x; red[o + 1u] = shF[0].y; red[o + 2u] = shF[0].z;
    red[o + 3u] = shT[0].x; red[o + 4u] = shT[0].y; red[o + 5u] = shT[0].z;
  }
}

@compute @workgroup_size(256)
fn reduce2(@builtin(local_invocation_id) lid: vec3<u32>) {
  var F = vec3<f32>(0.0); var T = vec3<f32>(0.0);
  for (var r = lid.x; r < NPART; r += 256u) {
    let o = r * 8u;
    F += vec3<f32>(red[o], red[o + 1u], red[o + 2u]);
    T += vec3<f32>(red[o + 3u], red[o + 4u], red[o + 5u]);
  }
  shF[lid.x] = F; shT[lid.x] = T;
  workgroupBarrier();
  for (var s = 128u; s > 0u; s >>= 1u) {
    if (lid.x < s) { shF[lid.x] += shF[lid.x + s]; shT[lid.x] += shT[lid.x + s]; }
    workgroupBarrier();
  }
  if (lid.x == 0u) {
    let o = HIST_OFF + st.hslot * 8u;
    red[o] = shF[0].x; red[o + 1u] = shF[0].y; red[o + 2u] = shF[0].z;
    red[o + 3u] = shT[0].x; red[o + 4u] = shT[0].y; red[o + 5u] = shT[0].z;
  }
}

@compute @workgroup_size(64)
fn sample_verts(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let v = lin_id(gid, nwg);
  if (v >= NVERT) { return; }
  let pb = vec3<f32>(probes[8u * v], probes[8u * v + 1u], probes[8u * v + 2u]);
  let nb = vec3<f32>(probes[8u * v + 3u], probes[8u * v + 4u], probes[8u * v + 5u]);
  let s = CENTER + from_body(pb + VOFF * nb);
  let b0 = floor(s);
  let fr = s - b0;
  var acc = 0.0; var wsum = 0.0;
  for (var k = 0u; k < 8u; k++) {
    let o = vec3<u32>(k & 1u, (k >> 1u) & 1u, (k >> 2u) & 1u);
    let c = vec3<i32>(b0) + vec3<i32>(o);
    if (any(c < vec3<i32>(1)) || any(c >= vec3<i32>(i32(NX) - 1, i32(NY) - 1, i32(NZ) - 1))) { continue; }
    let wv = select(1.0 - fr, fr, o == vec3<u32>(1u));
    let w = wv.x * wv.y * wv.z;
    let mc = cell_macro(cidx(vec3<u32>(c)));
    if (mc.ok && w > 0.0) { acc += w * mc.rho; wsum += w; }
  }
  if (wsum > 1e-6) {
    probes[8u * v + 6u] += acc / wsum;
    probes[8u * v + 7u] += 1.0;
  }
}

// slice pixels this level owns: (pixel index, cell index) pairs
@compute @workgroup_size(64)
fn sample_slices(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let k = lin_id(gid, nwg);
  if (k >= NPIX) { return; }
  let px = spix[2u * k];
  let cell = spix[2u * k + 1u];
  let mc = cell_macro(cell);
  if (mc.ok) {
    let b = px * 5u;
    sacc[b] += mc.rho; sacc[b + 1u] += mc.u.x; sacc[b + 2u] += mc.u.y; sacc[b + 3u] += mc.u.z; sacc[b + 4u] += 1.0;
  }
}

@compute @workgroup_size(64)
fn dump_macro(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let n = lin_id(gid, nwg);
  if (n >= N) { return; }
  let mc = cell_macro(n);
  let ty = flags[n] & 7u;
  if (mc.ok && ty == 0u) { dump[n] = vec4<f32>(mc.rho, mc.u); } else { dump[n] = vec4<f32>(bitcast<f32>(0x7fc00000u)); }
}
"#;

const STEP: &str = r#"
@compute @workgroup_size(64)
fn step(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let n = lin_id(gid, nwg);
  if (n >= N) { return; }
  let fg = flags[n];
  if ((fg & 7u) != 0u) { return; }
  let xyz = coords(n);
  var f = pull(n, xyz);
  let rec = fg >> 3u;
  var F = vec3<f32>(0.0); var T = vec3<f32>(0.0);
  var base = 0u;
  if (rec != 0u) {
    base = (rec - 1u) * RS;
    let mask = recs[base];
    let fin = f;
    let pc = vec3<f32>(xyz) - CENTER;
$WALLS$
  }
  let mo = moments(f);
  let rho = 1.0 + mo.rho1;
  let G = rho * GRAV;
  let u = (mo.j + 0.5 * G) / rho;
  // central second moments / rho (covariance) and the non-equilibrium stress
  let p = stress(mo, rho, u);
  var tau = TAU0;
  if (SMAG2 > 0.0) {
    let dv = deviator(p);
    let pp = dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2] + 2.0 * (dv[3] * dv[3] + dv[4] * dv[4] + dv[5] * dv[5]);
    tau = 0.5 * (TAU0 + sqrt(TAU0 * TAU0 + 18.0 * SMAG2 * sqrt(2.0 * pp) / rho));
  }
  if (N_SPONGE) {
    let sp = sponge(xyz);
    if (sp > 0.0 && SPONGE_TAU > tau) { tau += sp * sp * (SPONGE_TAU - tau); }
  }
  let om = 1.0 / tau;
  // relax: deviator with om, trace to equilibrium (bulk rate 1)
  let dv = deviator(p);
  var sg: array<f32, 6>;
  sg[0] = (1.0 / 3.0) + (1.0 - om) * dv[0] / rho;
  sg[1] = (1.0 / 3.0) + (1.0 - om) * dv[1] / rho;
  sg[2] = (1.0 / 3.0) + (1.0 - om) * dv[2] / rho;
  sg[3] = (1.0 - om) * dv[3] / rho;
  sg[4] = (1.0 - om) * dv[4] / rho;
  sg[5] = (1.0 - om) * dv[5] / rho;
  var m1 = vec3<f32>(0.0);
  if ($FORCING$) { m1 = 0.5 * G / rho; }
  var fo = gauss(rho, u, sg, m1);
  if (N_SPONGE && SIGMA > 0.0) {
    let sp = sponge(xyz);
    if (sp > 0.0) {
      // absorbing layer: pull density (and in a wind, velocity) toward the far field
      let ut = select(u, U_INF, ABS_U);
      let ft = feq(1.0, ut);
      let fc = feq(rho, u);
      let sgm = SIGMA * sp * sp;
      for (var i = 0u; i < 27u; i++) { fo[i] += sgm * (ft[i] - fc[i]); }
    }
  }
  if (rec != 0u) {
    recs[base + 15u] = bitcast<u32>(F.x); recs[base + 16u] = bitcast<u32>(F.y); recs[base + 17u] = bitcast<u32>(F.z);
    recs[base + 18u] = bitcast<u32>(T.x); recs[base + 19u] = bitcast<u32>(T.y); recs[base + 20u] = bitcast<u32>(T.z);
  }
  put_dst(n, fo);
  // moments for the finer level's ghosts (on the child box surface)
  if (HAS_CHILD) {
    let l = vec3<i32>(xyz) - CH_LO;
    if (all(l >= vec3<i32>(0)) && all(l < vec3<i32>(CH_N))) {
      let lu = vec3<u32>(l);
      if (any(lu == vec3<u32>(0u)) || any(lu == CH_N - vec3<u32>(1u))) {
        let o = (st.mw * CH_NODES + (lu.z * CH_N.y + lu.y) * CH_N.x + lu.x) * MW;
        mom_child[o] = rho; mom_child[o + 1u] = u.x; mom_child[o + 2u] = u.y; mom_child[o + 3u] = u.z;
        for (var k = 0u; k < 6u; k++) { mom_child[o + 4u + k] = p[k]; }
        mom_child[o + 10u] = tau;
      }
    }
  }
  // moments for the coarser level's restriction (nodes it shares)
  if (HAS_PARENT) {
    if ((xyz.x & 1u) == 0u && (xyz.y & 1u) == 0u && (xyz.z & 1u) == 0u) {
      let h = xyz / 2u;
      let o = ((h.z * PN.y + h.y) * PN.x + h.x) * MW;
      mom_up[o] = rho; mom_up[o + 1u] = u.x; mom_up[o + 2u] = u.y; mom_up[o + 3u] = u.z;
      for (var k = 0u; k < 6u; k++) { mom_up[o + 4u + k] = p[k]; }
      mom_up[o + 10u] = tau;
    }
  }
}
"#;

const LEVEL: &str = r#"
// domain faces (coarsest level): the far-field state built from the cell
// inside, written as this face cell's post-collision populations
@compute @workgroup_size(64)
fn face_bc(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let k = lin_id(gid, nwg);
  if (k >= NFACE) { return; }
  let b = lists[k];
  var c = coords(b);
  if (!PER.x) { c.x = clamp(c.x, 1u, NX - 2u); }
  if (!PER.y) { c.y = clamp(c.y, 1u, NY - 2u); }
  if (!PER.z) { c.z = clamp(c.z, 1u, NZ - 2u); }
  let m = cidx(c);
  var rho = 1.0; var u = U_INF; var pp = array<f32, 6>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
  if ((flags[m] & 7u) == 0u) {
    let mo = moments(own(m));
    rho = 1.0 + mo.rho1;
    u = mo.j / rho;
    pp = stress(mo, rho, u);
  }
  let ty = (flags[b] >> 3u) & 3u;
  if (ty == 1u) {
    if (TOTAL_P) {
      rho = (1.0 / 3.0 + 0.5 * dot(U_INF, U_INF)) / (1.0 / 3.0 + 0.5 * dot(u, u));
    } else {
      u = U_INF;
    }
  } else {
    rho = 1.0;
  }
  put_src(b, rebuild(rho, u, pp));
}

fn mom_at(slot: u32, node: vec3<u32>) -> array<f32, 11> {
  var m: array<f32, 11>;
  let o = (slot * CH_NODES_P + (node.z * PN.y + node.y) * PN.x + node.x) * MW;
  for (var k = 0u; k < 11u; k++) { m[k] = mom_parent[o + k]; }
  return m;
}

// fine ghosts: interpolate the parent's surface moments in space and time
@compute @workgroup_size(64)
fn ghost_fill(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let k = lin_id(gid, nwg);
  if (k >= NGHOST) { return; }
  let n = lists[NFACE + k];
  let xyz = coords(n);
  let h0 = xyz / 2u;
  let odd = xyz & vec3<u32>(1u);
  var acc: array<f32, 11>;
  for (var j = 0u; j < 11u; j++) { acc[j] = 0.0; }
  var wsum = 0.0;
  for (var c = 0u; c < 8u; c++) {
    let o = vec3<u32>(c & 1u, (c >> 1u) & 1u, (c >> 2u) & 1u);
    // a corner offset only where the node lies between parent nodes
    if (any((o == vec3<u32>(1u)) & (odd == vec3<u32>(0u)))) { continue; }
    let node = h0 + o;
    let w = select(1.0, 0.5, odd.x == 1u) * select(1.0, 0.5, odd.y == 1u) * select(1.0, 0.5, odd.z == 1u);
    let a = mom_at(st.mold, node);
    let b = mom_at(st.mnew, node);
    for (var j = 0u; j < 11u; j++) { acc[j] += w * ((1.0 - st.blend) * a[j] + st.blend * b[j]); }
    wsum += w;
  }
  let rho = acc[0] / wsum;
  let u = vec3<f32>(acc[1], acc[2], acc[3]) / wsum;
  let tc = acc[10] / wsum;
  let tf = 0.5 + 2.0 * (tc - 0.5);
  // same physical stress at half the time step: Pi_f = Pi_c * tf / (2 tc),
  // then the post-collision part (1 - 1/tf)
  let sc = tf / (2.0 * tc) * (1.0 - 1.0 / tf);
  var p: array<f32, 6>;
  for (var j = 0u; j < 6u; j++) { p[j] = acc[4u + j] / wsum * sc; }
  put_src(n, rebuild(rho, u, p));
}

// coarse nodes covered by the child box: take the child's coincident nodes
@compute @workgroup_size(64)
fn restrict_fill(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let k = lin_id(gid, nwg);
  if (k >= NRESTRICT) { return; }
  let n = lists[NFACE + NGHOST + k];
  let l = vec3<u32>(vec3<i32>(coords(n)) - CH_LO);
  let o = ((l.z * CH_N.y + l.y) * CH_N.x + l.x) * MW;
  let rho = mom_child_up[o];
  let u = vec3<f32>(mom_child_up[o + 1u], mom_child_up[o + 2u], mom_child_up[o + 3u]);
  let tf = mom_child_up[o + 10u];
  let tc = 0.5 + 0.5 * (tf - 0.5);
  let sc = 2.0 * tc / tf * (1.0 - 1.0 / tc);
  var p: array<f32, 6>;
  for (var j = 0u; j < 6u; j++) { p[j] = mom_child_up[o + 4u + j] * sc; }
  put_src(n, rebuild(rho, u, p));
}
"#;

