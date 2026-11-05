//! The multi-level D3Q27 lattice on the GPU: nested node-aligned boxes,
//! each half the cell size of its parent and stepped twice per parent step
//! (acoustic scaling: the same lattice velocity on every level). The still
//! part lives on the finest level; the coarsest level holds the domain
//! faces. Lattice units of the finest level unless stated.

use crate::d3q27::{Q, REST, slot};
use crate::gpu::{Gpu, gpu};
use crate::lattice::{FaceBc, Storage};
use crate::mshader::{self, LevelShader, MW, RED_CHUNK, RS, SliceDef, T_FACE, T_GHOST, T_RESTRICT, T_SOLID, WG};
use crate::voxel::StaticVoxels;
use anyhow::{Result, bail};
use bytemuck::{Pod, Zeroable};

/// One level's box: `lo` is the parent node of this level's node 0 and `n`
/// the number of parent nodes it spans per axis (dims = 2 (n - 1) + 1).
#[derive(Clone, Copy, Debug)]
pub struct LevelBox {
    pub lo: [i32; 3],
    pub n: [u32; 3],
}

#[derive(Clone, Debug)]
pub struct MultiSetup {
    /// coarsest level size in its cells
    pub dims0: [u32; 3],
    /// boxes of levels 1.. (each in its parent's node coordinates)
    pub boxes: Vec<LevelBox>,
    pub faces: [FaceBc; 6],
    /// lattice viscosity on the coarsest level (doubles per finer level)
    pub nu0: f64,
    pub smagorinsky: f64,
    pub u_inf: [f64; 3],
    pub u_init: [f64; 3],
    /// acceleration on the coarsest level, lattice units (halves per level)
    pub body_force0: [f64; 3],
    /// sponge widths on the coarsest level (-x +x -y +y -z +z, cells)
    pub sponge: [u32; 6],
    pub sponge_tau: f64,
    pub sponge_sigma: f64,
    pub absorb_velocity: bool,
    pub total_pressure_inlet: bool,
    pub storage: Storage,
    /// moment reference, finest-level coordinates
    pub center: [f64; 3],
    pub debug_dump: bool,
}

impl MultiSetup {
    pub fn levels(&self) -> usize {
        self.boxes.len() + 1
    }
    pub fn dims(&self, l: usize) -> [u32; 3] {
        if l == 0 {
            self.dims0
        } else {
            let b = self.boxes[l - 1];
            [2 * (b.n[0] - 1) + 1, 2 * (b.n[1] - 1) + 1, 2 * (b.n[2] - 1) + 1]
        }
    }
    pub fn cells(&self) -> Vec<u64> {
        (0..self.levels()).map(|l| self.dims(l).iter().map(|&d| d as u64).product()).collect()
    }
    /// bytes on the GPU (populations A/B, flags, moment buffers), excluding geometry
    pub fn bytes(&self) -> u64 {
        self.cells().iter().map(|&n| n * (2 * Q as u64 * self.storage.pop_bytes() + 4 + 3 * MW as u64 * 4 / 8)).sum()
    }
}

pub enum MGeometry {
    None,
    Static(StaticVoxels),
}

/// Sampling: surface vertices (body coordinates relative to the centre,
/// finest cells), and slices given in finest-level node coordinates with a
/// pixel spacing of `pitch` finest cells.
#[derive(Clone, Debug, Default)]
pub struct MProbes {
    pub verts: Vec<([f64; 3], [f64; 3])>,
    pub vertex_offset: f64,
    pub slices: Vec<(SliceDef, u32)>,
}

pub use crate::lattice::StepIn;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct StepParams {
    r: [[f32; 4]; 3],
    q: [[f32; 4]; 3],
    omega: [f32; 4],
    src: u32,
    hslot: u32,
    mw: u32,
    mold: u32,
    mnew: u32,
    seed: u32,
    blend: f32,
    _p3: u32,
    _pad: [u32; 28],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Face,
    Step,
    Ghost,
    Restrict,
    Reduce,
    Verts,
    Slices,
}

struct LevelGpu {
    cfg: LevelShader,
    dims: [u32; 3],
    n: u64,
    pipes: Vec<(Kind, wgpu::ComputePipeline)>,
    init: wgpu::ComputePipeline,
    dump: wgpu::ComputePipeline,
    bg: wgpu::BindGroup,
    _bufs: Vec<wgpu::Buffer>,
    /// which population set is the source for the next step
    src: u32,
    /// moment slot written by the latest step
    mslot: u32,
    steps: u64,
}

impl LevelGpu {
    fn pipe(&self, k: Kind) -> &wgpu::ComputePipeline {
        &self.pipes.iter().find(|p| p.0 == k).unwrap().1
    }
}

