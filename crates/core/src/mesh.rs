//! Meshes. Surface meshes (triangles) for display, the wind tunnel and the
//! propeller model; quadratic tetrahedral meshes (tet10) for the stress test.
//!
//! Tet10 node order (CalculiX / Abaqus C3D10, 0-based):
//! corners 0..3, then edge midpoints 4=(0,1) 5=(1,2) 6=(2,0) 7=(0,3) 8=(1,3) 9=(2,3).
//! Gmsh's own order swaps 8 and 9; the mesher converts before writing.
//!
//! Tri6 boundary face order: corners a, b, c (outward normal by the
//! right-hand rule), then midpoints (a,b) (b,c) (c,a).

use crate::{Vec3, cross, norm, sub};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// A triangle surface. `patch[i]` is the surface group of triangle i (a CAD
/// face, or a region the mesher found by angle); 0 when unknown.
#[derive(Clone, Debug, Default)]
pub struct TriMesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub patch: Vec<u32>,
}

impl TriMesh {
    pub fn bounds(&self) -> (Vec3, Vec3) {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        (lo, hi)
    }

    pub fn scaled(&self, s: f64) -> TriMesh {
        TriMesh {
            positions: self.positions.iter().map(|p| [p[0] * s, p[1] * s, p[2] * s]).collect(),
            triangles: self.triangles.clone(),
            patch: self.patch.clone(),
        }
    }

    pub fn triangle_area(&self, t: usize) -> f64 {
        let [a, b, c] = self.triangles[t];
        let (a, b, c) = (self.positions[a as usize], self.positions[b as usize], self.positions[c as usize]);
        0.5 * norm(cross(sub(b, a), sub(c, a)))
    }

    /// Area-weighted unit normal of a triangle.
    pub fn triangle_normal(&self, t: usize) -> Vec3 {
        let [a, b, c] = self.triangles[t];
        let (a, b, c) = (self.positions[a as usize], self.positions[b as usize], self.positions[c as usize]);
        crate::normalize(cross(sub(b, a), sub(c, a)))
    }

    /// Signed volume (positive for a closed, outward-facing surface).
    pub fn volume(&self) -> f64 {
        let mut v = 0.0;
        for t in &self.triangles {
            let (a, b, c) = (self.positions[t[0] as usize], self.positions[t[1] as usize], self.positions[t[2] as usize]);
            v += crate::dot(a, cross(b, c)) / 6.0;
        }
        v
    }
}

/// Quadratic tetrahedral volume mesh with its boundary faces.
#[derive(Clone, Debug, Default)]
pub struct TetMesh {
    pub nodes: Vec<Vec3>,
    pub tets: Vec<[u32; 10]>,
    pub faces: Vec<[u32; 6]>,
    /// surface group of each boundary face, same numbering as the model's patches
    pub face_patch: Vec<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TetMeshHeader {
    pub nodes: usize,
    pub tets: usize,
    pub faces: usize,
    /// metres per file unit (the mesher writes metres, so 1.0)
    pub unit_m: f64,
}

impl TetMesh {
    /// Reads the mesher's raw files from `dir`: mesh.json, nodes.f64, tets.u32,
    /// faces.u32, face_patch.u32 (little-endian).
    pub fn read_dir(dir: &Path) -> Result<TetMesh> {
        let h: TetMeshHeader = serde_json::from_slice(&std::fs::read(dir.join("mesh.json")).context("mesh.json")?)?;
        let nodes_raw = read_le::<f64>(&dir.join("nodes.f64"))?;
        let tets_raw = read_le::<u32>(&dir.join("tets.u32"))?;
        let faces_raw = read_le::<u32>(&dir.join("faces.u32"))?;
        let patch = read_le::<u32>(&dir.join("face_patch.u32"))?;
        if nodes_raw.len() != h.nodes * 3 || tets_raw.len() != h.tets * 10 || faces_raw.len() != h.faces * 6 || patch.len() != h.faces {
            bail!("mesh files do not match mesh.json");
        }
        let s = h.unit_m;
        let m = TetMesh {
            nodes: nodes_raw.chunks_exact(3).map(|c| [c[0] * s, c[1] * s, c[2] * s]).collect(),
            tets: tets_raw.chunks_exact(10).map(|c| c.try_into().unwrap()).collect(),
            faces: faces_raw.chunks_exact(6).map(|c| c.try_into().unwrap()).collect(),
            face_patch: patch,
        };
        m.check()?;
        Ok(m)
    }

