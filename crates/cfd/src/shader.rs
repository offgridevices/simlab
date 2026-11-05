//! WGSL source for the lattice Boltzmann kernels, generated per simulation
//! so grid sizes, relaxation times and the population layout are compile-time
//! constants (the Metal compiler then folds every index computation).
//!
//! Lattice: D3Q19. Streaming: the AA pattern (Bailey et al. 2009), in place,
//! so one copy of the populations is enough. The kernels are written for
//! one "input parity" `p`: before a main step of parity `p` the populations
//! arriving at cell `n` in direction `i` sit at
//!   p = 0: (n, i)            p = 1: (n - c_i, opp(i))
//! and the main step writes its post-collision values to
//!   p = 0: (n, opp(i))       p = 1: (n + c_i, i)
//! which is where the next step (parity 1 - p) reads them. Every memory slot
//! is read and written by exactly one thread per step, so no races.
//!
//! Collision: regularized BGK (Latt & Chopard 2006) with the Smagorinsky
//! eddy viscosity computed locally from the non-equilibrium stress, Guo
//! forcing for a body force, and a viscosity sponge near open faces.
//! Walls: Bouzidi-Firdaouss-Lallemand linear interpolated bounce-back with
//! the moving-wall term, and Galilean-invariant momentum exchange (Wen et
//! al. 2014) for forces.

pub const C: [[i32; 3]; 19] = [
    [0, 0, 0],
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
    [1, 1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, -1],
    [1, -1, 0],
    [-1, 1, 0],
    [1, 0, -1],
    [-1, 0, 1],
    [0, 1, -1],
    [0, -1, 1],
];

pub const fn opp(i: usize) -> usize {
    if i == 0 {
        0
    } else if i % 2 == 1 {
        i + 1
    } else {
        i - 1
    }
}

pub fn weight(i: usize) -> f64 {
    match i {
        0 => 1.0 / 3.0,
        1..=6 => 1.0 / 18.0,
        _ => 1.0 / 36.0,
    }
}

/// Record layout (u32 words per wall-adjacent cell).
pub const RS: u32 = 40;
pub const R_MASK: u32 = 0;
pub const R_CELL: u32 = 1;
pub const R_Q: u32 = 2; // 9 words, f16 pairs, directions 1..18
pub const R_FPOST: u32 = 11; // 18 f32, directions 1..18 (shifted by w_i)
pub const R_MACRO: u32 = 29; // rho, ux, uy, uz
pub const R_FT: u32 = 33; // fx fy fz tx ty tz

/// Number of population buffers is at most this (binding slots 0..).
pub const MAX_POP_BUFS: usize = 12;
pub const FP16_SCALE: f64 = 32768.0;
pub const WG: u32 = 64;
pub const RED_CHUNK: u32 = 4096;

#[derive(Clone, Debug)]
pub struct SliceDef {
    pub origin: [i32; 3],
    pub u: [i32; 3],
    pub v: [i32; 3],
    pub nu: u32,
    pub nv: u32,
}

#[derive(Clone, Debug)]
pub struct ShaderCfg {
    pub dims: [u32; 3],
    pub periodic: [bool; 3],
    pub fp16: bool,
    /// directions per population buffer
    pub per_buf: u32,
    pub tau0: f64,
    pub smag2: f64,
    pub sponge_tau: f64,
    /// strength of the absorbing layer at a face (per step)
    pub sigma: f64,
    /// absorbing layers also pull the velocity to `u_inf`
    pub absorb_velocity: bool,
    /// inlet faces hold the far-field total pressure instead of the velocity
    pub total_pressure_inlet: bool,
    pub sponge_lo: [u32; 3],
    pub sponge_hi: [u32; 3],
    pub u_inf: [f64; 3],
    pub u_init: [f64; 3],
    pub grav: [f64; 3],
    pub center: [f64; 3],
    pub moving: bool,
    pub nrec: u32,
    pub nface: u32,
    pub nvert: u32,
    pub box_lo: [i32; 3],
    pub box_n: [u32; 3],
    pub occ_lo: [f64; 3],
    pub occ_h: f64,
    pub occ_n: [u32; 3],
    pub bin_lo: [f64; 3],
    pub bin_n: [u32; 3],
    pub slices: Vec<SliceDef>,
    pub vertex_offset: f64,
    pub ntri: u32,
    pub wall_model: bool,
}

fn fl(x: f64) -> String {
    let s = format!("{:e}", x as f32);
    format!("f32({s})")
}
fn v3(a: [f64; 3]) -> String {
    format!("vec3<f32>({}, {}, {})", fl(a[0]), fl(a[1]), fl(a[2]))
}

/// Neighbour cell index expression for offset `d` (needs x,y,z,xp,xm,... in scope).
fn nb_expr(d: [i32; 3]) -> String {
    let ax = |c: i32, s: &str| match c {
        1 => format!("{s}p"),
        -1 => format!("{s}m"),
        _ => s.to_string(),
    };
    format!("(({} * NY + {}) * NX + {})", ax(d[2], "z"), ax(d[1], "y"), ax(d[0], "x"))
}

impl ShaderCfg {
    fn n(&self) -> u64 {
        self.dims[0] as u64 * self.dims[1] as u64 * self.dims[2] as u64
    }
    fn slot(&self, i: usize, cell: &str) -> String {
        let per = self.per_buf as usize;
        let b = i / per;
        let off = (i % per) as u64 * self.n();
        format!("pop{b}[{off}u + {cell}]")
    }
    fn load(&self, i: usize, cell: &str) -> String {
        if self.fp16 { format!("(f32({}) * INV_S)", self.slot(i, cell)) } else { self.slot(i, cell) }
    }
    fn store(&self, i: usize, cell: &str, v: &str) -> String {
        if self.fp16 {
            format!("{} = f16(sround({v} * FS, n * 19u + {i}u));", self.slot(i, cell))
        } else {
            format!("{} = {v};", self.slot(i, cell))
        }
    }

    pub fn n_pop_bufs(&self) -> usize {
        19usize.div_ceil(self.per_buf as usize)
    }

