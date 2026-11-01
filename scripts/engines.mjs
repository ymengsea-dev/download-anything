// Fetches the download engines that ship inside the app (yt-dlp, FFmpeg) for one target,
// verifying every file against a pinned SHA-256 before writing it. Nothing is executed.
//
//   pnpm engines                      # this machine's target (runs before `tauri dev` / `tauri build`)
//   pnpm engines --target x86_64-pc-windows-msvc
//
// macOS: FFmpeg is compiled from official source by scripts/build-ffmpeg-macos.sh (LGPL, shareable),
// because the ready-made Mac builds are "nonfree". It's rebuilt only when that script changes.
//
// Output: src-tauri/binaries/<name>-<target-triple>[.exe] (Tauri `externalBin`) and
// src-tauri/binaries/licenses/ (bundled as resources). Already-verified files are reused.
//
// To upgrade an engine: change its version/URLs below and paste the new SHA-256 digests from the
// release page (GitHub shows them per asset, or `gh api repos/<owner>/<repo>/releases/tags/<tag>`).
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, readdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { gunzipSync } from "node:zlib";

const YT_DLP = "2026.10.26";
const YT_DLP_URL = `https://github.com/yt-dlp/yt-dlp/releases/download/${YT_DLP}`;
const FFMPEG = "b6.1.1"; // eugeneware/ffmpeg-static release tag
const FFMPEG_URL = `https://github.com/eugeneware/ffmpeg-static/releases/download/${FFMPEG}`;

// Per target: yt-dlp [url, sha256]; FFmpeg either "build-macos" or [url, sha256, gzipped] plus its license.
const TARGETS = {
  "aarch64-apple-darwin": {
    ytDlp: [`${YT_DLP_URL}/yt-dlp_macos`, "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa"],
    ffmpeg: "build-macos",
  },
  "x86_64-apple-darwin": {
    ytDlp: [`${YT_DLP_URL}/yt-dlp_macos`, "0f922c15ccc0b83eefa1c8bc9104e434318c4bf16e408e1ee93b7745a6efe0aa"],
    ffmpeg: "build-macos",
  },
  "x86_64-pc-windows-msvc": {
    ytDlp: [`${YT_DLP_URL}/yt-dlp.exe`, "f5389d3698ecde4ef06f4e6b1ce0294fe9ac1396228c8990b8d3416d6d177a7d"],
    ffmpeg: [`${FFMPEG_URL}/ffmpeg-win32-x64.gz`, "8883a3dffbd0a16cf4ef95206ea05283f78908dbfb118f73c83f4951dcc06d77", true],
    license: [`${FFMPEG_URL}/win32-x64.LICENSE`, "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903"],
  },
  "x86_64-unknown-linux-gnu": {
    ytDlp: [`${YT_DLP_URL}/yt-dlp_linux`, "767ba9dce7052aa196f8b12652f433d76de128ba168966fa84b6394d5e6c6795"],
    ffmpeg: [`${FFMPEG_URL}/ffmpeg-linux-x64.gz`, "bfe8a8fc511530457b528c48d77b5737527b504a3797a9bc4866aeca69c2dffa", true],
    license: [`${FFMPEG_URL}/linux-x64.LICENSE`, "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903"],
  },
  "aarch64-unknown-linux-gnu": {
    ytDlp: [`${YT_DLP_URL}/yt-dlp_linux_aarch64`, "e87e9533e89fe5889abaa21f5114dfe7756d6aa6219b83551add04d48b18e483"],
    ffmpeg: [`${FFMPEG_URL}/ffmpeg-linux-arm64.gz`, "754a678672298bc68156adff58aa7385a592c2b30b1d0ae8750c45c915c4bac0", true],
    license: [`${FFMPEG_URL}/linux-x64.LICENSE`, "8ceb4b9ee5adedde47b31e975c1d90c73ad27b6b165a1dcd80c7c545eb65b903"],
  },
};

const root = fileURLToPath(new URL("..", import.meta.url));
const outDir = join(root, "src-tauri/binaries");
const args = process.argv.slice(2);
const target = args.includes("--target") ? args[args.indexOf("--target") + 1] : hostTriple();
const spec = TARGETS[target];
if (!spec) fail(`No engines are pinned for ${target}. Known targets: ${Object.keys(TARGETS).join(", ")}`);

