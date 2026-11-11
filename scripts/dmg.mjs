// Gives the macOS .dmg its install window (background, icon positions) from the files in design/dmg/.
// Tauri can't be relied on for this: it skips the Finder step on CI, and on recent macOS the step runs but
// nothing sticks. So the Finder layout is made once (--layout) and committed, then copied into each finished
// .dmg with hdiutil only, which needs no Finder and works the same locally and on CI.
//
// Usage:
//   node scripts/dmg.mjs build [tauri build args]   tauri build, then style the .dmg (a no-op on other OSes)
//   node scripts/dmg.mjs style [file.dmg ...]       style the given .dmg files, or this version's built ones
//   node scripts/dmg.mjs --layout                   after editing design/dmg/background.svg or the positions below:
//                                                   re-render the art and regenerate design/dmg/DS_Store (opens Finder)
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, renameSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("..", import.meta.url));
const { version, productName } = JSON.parse(readFileSync(join(root, "backend/tauri.conf.json"), "utf8"));
const designDir = join(root, "design/dmg");
const background = join(designDir, "background.tiff");
const dsStore = join(designDir, "DS_Store");

// Finder's window size includes its 32 pt title bar, so background.svg is 340 tall. Positions are icon centres.
const window = { x: 200, y: 120, width: 660, height: 372 };
const appIcon = { x: 170, y: 165 };
const applicationsIcon = { x: 490, y: 165 };

const [command, ...args] = process.argv.slice(2);
if (command === "--layout") {
  makeLayout();
} else if (command === "style") {
  styleAll(args);
} else if (command === "build") {
  execFileSync("pnpm", ["tauri", "build", ...args], { stdio: "inherit", cwd: root, shell: process.platform === "win32" });
  if (process.platform === "darwin") styleAll([]);
} else {
  console.error("Usage: node scripts/dmg.mjs build [args] | style [file.dmg ...] | --layout");
  process.exit(1);
}

// Finder caches a volume's background by path, so changed art needs a new file name to show up.
function backgroundName() {
  return `background-${createHash("sha256").update(readFileSync(background)).digest("hex").slice(0, 8)}.tiff`;
}

function hdiutil(...hdiArgs) {
  return execFileSync("hdiutil", hdiArgs, { stdio: "pipe" });
}

function styleAll(files) {
  const dmgs = files.length ? files : builtDmgs();
  if (dmgs.length === 0) {
    console.error(`No v${version} .dmg found. Run \`pnpm tauri build\` first.`);
    process.exit(1);
  }
  for (const dmg of dmgs) {
    style(dmg);
    console.log(`  styled ${dmg.split("/").pop()}`);
  }
}

// `tauri build` writes to target/release/bundle, or target/<triple>/release/bundle with --target.
function builtDmgs() {
  const target = join(root, "backend/target");
  return ["release/bundle/dmg"]
    .concat(readdirSync(target).map((t) => join(t, "release/bundle/dmg")))
    .map((d) => join(target, d))
    .filter((d) => existsSync(d))
    .flatMap((d) => readdirSync(d).filter((n) => n.endsWith(".dmg") && n.includes(version)).map((n) => join(d, n)));
}

