# SimLab

*SimLab by OffGrid* — test 3D-printed drone parts on your own Mac before you
print them. Drop in a propeller, a frame or a motor mount and ask:

- **Will it break?** A stress test in the filament or resin you print with,
  in the direction you print it (printed parts are weaker across layers).
  Hold it by some faces or let it fly free; load it with motors, crashes,
  spin. It shows where it breaks first and at what load or RPM.
- **How does air flow around it?** A wind tunnel on your Mac's graphics chip:
  drag, lift, surface pressure and the flow around the part. Spin it to see a
  propeller's thrust and torque.
- **How much does this propeller push?** Thrust, power and efficiency over
  RPM in under a second, from the blade shape in your file.

Every project, test and result is saved on your Mac. Claude can drive it too
(MCP), so you can say "try this frame in PETG and PA-CF and tell me which arm
breaks first".

## Download

For Apple Silicon Macs (M1 or newer), macOS 13 or later:

1. Download the newest **SimLab-…-mac-apple-silicon.zip** from
   [Releases](https://github.com/offgridevices/simlab/releases/latest) and
   double-click it.
2. Drag **SimLab** into **Applications**.
3. Open it. The download is free and not signed by Apple, so the first time
   macOS says it cannot check it: click **Done**, open **System Settings →
   Privacy & Security**, scroll down and click **Open Anyway** next to SimLab.
   After that it opens normally. If SimLab says its engine did not start,
   run this once in Terminal, then open it again:
   `xattr -dr com.apple.quarantine /Applications/SimLab.app`

Updates: **Settings → Updates → Check for updates** tells you when a newer
version is out and links to it.

## Or build it yourself

In Terminal:

```bash
git clone https://github.com/offgridevices/simlab.git
cd simlab
make install
```

The script checks for what it needs (Rust, Node, pnpm, uv) and asks before
installing anything missing with Homebrew. It builds SimLab and puts
**SimLab.app** in your Applications folder. After that you open it like any
app; no Terminal. Apps built on your own Mac need no paid Apple signing and no
Open Anyway step.

Updates: `make update`, or **Settings → Updates → Update** (pulls and rebuilds).

Either way, the first time it opens SimLab checks your Mac (Settings → System
check) and offers to set up the Gmsh mesher (about 60 MB, hash-checked, kept in
SimLab's own folder; it needs [uv](https://docs.astral.sh/uv/)).

Every command: run `make` on its own to list them (`install`, `update`, `test`,
`validate`, `claude`, `release`, `uninstall`, ...).

### Making a release

```bash
make release VERSION=0.2.0
```

That sets the version, commits and tags it on your Mac. Pushing the tag
(`git push origin HEAD v0.2.0`) makes GitHub build SimLab.app and attach it to
a new release (`.github/workflows/release.yml`).

## Connect Claude (optional)

```bash
make claude
```

Claude Desktop: Settings → Developer → Edit Config, and add under `mcpServers`:

```json
"simlab": { "command": "/Applications/SimLab.app/Contents/MacOS/simlab-engine", "args": ["mcp"] }
```

Claude works in the same projects you see in the app, live. If the app is
closed, the engine starts in the background and stops after 10 idle minutes.

## How it works

| Test | Method | Runs on |
|---|---|---|
| Strength | Finite elements (quadratic tetrahedra), printed-layer-aware materials, Tsai-Hill failure check, inertia relief for parts in flight | all CPU cores |
| Wind tunnel | Lattice Boltzmann (D3Q19) with turbulence model, moving blades | GPU (Metal) |
| Propeller | Blade element momentum theory with low-Reynolds airfoil data | CPU, < 1 s |
| Reference runs (optional) | CalculiX and OpenFOAM in Docker (Colima), to check the built-in answers | CPU |

Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). How close the answers
are to known results: [docs/VALIDATION.md](docs/VALIDATION.md). The material
library (23 filaments and resins, every value sourced from maker datasheets):
[crates/core/src/materials.json](crates/core/src/materials.json).

## Your data

Everything lives in `~/Library/Application Support/SimLab/` (a SQLite
database plus result files). It stays when you quit, update or reinstall, and
Time Machine backs it up. Nothing is uploaded anywhere.

## License

AGPL-3.0-or-later. Gmsh (GPL-2.0+), CalculiX (GPL-2.0+) and OpenFOAM
(GPL-3.0) run as separate programs.
