//! The GPU lattice: buffers, pipelines and time stepping. Everything here is
//! in lattice units (cell size 1, time step 1, reference density 1).
//! `tunnel` turns a study into a `LatticeSetup`; tests use this directly.

use crate::gpu::{Gpu, gpu};
use crate::shader::{self, C, RED_CHUNK, RS, ShaderCfg, SliceDef, WG, weight};
use crate::voxel::{SpinVoxels, StaticVoxels};
use anyhow::{Result, bail};
use bytemuck::{Pod, Zeroable};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Storage {
    /// populations stored as f32 (76 bytes per cell)
    Fp32,
    /// populations stored as scaled IEEE half (38 bytes per cell); all
    /// arithmetic stays f32
    Fp16,
}

impl Storage {
    pub fn pop_bytes(self) -> u64 {
        match self {
            Storage::Fp32 => 4,
            Storage::Fp16 => 2,
        }
    }
    /// bytes per cell: 19 populations + one u32 flag word
    pub fn bytes_per_cell(self) -> u64 {
        19 * self.pop_bytes() + 4
    }
}

/// Boundary condition of one domain face.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaceBc {
    /// velocity inlet: u = u_inf, density from the cell inside
    Velocity,
    /// open / pressure: density 1, velocity and stress from the cell inside
    Open,
    /// wraps around (both faces of that axis must say so)
    Periodic,
}

#[derive(Clone, Debug)]
pub struct LatticeSetup {
    pub dims: [u32; 3],
    /// faces -x, +x, -y, +y, -z, +z
    pub faces: [FaceBc; 6],
    /// lattice kinematic viscosity
    pub nu: f64,
    /// Smagorinsky constant C_s (0 = no turbulence model)
    pub smagorinsky: f64,
    pub u_inf: [f64; 3],
    pub u_init: [f64; 3],
    /// uniform acceleration (lattice units), e.g. to drive a periodic channel
    pub body_force: [f64; 3],
    /// viscosity sponge width per face (cells; 0 = none)
    pub sponge: [u32; 6],
    /// relaxation time reached at the face inside a sponge
    pub sponge_tau: f64,
    /// absorbing-layer strength at the face (0 = viscosity sponge only)
    pub sponge_sigma: f64,
    /// absorbing layers pull the velocity toward `u_inf` too (wind); otherwise density only
    pub absorb_velocity: bool,
    /// `Velocity` faces hold the far-field total pressure (Bernoulli) with
    /// the velocity taken from inside, instead of a fixed velocity
    pub total_pressure_inlet: bool,
    pub storage: Storage,
    /// moment reference point and spin centre (lattice coordinates)
    pub center: [f64; 3],
    /// keep a buffer for `macros()` (tests; costs 16 bytes per cell)
    pub debug_dump: bool,
    /// wall model (Spalding's law) in wall-adjacent cells
    pub wall_model: bool,
}

impl LatticeSetup {
    pub fn periodic(&self) -> [bool; 3] {
        [self.faces[0] == FaceBc::Periodic, self.faces[2] == FaceBc::Periodic, self.faces[4] == FaceBc::Periodic]
    }
    pub fn cells(&self) -> u64 {
        self.dims.iter().map(|&d| d as u64).product()
    }
}

pub enum Geometry {
    None,
    Static(StaticVoxels),
    /// a spinning part; the box (lattice cells) holds every cell the part can reach
    Spin { vox: SpinVoxels, box_lo: [i32; 3], box_n: [u32; 3] },
}

/// Where to sample: surface vertices (body coordinates relative to the
/// centre, unit normals) and lattice-aligned slices.
#[derive(Clone, Debug, Default)]
pub struct Probes {
    pub verts: Vec<([f64; 3], [f64; 3])>,
    pub slices: Vec<SliceDef>,
    /// how far from the surface the pressure is read (cells)
    pub vertex_offset: f64,
}

/// One time step's motion. `rot` columns are the body axes in lattice
/// coordinates; `omega` is the angular velocity (rad per step).
#[derive(Clone, Copy, Debug)]
pub struct StepIn {
    pub rot: [[f64; 3]; 3],
    pub rot_prev: [[f64; 3]; 3],
    pub omega: [f64; 3],
    /// sample vertices and slices after this step
    pub sample: bool,
}

