// Regenerates the app icons from design/icon/app-icon.png, then deletes the extras `tauri icon`
// always creates (Windows Store tiles, Android, iOS) that the installers don't use.
// Usage: pnpm icons   (after re-rendering design/icon/app-icon.png from design/icon/app-icon.svg)
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync, rmSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const iconsDir = join(root, "backend/icons");
const { bundle } = JSON.parse(readFileSync(join(root, "backend/tauri.conf.json"), "utf8"));
const keep = new Set(bundle.icon.map((p) => p.replace(/^icons\//, "")));

execFileSync("pnpm", ["tauri", "icon", join(root, "design/icon/app-icon.png")], { stdio: "inherit", cwd: root, shell: process.platform === "win32" });

for (const name of readdirSync(iconsDir)) {
  if (!keep.has(name)) {
    rmSync(join(iconsDir, name), { recursive: true, force: true });
    console.log(`  removed unused ${name}`);
  }
}
console.log(`Kept: ${[...keep].join(", ")}`);
