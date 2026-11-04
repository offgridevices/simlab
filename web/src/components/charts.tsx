// Charts: shadcn's Chart (Recharts) for every line chart in SimLab, with
// round-number axes in the mono font, units in the tooltip, series colours
// from the tokens (--chart-1 … --chart-5), and a Progress bar for meters.
import { CartesianGrid, Line, LineChart as RLineChart, XAxis, YAxis } from "recharts";
import { ChartContainer, ChartLegend, ChartLegendContent, ChartTooltip, ChartTooltipContent, type ChartConfig } from "@/components/ui/chart";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";

export interface Series {
  name: string;
  xs: number[];
  ys: number[];
  /** 1–5: which of the token chart colours; default by position */
  color?: number;
  /** a reference run drawn dashed */
  dashed?: boolean;
}

/** series as Recharts rows: one row per x, a column per series */
function rows(series: Series[]) {
  const byX = new Map<number, Record<string, number>>();
  series.forEach((s, i) =>
    s.xs.forEach((x, j) => {
      const y = s.ys[j];
      if (y === undefined || !Number.isFinite(y)) return;
      const r = byX.get(x) ?? { x };
      r[`s${i}`] = y;
      byX.set(x, r);
    }),
  );
  return [...byX.values()].sort((a, b) => a.x! - b.x!);
}

export function LineChart({
  series,
  yLog,
  height = 180,
  fmtX,
  fmtY,
  label,
  xLabel,
  yLabel,
  className,
}: {
  series: Series[];
  yLog?: boolean;
  height?: number;
  fmtX(v: number): string;
  fmtY(v: number): string;
  label: string;
  xLabel?: string;
  yLabel?: string;
  className?: string;
}) {
  const live = series.filter((s) => s.xs.length > 0);
  const data = rows(live);
  const config: ChartConfig = Object.fromEntries(live.map((s, i) => [`s${i}`, { label: s.name, color: `var(--chart-${s.color ?? (i % 5) + 1})` }]));
  return (
    <figure className={cn("flex min-w-0 flex-col gap-1.5", className)} aria-label={label}>
      {yLabel && <figcaption className="text-xs text-muted-foreground">{yLabel}</figcaption>}
      <ChartContainer config={config} className="aspect-auto w-full" style={{ height }}>
        <RLineChart data={data} margin={{ top: 6, right: 8, bottom: 0, left: 0 }} accessibilityLayer>
          <CartesianGrid vertical={false} />
          <XAxis
            dataKey="x"
            type="number"
            domain={["dataMin", "dataMax"]}
            tickFormatter={fmtX}
            tickLine={false}
            axisLine={false}
            tickMargin={6}
            minTickGap={24}
          />
          <YAxis
            scale={yLog ? "log" : "auto"}
            domain={yLog ? ["auto", "auto"] : [(min: number) => Math.min(0, min), "auto"]}
            allowDataOverflow={!!yLog}
            tickFormatter={fmtY}
            tickLine={false}
            axisLine={false}
            width={56}
            tickCount={5}
          />
          <ChartTooltip
            cursor
            content={<ChartTooltipContent indicator="line" valueFormatter={fmtY} labelFormatter={(_, p) => `${fmtX(Number(p?.[0]?.payload?.x))}${xLabel ? ` ${xLabel}` : ""}`} />}
          />
          {live.length > 1 && <ChartLegend content={<ChartLegendContent />} />}
          {live.map((s, i) => (
            <Line
              key={s.name}
              dataKey={`s${i}`}
              name={`s${i}`}
              type="monotone"
              stroke={`var(--color-s${i})`}
              strokeWidth={2}
              strokeDasharray={s.dashed ? "5 4" : undefined}
              dot={false}
              connectNulls
              isAnimationActive={false}
            />
          ))}
        </RLineChart>
      </ChartContainer>
    </figure>
  );
}

const TONE_FILL = { sky: "bg-sky", gold: "bg-gold", clay: "bg-clay", moss: "bg-moss", ember: "bg-ember" } as const;
const TONE_TRACK = { sky: "bg-sky/15", gold: "bg-gold/15", clay: "bg-clay/15", moss: "bg-moss/15", ember: "bg-ember/15" } as const;

/** a bar: how much of something is used or done (shadcn Progress in a status colour) */
export function Meter({ value, tone = "sky", className, label }: { value: number; tone?: keyof typeof TONE_FILL; className?: string; label?: string }) {
  return <Progress value={Math.max(0, Math.min(1, value)) * 100} aria-label={label} className={cn("h-1.5", TONE_TRACK[tone], className)} indicatorClassName={TONE_FILL[tone]} />;
}