    /// Full module for input parity `p`.
    pub fn source(&self, p: u32) -> String {
        let mut s = String::new();
        if self.fp16 {
            s.push_str("enable f16;\n");
        }
        let n = self.n();
        let [nx, ny, nz] = self.dims;
        let elem = if self.fp16 { "f16" } else { "f32" };
        s += &format!(
            "const NX: u32 = {nx}u;\nconst NY: u32 = {ny}u;\nconst NZ: u32 = {nz}u;\nconst N: u32 = {n}u;\n\
             const WG: u32 = {WG}u;\nconst RS: u32 = {RS}u;\n"
        );
        s += &format!("const TAU0: f32 = {};\nconst SMAG2: f32 = {};\nconst SPONGE_TAU: f32 = {};\n", fl(self.tau0), fl(self.smag2), fl(self.sponge_tau));
        let f3 = |a: [u32; 3]| [a[0] as f64, a[1] as f64, a[2] as f64];
        s += &format!("const SP_LO: vec3<f32> = {};\nconst SP_HI: vec3<f32> = {};\n", v3(f3(self.sponge_lo)), v3(f3(self.sponge_hi)));
        s += &format!("const U_INF: vec3<f32> = {};\nconst U_INIT: vec3<f32> = {};\nconst GRAV: vec3<f32> = {};\n", v3(self.u_inf), v3(self.u_init), v3(self.grav));
        s += &format!("const CENTER: vec3<f32> = {};\n", v3(self.center));
        s += &format!("const NREC: u32 = {}u;\nconst NFACE: u32 = {}u;\nconst NVERT: u32 = {}u;\n", self.nrec, self.nface, self.nvert);
        s += &format!("const NPART: u32 = {}u;\n", self.nrec.max(1).div_ceil(RED_CHUNK));
        s += &format!("const TOTAL_P: bool = {};\n", self.total_pressure_inlet);
        s += &format!("const SIGMA: f32 = {};\nconst ABS_U: bool = {};\n", fl(self.sigma), self.absorb_velocity);
        s += &format!("const FNEQ_FACE: f32 = {};\n", fl(1.0 - 1.0 / self.tau0));
        s += &format!("const FS: f32 = {};\nconst INV_S: f32 = {};\n", fl(FP16_SCALE), fl(1.0 / FP16_SCALE));
        s += &format!(
            "const BOX_LO: vec3<i32> = vec3<i32>({}, {}, {});\nconst BOX_N: vec3<u32> = vec3<u32>({}u, {}u, {}u);\n",
            self.box_lo[0], self.box_lo[1], self.box_lo[2], self.box_n[0], self.box_n[1], self.box_n[2]
        );
        s += &format!(
            "const OCC_LO: vec3<f32> = {};\nconst OCC_INVH: f32 = {};\nconst OCC_N: vec3<i32> = vec3<i32>({}, {}, {});\n",
            v3(self.occ_lo),
            fl(1.0 / self.occ_h),
            self.occ_n[0],
            self.occ_n[1],
            self.occ_n[2]
        );
        s += &format!(
            "const BIN_LO: vec3<f32> = {};\nconst BIN_N: vec3<i32> = vec3<i32>({}, {}, {});\n",
            v3(self.bin_lo),
            self.bin_n[0],
            self.bin_n[1],
            self.bin_n[2]
        );
        s += &format!("const WALL_MODEL: bool = {};\n", self.wall_model);
        s += &format!("const NTRI: u32 = {}u;\n", self.ntri);
        s += &format!("const VOFF: f32 = {};\n", fl(self.vertex_offset));
        let per = [self.periodic[0], self.periodic[1], self.periodic[2]];
        s += &format!("const PER: vec3<bool> = vec3<bool>({}, {}, {});\n", per[0], per[1], per[2]);

        // bindings
        for b in 0..self.n_pop_bufs() {
            s += &format!("@group(0) @binding({b}) var<storage, read_write> pop{b}: array<{elem}>;\n");
        }
        s += BINDINGS;

        s += &self.gen_index_helpers();
        s += &self.gen_load_in(p);
        s += &self.gen_store_out(p, "store_out", false);
        s += &self.gen_store_out(1 - p, "store_prev_out", true);
        s += &self.gen_store_in(p);
        s += &self.gen_load_prev_out(1 - p);
        s += &self.gen_moments();
        s += &self.gen_feq();
        s += &self.gen_fneq();
        s += COMMON;
        s += &self.gen_main();
        if self.moving {
            s += &self.gen_voxelize();
        }
        s
    }

    fn gen_index_helpers(&self) -> String {
        "fn coords(n: u32) -> vec3<u32> {\n  let z = n / (NX * NY);\n  let r = n - z * (NX * NY);\n  let y = r / NX;\n  return vec3<u32>(r - y * NX, y, z);\n}\n\
         fn cidx(c: vec3<u32>) -> u32 { return (c.z * NY + c.y) * NX + c.x; }\n"
            .to_string()
            + NEIGH_MACRO_DOC
    }

