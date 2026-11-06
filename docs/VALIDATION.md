# How accurate is SimLab?

Every number SimLab shows is checked against something outside SimLab: a published experiment, a textbook answer, or an industry solver (CalculiX for strength, OpenFOAM for air). This page lists every check, what it was compared with, and how close it came.

Run them all yourself with `make validate` (about an hour; needs the graphics chip and Docker). The tables below come from the run on 6 November 2026 on an M4 Max.

## Strength (finite elements)

**Against CalculiX 2.21 on the same mesh and loads** (the industry check; target within 0.1%):

| Case | Displacement, worst difference | Stress (von Mises), worst difference |
|---|---|---|
| Cantilever | 0.0002% | 0.0008% |
| Printed (layered) bar | 0.00008% | 0.0009% |
| Spinning part | 0.0005% | 0.004% |
| Free-flying part (inertia relief) | 0.0002% | 0.0009% |
| Plate with a hole | 0.0001% | 0.001% |
| Spinning, coned, printed blade at 50,000 rpm, solved fully nonlinear | 0.0003% | 0.008% (peak 47.8355 vs 47.8352 MPa, safety factor 0.72898 vs 0.72899) |

Each comparison is run both with CalculiX's own load cards and with identical nodal loads; both agree to the same digits. CalculiX's own answer is also checked to balance its loads.

**Against textbook and benchmark answers:**

| Benchmark | SimLab | Expected | Error |
|---|---|---|---|
| NAFEMS LE10 (thick elliptic plate), stress at point D | −5.3847 MPa | −5.38 MPa | 0.09% |
| Plate with a hole, stress concentration (Peterson) | Kt 2.5094 | 2.5065 | 0.12% (2.5% at the single peak node) |
| Cantilever tip deflection (Timoshenko) | 0.19041 mm | 0.19193 mm | 0.79% (mesh) |
| Cantilever mid-span stress (beam theory) | 30.063 MPa | 30.000 MPa | 0.21% |
| Thick cylinder under pressure (Lamé), bore hoop stress | 16.658 MPa | 16.667 MPa | 0.05% |
| Spinning disk, bore hoop stress | 16.931 MPa | 16.995 MPa | 0.38% |
| Spinning bar, centre stress | 6.9735 MPa | 6.9636 MPa | 0.14% |
| Stress stiffening of a beam in tension | 0.3890 | 0.3885 | 0.13% |
| Printed bar, along vs across layers (stiffness and safety factor) | exact | exact | 0% |
| Patch tests, free bodies, gravity-only body | machine precision | — | below 1e-12 |

The differences from textbook answers come from the mesh, not the solver: CalculiX on the same mesh gives the same numbers to 0.01%.

What this does not cover: how a real print fails (layer adhesion, voids, print settings). SimLab uses lab values for solid test bars and checks safety against the across-layer strength.


## Propellers (quick model, blade element momentum theory)

**Against measured data** (University of Illinois propeller database, volume 2). Error in static thrust (CT) and power (CP):

| Propeller | Blade section | Thrust | Power | Thrust across forward speeds |
|---|---|---|---|---|
| DA4022 5×3.75, 3-blade, 3D-printed | known | +0.2% | −0.5% | +4.2% |
| DA4002 5×3.75, 3D-printed | known | −7.1% | −24.0% | −7.0% |
| MicroInvent 4.3×3.5, 3-blade | assumed | −7.6% | −4.1% | +0.2% |
| MicroInvent 3.2×2.2 | assumed | −12.8% | −2.5% | −9.2% |
| GWS DD 3×2 | assumed | +16.2% | +20.4% | +10.4% |
| APC Sport 4.2×2 | assumed | +31.5% | +27.6% | +29.6% |
| GWS DD 2.5×0.8 | assumed | +42.2% | +43.3% | +22.9% |

With a known blade section, thrust comes within about 7%. With an assumed one (moulded props whose airfoil is not published), it can be off by 30–40%.

**Against your earlier OpenFOAM study of 3-inch props** (steady, fully turbulent, about 1.86 million cells; code to code, not a measurement):