pub struct MultiLattice {
    g: &'static Gpu,
    pub setup: MultiSetup,
    levels: Vec<LevelGpu>,
    bgl1: wgpu::BindGroupLayout,
    step_buf: wgpu::Buffer,
    step_cap: usize,
    bg1: wgpu::BindGroup,
    red: wgpu::Buffer,
    red_read: wgpu::Buffer,
    probes: wgpu::Buffer,
    sacc: wgpu::Buffer,
    recs: wgpu::Buffer,
    dumps: Vec<wgpu::Buffer>,
    hist_off: u32,
    hist_cap: usize,
    pub nrec: u32,
    pub nvert: u32,
    pub npix: u32,
    pub memory_bytes: u64,
    pub fine_steps: u64,
}

pub fn f16_bits(x: f32) -> u16 {
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

fn rec_words(cell: u32, mask: u32, q: &[f32; 27]) -> [u32; RS as usize] {
    let mut w = [0u32; RS as usize];
    w[0] = mask;
    w[1] = cell;
    let mut qs = [1.0f32; 26];
    for i in 0..Q {
        if i != REST {
            qs[slot(i)] = q[i];
        }
    }
    for k in 0..13 {
        w[2 + k] = f16_bits(qs[2 * k]) as u32 | ((f16_bits(qs[2 * k + 1]) as u32) << 16);
    }
    w
}

impl MultiLattice {
    pub fn new(setup: MultiSetup, geom: MGeometry, probes: MProbes) -> Result<MultiLattice> {
        let g = gpu()?;
        let dev = &g.device;
        let nl = setup.levels();
        let storage = if setup.storage == Storage::Fp16 && !g.f16 { Storage::Fp32 } else { setup.storage };
        let finest = nl - 1;
        let periodic0 = [setup.faces[0] == FaceBc::Periodic, setup.faces[2] == FaceBc::Periodic, setup.faces[4] == FaceBc::Periodic];
        if nl > 1 && periodic0.iter().any(|&p| p) {
            bail!("periodic faces only work without refinement");
        }
        // check nesting
        for l in 1..nl {
            let b = setup.boxes[l - 1];
            let pd = setup.dims(l - 1);
            for k in 0..3 {
                let hi = b.lo[k] as i64 + b.n[k] as i64 - 1;
                let margin = if l == 1 { 2 } else { 3 };
                if b.lo[k] < margin || hi > pd[k] as i64 - 1 - margin as i64 || b.n[k] < 8 {
                    bail!("refinement box {l} does not fit inside its parent");
                }
            }
        }
        let maxb = g.max_buffer();

        // geometry (finest level)
        let nrec = match &geom {
            MGeometry::Static(v) => v.walls.len() as u32,
            MGeometry::None => 0,
        };
        let st = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let mk = |label: &str, size: u64| -> wgpu::Buffer {
            dev.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size.max(16).next_multiple_of(16), usage: st, mapped_at_creation: false })
        };
        let oom = dev.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let mut mem = 0u64;

        // shared (finest-level) buffers
        let recs = mk("recs", nrec as u64 * RS as u64 * 4);
        let hist_cap = 1024usize;
        let npart = nrec.max(1).div_ceil(RED_CHUNK);
        let hist_off = npart * 8;
        let red = mk("red", (hist_off as u64 + hist_cap as u64 * 8) * 4);
        let red_read = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("red_read"),
            size: hist_cap as u64 * 32,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let nvert = probes.verts.len() as u32;
        let probes_b = mk("probes", nvert as u64 * 32);
        mem += recs.size() + red.size() + probes_b.size();

        // slice pixels: each pixel taken from the finest level covering it
        let npix: u32 = probes.slices.iter().map(|(s, _)| s.nu * s.nv).sum();
        let sacc = mk("sacc", npix as u64 * 20);
        mem += sacc.size();
        // finest-level node coordinates of each level's node 0 and its scale
        let mut org = vec![[0f64; 3]; nl];
        let mut scale = vec![1f64; nl];
        {
            // level 0 origin = 0, scale 2^(finest)
            scale[0] = (1u64 << finest) as f64;
            for l in 1..nl {
                let b = setup.boxes[l - 1];
                scale[l] = scale[l - 1] / 2.0;
                for k in 0..3 {
                    org[l][k] = org[l - 1][k] + b.lo[k] as f64 * scale[l - 1];
                }
            }
        }
        // finest coords -> (level, cell) for slices
        let mut pix_lists: Vec<Vec<u32>> = vec![Vec::new(); nl];
        {
            let mut base = 0u32;
            for (sd, _pitch) in &probes.slices {
                for iv in 0..sd.nv {
                    for iu in 0..sd.nu {
                        let p = [
                            (sd.origin[0] + iu as i32 * sd.u[0] + iv as i32 * sd.v[0]) as f64,
                            (sd.origin[1] + iu as i32 * sd.u[1] + iv as i32 * sd.v[1]) as f64,
                            (sd.origin[2] + iu as i32 * sd.u[2] + iv as i32 * sd.v[2]) as f64,
                        ];
                        // finest level whose interior (excluding the ghost layer) holds p
                        let mut chosen = None;
                        for l in (0..nl).rev() {
                            let d = setup.dims(l);
                            let mut ok = true;
                            let mut cell = [0u32; 3];
                            for k in 0..3 {
                                let x = ((p[k] - org[l][k]) / scale[l]).round();
                                let lo = if l == 0 { 1.0 } else { 2.0 };
                                if x < lo || x > d[k] as f64 - 1.0 - lo {
                                    ok = false;
                                    break;
                                }
                                cell[k] = x as u32;
                            }
                            if ok {
                                chosen = Some((l, (cell[2] * d[1] + cell[1]) * d[0] + cell[0]));
                                break;
                            }
                        }
                        if let Some((l, cidx)) = chosen {
                            pix_lists[l].push(base + iv * sd.nu + iu);
                            pix_lists[l].push(cidx);
                        }
                    }
                }
                base += sd.nu * sd.nv;
            }
        }

        // bind group layouts
        let sto = |b: u32| wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let bgl1 = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl1"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: true, min_binding_size: wgpu::BufferSize::new(256) },
                count: None,
            }],
        });
        let step_cap = 8192usize;
        let step_buf = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("steps"),
            size: (step_cap * 256) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg1 = dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg1"),
            layout: &bgl1,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &step_buf, offset: 0, size: wgpu::BufferSize::new(256) }),
            }],
        });

        // per-level buffers
        struct Pre {
            cfg: LevelShader,
            dims: [u32; 3],
            n: u64,
            pops: Vec<wgpu::Buffer>,
            flags: wgpu::Buffer,
            lists: wgpu::Buffer,
            mom_child: wgpu::Buffer,
            mom_up: wgpu::Buffer,
            spix: wgpu::Buffer,
            dump: wgpu::Buffer,
        }
        let mut pre: Vec<Pre> = Vec::new();
        let q = &g.queue;
        let mut rec_of: Vec<(u32, u32)> = Vec::new();
        match &geom {
            MGeometry::Static(v) => {
                let mut words: Vec<u32> = Vec::with_capacity(v.walls.len() * RS as usize);
                for (r, w) in v.walls.iter().enumerate() {
                    words.extend_from_slice(&rec_words(w.cell, w.mask, &w.q));
                    rec_of.push((w.cell, r as u32 + 1));
                }
                q.write_buffer(&recs, 0, bytemuck::cast_slice(&words));
            }
            MGeometry::None => {}
        }
        rec_of.sort_unstable();
        let mut solid: Vec<u32> = match &geom {
            MGeometry::Static(v) => v.solid.clone(),
            MGeometry::None => vec![],
        };
        solid.sort_unstable();
        if nvert > 0 {
            let v: Vec<f32> = probes
                .verts
                .iter()
                .flat_map(|(p, nn)| [p[0] as f32, p[1] as f32, p[2] as f32, nn[0] as f32, nn[1] as f32, nn[2] as f32, 0.0, 0.0])
                .collect();
            q.write_buffer(&probes_b, 0, bytemuck::cast_slice(&v));
        }

        for l in 0..nl {
            let dims = setup.dims(l);
            let n = dims.iter().map(|&d| d as u64).product::<u64>();
            if n >= u32::MAX as u64 {
                bail!("level {l} has too many cells");
            }
            let pb = storage.pop_bytes();
            let per_dir = n * pb * 2; // A and B
            if per_dir > maxb {
                bail!("level {l} is too large for one GPU buffer per direction");
            }
            let mut per = ((maxb / per_dir) as usize).clamp(1, Q);
            let nbuf = Q.div_ceil(per);
            if nbuf > mshader::MAX_POP_BUFS {
                bail!("level {l} needs too many GPU buffers");
            }
            per = Q.div_ceil(nbuf);
            let child = if l + 1 < nl { Some((setup.boxes[l].lo, setup.boxes[l].n)) } else { None };
            // cell types and lists
            let [nx, ny, nz] = dims;
            let mut faces: Vec<u32> = Vec::new();
            let mut ghosts: Vec<u32> = Vec::new();
            let mut restricted: Vec<u32> = Vec::new();
            let face_sub = |x: u32, y: u32, z: u32| -> Option<u32> {
                if l != 0 {
                    return None;
                }
                let on = [x == 0, x == nx - 1, y == 0, y == ny - 1, z == 0, z == nz - 1];
                let mut ty = None;
                for f in 0..6 {
                    if on[f] && setup.faces[f] != FaceBc::Periodic {
                        let t = if setup.faces[f] == FaceBc::Velocity { 1 } else { 2 };
                        ty = Some(match ty {
                            Some(1) => 1,
                            _ => t,
                        });
                    }
                }
                ty
            };
            let is_ghost = |x: u32, y: u32, z: u32| -> bool { l > 0 && (x == 0 || y == 0 || z == 0 || x == nx - 1 || y == ny - 1 || z == nz - 1) };
            let is_restr = |x: u32, y: u32, z: u32| -> bool {
                match child {
                    Some((lo, cn)) => {
                        let p = [x as i32, y as i32, z as i32];
                        (0..3).all(|k| p[k] >= lo[k] + 2 && p[k] <= lo[k] + cn[k] as i32 - 3)
                    }
                    None => false,
                }
            };
            let flags_b = mk(&format!("flags{l}"), n * 4);
            let slab = ((16u64 << 20) / (nx as u64 * ny as u64)).max(1);
            let mut z0 = 0u32;
            let (mut fi, mut si) = (0usize, 0usize);
            while z0 < nz {
                let z1 = (z0 as u64 + slab).min(nz as u64) as u32;
                let c0 = z0 as u64 * nx as u64 * ny as u64;
                let len = (z1 - z0) as u64 * nx as u64 * ny as u64;
                let mut fl = vec![0u32; len as usize];
                for (li, v) in fl.iter_mut().enumerate() {
                    let cc = c0 + li as u64;
                    let x = (cc % nx as u64) as u32;
                    let y = ((cc / nx as u64) % ny as u64) as u32;
                    let z = (cc / (nx as u64 * ny as u64)) as u32;
                    if let Some(t) = face_sub(x, y, z) {
                        *v = T_FACE | (t << 3);
                        faces.push(cc as u32);
                    } else if is_ghost(x, y, z) {
                        *v = T_GHOST;
                        ghosts.push(cc as u32);
                    } else if is_restr(x, y, z) {
                        *v = T_RESTRICT;
                        restricted.push(cc as u32);
                    }
                }
                if l == finest {
                    while si < solid.len() && (solid[si] as u64) < c0 + len {
                        let li = (solid[si] as u64 - c0) as usize;
                        if fl[li] == 0 {
                            fl[li] = T_SOLID;
                        }
                        si += 1;
                    }
                    while fi < rec_of.len() && (rec_of[fi].0 as u64) < c0 + len {
                        let li = (rec_of[fi].0 as u64 - c0) as usize;
                        if fl[li] & 7 == 0 || fl[li] & 7 == T_SOLID {
                            fl[li] = (fl[li] & 7) | (rec_of[fi].1 << 3);
                        }
                        fi += 1;
                    }
                }
                q.write_buffer(&flags_b, c0 * 4, bytemuck::cast_slice(&fl));
                z0 = z1;
            }
            let mut lists = faces.clone();
            lists.extend_from_slice(&ghosts);
            lists.extend_from_slice(&restricted);
            let lists_b = mk(&format!("lists{l}"), lists.len() as u64 * 4);
            q.write_buffer(&lists_b, 0, bytemuck::cast_slice(&lists));
            let ch_nodes = child.map(|(_, cn)| cn.iter().map(|&v| v as u64).product::<u64>()).unwrap_or(0);
            let mom_child = mk(&format!("momc{l}"), ch_nodes * 2 * MW as u64 * 4);
            let pn = [(nx - 1) / 2 + 1, (ny - 1) / 2 + 1, (nz - 1) / 2 + 1];
            let pnn = if l > 0 { pn.iter().map(|&v| v as u64).product::<u64>() } else { 0 };
            let mom_up = mk(&format!("momu{l}"), pnn * MW as u64 * 4);
            // initial moments: rest at the initial velocity
            let tau_l = 0.5 + 3.0 * setup.nu0 * (1u64 << l) as f64;
            let init_m = |nodes: u64| -> Vec<f32> {
                let mut v = vec![0f32; (nodes * MW as u64) as usize];
                for c in v.chunks_exact_mut(MW as usize) {
                    c[0] = 1.0;
                    c[1] = setup.u_init[0] as f32;
                    c[2] = setup.u_init[1] as f32;
                    c[3] = setup.u_init[2] as f32;
                    c[10] = tau_l as f32;
                }
                v
            };
            if ch_nodes > 0 {
                q.write_buffer(&mom_child, 0, bytemuck::cast_slice(&init_m(2 * ch_nodes)));
            }
            if pnn > 0 {
                q.write_buffer(&mom_up, 0, bytemuck::cast_slice(&init_m(pnn)));
            }
            let spix = mk(&format!("spix{l}"), pix_lists[l].len() as u64 * 4);
            q.write_buffer(&spix, 0, bytemuck::cast_slice(&pix_lists[l]));
            let dump = mk(&format!("dump{l}"), if setup.debug_dump { n * 16 } else { 16 });
            let mut pops = Vec::new();
            for b in 0..nbuf {
                let dirs = per.min(Q - b * per) as u64;
                pops.push(mk(&format!("pop{l}.{b}"), dirs * per_dir));
            }
            mem += pops.iter().map(|b| b.size()).sum::<u64>() + flags_b.size() + lists_b.size() + mom_child.size() + mom_up.size() + spix.size() + dump.size();
            let scale_l = (1u64 << (finest - l)) as f64; // finest cells per cell of this level
            let sp = |i: usize| if l == 0 { setup.sponge[i] as f64 } else { 0.0 };
            let cfg = LevelShader {
                level: l,
                dims,
                periodic: if l == 0 { periodic0 } else { [false; 3] },
                fp16: storage == Storage::Fp16,
                dirs_per_buf: per as u32,
                tau0: tau_l,
                smag2: setup.smagorinsky * setup.smagorinsky,
                sponge_lo: [sp(0), sp(2), sp(4)],
                sponge_hi: [sp(1), sp(3), sp(5)],
                sponge_tau: if l == 0 { setup.sponge_tau } else { 0.0 },
                sigma: if l == 0 { setup.sponge_sigma } else { 0.0 },
                absorb_velocity: setup.absorb_velocity,
                total_p: setup.total_pressure_inlet,
                u_inf: setup.u_inf,
                u_init: setup.u_init,
                grav: [setup.body_force0[0] / (1u64 << l) as f64, setup.body_force0[1] / (1u64 << l) as f64, setup.body_force0[2] / (1u64 << l) as f64],
                center: if l == finest {
                    setup.center
                } else {
                    [(setup.center[0] - org[l][0]) / scale_l, (setup.center[1] - org[l][1]) / scale_l, (setup.center[2] - org[l][2]) / scale_l]
                },
                nrec: if l == finest { nrec } else { 0 },
                child,
                has_parent: l > 0,
                nface: faces.len() as u32,
                nghost: ghosts.len() as u32,
                nrestrict: restricted.len() as u32,
                nvert: if l == finest { nvert } else { 0 },
                vertex_offset: probes.vertex_offset,
                npix: (pix_lists[l].len() / 2) as u32,
                hist_off,
            };
            pre.push(Pre { cfg, dims, n, pops, flags: flags_b, lists: lists_b, mom_child, mom_up, spix, dump });
        }
        if let Some(e) = pollster::block_on(oom.pop()) {
            bail!("not enough GPU memory ({:.1} GB): {e}", mem as f64 / 1e9);
        }

        let mut entries: Vec<wgpu::BindGroupLayoutEntry> = (0..mshader::MAX_POP_BUFS as u32).map(sto).collect();
        for b in (8..=15).chain(19..=22) {
            entries.push(sto(b));
        }
        let bgl0 = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("bgl0"), entries: &entries });
        let pl = dev.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("pl"), bind_group_layouts: &[Some(&bgl0), Some(&bgl1)], immediate_size: 0 });
        let dummy = mk("dummy", 16);
        let vscope = dev.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut levels: Vec<LevelGpu> = Vec::new();
        let mut dumps = Vec::new();
        for l in 0..nl {
            let p = &pre[l];
            let parent_mc = if l > 0 { &pre[l - 1].mom_child } else { &dummy };
            let child_up = if l + 1 < nl { &pre[l + 1].mom_up } else { &dummy };
            let fin = l == finest;
            let mut bge: Vec<wgpu::BindGroupEntry> = Vec::new();
            for b in 0..mshader::MAX_POP_BUFS {
                let buf = p.pops.get(b).unwrap_or(&dummy);
                bge.push(wgpu::BindGroupEntry { binding: b as u32, resource: buf.as_entire_binding() });
            }
            let others: [(u32, &wgpu::Buffer); 12] = [
                (8, &p.flags),
                (9, if fin { &recs } else { &dummy }),
                (10, &p.lists),
                (11, &p.mom_child),
                (12, parent_mc),
                (13, &p.mom_up),
                (14, child_up),
                (15, if fin { &red } else { &dummy }),
                (19, if fin { &probes_b } else { &dummy }),
                (20, &p.spix),
                (21, &sacc),
                (22, &p.dump),
            ];
            for (b, buf) in others {
                bge.push(wgpu::BindGroupEntry { binding: b, resource: buf.as_entire_binding() });
            }
            let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor { label: Some("bg0"), layout: &bgl0, entries: &bge });
            let src = p.cfg.source();
            if std::env::var_os("SIMLAB_CFD_DUMP_WGSL").is_some() {
                let _ = std::fs::write(std::env::temp_dir().join(format!("simlab_cfd_level{l}.wgsl")), &src);
            }
            // SAFETY: every index is bounded by sizes baked into the source.
            let module = unsafe {
                dev.create_shader_module_trusted(
                    wgpu::ShaderModuleDescriptor { label: Some("mlbm"), source: wgpu::ShaderSource::Wgsl(src.into()) },
                    wgpu::ShaderRuntimeChecks::unchecked(),
                )
            };
            let pipe = |e: &str| {
                dev.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some(e),
                    layout: Some(&pl),
                    module: &module,
                    entry_point: Some(e),
                    compilation_options: Default::default(),
                    cache: None,
                })
            };
            let mut pipes = vec![(Kind::Step, pipe("step")), (Kind::Slices, pipe("sample_slices"))];
            if l == 0 {
                pipes.push((Kind::Face, pipe("face_bc")));
            }
            if l > 0 {
                pipes.push((Kind::Ghost, pipe("ghost_fill")));
            }
            if l + 1 < nl {
                pipes.push((Kind::Restrict, pipe("restrict_fill")));
            }
            if fin {
                pipes.push((Kind::Reduce, pipe("reduce1")));
                pipes.push((Kind::Verts, pipe("sample_verts")));
            }
            let init = pipe("init");
            let dump = pipe("dump_macro");
            if fin {
                // reduce2 kept under a second kind slot
                pipes.push((Kind::Reduce, pipe("reduce2")));
            }
            levels.push(LevelGpu {
                cfg: p.cfg.clone(),
                dims: p.dims,
                n: p.n,
                pipes,
                init,
                dump,
                bg,
                _bufs: [p.pops.clone(), vec![p.flags.clone(), p.lists.clone(), p.mom_child.clone(), p.mom_up.clone(), p.spix.clone()]].concat(),
                src: 0,
                mslot: 1,
                steps: 0,
            });
            dumps.push(p.dump.clone());
        }
        if let Some(e) = pollster::block_on(vscope.pop()) {
            bail!("GPU kernels failed to build: {e}");
        }
        let mut ml = MultiLattice {
            g,
            setup: MultiSetup { storage, ..setup },
            levels,
            bgl1,
            step_buf,
            step_cap,
            bg1,
            red,
            red_read,
            probes: probes_b,
            sacc,
            recs,
            dumps,
            hist_off,
            hist_cap,
            nrec,
            nvert,
            npix,
            memory_bytes: mem,
            fine_steps: 0,
        };
        // initial state
        let p0 = ml.params(&StepIn::still(false), 0, 0, 0, 0, 0, 0.0);
        q.write_buffer(&ml.step_buf, 0, bytemuck::bytes_of(&p0));
        let mut enc = dev.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_bind_group(1, &ml.bg1, &[0]);
            for lv in &ml.levels {
                pass.set_bind_group(0, &lv.bg, &[]);
                disp(&mut pass, &lv.init, lv.n);
            }
        }
        q.submit([enc.finish()]);
        dev.poll(wgpu::PollType::wait_indefinitely()).ok();
        let _ = &ml.bgl1;
        ml.levels.iter_mut().for_each(|l| l.src = 0);
        Ok(ml)
    }

    #[allow(clippy::too_many_arguments)]
    fn params(&self, s: &StepIn, src: u32, hslot: u32, mw: u32, mold: u32, mnew: u32, blend: f32) -> StepParams {
        let col = |m: &[[f64; 3]; 3], k: usize| [m[0][k] as f32, m[1][k] as f32, m[2][k] as f32, 0.0];
        StepParams {
            r: [col(&s.rot, 0), col(&s.rot, 1), col(&s.rot, 2)],
            q: [col(&s.rot_prev, 0), col(&s.rot_prev, 1), col(&s.rot_prev, 2)],
            omega: [s.omega[0] as f32, s.omega[1] as f32, s.omega[2] as f32, 0.0],
            src,
            hslot,
            mw,
            mold,
            mnew,
            seed: (self.fine_steps as u32).wrapping_add(hslot).wrapping_mul(2_654_435_761).wrapping_add(src * 7919 + mw * 104729),
            blend,
            _p3: 0,
            _pad: [0; 28],
        }
    }

    /// finest steps per coarsest step
    pub fn sub_steps(&self) -> usize {
        1 << (self.levels.len() - 1)
    }

    pub fn level_dims(&self) -> Vec<[u32; 3]> {
        self.levels.iter().map(|l| l.dims).collect()
    }
    pub fn level_cells(&self) -> Vec<u64> {
        self.levels.iter().map(|l| l.n).collect()
    }

    /// Advances by `fine.len()` finest steps (a multiple of `sub_steps()`);
    /// returns force and moment on the part (finest lattice units) per step.
    pub fn run(&mut self, fine: &[StepIn]) -> Result<Vec<[f64; 6]>> {
        let ss = self.sub_steps();
        if fine.len() % ss != 0 {
            bail!("step count must be a multiple of {ss}");
        }
        let mut out = Vec::with_capacity(fine.len());
        let max_coarse = (self.hist_cap / ss).max(1);
        let mut done = 0usize;
        while done < fine.len() {
            let nc = ((fine.len() - done) / ss).min(max_coarse);
            let chunk = &fine[done..done + nc * ss];
            // build the schedule
            let mut ops: Vec<(usize, Kind, u64, usize)> = Vec::new(); // (level, kind, threads, entry)
            let mut ents: Vec<StepParams> = Vec::new();
            let mut fine_i = 0usize;
            for _ in 0..nc {
                self.schedule(0, chunk, &mut fine_i, &mut ops, &mut ents);
            }
            if ents.len() > self.step_cap {
                bail!("too many dispatches per batch");
            }
            let mut pbytes: Vec<u8> = Vec::with_capacity(ents.len() * 256);
            for e in &ents {
                pbytes.extend_from_slice(bytemuck::bytes_of(e));
            }
            self.g.queue.write_buffer(&self.step_buf, 0, &pbytes);
            let mut enc = self.g.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                for &(l, k, threads, e) in &ops {
                    let lv = &self.levels[l];
                    pass.set_bind_group(0, &lv.bg, &[]);
                    pass.set_bind_group(1, &self.bg1, &[(e * 256) as u32]);
                    if k == Kind::Reduce {
                        let pp: Vec<&wgpu::ComputePipeline> = lv.pipes.iter().filter(|p| p.0 == Kind::Reduce).map(|p| &p.1).collect();
                        pass.set_pipeline(pp[0]);
                        pass.dispatch_workgroups(self.nrec.max(1).div_ceil(RED_CHUNK), 1, 1);
                        pass.set_pipeline(pp[1]);
                        pass.dispatch_workgroups(1, 1, 1);
                    } else {
                        disp(&mut pass, lv.pipe(k), threads);
                    }
                }
            }
            let nf = chunk.len();
            enc.copy_buffer_to_buffer(&self.red, self.hist_off as u64 * 4, &self.red_read, 0, nf as u64 * 32);
            self.g.queue.submit([enc.finish()]);
            let h: Vec<f32> = read_mapped(self.g, &self.red_read, nf as u64 * 32)?;
            for k in 0..nf {
                let mut v = [0f64; 6];
                for j in 0..6 {
                    v[j] = h[k * 8 + j] as f64;
                }
                out.push(v);
            }
            self.fine_steps += nf as u64;
            done += nf;
        }
        Ok(out)
    }

    fn schedule(&mut self, l: usize, fine: &[StepIn], fine_i: &mut usize, ops: &mut Vec<(usize, Kind, u64, usize)>, ents: &mut Vec<StepParams>) {
        let nl = self.levels.len();
        let finest = nl - 1;
        let still = StepIn::still(false);
        let sin = if l == finest { fine[*fine_i] } else { still };
        let lsrc = self.levels[l].src;
        let lmw = 1 - self.levels[l].mslot;
        let hslot = (*fine_i % self.hist_cap) as u32;
        let push = |s: &Self, k: Kind, threads: u64, e: StepParams, ops: &mut Vec<_>, ents: &mut Vec<StepParams>| {
            let _ = s;
            ents.push(e);
            ops.push((l, k, threads, ents.len() - 1));
        };
        let base = self.params(&sin, lsrc, hslot, lmw, 0, 0, 0.0);
        if l == 0 && self.levels[0].cfg.nface > 0 {
            push(self, Kind::Face, self.levels[0].cfg.nface as u64, base, ops, ents);
        }
        push(self, Kind::Step, self.levels[l].n, base, ops, ents);
        self.levels[l].src = 1 - lsrc;
        self.levels[l].mslot = lmw;
        self.levels[l].steps += 1;
        if l == finest {
            let after = self.params(&sin, 1 - lsrc, hslot, lmw, 0, 0, 0.0);
            if self.nrec > 0 {
                push(self, Kind::Reduce, 1, after, ops, ents);
            }
            if sin.sample {
                if self.nvert > 0 {
                    push(self, Kind::Verts, self.nvert as u64, after, ops, ents);
                }
                // slices on every level at once, after the finest step that asked
                for lv in 0..nl {
                    let np = self.levels[lv].cfg.npix;
                    if np > 0 {
                        let e = self.params(&still, self.levels[lv].src, 0, 0, 0, 0, 0.0);
                        ents.push(e);
                        ops.push((lv, Kind::Slices, np as u64, ents.len() - 1));
                    }
                }
            }
            *fine_i += 1;
        }
        if l + 1 < nl {
            let mold = 1 - lmw;
            for sub in 0..2 {
                let c = l + 1;
                let csrc = self.levels[c].src;
                let e = self.params(&still, csrc, 0, 0, mold, lmw, 0.5 * sub as f32);
                ents.push(e);
                ops.push((c, Kind::Ghost, self.levels[c].cfg.nghost as u64, ents.len() - 1));
                self.schedule(c, fine, fine_i, ops, ents);
            }
            let e = self.params(&still, self.levels[l].src, 0, 0, 0, 0, 0.0);
            ents.push(e);
            ops.push((l, Kind::Restrict, self.levels[l].cfg.nrestrict as u64, ents.len() - 1));
        }
    }

    pub fn reset_samples(&mut self) {
        let mut enc = self.g.device.create_command_encoder(&Default::default());
        if self.nvert > 0 {
            // zero only the accumulators (words 6, 7 of each vertex): rewrite all
            let v: Vec<f32> = read_buffer(self.g, &self.probes, self.nvert as u64 * 32).unwrap_or_default();
            let mut w = v.clone();
            for k in 0..self.nvert as usize {
                w[8 * k + 6] = 0.0;
                w[8 * k + 7] = 0.0;
            }
            self.g.queue.write_buffer(&self.probes, 0, bytemuck::cast_slice(&w));
        }
        enc.clear_buffer(&self.sacc, 0, None);
        self.g.queue.submit([enc.finish()]);
    }

    pub fn vertex_means(&self) -> Result<Vec<f32>> {
        let nv = self.nvert as usize;
        if nv == 0 {
            return Ok(vec![]);
        }
        let v: Vec<f32> = read_buffer(self.g, &self.probes, nv as u64 * 32)?;
        Ok((0..nv).map(|i| if v[8 * i + 7] > 0.0 { v[8 * i + 6] / v[8 * i + 7] } else { f32::NAN }).collect())
    }

    pub fn slice_means(&self) -> Result<Vec<[f32; 4]>> {
        let np = self.npix as usize;
        if np == 0 {
            return Ok(vec![]);
        }
        let v: Vec<f32> = read_buffer(self.g, &self.sacc, np as u64 * 20)?;
        Ok((0..np)
            .map(|i| {
                let c = v[5 * i + 4];
                if c > 0.0 { [v[5 * i] / c, v[5 * i + 1] / c, v[5 * i + 2] / c, v[5 * i + 3] / c] } else { [f32::NAN; 4] }
            })
            .collect())
    }

    /// (rho, ux, uy, uz) of every cell of a level (needs `debug_dump`).
    pub fn macros(&self, l: usize) -> Result<Vec<[f32; 4]>> {
        if !self.setup.debug_dump {
            bail!("macros() needs debug_dump");
        }
        let lv = &self.levels[l];
        let e = self.params(&StepIn::still(false), lv.src, 0, 0, 0, 0, 0.0);
        self.g.queue.write_buffer(&self.step_buf, 0, bytemuck::bytes_of(&e));
        let mut enc = self.g.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_bind_group(0, &lv.bg, &[]);
            pass.set_bind_group(1, &self.bg1, &[0]);
            disp(&mut pass, &lv.dump, lv.n);
        }
        self.g.queue.submit([enc.finish()]);
        let v: Vec<f32> = read_buffer(self.g, &self.dumps[l], lv.n * 16)?;
        Ok(v.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect())
    }

    pub fn records(&self) -> Result<Vec<u32>> {
        read_buffer(self.g, &self.recs, self.nrec.max(1) as u64 * RS as u64 * 4)
    }
}

