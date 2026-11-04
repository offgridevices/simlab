//! Shared types for SimLab's solvers and engine. Everything here is SI
//! (metres, kilograms, seconds, newtons, pascals) unless a name says otherwise.

pub mod config;
pub mod decimate;
pub mod material;
pub mod mesh;
pub mod progress;
pub mod stl;

pub use config::*;
pub use material::*;
pub use mesh::*;
pub use progress::*;

pub type Vec3 = [f64; 3];

#[inline]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[inline]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
pub fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[inline]
pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
#[inline]
pub fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
#[inline]
pub fn normalize(a: Vec3) -> Vec3 {
    let n = norm(a);
    if n > 0.0 { scale(a, 1.0 / n) } else { a }
}