pub const IDENTITY: [[f64; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

impl StepIn {
    pub fn still(sample: bool) -> StepIn {
        StepIn { rot: IDENTITY, rot_prev: IDENTITY, omega: [0.0; 3], sample }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct StepParams {
    r: [[f32; 4]; 3],
    q: [[f32; 4]; 3],
    omega: [f32; 4],
    slot: u32,
    seed: u32,
    _pad: [u32; 34],
}

const KMAX: usize = 512;

struct Pipes {
    main: wgpu::ComputePipeline,
    face_gather: wgpu::ComputePipeline,
    face_write: wgpu::ComputePipeline,
    voxelize: Option<wgpu::ComputePipeline>,
    scatter: Option<wgpu::ComputePipeline>,
    qclear: Option<wgpu::ComputePipeline>,
    sample_verts: wgpu::ComputePipeline,
    sample_slices: wgpu::ComputePipeline,
    dump: wgpu::ComputePipeline,
}

pub struct Lattice {
    g: &'static Gpu,
    pub setup: LatticeSetup,
    pub cfg: ShaderCfg,
    pipes: [Pipes; 2],
    init: wgpu::ComputePipeline,
    reduce1: wgpu::ComputePipeline,
    reduce2: wgpu::ComputePipeline,
    bg0: wgpu::BindGroup,
    bg1: wgpu::BindGroup,
    step_buf: wgpu::Buffer,
    hist: wgpu::Buffer,
    hist_read: wgpu::Buffer,
    vacc: wgpu::Buffer,
    sacc: wgpu::Buffer,
    dump: wgpu::Buffer,
    _keep: Vec<wgpu::Buffer>,
    pub steps_done: u64,
    pub nrec: u32,
    pub nface: u32,
    pub npix: u32,
    pub memory_bytes: u64,
}

fn f16_bits(x: f32) -> u16 {
    // round-to-nearest conversion for finite values in the normal range
    let b = x.to_bits();
    let sign = ((b >> 16) & 0x8000) as u16;
    let exp = ((b >> 23) & 0xff) as i32 - 127 + 15;
    let man = b & 0x7f_ffff;
    if exp <= 0 {
        return sign;
    }
    if exp >= 31 {
        return sign | 0x7c00;
    }
    let mut h = (sign as u32) | ((exp as u32) << 10) | (man >> 13);
    if man & 0x1000 != 0 {
        h += 1;
    }
    h as u16
}

fn feq_shifted(u: [f64; 3]) -> [f32; 19] {
    let usq = 1.5 * (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]);
    let mut f = [0f32; 19];
    for i in 0..19 {
        let c = C[i];
        let cu = c[0] as f64 * u[0] + c[1] as f64 * u[1] + c[2] as f64 * u[2];
        f[i] = (weight(i) * (3.0 * cu + 4.5 * cu * cu - usq)) as f32;
    }
    f
}

fn record_words(cell: u32, mask: u32, q: &[f32], u: [f64; 3]) -> [u32; RS as usize] {
    let mut w = [0u32; RS as usize];
    w[0] = mask;
    w[1] = cell;
    for k in 0..9 {
        w[2 + k] = f16_bits(q[2 * k + 1]) as u32 | ((f16_bits(q[2 * k + 2]) as u32) << 16);
    }
    let fe = feq_shifted(u);
    for i in 1..19 {
        w[10 + i] = fe[i].to_bits();
    }
    w[29] = 1f32.to_bits();
    for k in 0..3 {
        w[30 + k] = (u[k] as f32).to_bits();
    }
    w
}

impl Lattice {
    /// Largest number of cells this storage can hold on this GPU, by the
    /// buffer layout alone (memory budgets are the caller's business).
    pub fn max_cells(storage: Storage) -> u64 {
        let maxb = gpu().map(|g| g.max_buffer()).unwrap_or((1 << 32) - 256);
        // at most 12 buffers for 19 directions: each holds at least two
        maxb / 2 / storage.pop_bytes()
    }

    pub fn new(setup: LatticeSetup, geom: Geometry, probes: Probes) -> Result<Lattice> {
        let g = gpu()?;
        let dev = &g.device;
        let n = setup.cells();
        if n == 0 || n >= u32::MAX as u64 {
            bail!("grid of {n} cells is out of range");
        }
        if setup.dims.iter().any(|&d| d < 3) {
            bail!("grid must be at least 3 cells in every direction");
        }
        let periodic = setup.periodic();
        for a in 0..3 {
            if (setup.faces[2 * a] == FaceBc::Periodic) != (setup.faces[2 * a + 1] == FaceBc::Periodic) {
                bail!("periodic faces must come in pairs");
            }
        }
        let storage = if setup.storage == Storage::Fp16 && !g.f16 { Storage::Fp32 } else { setup.storage };
        let pb = storage.pop_bytes();
        let per_dir = n * pb;
        let maxb = g.max_buffer();
        if per_dir > maxb {
            bail!("grid too large for one GPU buffer per direction ({} cells)", n);
        }
        let mut per = ((maxb / per_dir) as usize).clamp(1, 19);
        let nbuf = 19usize.div_ceil(per);
        if nbuf > shader::MAX_POP_BUFS {
            bail!("grid needs too many GPU buffers");
        }
        per = 19usize.div_ceil(nbuf);

        // geometry
        let (moving, nrec, box_lo, box_n) = match &geom {
            Geometry::Spin { box_lo, box_n, .. } => (true, box_n.iter().map(|&v| v as u64).product::<u64>() as u32, *box_lo, *box_n),
            Geometry::Static(v) => (false, v.walls.len() as u32, [0; 3], [1; 3]),
            Geometry::None => (false, 0, [0; 3], [1; 3]),
        };
        if moving {
            for k in 0..3 {
                if box_lo[k] < 1 || box_lo[k] as i64 + box_n[k] as i64 > setup.dims[k] as i64 - 1 {
                    bail!("the spinning part's swept box leaves the domain");
                }
            }
        }
        let (occ_lo, occ_h, occ_n, bin_lo, bin_n) = match &geom {
            Geometry::Spin { vox, .. } => (
                vox.occ_lo,
                vox.occ_h,
                [vox.occ_dims[0] as u32, vox.occ_dims[1] as u32, vox.occ_dims[2] as u32],
                vox.bins.lo,
                [vox.bins.dims[0] as u32, vox.bins.dims[1] as u32, vox.bins.dims[2] as u32],
            ),
            _ => ([0.0; 3], 1.0, [1; 3], [0.0; 3], [1; 3]),
        };

        // face cells (non-periodic axes)
        let [nx, ny, nz] = setup.dims;
        let mut faces: Vec<u32> = Vec::new();
        let face_type = |x: u32, y: u32, z: u32| -> Option<u32> {
            let mut on = [false; 6];
            on[0] = x == 0;
            on[1] = x == nx - 1;
            on[2] = y == 0;
            on[3] = y == ny - 1;
            on[4] = z == 0;
            on[5] = z == nz - 1;
            let mut ty = None;
            for f in 0..6 {
                if on[f] && setup.faces[f] != FaceBc::Periodic {
                    let t = if setup.faces[f] == FaceBc::Velocity { 2 } else { 3 };
                    ty = Some(match ty {
                        Some(2) => 2,
                        _ => t,
                    });
                }
            }
            ty
        };
        for z in 0..nz {
            for y in 0..ny {
                let edge_yz = y == 0 || y == ny - 1 || z == 0 || z == nz - 1;
                if edge_yz {
                    for x in 0..nx {
                        if face_type(x, y, z).is_some() {
                            faces.push((z * ny + y) * nx + x);
                        }
                    }
                } else {
                    for x in [0, nx - 1] {
                        if face_type(x, y, z).is_some() {
                            faces.push((z * ny + y) * nx + x);
                        }
                    }
                }
            }
        }
        let nface = faces.len() as u32;

        let npix: u32 = probes.slices.iter().map(|s| s.nu * s.nv).sum();
        let nvert = probes.verts.len() as u32;
        let tau0 = 0.5 + 3.0 * setup.nu;
        let cfg = ShaderCfg {
            dims: setup.dims,
            periodic,
            fp16: storage == Storage::Fp16,
            per_buf: per as u32,
            tau0,
            smag2: setup.smagorinsky * setup.smagorinsky,
            sponge_tau: setup.sponge_tau,
            sigma: setup.sponge_sigma,
            absorb_velocity: setup.absorb_velocity,
            total_pressure_inlet: setup.total_pressure_inlet,
            sponge_lo: [setup.sponge[0], setup.sponge[2], setup.sponge[4]],
            sponge_hi: [setup.sponge[1], setup.sponge[3], setup.sponge[5]],
            u_inf: setup.u_inf,
            u_init: setup.u_init,
            grav: setup.body_force,
            center: setup.center,
            moving,
            nrec,
            nface,
            nvert,
            box_lo,
            box_n,
            occ_lo,
            occ_h,
            occ_n,
            bin_lo,
            bin_n,
            slices: probes.slices.clone(),
            vertex_offset: probes.vertex_offset,
            ntri: match &geom {
                Geometry::Spin { vox, .. } => vox.tris.len() as u32,
                _ => 0,
            },
            wall_model: setup.wall_model,
        };

        let scope = dev.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let vscope = dev.push_error_scope(wgpu::ErrorFilter::Validation);
        let st = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let mk = |label: &str, size: u64| -> wgpu::Buffer {
            dev.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size.max(16).next_multiple_of(16), usage: st, mapped_at_creation: false })
        };
        let mut mem = 0u64;
        let mut pops = Vec::new();
        for b in 0..nbuf {
            let dirs = per.min(19 - b * per) as u64;
            let sz = dirs * per_dir;
            mem += sz;
            pops.push(mk(&format!("pop{b}"), sz));
        }
        let flags = mk("flags", n * 4);
        let recs = mk("recs", nrec as u64 * RS as u64 * 4);
        let faces_b = mk("faces", nface as u64 * 4);
        let facebuf = mk("facebuf", nface as u64 * 40);
        let npart = nrec.max(1).div_ceil(RED_CHUNK);
        let partials = mk("partials", npart as u64 * 32);
        let hist = mk("hist", KMAX as u64 * 32);
        let (occ_bits, bstart, blist, tris): (&[u32], &[u32], &[u32], Vec<f32>) = match &geom {
            Geometry::Spin { vox, .. } => (&vox.occ_bits, &vox.bins.start, &vox.bins.list, vox.tris.iter().flatten().copied().collect()),
            _ => (&[], &[], &[], vec![]),
        };
        let occ = mk("occ", occ_bits.len() as u64 * 4);
        let bin_start = mk("bin_start", bstart.len() as u64 * 4);
        let bin_tris = mk("bin_tris", blist.len() as u64 * 4);
        let tris_b = mk("tris", tris.len() as u64 * 4);
        let verts = mk("verts", nvert as u64 * 32);
        let vacc = mk("vacc", nvert as u64 * 8);
        let sacc = mk("sacc", npix as u64 * 20);
        let slicedef = mk("slicedef", (1 + 3 * probes.slices.len() as u64) * 16);
        let dump = mk("dump", if setup.debug_dump { n * 16 } else { 16 });
        let qscr = mk("qscr", if moving { nrec as u64 * 9 * 12 } else { 16 });
        mem += flags.size() + recs.size() + faces_b.size() + facebuf.size() + occ.size() + bin_start.size() + bin_tris.size() + tris_b.size() + verts.size() + vacc.size() + sacc.size() + dump.size() + qscr.size();
        let step_buf = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("steps"),
            size: (KMAX * 256) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let hist_read = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist_read"),
            size: KMAX as u64 * 32,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        if let Some(e) = pollster::block_on(vscope.pop()) {
            bail!("GPU setup failed: {e}");
        }
        if let Some(e) = pollster::block_on(scope.pop()) {
            bail!("not enough GPU memory for this grid ({:.1} GB): {e}", mem as f64 / 1e9);
        }

        // uploads
        let q = &g.queue;
        q.write_buffer(&faces_b, 0, bytemuck::cast_slice(&faces));
        if !occ_bits.is_empty() {
            q.write_buffer(&occ, 0, bytemuck::cast_slice(occ_bits));
            q.write_buffer(&bin_start, 0, bytemuck::cast_slice(bstart));
            q.write_buffer(&bin_tris, 0, bytemuck::cast_slice(blist));
            q.write_buffer(&tris_b, 0, bytemuck::cast_slice(&tris));
        }
        if moving {
            let mut init = vec![0u32; nrec as usize * 9 * 3];
            for c in init.chunks_exact_mut(3) {
                c[0] = 0x7f7f_ffff;
            }
            q.write_buffer(&qscr, 0, bytemuck::cast_slice(&init));
        }
        if nvert > 0 {
            let v: Vec<f32> = probes
                .verts
                .iter()
                .flat_map(|(p, nn)| [p[0] as f32, p[1] as f32, p[2] as f32, 0.0, nn[0] as f32, nn[1] as f32, nn[2] as f32, 0.0])
                .collect();
            q.write_buffer(&verts, 0, bytemuck::cast_slice(&v));
        }
        {
            let mut sd: Vec<i32> = vec![probes.slices.len() as i32, 0, 0, 0];
            for s in &probes.slices {
                sd.extend_from_slice(&[s.origin[0], s.origin[1], s.origin[2], s.nu as i32]);
                sd.extend_from_slice(&[s.u[0], s.u[1], s.u[2], s.nv as i32]);
                sd.extend_from_slice(&[s.v[0], s.v[1], s.v[2], 0]);
            }
            q.write_buffer(&slicedef, 0, bytemuck::cast_slice(&sd));
        }
        // records
        let mut rec_of: Vec<(u32, u32)> = Vec::new(); // (cell, record index + 1)
        match &geom {
            Geometry::Static(v) => {
                let mut words: Vec<u32> = Vec::with_capacity(v.walls.len() * RS as usize);
                for (r, w) in v.walls.iter().enumerate() {
                    words.extend_from_slice(&record_words(w.cell, w.mask, &w.q, setup.u_init));
                    rec_of.push((w.cell, r as u32 + 1));
                }
                q.write_buffer(&recs, 0, bytemuck::cast_slice(&words));
            }
            Geometry::Spin { .. } => {
                let ones = [1.0f32; 27];
                let mut words: Vec<u32> = Vec::with_capacity(nrec as usize * RS as usize);
                let mut k = 0u32;
                for z in 0..box_n[2] {
                    for y in 0..box_n[1] {
                        for x in 0..box_n[0] {
                            let c = ((z as i64 + box_lo[2] as i64) * ny as i64 + (y as i64 + box_lo[1] as i64)) * nx as i64 + x as i64 + box_lo[0] as i64;
                            words.extend_from_slice(&record_words(c as u32, 0, &ones, setup.u_init));
                            rec_of.push((c as u32, k + 1));
                            k += 1;
                        }
                    }
                }
                q.write_buffer(&recs, 0, bytemuck::cast_slice(&words));
            }
            Geometry::None => {}
        }
        rec_of.sort_unstable();
        let mut solid: Vec<u32> = match &geom {
            Geometry::Static(v) => v.solid.clone(),
            _ => vec![],
        };
        solid.sort_unstable();
        // flags, written in slabs
        {
            let slab = (16u64 << 20).max(nx as u64 * ny as u64) / (nx as u64 * ny as u64);
            let mut fi = 0usize;
            let mut si = 0usize;
            let mut z0 = 0u32;
            while z0 < nz {
                let z1 = (z0 as u64 + slab).min(nz as u64) as u32;
                let c0 = z0 as u64 * nx as u64 * ny as u64;
                let len = (z1 - z0) as u64 * nx as u64 * ny as u64;
                let mut fl = vec![0u32; len as usize];
                for (l, v) in fl.iter_mut().enumerate() {
                    let c = (c0 + l as u64) as u32;
                    let x = c % nx;
                    let y = (c / nx) % ny;
                    let z = c / (nx * ny);
                    if let Some(t) = face_type(x, y, z) {
                        *v = t;
                    }
                }
                while si < solid.len() && (solid[si] as u64) < c0 + len {
                    let l = (solid[si] as u64 - c0) as usize;
                    if fl[l] == 0 {
                        fl[l] = 1;
                    }
                    si += 1;
                }
                while fi < rec_of.len() && (rec_of[fi].0 as u64) < c0 + len {
                    let l = (rec_of[fi].0 as u64 - c0) as usize;
                    if fl[l] & 3 == 0 {
                        fl[l] = rec_of[fi].1 << 2;
                    }
                    fi += 1;
                }
                q.write_buffer(&flags, c0 * 4, bytemuck::cast_slice(&fl));
                z0 = z1;
            }
        }

        // pipelines
        let mut entries: Vec<wgpu::BindGroupLayoutEntry> = Vec::new();
        let sto = |b: u32| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        for b in 0..nbuf as u32 {
            entries.push(sto(b));
        }
        for b in 12..=27 {
            entries.push(sto(b));
        }
        let bgl0 = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("bgl0"), entries: &entries });
        let bgl1 = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl1"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(256),
                },
                count: None,
            }],
        });
        let pl = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("pl"), bind_group_layouts: &[Some(&bgl0), Some(&bgl1)], immediate_size: 0 });
        let others: [(u32, &wgpu::Buffer); 16] = [
            (12, &flags),
            (13, &recs),
            (14, &faces_b),
            (15, &facebuf),
            (16, &partials),
            (17, &hist),
            (18, &occ),
            (19, &bin_start),
            (20, &bin_tris),
            (21, &tris_b),
            (22, &verts),
            (23, &vacc),
            (24, &sacc),
            (25, &dump),
            (26, &slicedef),
            (27, &qscr),
        ];
        let mut bge: Vec<wgpu::BindGroupEntry> = Vec::new();
        for (b, buf) in pops.iter().enumerate() {
            bge.push(wgpu::BindGroupEntry { binding: b as u32, resource: buf.as_entire_binding() });
        }
        for (b, buf) in others.iter() {
            bge.push(wgpu::BindGroupEntry { binding: *b, resource: buf.as_entire_binding() });
        }
        let bg0 = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("bg0"), layout: &bgl0, entries: &bge });
        let bg1 = dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg1"),
            layout: &bgl1,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &step_buf, offset: 0, size: wgpu::BufferSize::new(256) }),
            }],
        });

        let vscope = dev.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = |p: u32| {
            let src = cfg.source(p);
            if std::env::var_os("SIMLAB_CFD_DUMP_WGSL").is_some() {
                let _ = std::fs::write(std::env::temp_dir().join(format!("simlab_cfd_{p}.wgsl")), &src);
            }
            // SAFETY: every index in the generated kernels is bounded by the
            // grid, record and list sizes baked into the source; bounds checks
            // are off for speed and so buffers near 4 GiB index correctly.
            unsafe {
                dev.create_shader_module_trusted(
                    wgpu::ShaderModuleDescriptor { label: Some("lbm"), source: wgpu::ShaderSource::Wgsl(src.into()) },
                    wgpu::ShaderRuntimeChecks::unchecked(),
                )
            }
        };
        let mods = [module(0), module(1)];
        let pipe = |m: &wgpu::ShaderModule, e: &str| {
            dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(e),
                layout: Some(&pl),
                module: m,
                entry_point: Some(e),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let mkpipes = |m: &wgpu::ShaderModule| Pipes {
            main: pipe(m, "main_step"),
            face_gather: pipe(m, "face_gather"),
            face_write: pipe(m, "face_write"),
            voxelize: if moving { Some(pipe(m, "voxelize")) } else { None },
            scatter: if moving { Some(pipe(m, "tri_scatter")) } else { None },
            qclear: if moving { Some(pipe(m, "qclear")) } else { None },
            sample_verts: pipe(m, "sample_verts"),
            sample_slices: pipe(m, "sample_slices"),
            dump: pipe(m, "dump_macro"),
        };
        let pipes = [mkpipes(&mods[0]), mkpipes(&mods[1])];
        let init = pipe(&mods[0], "init");
        let reduce1 = pipe(&mods[0], "reduce1");
        let reduce2 = pipe(&mods[0], "reduce2");
        if let Some(e) = pollster::block_on(vscope.pop()) {
            bail!("GPU kernels failed to build: {e}");
        }

        let lat = Lattice {
            g,
            setup: LatticeSetup { storage, ..setup },
            cfg,
            pipes,
            init,
            reduce1,
            reduce2,
            bg0,
            bg1,
            step_buf,
            hist,
            hist_read,
            vacc,
            sacc,
            dump,
            _keep: [pops, vec![flags, recs, faces_b, facebuf, partials, occ, bin_start, bin_tris, tris_b, verts, slicedef, qscr]].concat(),
            steps_done: 0,
            nrec,
            nface,
            npix,
            memory_bytes: mem,
        };
        // initial state
        q.write_buffer(&lat.step_buf, 0, bytemuck::bytes_of(&lat.params(&StepIn::still(false), 0)));
        let mut enc = dev.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &lat.bg0, &[]);
            pass.set_bind_group(1, &lat.bg1, &[0]);
            lat.disp(&mut pass, &lat.init, n);
        }
        q.submit([enc.finish()]);
        dev.poll(wgpu::PollType::wait_indefinitely()).ok();
        Ok(lat)
    }

    fn params(&self, s: &StepIn, slot: u32) -> StepParams {
        let seed = (self.steps_done as u32).wrapping_add(slot).wrapping_mul(2_654_435_761);
        let col = |m: &[[f64; 3]; 3], k: usize| [m[0][k] as f32, m[1][k] as f32, m[2][k] as f32, 0.0];
        StepParams {
            r: [col(&s.rot, 0), col(&s.rot, 1), col(&s.rot, 2)],
            q: [col(&s.rot_prev, 0), col(&s.rot_prev, 1), col(&s.rot_prev, 2)],
            omega: [s.omega[0] as f32, s.omega[1] as f32, s.omega[2] as f32, 0.0],
            slot,
            seed,
            _pad: [0; 34],
        }
    }

    fn disp(&self, pass: &mut wgpu::ComputePass, p: &wgpu::ComputePipeline, threads: u64) {
        if threads == 0 {
            return;
        }
        let wgs = threads.div_ceil(WG as u64);
        let x = wgs.min(65535);
        let y = wgs.div_ceil(x);
        pass.set_pipeline(p);
        pass.dispatch_workgroups(x as u32, y as u32, 1);
    }

    /// Runs the given steps; returns force and moment (lattice units, about
    /// `center`) on the geometry after each step.
    pub fn run(&mut self, steps: &[StepIn]) -> Result<Vec<[f64; 6]>> {
        let mut out = Vec::with_capacity(steps.len());
        for chunk in steps.chunks(KMAX) {
            let mut pbytes: Vec<u8> = Vec::with_capacity(chunk.len() * 256);
            for (k, s) in chunk.iter().enumerate() {
                pbytes.extend_from_slice(bytemuck::bytes_of(&self.params(s, k as u32)));
            }
            self.g.queue.write_buffer(&self.step_buf, 0, &pbytes);
            let mut enc = self.g.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_bind_group(0, &self.bg0, &[]);
                let n = self.setup.cells();
                let npart = self.nrec.max(1).div_ceil(RED_CHUNK);
                for (k, s) in chunk.iter().enumerate() {
                    let par = ((self.steps_done + k as u64) % 2) as usize;
                    pass.set_bind_group(1, &self.bg1, &[(k * 256) as u32]);
                    let pp = &self.pipes[par];
                    if let (Some(v), Some(sc), Some(qc)) = (&pp.voxelize, &pp.scatter, &pp.qclear) {
                        self.disp(&mut pass, sc, self.cfg.ntri as u64);
                        self.disp(&mut pass, v, self.nrec as u64);
                        self.disp(&mut pass, qc, self.nrec as u64 * 9);
                    }
                    if self.nface > 0 {
                        self.disp(&mut pass, &pp.face_gather, self.nface as u64);
                        self.disp(&mut pass, &pp.face_write, self.nface as u64);
                    }
                    self.disp(&mut pass, &pp.main, n);
                    pass.set_pipeline(&self.reduce1);
                    pass.dispatch_workgroups(npart, 1, 1);
                    pass.set_pipeline(&self.reduce2);
                    pass.dispatch_workgroups(1, 1, 1);
                    if s.sample {
                        let pn = &self.pipes[1 - par];
                        self.disp(&mut pass, &pn.sample_verts, self.cfg.nvert as u64);
                        self.disp(&mut pass, &pn.sample_slices, self.npix as u64);
                    }
                }
            }
            enc.copy_buffer_to_buffer(&self.hist, 0, &self.hist_read, 0, chunk.len() as u64 * 32);
            self.g.queue.submit([enc.finish()]);
            let h: Vec<f32> = self.read_mapped(&self.hist_read, chunk.len() as u64 * 32)?;
            for k in 0..chunk.len() {
                let mut v = [0f64; 6];
                for j in 0..6 {
                    v[j] = h[k * 8 + j] as f64;
                }
                out.push(v);
            }
            self.steps_done += chunk.len() as u64;
        }
        Ok(out)
    }

    fn read_mapped<T: Pod>(&self, buf: &wgpu::Buffer, size: u64) -> Result<Vec<T>> {
        let slice = buf.slice(0..size);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.g.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("GPU wait failed: {e}"))?;
        rx.recv()?.map_err(|e| anyhow::anyhow!("GPU read failed: {e}"))?;
        let v = {
            let view = slice.get_mapped_range().map_err(|e| anyhow::anyhow!("GPU map failed: {e:?}"))?;
            bytemuck::cast_slice::<u8, T>(&view).to_vec()
        };
        buf.unmap();
        Ok(v)
    }

    fn read_buffer<T: Pod>(&self, src: &wgpu::Buffer, size: u64) -> Result<Vec<T>> {
        let size = size.next_multiple_of(4);
        let staging = self.g.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("read"),
            size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.g.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(src, 0, &staging, 0, size);
        self.g.queue.submit([enc.finish()]);
        self.read_mapped(&staging, size)
    }

    /// Clears the vertex and slice accumulators (start of an averaging window).
    pub fn reset_samples(&mut self) {
        let mut enc = self.g.device.create_command_encoder(&Default::default());
        enc.clear_buffer(&self.vacc, 0, None);
        enc.clear_buffer(&self.sacc, 0, None);
        self.g.queue.submit([enc.finish()]);
    }

    /// Mean density at each probe vertex over the samples taken (NaN if none).
    pub fn vertex_means(&self) -> Result<Vec<f32>> {
        let nv = self.cfg.nvert as usize;
        if nv == 0 {
            return Ok(vec![]);
        }
        let v: Vec<f32> = self.read_buffer(&self.vacc, nv as u64 * 8)?;
        Ok((0..nv).map(|i| if v[2 * i + 1] > 0.0 { v[2 * i] / v[2 * i + 1] } else { f32::NAN }).collect())
    }

    /// Mean (rho, ux, uy, uz) per slice pixel, all slices in order (NaN where never fluid).
    pub fn slice_means(&self) -> Result<Vec<[f32; 4]>> {
        let np = self.npix as usize;
        if np == 0 {
            return Ok(vec![]);
        }
        let v: Vec<f32> = self.read_buffer(&self.sacc, np as u64 * 20)?;
        Ok((0..np)
            .map(|i| {
                let c = v[5 * i + 4];
                if c > 0.0 { [v[5 * i] / c, v[5 * i + 1] / c, v[5 * i + 2] / c, v[5 * i + 3] / c] } else { [f32::NAN; 4] }
            })
            .collect())
    }

    /// (rho, ux, uy, uz) of every cell now (needs `debug_dump`).
    pub fn macros(&self) -> Result<Vec<[f32; 4]>> {
        if !self.setup.debug_dump {
            bail!("macros() needs debug_dump");
        }
        let n = self.setup.cells();
        let par = (self.steps_done % 2) as usize;
        let mut enc = self.g.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &self.bg0, &[]);
            pass.set_bind_group(1, &self.bg1, &[0]);
            self.disp(&mut pass, &self.pipes[par].dump, n);
        }
        self.g.queue.submit([enc.finish()]);
        let v: Vec<f32> = self.read_buffer(&self.dump, n * 16)?;
        Ok(v.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect())
    }

    /// Record words (for tests/diagnostics).
    pub fn records(&self) -> Result<Vec<u32>> {
        let recs = &self._keep[self._keep.len() - 11];
        self.read_buffer(recs, self.nrec.max(1) as u64 * RS as u64 * 4)
    }
}
