//! Mesh topology helpers: outward boundary faces, node adjacency, element
//! colouring for conflict-free parallel assembly.

use anyhow::{Result, bail};
use rayon::prelude::*;
use simlab_core::{TetMesh, Vec3, cross, dot, sub};
use std::collections::HashMap;

/// Corner triples of the 4 tet faces, CalculiX C3D10 face numbering
/// (face k+1): F1 = 1-2-3, F2 = 1-4-2, F3 = 2-4-3, F4 = 3-4-1. Used to
/// identify faces only; orientation comes from the geometry.
pub const TET_FACES: [[usize; 3]; 4] = [[0, 1, 2], [0, 3, 1], [1, 3, 2], [2, 3, 0]];
/// The corner opposite each face.
pub const TET_FACE_OPPOSITE: [usize; 4] = [3, 2, 0, 1];

/// A boundary face, oriented outward, with the element it belongs to.
#[derive(Clone, Copy, Debug)]
pub struct BFace {
    /// tri6 nodes: corners a b c (outward by the right-hand rule), midpoints ab bc ca
    pub nodes: [u32; 6],
    pub patch: u32,
    pub tet: u32,
    /// CalculiX face number of `tet` (1..=4)
    pub ccx_face: u8,
}

/// Orients every boundary face of `mesh` outward using its owning tetrahedron.
/// Gmsh does not guarantee outward boundary normals.
pub fn boundary_faces(mesh: &TetMesh) -> Result<Vec<BFace>> {
    let key = |a: u32, b: u32, c: u32| {
        let mut k = [a, b, c];
        k.sort_unstable();
        k
    };
    let mut map: HashMap<[u32; 3], usize> = HashMap::with_capacity(mesh.faces.len() * 2);
    for (i, f) in mesh.faces.iter().enumerate() {
        map.insert(key(f[0], f[1], f[2]), i);
    }
    let mut owner: Vec<Option<(u32, u8)>> = vec![None; mesh.faces.len()];
    for (t, tet) in mesh.tets.iter().enumerate() {
        for (k, fc) in TET_FACES.iter().enumerate() {
            if let Some(&i) = map.get(&key(tet[fc[0]], tet[fc[1]], tet[fc[2]])) {
                owner[i] = Some((t as u32, k as u8));
            }
        }
    }
    let mut out = Vec::with_capacity(mesh.faces.len());
    for (i, f) in mesh.faces.iter().enumerate() {
        let Some((t, k)) = owner[i] else {
            bail!("boundary face {i} does not belong to any element");
        };
        let tet = &mesh.tets[t as usize];
        let opp = mesh.nodes[tet[TET_FACE_OPPOSITE[k as usize]] as usize];
        let (a, b, c) = (mesh.nodes[f[0] as usize], mesh.nodes[f[1] as usize], mesh.nodes[f[2] as usize]);
        let n = cross(sub(b, a), sub(c, a));
        let nodes = if dot(n, sub(opp, a)) > 0.0 { [f[0], f[2], f[1], f[5], f[4], f[3]] } else { *f };
        out.push(BFace { nodes, patch: mesh.face_patch[i], tet: t, ccx_face: k + 1 });
    }
    Ok(out)
}

/// Symmetric node adjacency (two nodes are neighbours when they share an
/// element), CSR with sorted rows, each row including the node itself.
#[derive(Clone, Debug)]
pub struct NodeGraph {
    pub ptr: Vec<usize>,
    pub adj: Vec<u32>,
}

impl NodeGraph {
    pub fn n(&self) -> usize {
        self.ptr.len() - 1
    }
    pub fn row(&self, i: usize) -> &[u32] {
        &self.adj[self.ptr[i]..self.ptr[i + 1]]
    }
}

/// node -> elements (CSR)
pub fn node_elements(n_nodes: usize, tets: &[[u32; 10]]) -> (Vec<usize>, Vec<u32>) {
    let mut cnt = vec![0usize; n_nodes + 1];
    for t in tets {
        for &v in t {
            cnt[v as usize + 1] += 1;
        }
    }
    for i in 0..n_nodes {
        cnt[i + 1] += cnt[i];
    }
    let mut fill = cnt.clone();
    let mut el = vec![0u32; cnt[n_nodes]];
    for (e, t) in tets.iter().enumerate() {
        for &v in t {
            el[fill[v as usize]] = e as u32;
            fill[v as usize] += 1;
        }
    }
    (cnt, el)
}