const exe = target.includes("windows") ? ".exe" : "";
const licenses = join(outDir, "licenses");
rmSync(licenses, { recursive: true, force: true }); // only this target's licenses get bundled
mkdirSync(licenses, { recursive: true });
await fetchVerified("yt-dlp", ...spec.ytDlp, false, join(outDir, `yt-dlp-${target}${exe}`), true);
if (spec.ffmpeg === "build-macos") {
  buildMacFfmpeg(join(outDir, `ffmpeg-${target}`));
} else {
  await fetchVerified("FFmpeg", ...spec.ffmpeg, join(outDir, `ffmpeg-${target}${exe}`), true);
  await fetchVerified("FFmpeg license", ...spec.license, false, join(licenses, "FFmpeg-LICENSE.txt"), false);
}
writeFileSync(join(licenses, "README.txt"), notice(spec.ffmpeg === "build-macos"));
// Verification stamps aren't licenses; keep them out of the bundled folder.
for (const f of readdirSync(licenses)) if (f.endsWith(".sha256")) rmSync(join(licenses, f));

/** Compiles FFmpeg from source (macOS only), reusing the last build until the script changes. */
function buildMacFfmpeg(dest) {
  const script = join(root, "scripts/build-ffmpeg-macos.sh");
  const buildId = createHash("sha256").update(readFileSync(script)).digest("hex");
  const stamp = `${dest}.build`;
  const cache = join(outDir, "ffmpeg-licenses-macos");
  if (existsSync(dest) && existsSync(stamp) && readFileSync(stamp, "utf8").trim() === buildId) {
    console.log("  FFmpeg: built copy already present");
  } else {
    if (process.platform !== "darwin") fail("the macOS FFmpeg can only be built on a Mac");
    console.log("  FFmpeg: compiling from official source (first time takes a few minutes)…");
    rmSync(cache, { recursive: true, force: true });
    execFileSync("bash", [script, target, dest, cache], { stdio: "inherit" });
    writeFileSync(stamp, buildId + "\n");
  }
  for (const name of ["FFmpeg-LICENSE.txt", "LAME-LICENSE.txt", "dav1d-LICENSE.txt"]) {
    writeFileSync(join(licenses, name), readFileSync(join(cache, name)));
  }
}

/** Downloads `url`, checks it against `sha256` (of the file as published), then writes `dest`. */
async function fetchVerified(name, url, sha256, gzipped, dest, executable) {
  const stamp = `${dest}.sha256`;
  if (existsSync(dest) && existsSync(stamp) && readFileSync(stamp, "utf8").trim() === sha256) {
    console.log(`  ${name}: verified copy already present`);
    return;
  }
  console.log(`  ${name}: downloading ${url}`);
  const res = await fetch(url);
  if (!res.ok) fail(`${name}: HTTP ${res.status} for ${url}`);
  const body = Buffer.from(await res.arrayBuffer());
  const actual = createHash("sha256").update(body).digest("hex");
  if (actual !== sha256) fail(`${name}: checksum mismatch for ${url}\n  expected ${sha256}\n  got      ${actual}`);

  const part = `${dest}.part`;
  writeFileSync(part, gzipped ? gunzipSync(body) : body);
  if (executable) chmodSync(part, 0o755);
  renameSync(part, dest);
  writeFileSync(stamp, sha256 + "\n");
  console.log(`  ${name}: verified (sha256 ${sha256.slice(0, 12)}…)`);
}

function hostTriple() {
  const out = execFileSync("rustc", ["-vV"], { encoding: "utf8" });
  return out.match(/^host: (.+)$/m)[1].trim();
}

function fail(message) {
  console.error(`engines: ${message}`);
  process.exit(1);
}

function notice(macBuild) {
  const ffmpeg = macBuild
    ? `FFmpeg 9.0.2, compiled from the official source by scripts/build-ffmpeg-macos.sh
  License: GNU Lesser General Public License 2.1 or later - see FFmpeg-LICENSE.txt
           (built without the GPL and "nonfree" options)
  Source:  https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz
  Includes LAME 3.100 (LGPL, see LAME-LICENSE.txt, source: https://lame.sourceforge.io)
       and dav1d 1.5.4 (BSD 2-clause, see dav1d-LICENSE.txt, source: https://code.videolan.org/videolan/dav1d)`
    : `FFmpeg (static build from https://github.com/eugeneware/ffmpeg-static/releases/tag/${FFMPEG})
  License: GNU General Public License version 3 - see FFmpeg-LICENSE.txt
  Source:  https://ffmpeg.org/download.html#get-sources`;
  return `Download Anything bundles these programs as separate executables:

yt-dlp ${YT_DLP} (unmodified)
  License: The Unlicense (public domain) - https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE
  Source:  https://github.com/yt-dlp/yt-dlp/tree/${YT_DLP}

${ffmpeg}
`;
}
