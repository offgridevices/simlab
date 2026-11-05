// Prints a prop STL's bounds and the hub bore (axis z assumed if thinnest)
fn main() -> anyhow::Result<()> {
    let p = std::env::args().nth(1).unwrap();
    let m = simlab_core::stl::read_stl(&std::fs::read(&p)?)?;
    let (lo, hi) = m.bounds();
    println!("{p}: {} tris, {} verts, bounds {:?} .. {:?}, volume {:.1} mm3", m.triangles.len(), m.positions.len(), lo, hi, m.volume());
    let a: Vec<f64> = std::env::args().skip(2).map(|s| s.parse().unwrap()).collect();
    let c = if a.len() >= 2 { [a[0], a[1]] } else { [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0] };
    let rs = a.get(2).copied().unwrap_or(4.0);
    // bore: triangles with near-horizontal normals (perpendicular to z) close to the centre
    let mut pts = vec![];
    for (t, tri) in m.triangles.iter().enumerate() {
        let n = m.triangle_normal(t);
        if n[2].abs() > 0.2 { continue; }
        for &v in tri {
            let p = m.positions[v as usize];
            let r = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
            if r < rs { pts.push([p[0], p[1]]); }
        }
    }
    // algebraic circle fit
    let n = pts.len() as f64;
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy, mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for p in &pts { let z = p[0]*p[0] + p[1]*p[1]; sx += p[0]; sy += p[1]; sxx += p[0]*p[0]; syy += p[1]*p[1]; sxy += p[0]*p[1]; sxz += p[0]*z; syz += p[1]*z; sz += z; }
    // solve [sxx sxy sx; sxy syy sy; sx sy n] [a b c] = [sxz syz sz]
    let m3 = [[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, n]];
    let r3 = [sxz, syz, sz];
    let det = |m: [[f64; 3]; 3]| m[0][0]*(m[1][1]*m[2][2]-m[1][2]*m[2][1]) - m[0][1]*(m[1][0]*m[2][2]-m[1][2]*m[2][0]) + m[0][2]*(m[1][0]*m[2][1]-m[1][1]*m[2][0]);
    let d = det(m3);
    let mut sol = [0.0; 3];
    for k in 0..3 { let mut mk = m3; for i in 0..3 { mk[i][k] = r3[i]; } sol[k] = det(mk) / d; }
    let (cx, cy) = (sol[0] / 2.0, sol[1] / 2.0);
    let r = (sol[2] + cx * cx + cy * cy).sqrt();
    println!("bore: {} points, centre ({cx:.3}, {cy:.3}), radius {r:.3} mm; bbox centre ({:.3}, {:.3}); z mid {:.3}", pts.len(), c[0], c[1], (lo[2] + hi[2]) / 2.0);
    let mut rmax: f64 = 0.0;
    for p in &m.positions { rmax = rmax.max(((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt()); }
    println!("tip radius {rmax:.3} mm");
    // blades: angular histogram at 75 % radius
    let mut hist = [0usize; 36];
    for p in &m.positions {
        let r = ((p[0] - cx).powi(2) + (p[1] - cy).powi(2)).sqrt();
        if (r - 0.75 * rmax).abs() < 1.5 {
            let ang = (p[1] - cy).atan2(p[0] - cx).to_degrees();
            hist[(((ang + 180.0) / 10.0) as usize).min(35)] += 1;
        }
    }
    let line: Vec<String> = hist.iter().enumerate().filter(|(_, h)| **h > 0).map(|(i, h)| format!("{}:{}", i as i32 * 10 - 180, h)).collect();
    println!("vertices near 75 % radius by angle: {}", line.join(" "));
    Ok(())
}

