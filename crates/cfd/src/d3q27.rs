//! The D3Q27 lattice. Direction `i` has velocity
//! `(i % 3 - 1, (i / 3) % 3 - 1, i / 9 - 1)`, so the rest population is 13
//! and the opposite of `i` is `26 - i`.

pub const Q: usize = 27;
pub const REST: usize = 13;

pub const fn c(i: usize) -> [i32; 3] {
    [(i % 3) as i32 - 1, ((i / 3) % 3) as i32 - 1, (i / 9) as i32 - 1]
}

pub const fn opp(i: usize) -> usize {
    26 - i
}

pub fn weight(i: usize) -> f64 {
    let v = c(i);
    match v[0].abs() + v[1].abs() + v[2].abs() {
        0 => 8.0 / 27.0,
        1 => 2.0 / 27.0,
        2 => 1.0 / 54.0,
        _ => 1.0 / 216.0,
    }
}

/// Slot of a moving direction in per-link arrays (0..26, rest skipped).
pub const fn slot(i: usize) -> usize {
    if i < REST { i } else { i - 1 }
}

