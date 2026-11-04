//! Blade geometry: chord, pitch, thickness and camber at radial stations.

use crate::airfoil::SectionShape;
use serde::{Deserialize, Serialize};

/// Which way the propeller turns, seen from the thrust side (from above a
/// drone, looking down at the prop). `Ccw` is a positive spin about the
/// thrust axis by the right-hand rule (core's `Spin` convention).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    Ccw,
    Cw,
}

impl Rotation {
    /// +1 for CCW (right-handed about the thrust axis), -1 for CW
    pub fn sign(self) -> f64 {
        match self {
            Rotation::Ccw => 1.0,
            Rotation::Cw => -1.0,
        }
    }
    pub fn from_sign(s: f64) -> Rotation {
        if s >= 0.0 { Rotation::Ccw } else { Rotation::Cw }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Rotation::Ccw => "ccw",
            Rotation::Cw => "cw",
        }
    }
}

/// One radial station of a blade (all lengths in metres).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct Station {
    pub r: f64,
    pub chord: f64,
    /// blade (pitch) angle of the chord line from the rotation plane, radians
    pub beta: f64,
    /// maximum thickness / chord
    pub thickness: f64,
    /// maximum camber / chord, positive toward the thrust side
    pub camber: f64,
}

impl Station {
    pub fn shape(&self) -> SectionShape {
        SectionShape { thickness: self.thickness, camber: self.camber }
    }
    /// geometric pitch at this station, metres per revolution
    pub fn pitch(&self) -> f64 {
        std::f64::consts::TAU * self.r * self.beta.tan()
    }
}

/// A propeller as the blade element model sees it. Stations are sorted by
/// radius and cover the blade from the root (`hub_radius`) to the tip.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BladeGeometry {
    pub blades: u32,
    /// tip radius, m
    pub radius: f64,
    /// where the blade starts (the root), m
    pub hub_radius: f64,
    pub rotation: Rotation,
    pub stations: Vec<Station>,
}

impl BladeGeometry {
    /// From a UIUC-style table: r/R, c/R, beta (deg), with one section shape
    /// (or a function of r/R) for the whole blade.
    pub fn from_table(
        blades: u32,
        radius: f64,
        r_over_r: &[f64],
        c_over_r: &[f64],
        beta_deg: &[f64],
        shape: impl Fn(f64) -> SectionShape,
    ) -> BladeGeometry {
        let stations: Vec<Station> = r_over_r
            .iter()
            .zip(c_over_r)
            .zip(beta_deg)
            .map(|((&x, &c), &b)| {
                let s = shape(x);
                Station { r: x * radius, chord: c * radius, beta: b.to_radians(), thickness: s.thickness, camber: s.camber }
            })
            .collect();
        let hub = stations.first().map(|s| s.r).unwrap_or(0.0);
        BladeGeometry { blades, radius, hub_radius: hub, rotation: Rotation::Ccw, stations }
    }

    /// Parses a UIUC propeller database geometry file ("r/R c/R beta").
    pub fn parse_uiuc(text: &str, blades: u32, radius: f64, shape: impl Fn(f64) -> SectionShape) -> anyhow::Result<BladeGeometry> {
        let mut rr = vec![];
        let mut cr = vec![];
        let mut b = vec![];
        for line in text.lines() {
            let v: Vec<f64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() >= 3 {
                rr.push(v[0]);
                cr.push(v[1]);
                b.push(v[2]);
            }
        }
        if rr.len() < 3 {
            anyhow::bail!("not a UIUC geometry table");
        }
        Ok(BladeGeometry::from_table(blades, radius, &rr, &cr, &b, shape))
    }

    /// Linear interpolation of a station at radius r (clamped to the ends).
    pub fn at(&self, r: f64) -> Station {
        let s = &self.stations;
        if s.is_empty() {
            return Station { r, chord: 0.0, beta: 0.0, thickness: 0.08, camber: 0.0 };
        }
        if r <= s[0].r {
            return Station { r, ..s[0] };
        }
        if r >= s[s.len() - 1].r {
            return Station { r, ..s[s.len() - 1] };
        }
        let i = s.partition_point(|x| x.r <= r).clamp(1, s.len() - 1);
        let (a, b) = (&s[i - 1], &s[i]);
        let t = (r - a.r) / (b.r - a.r).max(1e-15);
        let l = |x: f64, y: f64| x + (y - x) * t;
        Station {
            r,
            chord: l(a.chord, b.chord),
            beta: l(a.beta, b.beta),
            thickness: l(a.thickness, b.thickness),
            camber: l(a.camber, b.camber),
        }
    }

    pub fn diameter(&self) -> f64 {
        2.0 * self.radius
    }

    /// blade span / mean chord
    pub fn aspect_ratio(&self) -> f64 {
        let span = (self.radius - self.hub_radius).max(1e-9);
        let mut area = 0.0;
        for w in self.stations.windows(2) {
            area += 0.5 * (w[0].chord + w[1].chord) * (w[1].r - w[0].r);
        }
        if area <= 0.0 { 6.0 } else { span * span / area }
    }

    /// nominal pitch: geometric pitch at 75 % radius, metres
    pub fn pitch_75(&self) -> f64 {
        self.at(0.75 * self.radius).pitch()
    }

    /// solidity B c / (pi R) at 75 % radius
    pub fn solidity_75(&self) -> f64 {
        self.blades as f64 * self.at(0.75 * self.radius).chord / (std::f64::consts::PI * self.radius)
    }
}