function style(dmg) {
  const work = mkdtempSync(join(tmpdir(), "da-dmg-"));
  const writable = join(work, "rw.dmg");
  const mount = join(work, "mnt");
  try {
    hdiutil("convert", dmg, "-format", "UDRW", "-o", writable);
    mkdirSync(mount);
    hdiutil("attach", writable, "-nobrowse", "-noverify", "-noautoopen", "-mountpoint", mount);
    try {
      rmSync(join(mount, ".background"), { recursive: true, force: true });
      mkdirSync(join(mount, ".background"));
      copyFileSync(background, join(mount, ".background", backgroundName()));
      copyFileSync(dsStore, join(mount, ".DS_Store"));
      hideExtension(join(mount, `${productName}.app`));
    } finally {
      hdiutil("detach", mount, "-force");
    }
    const finished = join(work, "final.dmg");
    hdiutil("convert", writable, "-format", "UDZO", "-imagekey", "zlib-level=9", "-o", finished);
    copyFileSync(finished, `${dmg}.tmp`);
    renameSync(`${dmg}.tmp`, dmg);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

// So the label reads "Download Anything", not "Download Anything.app": Finder's hidden-extension flag.
function hideExtension(path) {
  const info = Buffer.alloc(32);
  try {
    Buffer.from(execFileSync("xattr", ["-px", "com.apple.FinderInfo", path], { stdio: "pipe" }).toString().replace(/\s/g, ""), "hex").copy(info);
  } catch {
    // No FinderInfo yet: start from zeros.
  }
  info[9] |= 0x10;
  execFileSync("xattr", ["-wx", "com.apple.FinderInfo", info.toString("hex"), path]);
}

function makeLayout() {
  const svg = join(designDir, "background.svg");
  const work = mkdtempSync(join(tmpdir(), "da-dmg-"));
  try {
    const png1x = join(work, "1x.png");
    const png2x = join(work, "2x.png");
    execFileSync("rsvg-convert", ["-w", "660", svg, "-o", png1x]);
    execFileSync("rsvg-convert", ["-w", "1320", svg, "-o", png2x]);
    execFileSync("tiffutil", ["-cathidpicheck", png1x, png2x, "-out", background], { stdio: "pipe" });

    // Finder saves the background as an alias to this path on a volume with this name, and keys icon
    // positions by file name, so the stand-in volume has to match the real .dmg.
    const volume = `/Volumes/${productName}`;
    if (existsSync(volume)) throw new Error(`Eject ${volume} first.`);
    const src = join(work, "src");
    mkdirSync(join(src, `${productName}.app`), { recursive: true });
    mkdirSync(join(src, ".background"));
    symlinkSync("/Applications", join(src, "Applications"));
    copyFileSync(background, join(src, ".background", backgroundName()));
    const image = join(work, "layout.dmg");
    hdiutil("create", "-srcfolder", src, "-volname", productName, "-fs", "HFS+", "-format", "UDRW", "-size", "20m", image);
    hdiutil("attach", image, "-nobrowse", "-noverify", "-noautoopen");
    try {
      // Finder silently loses everything set the first time it opens a new volume, so this runs twice.
      const pass = `tell application "Finder"
  activate
  set d to disk "${productName}"
  open d
  delay 2
  set w to container window of d
  set current view of w to icon view
  set toolbar visible of w to false
  set statusbar visible of w to false
  set bounds of w to {${window.x}, ${window.y}, ${window.x + window.width}, ${window.y + window.height}}
  set o to icon view options of w
  set arrangement of o to not arranged
  set icon size of o to 128
  set text size of o to 13
  set background picture of o to (POSIX file "${volume}/.background/${backgroundName()}") as alias
  set position of item "${productName}.app" of d to {${appIcon.x}, ${appIcon.y}}
  set position of item "Applications" of d to {${applicationsIcon.x}, ${applicationsIcon.y}}
  update d
  delay 1
  close w
  delay 1
  open d
  delay 5
  close container window of d
  delay 3
end tell`;
      for (let i = 0; i < 2; i++) execFileSync("osascript", [], { stdio: ["pipe", "inherit", "inherit"], input: pass });
      // Finder writes .DS_Store lazily, so wait for it.
      const saved = join(volume, ".DS_Store");
      for (let waited = 0; !existsSync(saved) || !readFileSync(saved).includes("backgroundImageAlias"); waited++) {
        if (waited === 30) throw new Error("Finder did not save the background. Try again.");
        Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 1000);
      }
      copyFileSync(saved, dsStore);
    } finally {
      hdiutil("detach", volume, "-force");
    }
    console.log("Updated design/dmg/background.tiff and design/dmg/DS_Store");
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}
