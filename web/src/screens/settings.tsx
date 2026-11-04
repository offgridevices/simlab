// Settings: how much of the Mac SimLab may use, the system check, connecting
// Claude, and your own materials and motors.
import { Label } from "@/components/ui/label";
import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { CheckIcon, CopyIcon, MonitorIcon, MoonIcon, PlusIcon, SunIcon, Trash2Icon } from "lucide-react";
import { Screen, TopBar } from "../components/topbar";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import { Loader } from "@/components/ui/skeleton";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/toggle-group";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { toast } from "sonner";
import { Confirm } from "../components/confirm";
import { useApp, useEvents, MOCK } from "../lib/app";
import { getTheme, setTheme, type ThemeChoice } from "../lib/theme";
import { n, type Tone } from "../lib/format";
import type { Check, CheckStatus, Health, Material, Motor, Process, Settings as SettingsT } from "../lib/types";
import { Segmented } from "../workspace/common";

export const MCP_COMMAND = `claude mcp add simlab -- "/Applications/SimLab.app/Contents/MacOS/simlab-engine" mcp`;
export const DESKTOP_CONFIG = JSON.stringify({ mcpServers: { simlab: { command: "/Applications/SimLab.app/Contents/MacOS/simlab-engine", args: ["mcp"] } } }, null, 2);

const CHECK_PILL: Record<CheckStatus, { tone: Tone; label: string }> = {
  ok: { tone: "moss", label: "Ready" },
  missing: { tone: "gold", label: "Missing" },
  error: { tone: "clay", label: "Problem" },
  working: { tone: "ember", label: "Working…" },
  optional: { tone: "neutral", label: "Optional" },
};

