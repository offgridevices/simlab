//! Turns the spanwise air load at one operating point into surface
//! tractions on the stress-test mesh's boundary faces.
//!
//! Every face is assigned to the radial band (blade element) its centroid
//! falls in. Within a band:
//! * thrust: each face gets an axial traction proportional to |n . axis|,
//!   so the band's faces on both sides share the band's thrust by projected
//!   (planform) area; the summed force is exactly the band's thrust.
//! * torque: each face gets a traction opposing the blade's motion,
//!   proportional to |n . e_theta| (its frontal projection), scaled so the
//!   summed moment about the axis is exactly the band's torque; the summed
//!   in-plane force magnitude is then the band's torque-producing force
//!   (dQ / r) up to the faces' radius spread inside the band.
//!
//! Faces outside the blade span (hub) get no load. Bands with no faces pass
//! their load to the nearest band that has some, so totals are conserved.

use crate::bemt::OperatingPoint;
use crate::extract::Frame;
use crate::geom::Rotation;
use serde::{Deserialize, Serialize};
use simlab_core::{Vec3, dot, scale};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct BoundaryFace {
    /// m
    pub centroid: Vec3,
    /// unit outward normal
    pub normal: Vec3,
    /// m^2
    pub area: f64,
}

/// Traction (Pa, a force per area vector) for every face, in the same order.
/// `frame` is the prop's centre and thrust axis in metres (the same one the
/// geometry was extracted with); `rotation` its spin direction.
pub fn face_tractions(point: &OperatingPoint, frame: &Frame, rotation: Rotation, faces: &[BoundaryFace]) -> Vec<Vec3> {
    let els = &point.elements;
    let mut out = vec![[0.0; 3]; faces.len()];
    if els.is_empty() || faces.is_empty() {
        return out;
    }
    let r_lo = els[0].r_lo;
    let r_hi = els[els.len() - 1].r_hi;
    let nb = els.len();
    let band_of = |r: f64| -> Option<usize> {
        if r < r_lo || r > r_hi * (1.0 + 1e-9) {
            return None;
        }
        let i = els.partition_point(|e| e.r_hi < r);
        Some(i.min(nb - 1))
    };
    let rot = rotation.sign();
    // per face: band, radius, tangential direction of the drag force
    let mut band = vec![None; faces.len()];
    let mut radius = vec![0.0; faces.len()];
    let mut drag_dir = vec![[0.0; 3]; faces.len()];
    let mut wa = vec![0.0; nb];
    let mut wt = vec![0.0; nb];
    let mut count = vec![0usize; nb];
    for (i, f) in faces.iter().enumerate() {
        let r = frame.radius_of(f.centroid);
        radius[i] = r;
        if let Some(b) = band_of(r) {
            band[i] = Some(b);
            let et = frame.e_theta(f.centroid);
            drag_dir[i] = scale(et, -rot);
            wa[b] += dot(f.normal, frame.axis).abs() * f.area;
            wt[b] += dot(f.normal, et).abs() * f.area * r;
            count[b] += 1;
        }
    }
    // band loads, moved off empty bands to the nearest non-empty one
    let mut thrust: Vec<f64> = els.iter().map(|e| e.dt_dr * (e.r_hi - e.r_lo)).collect();
    let mut torque: Vec<f64> = els.iter().map(|e| e.dq_dr * (e.r_hi - e.r_lo)).collect();
    let filled: Vec<usize> = (0..nb).filter(|&b| count[b] > 0 && wa[b] > 0.0 && wt[b] > 0.0).collect();
    if filled.is_empty() {
        return out;
    }
    for b in 0..nb {
        if !filled.contains(&b) {
            let t = *filled.iter().min_by_key(|&&k| (k as i64 - b as i64).abs()).unwrap();
            thrust[t] += thrust[b];
            torque[t] += torque[b];
            thrust[b] = 0.0;
            torque[b] = 0.0;
        }
    }
    for (i, f) in faces.iter().enumerate() {
        let Some(b) = band[i] else { continue };
        if !filled.contains(&b) {
            continue;
        }
        let ta = thrust[b] * dot(f.normal, frame.axis).abs() / wa[b];
        let tt = torque[b] * dot(f.normal, frame.e_theta(f.centroid)).abs() / wt[b];
        for k in 0..3 {
            out[i][k] = ta * frame.axis[k] + tt * drag_dir[i][k];
        }
    }
    out
}