    fn neigh_prelude() -> &'static str {
        "  let x = xyz.x; let y = xyz.y; let z = xyz.z;\n\
         let xp = select(x + 1u, 0u, x == NX - 1u); let xm = select(x - 1u, NX - 1u, x == 0u);\n\
         let yp = select(y + 1u, 0u, y == NY - 1u); let ym = select(y - 1u, NY - 1u, y == 0u);\n\
         let zp = select(z + 1u, 0u, z == NZ - 1u); let zm = select(z - 1u, NZ - 1u, z == 0u);\n"
    }

    fn gen_load_in(&self, p: u32) -> String {
        let mut s = String::from("fn load_in(n: u32, xyz: vec3<u32>) -> array<f32, 19> {\n  var f: array<f32, 19>;\n");
        if p == 1 {
            s += Self::neigh_prelude();
        }
        for i in 0..19 {
            let e = if p == 0 || i == 0 {
                self.load(i, "n")
            } else {
                let c = C[i];
                let cell = nb_expr([-c[0], -c[1], -c[2]]);
                self.load(opp(i), &cell)
            };
            s += &format!("  f[{i}] = {e};\n");
        }
        s += "  return f;\n}\n";
        s
    }

    /// Writes post-collision populations for a step of parity `p`.
    /// `guard`: skip targets that leave a non-periodic domain (face cells).
    fn gen_store_out(&self, p: u32, name: &str, guard: bool) -> String {
        let mut s = format!("fn {name}(n: u32, xyz: vec3<u32>, f: array<f32, 19>) {{\n");
        if p == 1 {
            s += Self::neigh_prelude();
        }
        for i in 0..19 {
            if p == 0 || i == 0 {
                s += &format!("  {}\n", self.store(opp(i), "n", &format!("f[{i}]")));
            } else {
                let c = C[i];
                let cell = nb_expr(c);
                let st = self.store(i, &cell, &format!("f[{i}]"));
                if guard {
                    let mut conds = vec![];
                    for (k, v) in ["x", "y", "z"].iter().enumerate() {
                        let lim = ["NX", "NY", "NZ"][k];
                        if c[k] == 1 {
                            conds.push(format!("(PER[{k}] || {v} < {lim} - 1u)"));
                        } else if c[k] == -1 {
                            conds.push(format!("(PER[{k}] || {v} > 0u)"));
                        }
                    }
                    s += &format!("  if ({}) {{ {st} }}\n", conds.join(" && "));
                } else {
                    s += &format!("  {st}\n");
                }
            }
        }
        s += "}\n";
        s
    }

    /// Reads back the post-collision populations a cell wrote in the previous
    /// step (parity `p`); they sit in its neighbours' input slots, which
    /// nothing touches between steps.
    fn gen_load_prev_out(&self, p: u32) -> String {
        let mut s = String::from("fn load_prev_out(n: u32, xyz: vec3<u32>) -> array<f32, 19> {\n  var f: array<f32, 19>;\n");
        if p == 1 {
            s += Self::neigh_prelude();
        }
        for i in 0..19 {
            let e = if p == 0 || i == 0 { self.load(opp(i), "n") } else { self.load(i, &nb_expr(C[i])) };
            s += &format!("  f[{i}] = {e};\n");
        }
        s += "  return f;\n}\n";
        s
    }

    fn gen_store_in(&self, p: u32) -> String {
        let mut s = String::from("fn store_in(n: u32, xyz: vec3<u32>, f: array<f32, 19>) {\n");
        if p == 1 {
            s += Self::neigh_prelude();
        }
        for i in 0..19 {
            if p == 0 || i == 0 {
                s += &format!("  {}\n", self.store(i, "n", &format!("f[{i}]")));
            } else {
                let c = C[i];
                let cell = nb_expr([-c[0], -c[1], -c[2]]);
                s += &format!("  {}\n", self.store(opp(i), &cell, &format!("f[{i}]")));
            }
        }
        s += "}\n";
        s
    }

    fn gen_moments(&self) -> String {
        let sum = |coef: &dyn Fn([i32; 3]) -> i32| -> String {
            let mut t = String::new();
            for (i, c) in C.iter().enumerate() {
                let k = coef(*c);
                if k == 1 {
                    t += &format!(" + f[{i}]");
                } else if k == -1 {
                    t += &format!(" - f[{i}]");
                } else if k != 0 {
                    t += &format!(" + {k}.0 * f[{i}]");
                }
            }
            if t.is_empty() { "0.0".into() } else { format!("0.0{t}") }
        };
        let mut s = String::from("struct Mom { rho1: f32, j: vec3<f32>, s: array<f32, 6> }\nfn moments(f: array<f32, 19>) -> Mom {\n  var m: Mom;\n");
        s += &format!("  m.rho1 = {};\n", sum(&|_c| 1));
        s += &format!("  m.j = vec3<f32>({}, {}, {});\n", sum(&|c| c[0]), sum(&|c| c[1]), sum(&|c| c[2]));
        let pairs = [(0, 0), (1, 1), (2, 2), (0, 1), (0, 2), (1, 2)];
        for (k, (a, b)) in pairs.iter().enumerate() {
            s += &format!("  m.s[{k}] = {};\n", sum(&|c| c[*a] * c[*b]));
        }
        s += "  return m;\n}\n";
        s
    }

    fn gen_feq(&self) -> String {
        let mut s = String::from(
            "fn feq_arr(rho1: f32, u: vec3<f32>) -> array<f32, 19> {\n  var f: array<f32, 19>;\n  let rho = 1.0 + rho1;\n  let usq = 1.5 * dot(u, u);\n",
        );
        for (i, c) in C.iter().enumerate() {
            let cu = cdot(*c, "u");
            s += &format!("  {{ let cu = {cu}; f[{i}] = {} * (rho1 + rho * (3.0 * cu + 4.5 * cu * cu - usq)); }}\n", fl(weight(i)));
        }
        s += "  return f;\n}\n";
        s
    }

    fn gen_fneq(&self) -> String {
        let mut s = String::from(
            "fn fneq_arr(p: array<f32, 6>) -> array<f32, 19> {\n  var f: array<f32, 19>;\n  let tr = (p[0] + p[1] + p[2]) * (1.0 / 3.0);\n",
        );
        for (i, c) in C.iter().enumerate() {
            let mut t = String::from("-tr");
            let pairs = [(0, 0, 0), (1, 1, 1), (2, 2, 2), (0, 1, 3), (0, 2, 4), (1, 2, 5)];
            for (a, b, k) in pairs {
                let v = c[a] * c[b] * if a == b { 1 } else { 2 };
                if v != 0 {
                    t += &format!(" + {v}.0 * p[{k}]");
                }
            }
            s += &format!("  f[{i}] = {} * ({t});\n", fl(4.5 * weight(i)));
        }
        s += "  return f;\n}\n";
        s
    }


    fn gen_main(&self) -> String {
        let mut s = String::from(MAIN_HEAD);
        // Bouzidi wall links
        let mut walls = String::new();
        for i in 1..19 {
            let o = opp(i);
            let c = C[i];
            let w = fl(weight(i));
            let (k, half) = ((i - 1) / 2, (i - 1) % 2);
            let hv = ["x", "y"][half];
            walls += &format!(
                "    if ((mask & {bit}u) != 0u) {{\n\
                 \x20     let q = max(unpack2x16float(recs[base + {qw}u]).{hv}, 0.001);\n\
                 \x20     let fpi = bitcast<f32>(recs[base + {fpi}u]) + {w};\n\
                 \x20     let fpo = bitcast<f32>(recs[base + {fpo}u]) + {w};\n\
                 \x20     let ci = vec3<f32>({cx}.0, {cy}.0, {cz}.0);\n\
                 \x20     let xw = pc + q * ci;\n\
                 \x20     let uw = cross(st.omega.xyz, xw);\n\
                 \x20     let cu = dot(ci, uw);\n\
                 \x20     var fb: f32;\n\
                 \x20     if (q >= 0.5) {{ fb = (fpi + (2.0 * q - 1.0) * fpo - 6.0 * {w} * cu) / (2.0 * q); }}\n\
                 \x20     else if ((mask & {obit}u) == 0u) {{ fb = 2.0 * q * fpi + (1.0 - 2.0 * q) * (fin[{i}] + {w}) - 6.0 * {w} * cu; }}\n\
                 \x20     else {{ fb = fpi - 6.0 * {w} * cu; }}\n\
                 \x20     f[{o}] = fb - {w};\n\
                 \x20     let fl = (ci - uw) * fpi + (ci + uw) * fb;\n\
                 \x20     F += fl; T += cross(xw, fl);\n\
                 \x20   }}\n",
                bit = 1u32 << i,
                obit = 1u32 << o,
                qw = R_Q as usize + k,
                fpi = R_FPOST as usize + i - 1,
                fpo = R_FPOST as usize + o - 1,
                cx = c[0],
                cy = c[1],
                cz = c[2],
            );
        }
        s = s.replace("$WALLS$", &walls);
        let mut save = String::new();
        for i in 1..19 {
            save += &format!("    recs[base + {}u] = bitcast<u32>(f[{i}]);\n", R_FPOST as usize + i - 1);
        }
        s = s.replace("$SAVE$", &save);
        let forcing = self.grav.iter().any(|g| *g != 0.0);
        let mut src = String::new();
        if forcing {
            // Guo forcing; the regularized projection drops the first-order
            // non-equilibrium momentum (j - rho u = -G/2), so it is added back
            // here to keep exactly the momentum budget of BGK + Guo
            src += "  let G = rho * GRAV;\n  let uG = dot(u, G);\n  let fo = 1.0 - 0.5 * om;\n";
            for (i, c) in C.iter().enumerate() {
                let cu = cdot(*c, "u");
                let cg = cdot(*c, "G");
                src += &format!("  {{ let cu = {cu}; let cg = {cg}; f[{i}] += {w} * (fo * (3.0 * (cg - uG) + 9.0 * cu * cg) - 1.5 * (1.0 - om) * cg); }}\n", w = fl(weight(i)));
            }
        }
        s = s.replace("$FORCING$", &src);
        s = s.replace("$MOVING$", if self.moving { "true" } else { "false" });
        s
    }

    fn gen_voxelize(&self) -> String {
        VOXELIZE.to_string()
    }
}