    pub fn write_dir(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let h = TetMeshHeader { nodes: self.nodes.len(), tets: self.tets.len(), faces: self.faces.len(), unit_m: 1.0 };
        std::fs::write(dir.join("mesh.json"), serde_json::to_vec_pretty(&h)?)?;
        write_le(&dir.join("nodes.f64"), self.nodes.iter().flatten().copied())?;
        write_le(&dir.join("tets.u32"), self.tets.iter().flatten().copied())?;
        write_le(&dir.join("faces.u32"), self.faces.iter().flatten().copied())?;
        write_le(&dir.join("face_patch.u32"), self.face_patch.iter().copied())?;
        Ok(())
    }

    pub fn check(&self) -> Result<()> {
        let n = self.nodes.len() as u32;
        for t in &self.tets {
            if t.iter().any(|&i| i >= n) {
                bail!("a tetrahedron points past the node list");
            }
        }
        for f in &self.faces {
            if f.iter().any(|&i| i >= n) {
                bail!("a boundary face points past the node list");
            }
        }
        Ok(())
    }

    /// The boundary as linear triangles for display: each tri6 face split
    /// into 4, keeping node numbering (so nodal fields map directly).
    pub fn display_surface(&self) -> TriMesh {
        let mut tris = Vec::with_capacity(self.faces.len() * 4);
        let mut patch = Vec::with_capacity(self.faces.len() * 4);
        for (f, &p) in self.faces.iter().zip(&self.face_patch) {
            let [a, b, c, ab, bc, ca] = *f;
            for t in [[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]] {
                tris.push(t);
                patch.push(p);
            }
        }
        TriMesh { positions: self.nodes.clone(), triangles: tris, patch }
    }
}

pub trait LeBytes: Copy + bytemuck::Pod + bytemuck::Zeroable {}
impl LeBytes for f64 {}
impl LeBytes for f32 {}
impl LeBytes for u32 {}

/// Reads a raw little-endian array (this crate only targets little-endian Macs).
pub fn read_le<T: LeBytes>(path: &Path) -> Result<Vec<T>> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() % std::mem::size_of::<T>() != 0 {
        bail!("{} has a partial value at the end", path.display());
    }
    let n = bytes.len() / std::mem::size_of::<T>();
    let mut out = vec![T::zeroed(); n];
    bytemuck::cast_slice_mut::<T, u8>(&mut out).copy_from_slice(&bytes);
    Ok(out)
}

pub fn write_le<T: LeBytes>(path: &Path, values: impl IntoIterator<Item = T>) -> Result<()> {
    let v: Vec<T> = values.into_iter().collect();
    std::fs::write(path, bytemuck::cast_slice::<T, u8>(&v)).with_context(|| format!("write {}", path.display()))
}

/// Display mesh files the web UI reads: positions f32×3, normals f32×3,
/// indices u32×3, patch u32 per triangle.
pub fn write_display_mesh(dir: &Path, prefix: &str, m: &TriMesh) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut normals = vec![[0.0f64; 3]; m.positions.len()];
    for t in &m.triangles {
        let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
        let n = cross(sub(b, a), sub(c, a));
        for &i in t {
            for k in 0..3 {
                normals[i as usize][k] += n[k];
            }
        }
    }
    write_le(&dir.join(format!("{prefix}.positions.f32")), m.positions.iter().flatten().map(|&x| x as f32))?;
    write_le(
        &dir.join(format!("{prefix}.normals.f32")),
        normals.iter().flat_map(|n| crate::normalize(*n)).map(|x| x as f32),
    )?;
    write_le(&dir.join(format!("{prefix}.indices.u32")), m.triangles.iter().flatten().copied())?;
    write_le(&dir.join(format!("{prefix}.patch.u32")), m.patch.iter().copied())?;
    Ok(())
}