export default function Settings() {
  const { api } = useApp();
  const [health, setHealth] = useState<Health | null>(null);
  const [settings, setSettings] = useState<SettingsT | null>(null);
  const [saved, setSaved] = useState<SettingsT | null>(null);
  const [theme, setThemeState] = useState<ThemeChoice>(getTheme);
  const [asking, setAsking] = useState<Check | null>(null);

  useEffect(() => {
    api.health().then(setHealth, (e: Error) => toast.error(e.message));
    api.settings().then((s) => {
      setSettings(s);
      setSaved(s);
    });
  }, [api]);
  useEvents((e) => {
    if (e.type === "check") setHealth((h) => (h ? { ...h, checks: h.checks.map((c) => (c.id === e.check.id ? e.check : c)) } : h));
  });

  const ramGb = health ? Math.floor(health.machine.ram_bytes / 1024 ** 3) : 16;
  const dirty = settings && saved && JSON.stringify(settings) !== JSON.stringify(saved);
  const save = async () => {
    if (!settings) return;
    try {
      const s = await api.saveSettings(settings);
      setSettings(s);
      setSaved(s);
      toast.success("Settings saved");
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Could not save");
    }
  };
  const runFix = async (c: Check) => {
    setAsking(null);
    try {
      await api.fix(c.id);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Could not start the fix");
    }
  };

  return (
    <Screen>
      <TopBar back current="settings" />
      <main className="mx-auto flex w-full max-w-2xl flex-col gap-8 px-4 pb-20 pt-10">
        <div className="flex flex-col gap-1">
          <h1 className="text-3xl font-semibold tracking-tight">Settings</h1>
          <p className="text-muted-foreground">
            {health ? [health.machine.cpu, `${ramGb} GB memory`, health.machine.gpu, `SimLab ${health.version}`].filter(Boolean).join(" · ") : "Reading this Mac…"}
          </p>
        </div>

        <Block title="Look">
          <ToggleGroup type="single" value={theme} onValueChange={(v) => v && (setTheme(v as ThemeChoice), setThemeState(v as ThemeChoice))} aria-label="Theme">
            <ToggleGroupItem value="auto">
              <MonitorIcon />
              Like the Mac
            </ToggleGroupItem>
            <ToggleGroupItem value="light">
              <SunIcon />
              Light
            </ToggleGroupItem>
            <ToggleGroupItem value="dark">
              <MoonIcon />
              Dark
            </ToggleGroupItem>
          </ToggleGroup>
        </Block>

        <Block title="How much of this Mac SimLab may use" description="Tests that need more memory than this get a coarser setup or ask first. Leave room for your other apps.">
          {!settings || !health ? (
            <Loader />
          ) : (
            <div className="flex flex-col gap-6">
              <Label className="flex flex-col items-stretch gap-3">
                <span className="flex items-baseline justify-between gap-2 text-sm font-medium">
                  Memory budget
                  <span className="num text-base">
                    {settings.memory_budget_gb} GB <span className="text-sm font-normal text-muted-foreground">of {ramGb} GB</span>
                  </span>
                </span>
                <Slider min={1} max={Math.max(2, ramGb - 2)} step={1} value={[settings.memory_budget_gb]} onValueChange={([v]) => setSettings({ ...settings, memory_budget_gb: v! })} aria-label="Memory budget in GB" data-testid="memory-slider" />
                <span className="text-xs text-muted-foreground">Half the Mac's memory ({Math.round(ramGb / 2)} GB) is a good start.</span>
              </Label>
              <Label className="flex flex-col items-stretch gap-3">
                <span className="flex items-baseline justify-between gap-2 text-sm font-medium">
                  Processor cores for the strength test
                  <span className="num text-base">
                    {settings.cpu_threads} <span className="text-sm font-normal text-muted-foreground">of {health.machine.cores}</span>
                  </span>
                </span>
                <Slider min={1} max={health.machine.cores} step={1} value={[settings.cpu_threads]} onValueChange={([v]) => setSettings({ ...settings, cpu_threads: v! })} aria-label="Processor cores" />
              </Label>
              <div className="flex flex-col gap-2">
                <span className="text-sm font-medium">Files without units (STL, OBJ) are in</span>
                <Segmented
                  label="Default units"
                  value={String(settings.default_units_mm)}
                  onChange={(v) => setSettings({ ...settings, default_units_mm: Number(v) })}
                  options={[
                    { id: "1", label: "Millimetres" },
                    { id: "25.4", label: "Inches" },
                    { id: "10", label: "Centimetres" },
                  ]}
                />
              </div>
              <Label className="flex items-center justify-between gap-4 text-sm">
                <span>
                  <span className="font-medium">Double-check with reference solvers</span>
                  <span className="block text-muted-foreground">Offers to re-run tests in CalculiX and OpenFOAM (in Docker). Slower; needs the system check below.</span>
                </span>
                <Switch checked={settings.reference_engine_enabled} onCheckedChange={(v) => setSettings({ ...settings, reference_engine_enabled: v })} aria-label="Double-check with reference solvers" />
              </Label>
              <div className="flex items-center gap-3">
                <Button onClick={save} disabled={!dirty} data-testid="save-settings">
                  {dirty ? "Save" : "Saved"}
                </Button>
              </div>
            </div>
          )}
        </Block>

        <Block title="System check" description="What SimLab needs on this Mac, and whether it is there.">
          {!health ? (
            <Loader />
          ) : (
            <Card className="gap-0 overflow-hidden py-0 shadow-none" data-testid="checks" role="list">
              {health.checks.map((c) => (
                <div key={c.id} role="listitem" className="flex flex-wrap items-center gap-x-3 gap-y-2 border-b px-3 py-3 last:border-b-0" data-testid="check-row">
                  <span className="flex min-w-0 flex-[1_1_14rem] flex-col">
                    <span className="text-sm font-medium">{c.label}</span>
                    <span className="text-xs text-muted-foreground">{c.detail}</span>
                  </span>
                  <Badge tone={CHECK_PILL[c.status].tone}>{CHECK_PILL[c.status].label}</Badge>
                  {c.fix && c.status !== "working" && (
                    <Button size="sm" variant="outline" onClick={() => setAsking(c)} data-testid={`fix-${c.id}`}>
                      {c.fix.label}
                    </Button>
                  )}
                </div>
              ))}
            </Card>
          )}
          <p className="text-xs text-muted-foreground">Your projects and results are kept in {health?.data_dir ?? "SimLab's folder"}. Nothing is uploaded.</p>
        </Block>

        <Block title="Connect Claude" description="Let Claude set up and run tests for you. Ask it things like “Test my frame in PETG at full throttle” and watch the tests appear here.">
          <div className="flex flex-col gap-2">
            <span className="text-sm font-medium">Claude Code</span>
            <span className="text-sm text-soft">Paste this into Terminal once:</span>
            <CopyBlock text={MCP_COMMAND} testId="copy-mcp" />
          </div>
          <div className="flex flex-col gap-2">
            <span className="text-sm font-medium">Claude Desktop</span>
            <span className="text-sm text-soft">
              In Claude, open Settings, then Developer, then Edit Config. Paste this into the file it opens (keep anything already there inside <span className="num">mcpServers</span>), save, and restart Claude.
            </span>
            <CopyBlock text={DESKTOP_CONFIG} testId="copy-desktop" />
          </div>
          {MOCK && <p className="text-xs text-gold-text">Demo mode: the commands are real, but there is no engine behind this page.</p>}
        </Block>

        <MaterialsEditor />
        <MotorsEditor />
      </main>

      <Confirm
        open={!!asking}
        onOpenChange={(o) => !o && setAsking(null)}
        title={asking?.label ?? ""}
        description={asking?.fix?.asks ?? ""}
        action={asking?.fix?.label ?? "Go ahead"}
        cancel="Not now"
        destructive={false}
        onConfirm={() => (asking ? runFix(asking) : undefined)}
        testId="fix-confirm"
      />
    </Screen>
  );
}