| Prop | rpm | OpenFOAM thrust | SimLab quick model | Error | Torque error |
|---|---|---|---|---|---|
| E-v5 | 17,000 | 45.8 g | 47.7 g | +4.1% | −11.9% |
| E-v5 | 25,000 | 101.0 g | 106.5 g | +5.4% | −12.6% |
| E-v5 | 45,800 | 346.0 g | 374.5 g | +8.2% | −12.2% |
| E-v5-T110 | 45,800 | 344.7 g | 372.7 g | +8.1% | −11.3% |
| E-v5-T90 | 45,800 | 342.6 g | 372.6 g | +8.8% | −10.9% |
| P-v2-e011 | 17,000 / 45,800 | 49.9 / 378.4 g | 53.5 / 420.7 g | +7.2% / +11.2% | −6% |
| A3-p18 | 17,000 / 45,800 | 50.1 / 379.0 g | 53.6 / 421.1 g | +6.9% / +11.1% | −6% |
| A3-p20 | 45,800 | 412.6 g | 467.8 g | +13.4% | −1.7% |
| P-v2 | 17,000 / 45,800 | 48.0 / 362.7 g | 55.0 / 432.0 g | +14.7% / +19.1% | −12% |
| A-v2 | 17,000 / 45,200 | 47.0 / 342.5 g | 57.7 / 441.4 g | +22.8% / +28.9% | +3% / +6% |
| P-v1 | 17,000 / 45,200 | 46.4 / 335.0 g | 59.6 / 453.4 g | +28.4% / +35.3% | −1% / +1% |

The quick model reads high, and more so the steeper the blade pitch (E-v5 has about 41 mm pitch at 75% radius; A-v2 and P-v1 about 55 mm). Overall error: thrust 17%, torque 9% (root mean square). The app's "How sure" note says this. The precise number comes from the propeller wind-tunnel test with OpenFOAM, which matched the same study to within 0.6% (E-v5 at 45,800 rpm: 345.8 g vs 346.0 g; P-v2-e011: 380.0 g vs 378.4 g; E-v5 at 17,000 rpm: 45.7 g vs 45.8 g).

Blade shape read from the part file: chord exact, pitch within 0.74%, on a published propeller.


## Air: checks against textbook answers

**Accurate test (OpenFOAM):**

| Check | SimLab | Expected | Error | Time |
|---|---|---|---|---|
| Sphere drag, Reynolds number 100 | 1.0934 | 1.0917 | +0.16% | 54 s |
| Sphere drag, Reynolds number 10,000 (transition model) | 0.4078 | 0.405 | +0.7% | 138 s |
| Sphere drag, time-accurate run averaged over 6 flow passes, Re 10,000 | 0.4001 | 0.40 | 0% | 293 s |

The per-speed accuracy the app quotes for OpenFOAM (Reynolds number 100 to 100,000, rounded and sharp-edged parts) is in the shape-by-shape table below.

**Quick preview (graphics chip):**

| Check | SimLab | Expected | Error |
|---|---|---|---|
| Sphere drag, Reynolds number 20 | 2.689 | 2.662 | +1.0% |
| Sphere drag, Reynolds number 100 | 1.081 | 1.089 | −0.7% |
| Sphere drag, Reynolds number 100, nested grid | 1.093 | 1.089 | +0.3% |
| Sphere drag, Reynolds number 300 | 0.634 | 0.653 | −2.9% |
| Sphere drag, Reynolds number 1,000 | 0.474 | 0.471 | +0.6% |
| Sphere drag, Reynolds number 10,000 | 0.392 | 0.405 | −3.4% |
| Thin disk square to the wind, Reynolds number 10,000 | 1.111 | 1.17 | −5.1% |
| Spinning sphere, torque (Stokes) | 1.146e-3 | 1.163e-3 N·m | −1.4% |
| Packed spheres, slow-flow drag (Hasimoto, Sangani–Acrivos) | 1.499 | 1.530 | −2.0% |
| Channel flow (Poiseuille), error halving with each grid doubling | second order | second order | — |

Spinning parts in the quick preview are for seeing the flow only: on E-v5 and P-v2 its thrust read 29–63% low against OpenFOAM, and the app says so.


## Air: the two engines, shape by shape

SimLab has two air engines for still parts: the **quick preview** (lattice Boltzmann on the graphics chip) and the **accurate test** (OpenFOAM, in Docker). Both were run on the same shapes, through the app's own engine, after the fixes listed below (November 2026). Drag coefficient is on frontal area.

