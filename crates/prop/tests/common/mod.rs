//! Shared UIUC fixtures and comparison helpers for the validation tests.
#![allow(dead_code)]

use simlab_core::Air;
use simlab_prop::*;

pub const INCH: f64 = 0.0254;
pub const UIUC_V2: &str = "https://m-selig.ae.illinois.edu/props/volume-2/data/";

pub struct Case {
    pub name: &'static str,
    /// file name stem in the UIUC database
    pub stem: &'static str,
    /// true diameter (Deters et al. 2014, Tables 1 and 2), inches
    pub diameter_in: f64,
    pub blades: u32,
    pub geom: &'static str,
    pub stat: &'static str,
    /// (RPM, advancing-flow file)
    pub adv: Vec<(f64, &'static str)>,
    pub shape: SectionShape,
    pub section_note: &'static str,
}

impl Case {
    pub fn radius(&self) -> f64 {
        0.5 * self.diameter_in * INCH
    }
    pub fn geometry(&self) -> BladeGeometry {
        let s = self.shape;
        BladeGeometry::parse_uiuc(self.geom, self.blades, self.radius(), move |_| s).unwrap()
    }
}

/// UIUC's sections are not published for the off-the-shelf props; these are
/// thin injection-moulded blades, so one generic thin cambered section is used
/// for all of them (not tuned per prop).
pub const MOULDED: SectionShape = SectionShape { thickness: 0.07, camber: 0.04 };
/// DA4002 / DA4022: the SDA1075 airfoil along the whole blade (Deters et al.
/// 2014, Fig. 24 and Table 4): 12 % thick, about 2 % camber.
pub const SDA1075: SectionShape = SectionShape { thickness: 0.12, camber: 0.02 };

macro_rules! f {
    ($n:literal) => {
        include_str!(concat!("../data/uiuc/", $n))
    };
}

pub fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "GWS DD 2.5x0.8",
            stem: "gwsdd_2.5x0.8",
            diameter_in: 2.55,
            blades: 2,
            geom: f!("gwsdd_2.5x0.8_geom.txt"),
            stat: f!("gwsdd_2.5x0.8_static_0500rd.txt"),
            adv: vec![
                (15030.0, f!("gwsdd_2.5x0.8_0503rd_15030.txt")),
                (20016.0, f!("gwsdd_2.5x0.8_0504rd_20016.txt")),
                (25063.0, f!("gwsdd_2.5x0.8_0505rd_25063.txt")),
            ],
            shape: MOULDED,
            section_note: "generic moulded",
        },
        Case {
            name: "GWS DD 3x2",
            stem: "gwsdd_3x2",
            diameter_in: 3.25,
            blades: 2,
            geom: f!("gwsdd_3x2_geom.txt"),
            stat: f!("gwsdd_3x2_static_0392rd.txt"),
            adv: vec![(9992.0, f!("gwsdd_3x2_0394rd_9992.txt")), (19047.0, f!("gwsdd_3x2_0397rd_19047.txt"))],
            shape: MOULDED,
            section_note: "generic moulded",
        },
        Case {
            name: "MicroInvent 3.2x2.2",
            stem: "mi_3.2x2.2",
            diameter_in: 3.20,
            blades: 2,
            geom: f!("mi_3.2x2.2_geom.txt"),
            stat: f!("mi_3.2x2.2_static_0519rd.txt"),
            adv: vec![
                (8027.0, f!("mi_3.2x2.2_0540rd_8027.txt")),
                (12028.0, f!("mi_3.2x2.2_0541rd_12028.txt")),
                (16025.0, f!("mi_3.2x2.2_0542rd_16025.txt")),
                (20051.0, f!("mi_3.2x2.2_0544rd_20051.txt")),
            ],
            shape: MOULDED,
            section_note: "generic moulded",
        },
        Case {
            name: "APC Sport 4.2x2",
            stem: "apcsp_4.2x2",
            diameter_in: 4.20,
            blades: 2,
            geom: f!("apcsp_4.2x2_geom.txt"),
            stat: f!("apcsp_4.2x2_static_0597rd.txt"),
            adv: vec![
                (6021.0, f!("apcsp_4.2x2_0598rd_6021.txt")),
                (9050.0, f!("apcsp_4.2x2_0599rd_9050.txt")),
                (12047.0, f!("apcsp_4.2x2_0600rd_12047.txt")),
                (15064.0, f!("apcsp_4.2x2_0602rd_15064.txt")),
            ],
            shape: MOULDED,
            section_note: "generic moulded",
        },
        Case {
            name: "MicroInvent 4.3x3.5 (3-blade)",
            stem: "mit_4.3x3.5",
            diameter_in: 4.40,
            blades: 3,
            geom: f!("mit_4.3x3.5_geom.txt"),
            stat: f!("mit_4.3x3.5_static_0575rd.txt"),
            adv: vec![
                (6042.0, f!("mit_4.3x3.5_0577rd_6042.txt")),
                (7551.0, f!("mit_4.3x3.5_0578rd_7551.txt")),
                (9055.0, f!("mit_4.3x3.5_0579rd_9055.txt")),
            ],
            shape: MOULDED,
            section_note: "generic moulded",
        },
        Case {
            name: "DA4002 5x3.75 (3D-printed)",
            stem: "da4002_5x3.75",
            diameter_in: 5.0,
            blades: 2,
            geom: f!("da4002_5x3.75_geom.txt"),
            stat: f!("da4002_5x3.75_static_1121md.txt"),
            adv: vec![
                (4031.0, f!("da4002_5x3.75_1123md_4031.txt")),
                (5039.0, f!("da4002_5x3.75_1124md_5039.txt")),
                (6045.0, f!("da4002_5x3.75_1125md_6045.txt")),
            ],
            shape: SDA1075,
            section_note: "SDA1075 (stated)",
        },
        Case {
            name: "DA4022 5x3.75 3-blade (3D-printed)",
            stem: "da4022_5x3.75_3b",
            diameter_in: 5.0,
            blades: 3,
            geom: f!("da4022_5x3.75_geom.txt"),
            stat: f!("da4022_5x3.75_3b_static_0690md.txt"),
            adv: vec![
                (4028.0, f!("da4022_5x3.75_3b_0692md_4028.txt")),
                (5036.0, f!("da4022_5x3.75_3b_0693md_5036.txt")),
                (6034.0, f!("da4022_5x3.75_3b_0694md_6034.txt")),
            ],
            shape: SDA1075,
            section_note: "SDA1075 (stated)",
        },
    ]
}