function Block({ title, description, children }: { title: string; description?: ReactNode; children: ReactNode }) {
  return (
    <Card className="gap-4">
      <CardHeader>
        <CardTitle className="text-base">{title}</CardTitle>
        {description && <CardDescription>{description}</CardDescription>}
      </CardHeader>
      <CardContent className="flex flex-col gap-4">{children}</CardContent>
    </Card>
  );
}

export function CopyBlock({ text, testId }: { text: string; testId?: string }) {
  const [done, setDone] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // older WebKit without clipboard access: select the text instead
      const t = document.createElement("textarea");
      t.value = text;
      document.body.appendChild(t);
      t.select();
      document.execCommand("copy");
      t.remove();
    }
    setDone(true);
    toast.success("Copied");
    setTimeout(() => setDone(false), 1600);
  };
  return (
    <div className="relative rounded-lg border bg-sunken">
      <pre className="num overflow-x-auto whitespace-pre p-3 pr-20 text-xs text-soft">{text}</pre>
      <Button size="xs" variant="outline" className="absolute right-2 top-2" onClick={copy} data-testid={testId} aria-label="Copy">
        {done ? <CheckIcon /> : <CopyIcon />}
        {done ? "Copied" : "Copy"}
      </Button>
    </div>
  );
}

// ---------------------------------------------------------------- your own library

function Num({ label, value, onChange, unit, step }: { label: string; value: number; onChange(v: number): void; unit?: string; step?: number }) {
  return (
    <Label className="flex flex-col items-stretch gap-1 text-xs font-medium text-muted-foreground">
      {label}
      <span className="flex items-center gap-1.5">
        <Input type="number" inputMode="decimal" step={step ?? "any"} className="num" value={Number.isFinite(value) ? value : ""} onChange={(e) => onChange(Number(e.target.value))} />
        {unit && <span className="shrink-0 text-xs font-normal">{unit}</span>}
      </span>
    </Label>
  );
}

const EMPTY_MATERIAL: Material = { id: "", name: "", process: "fdm", density: 1240, e_xy_mpa: 3000, e_z_mpa: 2600, nu: 0.35, uts_xy_mpa: 50, uts_z_mpa: 30, notes: "", custom: true };