fn cdot(c: [i32; 3], v: &str) -> String {
    let mut t = String::new();
    for (k, comp) in ["x", "y", "z"].iter().enumerate() {
        match c[k] {
            1 => t += &format!(" + {v}.{comp}"),
            -1 => t += &format!(" - {v}.{comp}"),
            _ => {}
        }
    }
    if t.is_empty() { "0.0".into() } else { format!("(0.0{t})") }
}

const NEIGH_MACRO_DOC: &str = "";

const BINDINGS: &str = r#"
@group(0) @binding(12) var<storage, read_write> flags: array<u32>;
@group(0) @binding(13) var<storage, read_write> recs: array<u32>;
@group(0) @binding(14) var<storage, read_write> faces: array<u32>;
@group(0) @binding(15) var<storage, read_write> facebuf: array<f32>;
@group(0) @binding(16) var<storage, read_write> partials: array<f32>;
@group(0) @binding(17) var<storage, read_write> hist: array<f32>;
@group(0) @binding(18) var<storage, read_write> occ: array<u32>;
@group(0) @binding(19) var<storage, read_write> bin_start: array<u32>;
@group(0) @binding(20) var<storage, read_write> bin_tris: array<u32>;
@group(0) @binding(21) var<storage, read_write> tris: array<vec4<f32>>;
@group(0) @binding(22) var<storage, read_write> verts: array<vec4<f32>>;
@group(0) @binding(23) var<storage, read_write> vacc: array<f32>;
@group(0) @binding(24) var<storage, read_write> sacc: array<f32>;
@group(0) @binding(25) var<storage, read_write> dump: array<vec4<f32>>;
@group(0) @binding(26) var<storage, read_write> slicedef: array<vec4<i32>>;
@group(0) @binding(27) var<storage, read_write> qscr: array<atomic<u32>>;

struct Step {
  r0: vec4<f32>, r1: vec4<f32>, r2: vec4<f32>,
  q0: vec4<f32>, q1: vec4<f32>, q2: vec4<f32>,
  omega: vec4<f32>,
  slot: u32, p1: u32, p2: u32, p3: u32,
}
@group(1) @binding(0) var<uniform> st: Step;
"#;

const COMMON: &str = r#"
// FP16 storage: stochastic rounding (unbiased on average), so slow,
// diffusive changes smaller than half a unit in the last place still
// accumulate instead of stalling
fn sround(x: f32, key: u32) -> f32 {
  var h = (key * 0x9E3779B1u) ^ (st.p1 * 0x85EBCA77u + 0x632BE5ABu);
  h ^= h >> 16u; h *= 0x7feb352du; h ^= h >> 15u; h *= 0x846ca68bu; h ^= h >> 16u;
  let r = f32(h >> 8u) * (1.0 / 16777216.0) - 0.5;
  let e = i32((bitcast<u32>(x) >> 23u) & 0xffu) - 127;
  let ulp = exp2(f32(max(e, -14) - 10));
  return clamp(x + r * ulp, -65000.0, 65000.0);
}

fn lin_id(gid: vec3<u32>, nwg: vec3<u32>) -> u32 {
  return gid.x + gid.y * (nwg.x * WG);
}

// body coordinates of a lattice offset from CENTER (rotation now / before)
fn to_body(v: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(st.r0.xyz, v), dot(st.r1.xyz, v), dot(st.r2.xyz, v)); }
fn to_body_prev(v: vec3<f32>) -> vec3<f32> { return vec3<f32>(dot(st.q0.xyz, v), dot(st.q1.xyz, v), dot(st.q2.xyz, v)); }
fn from_body(b: vec3<f32>) -> vec3<f32> { return st.r0.xyz * b.x + st.r1.xyz * b.y + st.r2.xyz * b.z; }

fn sponge(xyz: vec3<u32>) -> f32 {
  let p = vec3<f32>(xyz);
  let hi = vec3<f32>(f32(NX - 1u), f32(NY - 1u), f32(NZ - 1u)) - p;
  var s = 0.0;
  for (var k = 0u; k < 3u; k++) {
    let lo = SP_LO; let hw = SP_HI;
    if (lo[k] > 0.0) { s = max(s, 1.0 - p[k] / lo[k]); }
    if (hw[k] > 0.0) { s = max(s, 1.0 - hi[k] / hw[k]); }
  }
  return clamp(s, 0.0, 1.0);
}

// Wall model for boundary layers thinner than a cell: the first fluid cell's
// velocity along the wall is matched to Spalding's law of the wall, and the
// cell's viscosity is set so the lattice carries the matching wall shear
// stress (tau_w = nu_eff * u_t / y). Where the cell sits in the viscous
// sublayer this gives back the molecular viscosity, so resolved flows are
// unchanged.
fn wall_tau(base: u32, mask: u32, pc: vec3<f32>, u: vec3<f32>, tau_les: f32) -> f32 {
  var cvs = array<vec3<f32>, 19>(
    vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(-1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, -1.0, 0.0),
    vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(-1.0, -1.0, 0.0), vec3<f32>(1.0, 0.0, 1.0),
    vec3<f32>(-1.0, 0.0, -1.0), vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.0, -1.0, -1.0), vec3<f32>(1.0, -1.0, 0.0), vec3<f32>(-1.0, 1.0, 0.0),
    vec3<f32>(1.0, 0.0, -1.0), vec3<f32>(-1.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, -1.0), vec3<f32>(0.0, -1.0, 1.0));
  var nsum = vec3<f32>(0.0);
  for (var i = 1u; i < 19u; i++) {
    if ((mask & (1u << i)) != 0u) {
      let q = unpack2x16float(recs[base + 2u + (i - 1u) / 2u])[(i - 1u) % 2u];
      nsum += cvs[i] * select(1.0 / 36.0, 1.0 / 18.0, i < 7u) * (1.5 - q);
    }
  }
  let nl = length(nsum);
  if (nl < 1e-6) { return tau_les; }
  let nw = nsum / nl;
  var y = 2.0;
  for (var i = 1u; i < 19u; i++) {
    if ((mask & (1u << i)) != 0u) {
      let cn = dot(cvs[i], nw);
      if (cn > 0.3) {
        let q = unpack2x16float(recs[base + 2u + (i - 1u) / 2u])[(i - 1u) % 2u];
        y = min(y, max(q, 0.02) * cn);
      }
    }
  }
  if (y > 1.9) { return tau_les; }
  let uwall = cross(st.omega.xyz, pc + y * nw);
  let ur = u - uwall;
  let ut = length(ur - dot(ur, nw) * nw);
  let nu0 = (TAU0 - 0.5) / 3.0;
  if (ut < 1e-7) { return tau_les; }
  // Spalding: y+ = u+ + e^(-kB) (e^(k u+) - 1 - k u+ - (k u+)^2/2 - (k u+)^3/6)
  let kap = 0.41;
  let eb = exp(-kap * 5.2);
  var ust = sqrt(nu0 * ut / y);
  for (var it = 0; it < 12; it++) {
    let up = ut / ust;
    let yp = y * ust / nu0;
    let ku = min(kap * up, 30.0);
    let g = up + eb * (exp(ku) - 1.0 - ku - 0.5 * ku * ku - ku * ku * ku / 6.0) - yp;
    // d/d(ust): du+/du* = -up/ust, dy+/du* = y/nu0
    let dgdup = 1.0 + eb * kap * (exp(ku) - 1.0 - ku - 0.5 * ku * ku);
    let dg = dgdup * (-up / ust) - y / nu0;
    let step = g / dg;
    ust = clamp(ust - step, 0.1 * ust, 10.0 * ust);
    if (abs(step) < 1e-5 * ust) { break; }
  }
  let nuw = ust * ust * y / ut;
  // never below the turbulence model's viscosity (stability)
  return max(0.5 + 3.0 * max(nuw, nu0), tau_les);
}

