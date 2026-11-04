//! STL reading (binary and ASCII) with vertex welding, so solvers get a
//! connected surface.

use crate::{TriMesh, Vec3};
use anyhow::{Result, bail};
use std::collections::HashMap;

pub fn read_stl(bytes: &[u8]) -> Result<TriMesh> {
    let tris = if is_binary(bytes) { read_binary(bytes)? } else { read_ascii(bytes)? };
    if tris.is_empty() {
        bail!("the STL has no triangles");
    }
    Ok(weld(&tris))
}

fn is_binary(b: &[u8]) -> bool {
    if b.len() < 84 {
        return false;
    }
    let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
    // binary files can also start with "solid", so trust the size
    84 + n * 50 == b.len()
}

fn read_binary(b: &[u8]) -> Result<Vec<[Vec3; 3]>> {
    let n = u32::from_le_bytes([b[80], b[81], b[82], b[83]]) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let o = 84 + i * 50 + 12;
        let f = |k: usize| f32::from_le_bytes(b[o + 4 * k..o + 4 * k + 4].try_into().unwrap()) as f64;
        out.push([[f(0), f(1), f(2)], [f(3), f(4), f(5)], [f(6), f(7), f(8)]]);
    }
    Ok(out)
}

fn read_ascii(b: &[u8]) -> Result<Vec<[Vec3; 3]>> {
    let s = std::str::from_utf8(b).map_err(|_| anyhow::anyhow!("not a valid STL file"))?;
    let mut out = Vec::new();
    let mut cur: Vec<Vec3> = Vec::with_capacity(3);
    for line in s.lines() {
        let mut it = line.split_whitespace();
        if it.next() == Some("vertex") {
            let v: Vec<f64> = it.take(3).filter_map(|x| x.parse().ok()).collect();
            if v.len() != 3 {
                bail!("bad vertex line in STL: {line}");
            }
            cur.push([v[0], v[1], v[2]]);
            if cur.len() == 3 {
                out.push([cur[0], cur[1], cur[2]]);
                cur.clear();
            }
        }
    }
    Ok(out)
}

/// Merges vertices closer than 1e-6 of the part's size; drops degenerate triangles.
pub fn weld(tris: &[[Vec3; 3]]) -> TriMesh {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for t in tris {
        for p in t {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    let size = (0..3).map(|k| hi[k] - lo[k]).fold(0.0, f64::max).max(1e-12);
    let q = 1e-6 * size;
    let mut map: HashMap<[i64; 3], u32> = HashMap::with_capacity(tris.len() / 2);
    let mut positions = Vec::new();
    let mut triangles = Vec::with_capacity(tris.len());
    for t in tris {
        let mut idx = [0u32; 3];
        for (j, p) in t.iter().enumerate() {
            let key = [(p[0] / q).round() as i64, (p[1] / q).round() as i64, (p[2] / q).round() as i64];
            idx[j] = *map.entry(key).or_insert_with(|| {
                positions.push(*p);
                (positions.len() - 1) as u32
            });
        }
        if idx[0] != idx[1] && idx[1] != idx[2] && idx[0] != idx[2] {
            triangles.push(idx);
        }
    }
    let patch = vec![0; triangles.len()];
    TriMesh { positions, triangles, patch }
}

pub fn write_binary_stl(m: &TriMesh) -> Vec<u8> {
    let mut out = vec![0u8; 80];
    out.extend_from_slice(&(m.triangles.len() as u32).to_le_bytes());
    for (i, t) in m.triangles.iter().enumerate() {
        let n = m.triangle_normal(i);
        for x in n {
            out.extend_from_slice(&(x as f32).to_le_bytes());
        }
        for &v in t {
            for x in m.positions[v as usize] {
                out.extend_from_slice(&(x as f32).to_le_bytes());
            }
        }
        out.extend_from_slice(&[0, 0]);
    }
    out
}
