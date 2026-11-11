// Copies the installer from the last `tauri build` into builds/v<version>/ (the .dmg on macOS).
// Usage: pnpm release:local   (or `node scripts/collect-build.mjs` after a build)
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const { version } = JSON.parse(readFileSync(join(root, "backend/tauri.conf.json"), "utf8"));
const dest = join(root, "builds", `v${version}`);

// `tauri build` writes to target/release/bundle, or target/<triple>/release/bundle with --target.
const target = join(root, "backend/target");
const bundleDirs = [join(target, "release/bundle")]
  .concat(readdirSync(target).map((t) => join(target, t, "release/bundle")))
  .filter((d) => existsSync(d));

// The installer folder and file type per OS. Only files for this version are taken, so leftovers
// from older builds aren't copied. Names include the arch, so arm64 and x64 builds sit side by side.
const kinds = {
  darwin: { dmg: ".dmg" },
  win32: { msi: ".msi", nsis: ".exe" },
  linux: { appimage: ".AppImage", deb: ".deb" },
}[process.platform] ?? {};

const found = [];
for (const dir of bundleDirs) {
  for (const [kind, ext] of Object.entries(kinds)) {
    const kindDir = join(dir, kind);
    if (!existsSync(kindDir)) continue;
    for (const name of readdirSync(kindDir)) {
      if (name.endsWith(ext) && name.includes(version)) found.push(join(kindDir, name));
    }
  }
}

if (found.length === 0) {
  console.error(`No v${version} installers found. Run \`pnpm tauri build\` first.`);
  process.exit(1);
}

mkdirSync(dest, { recursive: true });
for (const src of found) {
  const name = src.split(/[\\/]/).pop();
  copyFileSync(src, join(dest, name));
  console.log(`  ${name}`);
}
console.log(`Copied to builds/v${version}/`);