struct Macro { rho: f32, u: vec3<f32>, ok: bool }

// density and velocity of a cell between steps (records hold them for wall cells)
fn cell_macro(m: u32) -> Macro {
  var r: Macro;
  let fg = flags[m];
  if ((fg & 3u) != 0u) { r.ok = false; return r; }
  r.ok = true;
  let rec = fg >> 2u;
  if (rec != 0u) {
    let base = (rec - 1u) * RS;
    r.rho = bitcast<f32>(recs[base + 29u]);
    r.u = vec3<f32>(bitcast<f32>(recs[base + 30u]), bitcast<f32>(recs[base + 31u]), bitcast<f32>(recs[base + 32u]));
    return r;
  }
  // post-collision populations of the last step (their inputs next to a
  // face are only written at the start of the next step)
  let f = load_prev_out(m, coords(m));
  let mo = moments(f);
  r.rho = 1.0 + mo.rho1;
  r.u = mo.j / r.rho - 0.5 * GRAV;
  return r;
}

@compute @workgroup_size(64)
fn init(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let n = lin_id(gid, nwg);
  if (n >= N) { return; }
  store_in(n, coords(n), feq_arr(0.0, U_INIT));
}

@compute @workgroup_size(64)
fn face_gather(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let k = lin_id(gid, nwg);
  if (k >= NFACE) { return; }
  let b = faces[k];
  var c = coords(b);
  if (!PER.x) { c.x = clamp(c.x, 1u, NX - 2u); }
  if (!PER.y) { c.y = clamp(c.y, 1u, NY - 2u); }
  if (!PER.z) { c.z = clamp(c.z, 1u, NZ - 2u); }
  let m = cidx(c);
  let fg = flags[m];
  var rho = 1.0; var u = U_INF; var pi = array<f32, 6>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
  if ((fg & 3u) == 0u) {
    // the neighbour's post-collision state from the previous step: same
    // density and velocity as before collision, stress already relaxed
    let mo = moments(load_prev_out(m, c));
    rho = 1.0 + mo.rho1;
    u = mo.j / rho - 0.5 * GRAV;
    pi[0] = mo.s[0] - rho * u.x * u.x - mo.rho1 / 3.0;
    pi[1] = mo.s[1] - rho * u.y * u.y - mo.rho1 / 3.0;
    pi[2] = mo.s[2] - rho * u.z * u.z - mo.rho1 / 3.0;
    pi[3] = mo.s[3] - rho * u.x * u.y;
    pi[4] = mo.s[4] - rho * u.x * u.z;
    pi[5] = mo.s[5] - rho * u.y * u.z;
  }
  let o = k * 10u;
  facebuf[o] = rho; facebuf[o + 1u] = u.x; facebuf[o + 2u] = u.y; facebuf[o + 3u] = u.z;
  for (var i = 0u; i < 6u; i++) { facebuf[o + 4u + i] = pi[i]; }
}

@compute @workgroup_size(64)
fn face_write(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let k = lin_id(gid, nwg);
  if (k >= NFACE) { return; }
  let b = faces[k];
  let o = k * 10u;
  var rho = facebuf[o];
  var u = vec3<f32>(facebuf[o + 1u], facebuf[o + 2u], facebuf[o + 3u]);
  var pi: array<f32, 6>;
  for (var i = 0u; i < 6u; i++) { pi[i] = facebuf[o + 4u + i]; }
  let ty = flags[b] & 3u;
  if (ty == 2u) {
    if (TOTAL_P) {
      // free-stream inflow: the far field's total pressure, velocity from
      // inside, so the inflow can feel the part ahead of it like real air
      rho = (1.0 / 3.0 + 0.5 * dot(U_INF, U_INF)) / (1.0 / 3.0 + 0.5 * dot(u, u));
    } else {
      u = U_INF;          // velocity inlet: density from inside
    }
  } else {
    rho = 1.0;            // open: far-field pressure, velocity from inside
  }
  let fe = feq_arr(rho - 1.0, u);
  let fq = fneq_arr(pi);
  var f: array<f32, 19>;
  for (var i = 0u; i < 19u; i++) { f[i] = fe[i] + fq[i]; }
  store_prev_out(b, coords(b), f);
}

var<workgroup> shF: array<vec3<f32>, 256>;
var<workgroup> shT: array<vec3<f32>, 256>;

@compute @workgroup_size(256)
fn reduce1(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
  var F = vec3<f32>(0.0); var T = vec3<f32>(0.0);
  let start = wid.x * 4096u;
  let end = min(start + 4096u, NREC);
  for (var r = start + lid.x; r < end; r += 256u) {
    let b = r * RS + 33u;
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
    partials[o] = shF[0].x; partials[o + 1u] = shF[0].y; partials[o + 2u] = shF[0].z;
    partials[o + 3u] = shT[0].x; partials[o + 4u] = shT[0].y; partials[o + 5u] = shT[0].z;
  }
}

@compute @workgroup_size(256)
fn reduce2(@builtin(local_invocation_id) lid: vec3<u32>) {
  var F = vec3<f32>(0.0); var T = vec3<f32>(0.0);
  for (var r = lid.x; r < NPART; r += 256u) {
    let o = r * 8u;
    F += vec3<f32>(partials[o], partials[o + 1u], partials[o + 2u]);
    T += vec3<f32>(partials[o + 3u], partials[o + 4u], partials[o + 5u]);
  }
  shF[lid.x] = F; shT[lid.x] = T;
  workgroupBarrier();
  for (var s = 128u; s > 0u; s >>= 1u) {
    if (lid.x < s) { shF[lid.x] += shF[lid.x + s]; shT[lid.x] += shT[lid.x + s]; }
    workgroupBarrier();
  }
  if (lid.x == 0u) {
    let o = st.slot * 8u;
    hist[o] = shF[0].x; hist[o + 1u] = shF[0].y; hist[o + 2u] = shF[0].z;
    hist[o + 3u] = shT[0].x; hist[o + 4u] = shT[0].y; hist[o + 5u] = shT[0].z;
  }
}