pub fn node_graph(n_nodes: usize, tets: &[[u32; 10]]) -> NodeGraph {
    let (nptr, nel) = node_elements(n_nodes, tets);
    let rows: Vec<Vec<u32>> = (0..n_nodes)
        .into_par_iter()
        .map_init(Vec::new, |buf: &mut Vec<u32>, i| {
            buf.clear();
            for &e in &nel[nptr[i]..nptr[i + 1]] {
                buf.extend_from_slice(&tets[e as usize]);
            }
            if buf.is_empty() {
                buf.push(i as u32);
            }
            buf.sort_unstable();
            buf.dedup();
            buf.clone()
        })
        .collect();
    let mut ptr = Vec::with_capacity(n_nodes + 1);
    ptr.push(0);
    let mut total = 0;
    for r in &rows {
        total += r.len();
        ptr.push(total);
    }
    let mut adj = Vec::with_capacity(total);
    for r in rows {
        adj.extend_from_slice(&r);
    }
    NodeGraph { ptr, adj }
}

/// Greedy element colouring: no two elements of one colour share a node.
/// Returns elements grouped by colour.
pub fn color_elements(n_nodes: usize, tets: &[[u32; 10]]) -> Vec<Vec<u32>> {
    // per-node bitmask of colours used, in 64-colour words
    let mut words = 1usize;
    let mut used: Vec<u64> = vec![0; n_nodes];
    let mut colors: Vec<Vec<u32>> = Vec::new();
    for (e, t) in tets.iter().enumerate() {
        let mut c = None;
        'search: for w in 0..words {
            let mut m = 0u64;
            for &v in t {
                m |= used[v as usize * words + w];
            }
            if m != u64::MAX {
                c = Some(w * 64 + (!m).trailing_zeros() as usize);
                break 'search;
            }
        }
        let c = match c {
            Some(c) => c,
            None => {
                // grow the bitmask by one word
                let nw = words + 1;
                let mut nu = vec![0u64; n_nodes * nw];
                for i in 0..n_nodes {
                    nu[i * nw..i * nw + words].copy_from_slice(&used[i * words..(i + 1) * words]);
                }
                used = nu;
                words = nw;
                (words - 1) * 64
            }
        };
        for &v in t {
            used[v as usize * words + c / 64] |= 1 << (c % 64);
        }
        if colors.len() <= c {
            colors.resize_with(c + 1, Vec::new);
        }
        colors[c].push(e as u32);
    }
    colors
}

/// Element node coordinates.
#[inline]
pub fn tet_coords(mesh: &TetMesh, e: usize) -> [Vec3; 10] {
    let t = &mesh.tets[e];
    std::array::from_fn(|i| mesh.nodes[t[i] as usize])
}

#[inline]
pub fn face_coords(mesh: &TetMesh, f: &BFace) -> [Vec3; 6] {
    std::array::from_fn(|i| mesh.nodes[f.nodes[i] as usize])
}

/// Boundary faces of a tet10 mesh found from the elements (faces used by
/// exactly one element), as tri6 in mesh order with patch 0. For meshes
/// that come without boundary faces (e.g. a CalculiX deck).
pub fn exterior_faces(tets: &[[u32; 10]]) -> Vec<[u32; 6]> {
    let mut seen: HashMap<[u32; 3], (usize, usize, u32)> = HashMap::with_capacity(tets.len() * 2);
    for (e, t) in tets.iter().enumerate() {
        for (k, fc) in TET_FACES.iter().enumerate() {
            let mut key = [t[fc[0]], t[fc[1]], t[fc[2]]];
            key.sort_unstable();
            seen.entry(key).and_modify(|v| v.2 += 1).or_insert((e, k, 1));
        }
    }
    let mut out: Vec<(usize, usize)> = seen.values().filter(|v| v.2 == 1).map(|v| (v.0, v.1)).collect();
    out.sort_unstable();
    out.iter()
        .map(|&(e, k)| {
            let t = &tets[e];
            let [a, b, c] = TET_FACES[k];
            let mid = |x: usize, y: usize| {
                let i = crate::elem::TET_EDGES.iter().position(|&(p, q)| (p == x && q == y) || (p == y && q == x)).unwrap();
                t[4 + i]
            };
            [t[a], t[b], t[c], mid(a, b), mid(b, c), mid(c, a)]
        })
        .collect()
}
