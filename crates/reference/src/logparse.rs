//! Follows the container's output line by line: which step is running, the
//! solver's iterations, residuals and forces, mesh size and quality, y+,
//! and OpenFOAM's fatal errors.

use simlab_core::Vec3;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Stage(String),
    /// one solver iteration (steady) or time step (transient) finished:
    /// its number in this stage (1, 2, …), the solver's time (iteration
    /// count for simpleFoam, seconds for pimpleFoam), total force (N), total
    /// moment (N·m) about CofR, initial residuals
    Iteration { it: u32, time: f64, force: Vec3, moment: Vec3, residuals: BTreeMap<String, f64> },
    /// snappyHexMesh's own progress, in words
    Meshing(String),
}

#[derive(Default, Debug)]
pub struct LogState {
    pub stage: String,
    /// iterations (steady) or time steps (transient) so far in this stage
    pub iteration: u32,
    /// the solver's time: iteration number (simpleFoam) or seconds (pimpleFoam)
    pub time: f64,
    /// largest Courant number of the latest time step (transient only)
    pub courant_max: Option<f64>,
    pub residuals: BTreeMap<String, f64>,
    in_forces: bool,
    in_moments: bool,
    force: Option<Vec3>,
    pub cells: Option<u64>,
    pub mesh_failed_checks: Option<u32>,
    pub mesh_notes: Vec<String>,
    /// (min, max, average)
    pub y_plus: Option<(f64, f64, f64)>,
    /// percentage of the requested layer thickness snappyHexMesh achieved
    pub layer_coverage: Option<f64>,
    pub fatal: Option<String>,
    /// simpleFoam met its own residual targets and stopped
    pub residuals_converged: bool,
    fatal_lines: u32,
    pub tail: Vec<String>,
    pub last_meshing: String,
}