| Shape | Reynolds number | Published value | Quick preview, Draft | Quick preview, Normal | OpenFOAM, Draft | OpenFOAM, Normal |
|---|---|---|---|---|---|---|
| Sphere, 20 mm | 10,000 | 0.405 (0.39–0.42) | 0.433, +7% (4 min) | — | 0.408, +1% | 0.374, −8% (34 min) |
| Sphere | 20,000 | 0.441 (0.39–0.45) | 0.453, +3% (3 min) | 0.404, −8% (47 min) | 0.402, −9% | 0.373, −15% (28 min) |
| Sphere | 100,000 | 0.47 (0.43–0.50) | 0.456, −3% (4 min) | 0.416, −11% (40 min) | 0.348, −26% | — |
| Square plate 20×20×1 mm, face-on | 20,000 | 1.14–1.17 | 1.013, −12% (4 min) | 1.141, −1% (36 min) | 1.241, +7% (7 min) | 1.231, +6% (66 min) |
| Cube, face-on | 20,000 | 1.05–1.07 | 0.757, −28% (3 min) | 1.017, −4% (47 min) | 1.058, 0% (20 min) | 1.037, −2% (107 min) |
| Short cylinder, length 5 diameters, free ends | 20,000 | about 0.74 (secondary source only) | 0.828 (1 min) | 0.800 (2 min) | 0.615 (4 min) | 0.655 (20 min) |
| Bar 150×12×6 mm, wind on the 12 mm face | 12,000 | about 1.5–2.0 (low confidence) | 1.267 (1 min) | 1.287 (5.5 min) | 1.380 (10 min) | 1.569 (49 min) |
| Same bar, wind on the 6 mm face | 6,000 | 1.15–1.45 | 1.242 (3 min) | 1.274 (20 min) | 1.294 (18 min) | 1.241 (123 min) |
| E-v5 propeller held still, face-on | — | none | 1.228 (2 min) | — | 1.225 (3 min) | 1.363, did not settle (66 min) |
| E-v5 propeller held still, edge-on | — | none | 0.800 (1.5 min) | — | 0.743 (16 min) | — |

What this shows:

- **Shape matters more than speed.** Below Reynolds number 5,000, OpenFOAM is within 1–2% on every shape tried.
- **Flat-faced, sharp-edged parts** (plates, cubes, bars, frames, most drone parts): OpenFOAM is within about 1–7% already at Draft. The quick preview reads 12–30% low on a Draft grid and needs Normal to come within about 4%.
- **Rounded parts above about Reynolds number 30,000** (balls, domes, spinner caps): the quick preview is better. OpenFOAM's steady model cannot follow the swinging wake behind a ball, so it reads 15–26% low there, and a finer mesh does not fix that.
- OpenFOAM stays the default engine for still parts. SimLab looks at the part's outline as the wind sees it and, for a rounded part above Reynolds number 10,000 (by its thickness across the wind), warns on the OpenFOAM result and offers to check it with the quick preview. On the comparison shapes it marks the ball and the round rods across the wind as rounded, and the cube, plates, bars and both propellers (any direction) as sharp-edged.

Notes on the table:

- OpenFOAM sphere Draft numbers at 10,000 and 100,000 come from the run that introduced the transition model (commit 6623400); the code they depend on has not changed since.
- Quick-preview Normal numbers for the plate, cube and sphere at 20,000 replay each run's recorded force history under the new averaging rule (the replay reproduces the old reported numbers to 0.01%).
- OpenFOAM times were measured with two runs side by side at 6–8 cores each; a single run is faster.
- References: plate, Fail et al. 1957 (ARC R&M 3120) 1.143 and Bearman 1971 1.152; sphere, Clift and Morrison correlations; bar, NACA TN 3038, Nakaguchi 1968 and Eurocode EN 1991-1-4 with its length correction; cube and short cylinder, White (Table 7.3) as quoted by secondary sources.

Not yet checked: Fine-detail runs; whether either engine settles on rounded parts as the grid gets finer (both read lower at Normal than at Draft); the plate read 5% lower again with the tunnel sides at 5 widths instead of 3, within its own wake swing but not fully settled.

### Fixes that came out of this comparison

- **Slender parts get enough cells across their thickness.** A 150 mm arm used to get 5 grid cells across 12 mm. The grid is now refined until the thinner side across the wind has 16 / 24 / 32 cells (Draft / Normal / Fine), within the memory budget, with a plain warning if it has to stop short. Both engines.
- **Quick preview: tunnel sides moved out.** Sides 1.5 part-widths away made bluff parts read 8–11% low (plate 0.986 → 1.073, cube 0.703 → 0.778, sphere 0.424 → 0.457). They now sit 3 widths out.
- **Quick preview: averaging waits for the wake.** Sharp-edged wakes keep growing for 15–20 flow passes, so averaging now starts after 35% of the run (cube at Normal 0.973 → 1.017, plate 1.102 → 1.141).
- What is left on sharp edges is grid resolution: on the cube, going from 64 to 128 cells across raised drag from 0.778 to 0.904. The wall treatment and reference area were checked and are not the cause.
