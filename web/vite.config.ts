/// <reference types="vitest/config" />
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { fileURLToPath } from "node:url";

// Preload the body-text font so the first paint already has it.
function preloadFonts(names: string[]): Plugin {
  return {
    name: "simlab:preload-fonts",
    apply: "build",
    transformIndexHtml: {
      order: "post",
      handler(_html, ctx) {
        const files = Object.keys(ctx.bundle ?? {}).filter((f) => f.endsWith(".woff2") && names.some((n) => f.includes(n)));
        return files.map((f) => ({
          tag: "link",
          attrs: { rel: "preload", href: `./${f}`, as: "font", type: "font/woff2", crossorigin: "" },
          injectTo: "head" as const,
        }));
      },
    },
  };
}

// The engine (http://127.0.0.1:7311) serves the built files from web/dist and
// the API under /api. In development Vite forwards /api to it; with
// VITE_MOCK=1 (`pnpm demo`) the app runs on its own, on made-up data.
const ENGINE = process.env.SIMLAB_ENGINE ?? "http://127.0.0.1:7311";

export default defineConfig({
  // relative asset paths: works from the engine's root and from a file:// or Tauri asset URL
  base: "./",
  plugins: [react(), tailwindcss(), preloadFonts(["geist-latin-wght-normal"])],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  server: {
    host: "127.0.0.1",
    port: 5311,
    proxy: { "/api": { target: ENGINE, changeOrigin: true } },
  },
  build: {
    target: "es2022",
    outDir: "dist",
    chunkSizeWarningLimit: 700,
    rollupOptions: {
      output: { manualChunks: (id) => (id.includes("node_modules/three") ? "three" : undefined) },
    },
  },
  test: {
    environment: "jsdom",
    include: ["test/**/*.test.ts", "test/**/*.test.tsx"],
    execArgv: ["--no-experimental-webstorage"],
  },
});
