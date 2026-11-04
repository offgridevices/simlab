// Every colour and type size lives in src/styles/tokens.css (colour-scale
// stops in src/lib/colormap-data.ts). This fails when a raw colour or a pixel
// font size slips into any other file, so one change in tokens.css really
// changes the whole app.
import { describe, expect, it } from "vitest";

// every source file's text, through Vite (no Node types needed)
const SOURCES = import.meta.glob("../src/**/*.{ts,tsx,css}", { query: "?raw", import: "default", eager: true }) as Record<string, string>;
const ALLOWED = new Set(["styles/tokens.css", "lib/colormap-data.ts"]);

const RULES: { name: string; re: RegExp }[] = [
  { name: "hex colour", re: /#[0-9a-fA-F]{3,8}\b(?![\w-])/g },
  { name: "0x colour (three.js)", re: /\b0x[0-9a-fA-F]{6}\b/g },
  { name: "rgb()/hsl() colour", re: /\b(?:rgba?|hsla?)\(/g },
  { name: "pixel font size", re: /\btext-\[\d+(?:\.\d+)?px\]|font-size\s*:\s*\d+(?:\.\d+)?px|fontSize\s*[:=]\s*["'{]?\d/g },
  { name: "named colour class", re: /\b(?:bg|text|border|shadow|fill|stroke)-(?:black|white)\b/g },
];

describe("no raw styles outside the tokens", () => {
  it("finds none", () => {
    const hits: string[] = [];
    expect(Object.keys(SOURCES).length).toBeGreaterThan(40);
    for (const [path, text] of Object.entries(SOURCES)) {
      const rel = path.replace(/^\.\.\/src\//, "");
      if (ALLOWED.has(rel)) continue;
      text.split("\n").forEach((line, i) => {
        for (const r of RULES) for (const m of line.matchAll(r.re)) hits.push(`${rel}:${i + 1} ${r.name}: ${m[0]}`);
      });
    }
    expect(hits).toEqual([]);
  });
});