@compute @workgroup_size(64)
fn sample_verts(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let v = lin_id(gid, nwg);
  if (v >= NVERT) { return; }
  let pb = verts[2u * v].xyz;
  let nb = verts[2u * v + 1u].xyz;
  let s = CENTER + from_body(pb + VOFF * nb);
  let b0 = floor(s);
  let fr = s - b0;
  var acc = 0.0; var wsum = 0.0;
  for (var k = 0u; k < 8u; k++) {
    let o = vec3<u32>(k & 1u, (k >> 1u) & 1u, (k >> 2u) & 1u);
    let c = vec3<i32>(b0) + vec3<i32>(o);
    if (any(c < vec3<i32>(1)) || any(c >= vec3<i32>(i32(NX) - 1, i32(NY) - 1, i32(NZ) - 1))) { continue; }
    let wv = select(1.0 - fr, fr, vec3<bool>(o == vec3<u32>(1u)));
    let w = wv.x * wv.y * wv.z;
    let mc = cell_macro(cidx(vec3<u32>(c)));
    if (mc.ok && w > 0.0) { acc += w * mc.rho; wsum += w; }
  }
  if (wsum > 1e-6) {
    vacc[2u * v] += acc / wsum;
    vacc[2u * v + 1u] += 1.0;
  }
}

@compute @workgroup_size(64)
fn sample_slices(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  var k = lin_id(gid, nwg);
  let nsl = u32(slicedef[0].x);
  var acc = 0u;
  for (var s = 0u; s < nsl; s++) {
    let o = slicedef[1u + 3u * s];
    let du = slicedef[2u + 3u * s];
    let dv = slicedef[3u + 3u * s];
    let nu = u32(o.w); let nv = u32(du.w);
    if (k < acc + nu * nv) {
      let l = k - acc;
      let iv = l / nu; let iu = l - iv * nu;
      let c = o.xyz + i32(iu) * du.xyz + i32(iv) * dv.xyz;
      let mc = cell_macro(cidx(vec3<u32>(c)));
      if (mc.ok) {
        let b = k * 5u;
        sacc[b] += mc.rho; sacc[b + 1u] += mc.u.x; sacc[b + 2u] += mc.u.y; sacc[b + 3u] += mc.u.z; sacc[b + 4u] += 1.0;
      }
      return;
    }
    acc += nu * nv;
  }
}

@compute @workgroup_size(64)
fn dump_macro(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let n = lin_id(gid, nwg);
  if (n >= N) { return; }
  let mc = cell_macro(n);
  if (mc.ok) { dump[n] = vec4<f32>(mc.rho, mc.u); } else { dump[n] = vec4<f32>(bitcast<f32>(0x7fc00000u)); }
}
"#;

const MAIN_HEAD: &str = r#"
@compute @workgroup_size(64)
fn main_step(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let n = lin_id(gid, nwg);
  if (n >= N) { return; }
  let fg = flags[n];
  if ((fg & 3u) != 0u) { return; }
  let xyz = coords(n);
  var f = load_in(n, xyz);
  let rec = fg >> 2u;
  var F = vec3<f32>(0.0); var T = vec3<f32>(0.0);
  var base = 0u;
  var mask = 0u;
  if (rec != 0u) {
    base = (rec - 1u) * RS;
    mask = recs[base];
    let fin = f;
    let pc = vec3<f32>(xyz) - CENTER;
$WALLS$
  }
  let mo = moments(f);
  let rho = 1.0 + mo.rho1;
  let u = mo.j / rho + 0.5 * GRAV;
  var pi: array<f32, 6>;
  pi[0] = mo.s[0] - rho * u.x * u.x - mo.rho1 / 3.0;
  pi[1] = mo.s[1] - rho * u.y * u.y - mo.rho1 / 3.0;
  pi[2] = mo.s[2] - rho * u.z * u.z - mo.rho1 / 3.0;
  pi[3] = mo.s[3] - rho * u.x * u.y;
  pi[4] = mo.s[4] - rho * u.x * u.z;
  pi[5] = mo.s[5] - rho * u.y * u.z;
  // Smagorinsky: tau from |Pi_neq| (closed form, no finite differences)
  let pp = pi[0] * pi[0] + pi[1] * pi[1] + pi[2] * pi[2] + 2.0 * (pi[3] * pi[3] + pi[4] * pi[4] + pi[5] * pi[5]);
  let qn = sqrt(2.0 * pp);
  var tau = 0.5 * (TAU0 + sqrt(TAU0 * TAU0 + 18.0 * SMAG2 * qn / rho));
  if (WALL_MODEL && mask != 0u) {
    tau = wall_tau(base, mask, vec3<f32>(xyz) - CENTER, u, tau);
  }
  let sp = sponge(xyz);
  if (sp > 0.0 && SPONGE_TAU > tau) { tau += sp * sp * (SPONGE_TAU - tau); }
  let om = 1.0 / tau;
  let fe = feq_arr(mo.rho1, u);
  let fq = fneq_arr(pi);
  for (var i = 0u; i < 19u; i++) { f[i] = fe[i] + (1.0 - om) * fq[i]; }
  if (sp > 0.0 && SIGMA > 0.0) {
    // absorbing layer: pull density (and in a wind, velocity) toward the far
    // field so sound and eddies leave instead of ringing in the box
    let ut = select(u, U_INF, ABS_U);
    let ft = feq_arr(0.0, ut);
    let sg = SIGMA * sp * sp;
    for (var i = 0u; i < 19u; i++) { f[i] += sg * (ft[i] - fe[i]); }
  }
  let u_out = u;
$FORCING$
  if (rec != 0u) {
$SAVE$
    recs[base + 29u] = bitcast<u32>(rho);
    recs[base + 30u] = bitcast<u32>(u_out.x);
    recs[base + 31u] = bitcast<u32>(u_out.y);
    recs[base + 32u] = bitcast<u32>(u_out.z);
    if ($MOVING$) {
      F += vec3<f32>(bitcast<f32>(recs[base + 33u]), bitcast<f32>(recs[base + 34u]), bitcast<f32>(recs[base + 35u]));
      T += vec3<f32>(bitcast<f32>(recs[base + 36u]), bitcast<f32>(recs[base + 37u]), bitcast<f32>(recs[base + 38u]));
    }
    recs[base + 33u] = bitcast<u32>(F.x); recs[base + 34u] = bitcast<u32>(F.y); recs[base + 35u] = bitcast<u32>(F.z);
    recs[base + 36u] = bitcast<u32>(T.x); recs[base + 37u] = bitcast<u32>(T.y); recs[base + 38u] = bitcast<u32>(T.z);
  }
  store_out(n, xyz, f);
}
"#;

const VOXELIZE: &str = r#"
fn occ_at(b: vec3<f32>) -> bool {
  let g = vec3<i32>(round((b - OCC_LO) * OCC_INVH));
  if (any(g < vec3<i32>(0)) || any(g >= OCC_N)) { return false; }
  let l = u32((g.z * OCC_N.y + g.y) * OCC_N.x + g.x);
  return ((occ[l >> 5u] >> (l & 31u)) & 1u) != 0u;
}

