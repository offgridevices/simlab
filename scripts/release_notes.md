## Install

For Apple Silicon Macs (M1 or newer), macOS 13 or later.

1. Download **{{ZIP}}** below and double-click it.
2. Drag **SimLab** into your **Applications** folder.
3. Open it. This copy is free and not signed by Apple, so the first time macOS says it cannot check it. Click **Done**, then open **System Settings → Privacy & Security**, scroll down, and click **Open Anyway** next to SimLab. After that it opens normally.

If SimLab then says its engine did not start (macOS holds back the engine inside the app too), or you prefer the terminal, run this once instead of step 3:

```bash
xattr -dr com.apple.quarantine /Applications/SimLab.app
```

SimLab sets up its mesher (Gmsh) the first time it runs and checks everything else it needs on its **Setup** page. It needs [uv](https://docs.astral.sh/uv/) for the mesher; the Setup page says so if it is missing.

Your projects and results stay on your Mac in `~/Library/Application Support/SimLab`.

### Connect Claude Code (optional)

```bash
claude mcp add simlab -- /Applications/SimLab.app/Contents/MacOS/simlab-engine mcp
```

Checksum: see `{{ZIP}}.sha256`.