/// rows of numbers, header lines skipped
pub fn table(text: &str) -> Vec<Vec<f64>> {
    text.lines()
        .filter_map(|l| {
            let v: Vec<f64> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            if v.len() >= 3 { Some(v) } else { None }
        })
        .collect()
}

pub struct Comparison {
    pub name: String,
    pub section_note: String,
    /// mean and RMS of (model - measured) / measured over the static RPM sweep
    pub static_ct_mean: f64,
    pub static_ct_rms: f64,
    pub static_cp_mean: f64,
    pub static_cp_rms: f64,
    /// advancing flow: RMS of (model - measured) / static measured value
    pub adv_ct_rms: f64,
    pub adv_cp_rms: f64,
    /// advancing flow: mean of (model - measured) / static measured value
    pub adv_ct_mean: f64,
    pub adv_cp_mean: f64,
    /// peak efficiency, measured and model, averaged over the RPMs
    pub eta_meas: f64,
    pub eta_model: f64,
    pub rows: Vec<String>,
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}
fn rms(v: &[f64]) -> f64 {
    (v.iter().map(|x| x * x).sum::<f64>() / v.len().max(1) as f64).sqrt()
}

pub fn compare(case: &Case, g: &BladeGeometry, air: &Air, opts: &ModelOptions) -> Comparison {
    let d = 2.0 * case.radius();
    let mut rows = vec![];
    let st = table(case.stat);
    let (mut ect, mut ecp) = (vec![], vec![]);
    rows.push("  static     rpm   CT meas  CT model   err |  CP meas  CP model   err".to_string());
    for r in &st {
        let p = solve(g, r[0], 0.0, air, opts);
        let (a, b) = ((p.ct - r[1]) / r[1], (p.cp - r[2]) / r[2]);
        ect.push(a);
        ecp.push(b);
        rows.push(format!(
            "          {:6.0}  {:.4}   {:.4}  {:+5.1}% |  {:.4}   {:.4}  {:+5.1}%",
            r[0],
            r[1],
            p.ct,
            100.0 * a,
            r[2],
            p.cp,
            100.0 * b
        ));
    }
    let ct0 = |rpm: f64| -> (f64, f64) {
        let mut best = &st[0];
        for r in &st {
            if (r[0] - rpm).abs() < (best[0] - rpm).abs() {
                best = r;
            }
        }
        (best[1], best[2])
    };
    let (mut act, mut acp, mut em, mut ep) = (vec![], vec![], vec![], vec![]);
    for (rpm, text) in &case.adv {
        let (c0, p0) = ct0(*rpm);
        let n = rpm / 60.0;
        rows.push(format!("  {rpm:.0} rpm     J   CT meas  CT model |  CP meas  CP model |  eta meas eta model"));
        let mut eta_m: f64 = 0.0;
        let mut eta_p: f64 = 0.0;
        for r in table(text) {
            let v = r[0] * n * d;
            let p = solve(g, *rpm, v, air, opts);
            act.push((p.ct - r[1]) / c0);
            acp.push((p.cp - r[2]) / p0);
            let pe = p.efficiency.unwrap_or(0.0);
            eta_m = eta_m.max(r.get(3).copied().unwrap_or(0.0));
            eta_p = eta_p.max(pe);
            rows.push(format!(
                "            {:.3}   {:.4}   {:.4}  |  {:.4}   {:.4}  |  {:.3}   {:.3}",
                r[0],
                r[1],
                p.ct,
                r[2],
                p.cp,
                r.get(3).copied().unwrap_or(f64::NAN),
                pe
            ));
        }
        em.push(eta_m);
        ep.push(eta_p);
    }
    Comparison {
        name: case.name.to_string(),
        section_note: case.section_note.to_string(),
        static_ct_mean: mean(&ect),
        static_ct_rms: rms(&ect),
        static_cp_mean: mean(&ecp),
        static_cp_rms: rms(&ecp),
        adv_ct_rms: rms(&act),
        adv_cp_rms: rms(&acp),
        adv_ct_mean: mean(&act),
        adv_cp_mean: mean(&acp),
        eta_meas: mean(&em),
        eta_model: mean(&ep),
        rows,
    }
}

impl Comparison {
    pub fn table(&self) -> String {
        format!("\n=== {} [{}]\n{}", self.name, self.section_note, self.rows.join("\n"))
    }
}

pub fn summary_table(rs: &[Comparison]) -> String {
    let mut s = String::from(
        "| prop | section | static CT err (mean / rms) | static CP err (mean / rms) | J-sweep CT err (mean / rms, of static CT) | J-sweep CP err (mean / rms, of static CP) | peak eta meas / model |\n|---|---|---|---|---|---|---|\n",
    );
    for r in rs {
        s += &format!(
            "| {} | {} | {:+.1}% / {:.1}% | {:+.1}% / {:.1}% | {:+.1}% / {:.1}% | {:+.1}% / {:.1}% | {:.2} / {:.2} |\n",
            r.name,
            r.section_note,
            100.0 * r.static_ct_mean,
            100.0 * r.static_ct_rms,
            100.0 * r.static_cp_mean,
            100.0 * r.static_cp_rms,
            100.0 * r.adv_ct_mean,
            100.0 * r.adv_ct_rms,
            100.0 * r.adv_cp_mean,
            100.0 * r.adv_cp_rms,
            r.eta_meas,
            r.eta_model
        );
    }
    s
}
