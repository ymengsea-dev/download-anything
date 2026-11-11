// Bundles the React UI into dist/ with esbuild.
//   node scripts/esbuild.mjs          production build (minified)
//   node scripts/esbuild.mjs --serve  dev server on :1420 with live reload (used by `tauri dev`)
import * as esbuild from "esbuild";
import { copyFileSync, mkdirSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const outdir = join(root, "dist");
const serve = process.argv.includes("--serve");
const port = 1420; // must match build.devUrl in backend/tauri.conf.json

rmSync(outdir, { recursive: true, force: true });
mkdirSync(outdir, { recursive: true });
copyFileSync(join(root, "frontend/index.html"), join(outdir, "index.html"));

/** @type {esbuild.BuildOptions} */
const options = {
  entryPoints: { main: join(root, "frontend/main.tsx") },
  outdir,
  bundle: true,
  format: "esm",
  jsx: "automatic",
  // Small images (the logo) are inlined as data: URLs.
  loader: { ".png": "dataurl" },
  // Oldest webviews Tauri 2 supports: WebView2 (Chromium), WKWebView (Safari), WebKitGTK.
  target: ["es2021", "chrome105", "safari15"],
  define: { "process.env.NODE_ENV": JSON.stringify(serve ? "development" : "production") },
  minify: !serve,
  sourcemap: serve,
  legalComments: "none",
  logLevel: "info",
  // Reload the window whenever a rebuild finishes.
  banner: serve
    ? { js: `new EventSource("/esbuild").addEventListener("change", () => location.reload());` }
    : undefined,
};

if (serve) {
  const ctx = await esbuild.context(options);
  await ctx.watch();
  await ctx.serve({ servedir: outdir, port, host: process.env.TAURI_DEV_HOST || "127.0.0.1" });
  console.log(`Dev server on http://localhost:${port}`);
} else {
  await esbuild.build(options);
}