// Solid or fluid, for the rotation now or the one before. Every cell asks
// with exactly the same arithmetic for itself and its neighbours, so all
// cells agree on which cells are solid (a disagreement would let a fluid
// cell read populations a solid cell never wrote).
fn solid_at(p: vec3<i32>, before: bool) -> bool {
  let pc = vec3<f32>(p) - CENTER;
  if (before) { return occ_at(to_body_prev(pc)); }
  return occ_at(to_body(pc));
}

fn ray_tri(o: vec3<f32>, d: vec3<f32>, v0: vec3<f32>, e1: vec3<f32>, e2: vec3<f32>) -> f32 {
  let pv = cross(d, e2);
  let det = dot(e1, pv);
  if (abs(det) < 1e-12) { return 9.0; }
  let inv = 1.0 / det;
  let tv = o - v0;
  let a = dot(tv, pv) * inv;
  if (a < 0.0 || a > 1.0) { return 9.0; }
  let qv = cross(tv, e1);
  let b = dot(d, qv) * inv;
  if (b < 0.0 || a + b > 1.0) { return 9.0; }
  let t = dot(e2, qv) * inv;
  if (t < -1e-4 || t > 1.0) { return 9.0; }
  return max(t, 0.0);
}

// Every triangle of the spinning part, rotated to this step's angle, is
// tested against the lattice links around it; hits go to the per-link
// scratch (nearest t, farthest t, count) of the swept box.
@compute @workgroup_size(64)
fn tri_scatter(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let t = lin_id(gid, nwg);
  if (t >= NTRI) { return; }
  let a = CENTER + from_body(tris[3u * t].xyz);
  let e1 = from_body(tris[3u * t + 1u].xyz);
  let e2 = from_body(tris[3u * t + 2u].xyz);
  let nrm = cross(e1, e2);
  let nl = length(nrm);
  if (nl < 1e-12) { return; }
  let nn = nrm / nl;
  let b = a + e1;
  let c = a + e2;
  let lo = max(vec3<i32>(ceil(min(min(a, b), c) - 1.415)), BOX_LO);
  let hi = min(vec3<i32>(floor(max(max(a, b), c) + 1.415)), BOX_LO + vec3<i32>(BOX_N) - vec3<i32>(1));
  var cv = array<vec3<f32>, 19>(
    vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(-1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, -1.0, 0.0),
    vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(-1.0, -1.0, 0.0), vec3<f32>(1.0, 0.0, 1.0),
    vec3<f32>(-1.0, 0.0, -1.0), vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.0, -1.0, -1.0), vec3<f32>(1.0, -1.0, 0.0), vec3<f32>(-1.0, 1.0, 0.0),
    vec3<f32>(1.0, 0.0, -1.0), vec3<f32>(-1.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, -1.0), vec3<f32>(0.0, -1.0, 1.0));
  for (var z = lo.z; z <= hi.z; z++) {
    for (var y = lo.y; y <= hi.y; y++) {
      for (var x = lo.x; x <= hi.x; x++) {
        let p = vec3<f32>(f32(x), f32(y), f32(z));
        let d0 = dot(nn, p - a);
        if (abs(d0) > 1.415) { continue; }
        let l = vec3<u32>(vec3<i32>(x, y, z) - BOX_LO);
        let k = (l.z * BOX_N.y + l.y) * BOX_N.x + l.x;
        for (var i = 1u; i < 19u; i += 2u) {
          let d1 = d0 + dot(nn, cv[i]);
          if (d0 * d1 > 0.0 && abs(d0) > 1e-6) { continue; }
          let tt = ray_tri(p, cv[i], a, e1, e2);
          if (tt <= 1.0) {
            let o = (k * 9u + (i - 1u) / 2u) * 3u;
            let tb = bitcast<u32>(max(tt, 0.0));
            atomicMin(&qscr[o], tb);
            atomicMax(&qscr[o + 1u], tb);
            atomicAdd(&qscr[o + 2u], 1u);
          }
        }
      }
    }
  }
}

@compute @workgroup_size(64)
fn qclear(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let k = lin_id(gid, nwg);
  if (k >= NREC * 9u) { return; }
  atomicStore(&qscr[3u * k], 0x7f7fffffu);
  atomicStore(&qscr[3u * k + 1u], 0u);
  atomicStore(&qscr[3u * k + 2u], 0u);
}

