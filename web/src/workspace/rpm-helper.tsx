// "How fast does it spin?" for people who don't know their prop's rpm: work
// it out from the motor's KV and the battery. Top speed under load is about
// KV × battery volts (4.2 V a cell, full) × 0.7.
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { n } from "../lib/format";
import { NumberField } from "./common";

const CELL_V = 4.2;
// a 5-inch prop at full throttle holds the motor to about 70% of its no-load speed
const UNDER_LOAD = 0.7;

export const rpmFrom = (kv: number, cells: number) => Math.round((kv * cells * CELL_V * UNDER_LOAD) / 100) * 100;

export function RpmHelper({ onUse, cta = "Use this speed" }: { onUse(rpm: number): void; cta?: string }) {
  const [kv, setKv] = useState(1900);
  const [cells, setCells] = useState(6);
  const rpm = rpmFrom(kv, cells);
  return (
    <div className="flex flex-col gap-2 rounded-lg border bg-sunken/40 p-3" data-testid="rpm-helper">
      <span className="text-xs font-medium text-muted-foreground">Don't know the speed? Work it out from your motor and battery</span>
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <NumberField label="Motor KV" value={kv} min={50} step={50} width="w-20" unit="KV" onChange={setKv} testId="rpm-kv" />
        <span className="text-muted-foreground">on a</span>
        <Select value={String(cells)} onValueChange={(v) => setCells(Number(v))}>
          <SelectTrigger className="h-8 w-auto" aria-label="Battery size" data-testid="rpm-cells">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {[1, 2, 3, 4, 5, 6, 8, 12].map((c) => (
              <SelectItem key={c} value={String(c)}>
                {c}S battery ({n(c * 3.7, 3)} V)
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-sm">
          Full throttle: about <span className="num font-semibold">{rpm.toLocaleString("en-US")} rpm</span>
        </span>
        <Button size="xs" variant="outline" onClick={() => onUse(rpm)} data-testid="rpm-use">
          {cta}
        </Button>
      </div>
      <p className="text-2xs text-muted-foreground">KV is printed on the motor or its box (like "1900KV"). Typical 5-inch quads: 1900KV on 6S, 2450KV on 4S. 7-inch: about 1300KV on 6S.</p>
    </div>
  );
}