fn vec3(s: &str) -> Option<Vec3> {
    let a = s.find('(')?;
    let b = s[a..].find(')')? + a;
    let v: Vec<f64> = s[a + 1..b].split_whitespace().filter_map(|x| x.parse().ok()).collect();
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

impl LogState {
    pub fn feed(&mut self, line: &str) -> Vec<Event> {
        let mut ev = vec![];
        let t = line.trim();
        self.tail.push(line.to_string());
        if self.tail.len() > 200 {
            self.tail.drain(..100);
        }
        if let Some(name) = t.strip_prefix("@@SIMLAB stage ") {
            self.stage = name.trim().to_string();
            self.iteration = 0;
            ev.push(Event::Stage(self.stage.clone()));
            return ev;
        }
        // fatal errors: keep the next lines as the message
        if t.contains("FOAM FATAL") {
            self.fatal = Some(String::new());
            self.fatal_lines = 12;
            return ev;
        }
        if self.fatal_lines > 0 {
            self.fatal_lines -= 1;
            if !t.is_empty()
                && let Some(f) = self.fatal.as_mut() {
                    if !f.is_empty() {
                        f.push(' ');
                    }
                    f.push_str(t);
                }
        }
        match self.stage.as_str() {
            // "init": the steady start of a transient run
            "solve" | "init" => self.solver_line(t, &mut ev),
            "snappy" => {
                let words = [
                    "Surface refinement iteration",
                    "Feature refinement iteration",
                    "Shell refinement iteration",
                    "Removing mesh beyond surface intersections",
                    "Morph iteration",
                    "Layer addition iteration",
                    "Adding layers",
                    "Snapping",
                ];
                if let Some(w) = words.iter().find(|w| t.starts_with(**w)) {
                    let msg = t.trim_end_matches(['-', ' ']).to_string();
                    if msg != self.last_meshing {
                        self.last_meshing = msg.clone();
                        let _ = w;
                        ev.push(Event::Meshing(msg));
                    }
                }
                // final layer table row: name faces target-layers mesh-layers thickness[m] thickness[%]
                // (the planned-layers table before it has 5 columns)
                let w: Vec<&str> = t.split_whitespace().collect();
                if w.len() == 6 && w[0] == crate::case::PART
                    && let (Ok(_), Ok(pct)) = (w[1].parse::<u64>(), w[w.len() - 1].parse::<f64>()) {
                        self.layer_coverage = Some(pct);
                    }
            }
            "checkMesh" => {
                if let Some(rest) = t.strip_prefix("cells:") {
                    self.cells = rest.trim().parse().ok();
                }
                if let Some(rest) = t.strip_prefix("Failed ") {
                    self.mesh_failed_checks = rest.split_whitespace().next().and_then(|x| x.parse().ok());
                }
                if t.starts_with("***") {
                    self.mesh_notes.push(t.trim_start_matches('*').trim().to_string());
                }
            }
            _ => {}
        }
        ev
    }

    fn solver_line(&mut self, t: &str, ev: &mut Vec<Event>) {
        if let Some(rest) = t.strip_prefix("Time = ") {
            if let Ok(time) = rest.trim().trim_end_matches('s').trim().parse::<f64>() {
                self.time = time;
                self.iteration += 1;
                self.residuals.clear();
                self.in_forces = false;
                self.in_moments = false;
                self.force = None;
            }
            return;
        }
        if t.contains("Solving for ") && t.contains("Initial residual = ") {
            let field = t.split("Solving for ").nth(1).and_then(|s| s.split(',').next()).unwrap_or("").trim().to_string();
            let r = t
                .split("Initial residual = ")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse::<f64>().ok());
            if let Some(r) = r {
                self.residuals.entry(field).or_insert(r);
            }
            return;
        }
        if t.starts_with("Sum of forces") {
            self.in_forces = true;
            self.in_moments = false;
            return;
        }
        if t.starts_with("Sum of moments") {
            self.in_moments = true;
            self.in_forces = false;
            return;
        }
        if t.starts_with("Total") {
            if self.in_forces {
                self.force = vec3(t);
                self.in_forces = false;
            } else if self.in_moments {
                self.in_moments = false;
                if let (Some(f), Some(m)) = (self.force.take(), vec3(t)) {
                    ev.push(Event::Iteration { it: self.iteration, time: self.time, force: f, moment: m, residuals: self.residuals.clone() });
                }
            }
            return;
        }
        if let Some(rest) = t.strip_prefix("Courant Number mean:") {
            self.courant_max = rest.split("max:").nth(1).and_then(|x| x.trim().parse().ok());
            return;
        }
        if t.starts_with("SIMPLE solution converged") {
            self.residuals_converged = true;
        }
        if t.starts_with("patch ") && t.contains("y+ :") {
            let num = |key: &str| t.split(key).nth(1).and_then(|s| s.split(',').next()).and_then(|s| s.trim().parse::<f64>().ok());
            if let (Some(a), Some(b), Some(c)) = (num("min = "), num("max = "), num("average = ")) {
                self.y_plus = Some((a, b, c));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_simplefoam_iteration() {
        let log = "@@SIMLAB stage solve
Time = 2500

smoothSolver:  Solving for Ux, Initial residual = 8.3203714e-06, Final residual = 6.8861043e-07, No Iterations 2
GAMG:  Solving for p, Initial residual = 3.3873241e-07, Final residual = 9.1158432e-08, No Iterations 1
GAMG:  Solving for p, Initial residual = 1.0199376e-07, Final residual = 3.7111395e-08, No Iterations 1
smoothSolver:  Solving for k, Initial residual = 3.2845999e-06, Final residual = 2.5223314e-07, No Iterations 2
ExecutionTime = 2304.76 s  ClockTime = 2423 s

forces forces write:
    Sum of forces
        Total    : (0.0012375955 -0.00023399458 0.44896341)
        Pressure : (0.0012572634 -0.00050273419 0.45344568)
        Viscous  : (-1.9667838e-05 0.0002687396 -0.0044822715)
    Sum of moments
        Total    : (3.7695766e-05 -1.6867419e-05 -0.0031216408)
        Pressure : (3.8191867e-05 -1.3865128e-05 -0.0025964896)
    writing force and moment files.

yPlus yPlus write:
    writing field yPlus
    patch part y+ : min = 0.147138, max = 48.704536, average = 10.250017
";
        let mut s = LogState::default();
        let ev: Vec<Event> = log.lines().flat_map(|l| s.feed(l)).collect();
        assert_eq!(ev[0], Event::Stage("solve".into()));
        match &ev[1] {
            Event::Iteration { it, time, force, moment, residuals } => {
                assert_eq!(*it, 1);
                assert_eq!(*time, 2500.0);
                assert_eq!(force[2], 0.44896341);
                assert_eq!(moment[2], -0.0031216408);
                assert_eq!(residuals["p"], 3.3873241e-07);
                assert_eq!(residuals["Ux"], 8.3203714e-06);
            }
            e => panic!("{e:?}"),
        }
        assert_eq!(s.y_plus, Some((0.147138, 48.704536, 10.250017)));
    }

    #[test]
    fn parses_checkmesh_and_fatal() {
        let mut s = LogState::default();
        for l in ["@@SIMLAB stage checkMesh", "    cells:            1861835", " ***Max skewness = 4.58, 1 highly skew faces", "Failed 1 mesh checks."] {
            s.feed(l);
        }
        assert_eq!(s.cells, Some(1861835));
        s.feed("@@SIMLAB stage snappy");
        s.feed("part 67376    3      3.47e-05  0.000126");
        s.feed("part 67376    3        3        0.000126  97.5");
        assert_eq!(s.layer_coverage, Some(97.5));
        assert_eq!(s.mesh_failed_checks, Some(1));
        assert_eq!(s.mesh_notes.len(), 1);
        for l in ["--> FOAM FATAL ERROR: (openfoam-2606)", "", "    Cannot find patchField entry for sideA", "", "    From ..."] {
            s.feed(l);
        }
        assert!(s.fatal.as_ref().unwrap().contains("Cannot find patchField entry for sideA"));
    }

    #[test]
    fn parses_pimplefoam_time_steps() {
        let log = "@@SIMLAB stage init
Time = 1

    Sum of forces
        Total    : (1 0 0)
    Sum of moments
        Total    : (0 0 0)
@@SIMLAB stage solve
Courant Number mean: 0.137 max: 0.98
deltaT = 6.25e-06
Time = 1.25e-05

PIMPLE: iteration 1
GAMG:  Solving for p, Initial residual = 0.01, Final residual = 1e-4, No Iterations 3
PIMPLE: iteration 2
GAMG:  Solving for p, Initial residual = 0.001, Final residual = 1e-5, No Iterations 3
    Sum of forces
        Total    : (0.0154 0 1e-5)
    Sum of moments
        Total    : (0 0 0)
Courant Number mean: 0.137 max: 1.01
Time = 1.875e-05
    Sum of forces
        Total    : (0.0155 0 2e-5)
    Sum of moments
        Total    : (0 0 0)
";
        let mut s = LogState::default();
        let ev: Vec<Event> = log.lines().flat_map(|l| s.feed(l)).collect();
        let its: Vec<(u32, f64, f64)> = ev
            .iter()
            .filter_map(|e| match e {
                Event::Iteration { it, time, force, residuals, .. } => Some((*it, *time, force[0] + residuals.get("p").copied().unwrap_or(0.0))),
                _ => None,
            })
            .collect();
        // the steady start, then steps counted afresh in the solve stage, times in seconds
        assert_eq!(its.len(), 3);
        assert_eq!((its[0].0, its[0].1), (1, 1.0));
        assert_eq!((its[1].0, its[1].1), (1, 1.25e-5));
        assert!((its[1].2 - (0.0154 + 0.01)).abs() < 1e-12, "first PIMPLE residual kept");
        assert_eq!((its[2].0, its[2].1), (2, 1.875e-5));
        assert_eq!(s.courant_max, Some(1.01));
        assert_eq!(s.iteration, 2);
    }
}