// Re-voxelizes the swept box of a spinning part for the rotation in `st`:
// solid/fluid flags, wall links with q, refill of cells the part uncovered,
// and the momentum of cells it covered or uncovered (added to the force).
@compute @workgroup_size(64)
fn voxelize(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
  let k = lin_id(gid, nwg);
  if (k >= NREC) { return; }
  let lx = k % BOX_N.x;
  let ly = (k / BOX_N.x) % BOX_N.y;
  let lz = k / (BOX_N.x * BOX_N.y);
  let p = vec3<i32>(i32(lx), i32(ly), i32(lz)) + BOX_LO;
  let xyz = vec3<u32>(p);
  let n = cidx(xyz);
  let pc = vec3<f32>(p) - CENTER;
  let b = to_body(pc);
  let bp = to_body_prev(pc);
  let solid_now = solid_at(p, false);
  let solid_before = solid_at(p, true);
  let base = k * RS;
  if (solid_now) {
    flags[n] = 1u | ((k + 1u) << 2u);
    recs[base] = 0u;
    var F = vec3<f32>(0.0);
    if (!solid_before) {
      let rho = bitcast<f32>(recs[base + 29u]);
      F = rho * vec3<f32>(bitcast<f32>(recs[base + 30u]), bitcast<f32>(recs[base + 31u]), bitcast<f32>(recs[base + 32u]));
    }
    let T = cross(pc, F);
    recs[base + 33u] = bitcast<u32>(F.x); recs[base + 34u] = bitcast<u32>(F.y); recs[base + 35u] = bitcast<u32>(F.z);
    recs[base + 36u] = bitcast<u32>(T.x); recs[base + 37u] = bitcast<u32>(T.y); recs[base + 38u] = bitcast<u32>(T.z);
    return;
  }
  var cv = array<vec3<f32>, 19>(
    vec3<f32>(0.0, 0.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(-1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, -1.0, 0.0),
    vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, -1.0), vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(-1.0, -1.0, 0.0), vec3<f32>(1.0, 0.0, 1.0),
    vec3<f32>(-1.0, 0.0, -1.0), vec3<f32>(0.0, 1.0, 1.0), vec3<f32>(0.0, -1.0, -1.0), vec3<f32>(1.0, -1.0, 0.0), vec3<f32>(-1.0, 1.0, 0.0),
    vec3<f32>(1.0, 0.0, -1.0), vec3<f32>(-1.0, 0.0, 1.0), vec3<f32>(0.0, 1.0, -1.0), vec3<f32>(0.0, -1.0, 1.0));
  var db: array<vec3<f32>, 19>;
  var qv: array<f32, 19>;
  var qx: array<f32, 19>;
  var nh: array<u32, 19>;
  for (var i = 1u; i < 19u; i++) { db[i] = to_body(cv[i]); qv[i] = 9.0; qx[i] = -1.0; nh[i] = 0u; }
  // link hits found by tri_scatter this step (nearest, farthest, count).
  // Each link is stored once, at the cell it leaves in the positive
  // direction; the cell at its other end reads the same record mirrored,
  // so both ends always agree on whether the link is cut.
  for (var i = 1u; i < 19u; i++) {
    if ((i & 1u) == 1u) {
      let o = (k * 9u + (i - 1u) / 2u) * 3u;
      let n_ = atomicLoad(&qscr[o + 2u]);
      if (n_ > 0u) { qv[i] = bitcast<f32>(atomicLoad(&qscr[o])); qx[i] = bitcast<f32>(atomicLoad(&qscr[o + 1u])); nh[i] = n_; }
    } else {
      let q = vec3<i32>(i32(lx), i32(ly), i32(lz)) + vec3<i32>(cv[i]);
      if (all(q >= vec3<i32>(0)) && all(q < vec3<i32>(BOX_N))) {
        let k2 = u32((q.z * i32(BOX_N.y) + q.y) * i32(BOX_N.x) + q.x);
        let o = (k2 * 9u + (i - 2u) / 2u) * 3u;
        let n_ = atomicLoad(&qscr[o + 2u]);
        if (n_ > 0u) {
          qv[i] = 1.0 - bitcast<f32>(atomicLoad(&qscr[o + 1u]));
          qx[i] = 1.0 - bitcast<f32>(atomicLoad(&qscr[o]));
          nh[i] = n_;
        }
      }
    }
  }
  var mask = 0u;
  for (var i = 1u; i < 19u; i++) {
    // a wall where the neighbour is solid, or where the link crosses a
    // sheet thinner than a cell (two hits); q from the nearest hit
    if (solid_at(p + vec3<i32>(cv[i]), false)) {
      // no hit although the neighbour is solid: the body-frame occupancy
      // put the surface within ~0.1 cell of the neighbour's centre
      if (nh[i] == 0u) { qv[i] = 1.0; }
      mask |= (1u << i);
    } else if (nh[i] >= 2u && qx[i] - qv[i] > 1e-5) {
      mask |= (1u << i);
    } else {
      qv[i] = 1.0;
    }
  }
  for (var w = 0u; w < 9u; w++) {
    recs[base + 2u + w] = pack2x16float(vec2<f32>(min(qv[2u * w + 1u], 1.0), min(qv[2u * w + 2u], 1.0)));
  }
  recs[base] = mask;
  var F = vec3<f32>(0.0);
  if (solid_before) {
    // A cell the part just uncovered. Its fluid neighbours streamed real
    // populations into it last step, and they stay. But this step the
    // neighbours pull what it would have sent out last step, which it never
    // computed: those outgoing populations are filled with the equilibrium
    // at the wall velocity and the density of nearby fluid (the refill).
    // The part gives up their momentum.
    var rs = 0.0; var cnt = 0.0;
    var pis = array<f32, 6>(0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    var fluid_nb: array<bool, 19>;
    for (var i = 1u; i < 19u; i++) {
      let dprev = to_body_prev(cv[i]);
      let fluid_now = !solid_at(p + vec3<i32>(cv[i]), false);
      let fluid_before = !solid_at(p + vec3<i32>(cv[i]), true);
      fluid_nb[i] = fluid_now;
      if (fluid_now && fluid_before) {
        let q = vec3<i32>(i32(lx), i32(ly), i32(lz)) + vec3<i32>(cv[i]);
        if (all(q >= vec3<i32>(0)) && all(q < vec3<i32>(BOX_N))) {
          let k2 = u32((q.z * i32(BOX_N.y) + q.y) * i32(BOX_N.x) + q.x);
          rs += bitcast<f32>(recs[k2 * RS + 29u]);
          cnt += 1.0;
          // the neighbour's post-collision stress, for the refill's
          // non-equilibrium part
          var fp: array<f32, 19>;
          fp[0] = 0.0;
          for (var j = 1u; j < 19u; j++) { fp[j] = bitcast<f32>(recs[k2 * RS + 10u + j]); }
          let mo2 = moments(fp);
          let r2 = bitcast<f32>(recs[k2 * RS + 29u]);
          let u2 = vec3<f32>(bitcast<f32>(recs[k2 * RS + 30u]), bitcast<f32>(recs[k2 * RS + 31u]), bitcast<f32>(recs[k2 * RS + 32u]));
          // (the rest population is not stored; it does not enter the
          // traceless off-diagonal part used below)
          pis[3] += mo2.s[3] - r2 * u2.x * u2.y;
          pis[4] += mo2.s[4] - r2 * u2.x * u2.z;
          pis[5] += mo2.s[5] - r2 * u2.y * u2.z;
          let tr = (mo2.s[0] + mo2.s[1] + mo2.s[2]) / 3.0;
          pis[0] += mo2.s[0] - tr - r2 * (u2.x * u2.x - dot(u2, u2) / 3.0);
          pis[1] += mo2.s[1] - tr - r2 * (u2.y * u2.y - dot(u2, u2) / 3.0);
          pis[2] += mo2.s[2] - tr - r2 * (u2.z * u2.z - dot(u2, u2) / 3.0);
        }
      }
    }
    let rho = select(1.0, rs / cnt, cnt > 0.0);
    let uw = cross(st.omega.xyz, pc);
    var fe = feq_arr(rho - 1.0, uw);
    if (cnt > 0.0) {
      for (var j = 0u; j < 6u; j++) { pis[j] /= cnt; }
      let fq = fneq_arr(pis);
      for (var j = 0u; j < 19u; j++) { fe[j] += fq[j]; }
    }
    store_prev_out(n, xyz, fe);
    // as if the cell had been fluid at equilibrium last step: what it sends
    // to fluid neighbours enters the fluid, what it sends into the part is
    // bounced back (and counted by the links), so the part gives up the
    // refill's whole momentum
    F = -rho * uw;
    for (var i = 1u; i < 19u; i++) { recs[base + 10u + i] = bitcast<u32>(fe[i]); }
    recs[base + 29u] = bitcast<u32>(rho);
    recs[base + 30u] = bitcast<u32>(uw.x); recs[base + 31u] = bitcast<u32>(uw.y); recs[base + 32u] = bitcast<u32>(uw.z);
  }
  let T = cross(pc, F);
  recs[base + 33u] = bitcast<u32>(F.x); recs[base + 34u] = bitcast<u32>(F.y); recs[base + 35u] = bitcast<u32>(F.z);
  recs[base + 36u] = bitcast<u32>(T.x); recs[base + 37u] = bitcast<u32>(T.y); recs[base + 38u] = bitcast<u32>(T.z);
  flags[n] = (k + 1u) << 2u;
}
"#;