function MaterialsEditor() {
  const { api, materials, reloadLibrary } = useApp();
  const [draft, setDraft] = useState<Material | null>(null);
  const [error, setError] = useState("");
  const mine = (materials ?? []).filter((m) => m.custom);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!draft) return;
    setError("");
    try {
      await api.saveMaterial({ ...draft, e_z_mpa: draft.process === "fdm" ? draft.e_z_mpa : draft.e_xy_mpa, uts_z_mpa: draft.process === "fdm" ? draft.uts_z_mpa : draft.uts_xy_mpa });
      setDraft(null);
      reloadLibrary();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save");
    }
  };
  return (
    <Block title="Your materials" description="A filament or resin that is not in the list? Add it from its data sheet.">
      {mine.length > 0 && (
        <ul className="overflow-hidden rounded-lg border">
          {mine.map((m) => (
            <li key={m.id} className="flex items-center gap-3 border-b px-3 py-2.5 last:border-b-0">
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="text-sm font-medium">{m.name}</span>
                <span className="num text-xs text-muted-foreground">
                  {m.process.toUpperCase()} · {n(m.uts_xy_mpa)} / {n(m.uts_z_mpa)} MPa · {n(m.density / 1000)} g/cm³
                </span>
              </span>
              <Button variant="ghost" size="xs" onClick={() => setDraft({ ...m })}>
                Edit
              </Button>
              <Button variant="ghost" size="icon-sm" onClick={async () => (await api.deleteMaterial(m.id), reloadLibrary())} aria-label={`Delete ${m.name}`}>
                <Trash2Icon />
              </Button>
            </li>
          ))}
        </ul>
      )}
      {draft ? (
        <form className="flex flex-col gap-4 rounded-lg border p-4" onSubmit={submit} data-testid="material-form">
          <div className="flex flex-wrap items-end gap-3">
            <Label className="flex min-w-48 flex-1 flex-col gap-1 text-xs font-medium text-muted-foreground">
              Name
              <Input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} placeholder="My PETG-CF" required />
            </Label>
            <Label className="flex flex-col items-stretch gap-1 text-xs font-medium text-muted-foreground">
              Start from
              <Select
                value=""
                onValueChange={(v) => {
                  const m = materials?.find((x) => x.id === v);
                  if (m) setDraft({ ...m, id: draft.id, name: draft.name || `${m.name} (mine)`, custom: true });
                }}
              >
                <SelectTrigger className="w-48">
                  <SelectValue placeholder="A built-in…" />
                </SelectTrigger>
                <SelectContent>
                  {materials?.filter((m) => !m.custom).map((m) => (
                    <SelectItem key={m.id} value={m.id}>
                      {m.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </Label>
          </div>
          <Segmented
            label="How it is made"
            value={draft.process}
            onChange={(v: Process) => setDraft({ ...draft, process: v })}
            options={[
              { id: "fdm", label: "Filament" },
              { id: "resin", label: "Resin" },
              { id: "other", label: "Other" },
            ]}
          />
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
            <Num label={draft.process === "fdm" ? "Strength along layers" : "Strength"} unit="MPa" value={draft.uts_xy_mpa} onChange={(v) => setDraft({ ...draft, uts_xy_mpa: v })} />
            {draft.process === "fdm" && <Num label="Strength across layers" unit="MPa" value={draft.uts_z_mpa} onChange={(v) => setDraft({ ...draft, uts_z_mpa: v })} />}
            <Num label={draft.process === "fdm" ? "Stiffness along layers" : "Stiffness"} unit="MPa" value={draft.e_xy_mpa} onChange={(v) => setDraft({ ...draft, e_xy_mpa: v })} />
            {draft.process === "fdm" && <Num label="Stiffness across layers" unit="MPa" value={draft.e_z_mpa} onChange={(v) => setDraft({ ...draft, e_z_mpa: v })} />}
            <Num label="Density" unit="g/cm³" value={draft.density / 1000} onChange={(v) => setDraft({ ...draft, density: v * 1000 })} step={0.01} />
            <Num label="Poisson's ratio" value={draft.nu} onChange={(v) => setDraft({ ...draft, nu: v })} step={0.01} />
          </div>
          <p className="text-xs text-muted-foreground">Data sheets call strength "tensile strength" and stiffness "tensile modulus" (1 GPa = 1000 MPa). Across-layer strength is often a third to a half lower; ask the maker if it is not listed.</p>
          {error && <p className="text-sm text-destructive">{error}</p>}
          <div className="flex gap-2">
            <Button type="submit" data-testid="save-material">
              Save material
            </Button>
            <Button type="button" variant="outline" onClick={() => setDraft(null)}>
              Cancel
            </Button>
          </div>
        </form>
      ) : (
        <Button variant="outline" className="self-start" onClick={() => setDraft({ ...EMPTY_MATERIAL })} data-testid="add-material">
          <PlusIcon />
          Add a material
        </Button>
      )}
    </Block>
  );
}

const EMPTY_MOTOR: Motor = { id: "", name: "", kv: 1950, mass_g: 33, max_thrust_g: 1400, max_current_a: 40, voltage_v: 22.2, prop: "5 inch", mount_mm: 16, custom: true };

function MotorsEditor() {
  const { api, motors, reloadLibrary } = useApp();
  const [draft, setDraft] = useState<Motor | null>(null);
  const [error, setError] = useState("");
  const mine = (motors ?? []).filter((m) => m.custom);
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (!draft) return;
    setError("");
    try {
      await api.saveMotor(draft);
      setDraft(null);
      reloadLibrary();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Could not save");
    }
  };
  return (
    <Block title="Your motors" description="Add a motor from its thrust table, so strength tests can use its full-throttle push and twist.">
      {mine.length > 0 && (
        <ul className="overflow-hidden rounded-lg border">
          {mine.map((m) => (
            <li key={m.id} className="flex items-center gap-3 border-b px-3 py-2.5 last:border-b-0">
              <span className="flex min-w-0 flex-1 flex-col">
                <span className="text-sm font-medium">{m.name}</span>
                <span className="num text-xs text-muted-foreground">
                  {n(m.max_thrust_g)} g · {n(m.mass_g)} g motor · {m.mount_mm} mm mount
                </span>
              </span>
              <Button variant="ghost" size="xs" onClick={() => setDraft({ ...m })}>
                Edit
              </Button>
              <Button variant="ghost" size="icon-sm" onClick={async () => (await api.deleteMotor(m.id), reloadLibrary())} aria-label={`Delete ${m.name}`}>
                <Trash2Icon />
              </Button>
            </li>
          ))}
        </ul>
      )}
      {draft ? (
        <form className="flex flex-col gap-4 rounded-lg border p-4" onSubmit={submit} data-testid="motor-form">
          <Label className="flex flex-col items-stretch gap-1 text-xs font-medium text-muted-foreground">
            Name
            <Input value={draft.name} onChange={(e) => setDraft({ ...draft, name: e.target.value })} placeholder="2306 2400 KV" required />
          </Label>
          <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
            <Num label="Push at full throttle" unit="g" value={draft.max_thrust_g} onChange={(v) => setDraft({ ...draft, max_thrust_g: v })} />
            <Num label="Motor weight" unit="g" value={draft.mass_g} onChange={(v) => setDraft({ ...draft, mass_g: v })} />
            <Num label="KV" unit="rpm/V" value={draft.kv} onChange={(v) => setDraft({ ...draft, kv: v })} />
            <Num label="Current at full throttle" unit="A" value={draft.max_current_a} onChange={(v) => setDraft({ ...draft, max_current_a: v })} />
            <Num label="Battery voltage" unit="V" value={draft.voltage_v} onChange={(v) => setDraft({ ...draft, voltage_v: v })} />
            <Num label="Bolt pattern" unit="mm" value={draft.mount_mm} onChange={(v) => setDraft({ ...draft, mount_mm: v })} />
          </div>
          <Label className="flex flex-col items-stretch gap-1 text-xs font-medium text-muted-foreground">
            Propeller used for the push number
            <Input value={draft.prop} onChange={(e) => setDraft({ ...draft, prop: e.target.value })} />
          </Label>
          {error && <p className="text-sm text-destructive">{error}</p>}
          <div className="flex gap-2">
            <Button type="submit" data-testid="save-motor">
              Save motor
            </Button>
            <Button type="button" variant="outline" onClick={() => setDraft(null)}>
              Cancel
            </Button>
          </div>
        </form>
      ) : (
        <Button variant="outline" className="self-start" onClick={() => setDraft({ ...EMPTY_MOTOR })} data-testid="add-motor">
          <PlusIcon />
          Add a motor
        </Button>
      )}
    </Block>
  );
}