fn disp(pass: &mut wgpu::ComputePass, p: &wgpu::ComputePipeline, threads: u64) {
    if threads == 0 {
        return;
    }
    let wgs = threads.div_ceil(WG as u64);
    let x = wgs.min(65535);
    let y = wgs.div_ceil(x);
    pass.set_pipeline(p);
    pass.dispatch_workgroups(x as u32, y as u32, 1);
}

fn read_mapped<T: Pod>(g: &Gpu, buf: &wgpu::Buffer, size: u64) -> Result<Vec<T>> {
    let slice = buf.slice(0..size);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    g.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("GPU wait failed: {e}"))?;
    rx.recv()?.map_err(|e| anyhow::anyhow!("GPU read failed: {e}"))?;
    let v = {
        let view = slice.get_mapped_range().map_err(|e| anyhow::anyhow!("GPU map failed: {e:?}"))?;
        bytemuck::cast_slice::<u8, T>(&view).to_vec()
    };
    buf.unmap();
    Ok(v)
}

fn read_buffer<T: Pod>(g: &Gpu, src: &wgpu::Buffer, size: u64) -> Result<Vec<T>> {
    let size = size.next_multiple_of(4);
    let staging = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("read"),
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = g.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(src, 0, &staging, 0, size);
    g.queue.submit([enc.finish()]);
    read_mapped(g, &staging, size)
}

