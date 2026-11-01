// Copies the installers from the last `tauri build` into builds/v<version>/<os>-<arch>/.
// Usage: pnpm release:local   (or `node scripts/collect-build.mjs` after a build)
import { cpSync, createReadStream, existsSync, readdirSync, readFileSync, rmSync, mkdirSync, statSync, writeFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const { version, productName } = JSON.parse(readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"));

const os = { darwin: "macos", win32: "windows", linux: "linux" }[process.platform] ?? process.platform;
const arch = { arm64: "aarch64", x64: "x86_64" }[process.arch] ?? process.arch;
const dest = join(root, "builds", `v${version}`, `${os}-${arch}`);

// `tauri build` writes to target/release/bundle, or target/<triple>/release/bundle with --target.
const target = join(root, "src-tauri/target");
const bundleDirs = [join(target, "release/bundle")]
  .concat(readdirSync(target).map((t) => join(target, t, "release/bundle")))
  .filter((d) => existsSync(d));

// Installer folders and file types per OS. Only files for this version are taken,
// so leftovers from older builds aren't copied. The .app has no version in its name.
const kinds = {
  dmg: [".dmg"],
  macos: [".app"],
  msi: [".msi"],
  nsis: [".exe"],
  appimage: [".AppImage"],
  deb: [".deb"],
  rpm: [".rpm"],
};

const found = [];
for (const dir of bundleDirs) {
  for (const [kind, exts] of Object.entries(kinds)) {
    const kindDir = join(dir, kind);
    if (!existsSync(kindDir)) continue;
    for (const name of readdirSync(kindDir)) {
      const matchesExt = exts.some((e) => name.endsWith(e));
      const matchesVersion = name.includes(version) || name === `${productName}.app`;
      if (matchesExt && matchesVersion) found.push(join(kindDir, name));
    }
  }
}

if (found.length === 0) {
  console.error(`No v${version} installers found. Run \`pnpm tauri build\` first.`);
  process.exit(1);
}

rmSync(dest, { recursive: true, force: true });
mkdirSync(dest, { recursive: true });

const sums = [];
for (const src of found) {
  const name = src.split(/[\\/]/).pop();
  cpSync(src, join(dest, name), { recursive: true, verbatimSymlinks: true });
  if (statSync(src).isFile()) sums.push(`${await sha256(src)}  ${name}`);
  console.log(`  ${name}`);
}
if (sums.length) writeFileSync(join(dest, "SHA256SUMS"), sums.join("\n") + "\n");
console.log(`Copied to builds/v${version}/${os}-${arch}/`);

function sha256(file) {
  return new Promise((resolve, reject) => {
    const hash = createHash("sha256");
    createReadStream(file)
      .on("data", (d) => hash.update(d))
      .on("end", () => resolve(hash.digest("hex")))
      .on("error", reject);
  });
}
