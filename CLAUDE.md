# CLAUDE.md

Download Anything: a lightweight cross-platform desktop app (macOS/Windows/Linux). Paste a URL and it
downloads the video, images or audio on the page. If the page has images plus audio and no video, it
renders a slideshow mp4. Product behaviour is documented in [docs/FEATURES.md](docs/FEATURES.md); keep that
file in sync with user-facing changes.

## Stack

- **Tauri 2** (Rust backend, OS webview). **React 19 + TypeScript** UI, bundled by **esbuild** via
  [scripts/esbuild.mjs](scripts/esbuild.mjs).
- **pnpm** only (pinned in `package.json` → `packageManager`). Never use npm or yarn commands.
- **Standalone:** yt-dlp and FFmpeg are bundled as Tauri sidecars (`bundle.externalBin`). `pnpm engines`
  ([scripts/engines.mjs](scripts/engines.mjs)) fetches pinned versions into `src-tauri/binaries/` (git-ignored) and
  verifies pinned SHA-256s; it runs before `tauri dev`/`build`, and once by hand before plain `cargo build`/`test`.
  On macOS yt-dlp is the official folder build instead of a sidecar: `yt-dlp_macos.zip` is a resource
  (`tauri.macos.conf.json`) that the app unpacks once into app data. The single-file build took 7s to start on every
  run, because macOS checks each library it unpacks to a new temp folder.
  The app never downloads dependencies by itself. The only exception is the user-clicked "Update yt-dlp",
  verified against yt-dlp's SHA2-256SUMS ([src-tauri/src/binaries.rs](src-tauri/src/binaries.rs)).
  Don't add PATH lookups, auto-updates or runtime downloads of tools. gallery-dl was removed on purpose:
  prefer reading yt-dlp's data (photos.rs) over adding tools.

## Commands

```sh
pnpm install
pnpm tauri dev            # run the app (starts `pnpm dev` = esbuild dev server on :1420)
pnpm build                # tsc type-check + production bundle into dist/
pnpm tauri build          # release build → src-tauri/target/release/bundle/
pnpm release:local        # release build + copy installers to builds/v<version>/<os>-<arch>/ (git-ignored)

cd src-tauri
cargo test                       # unit tests
cargo test -- --include-ignored  # + e2e tests (network, real yt-dlp/ffmpeg)
```

Check changes in `pnpm tauri dev`. **Don't run `pnpm tauri build` / `pnpm release:local`** unless the owner asks:
they build installers only once a change meets their expectations.

Before calling work done: `pnpm build` and `cargo build` with no warnings, and `cargo test`. For engine
changes also run the ignored e2e tests.

## Layout

- `src/App.tsx`: the main screen; `src/Settings.tsx`: the settings screen (gear icon, top right), which holds the
  save folder and the engine list.
- `src/api.ts`: typed `invoke`/`listen` wrappers. Keep them in sync with
  `src-tauri/src/lib.rs` commands and the serde types in `probe.rs`.
- `src-tauri/src/`
  - `lib.rs`: Tauri commands, job registry (cancel via `watch` channel), progress events
    (`setup-progress`, `job-progress`).
  - `probe.rs`: URL → `MediaItem`s. Order: direct file link → yt-dlp → photos.rs → HTML/og-tag scan. `Source` tells
    `download.rs` how to fetch each item.
  - `photos.rs`: public photo posts that yt-dlp reads but won't download (it only does video). `saved_pages`
    runs yt-dlp with `--write-pages`, then a small reader per site picks the photos: Instagram
    (`image_versions2`; first candidate = full size), TikTok photo mode (`/photo/<id>` rewritten to
    `/video/<id>`; photos from `__UNIVERSAL_DATA_FOR_REHYDRATION__`, sound from yt-dlp's own audio format),
    Facebook (`all_subattachments`, else the `fbid` photo). Prefer this "reuse yt-dlp's data" approach over adding
    tools. Known public-data limits: no music on Instagram photo posts, no TikTok `/music/` pages.
  - `download.rs`: one item → file (yt-dlp / direct HTTP).
  - `render.rs`: images + audio → mp4 (ffmpeg, libx264 + aac).
  - `media.rs`: `inspect` (ffmpeg banner → duration/size/codecs) and `make_playable`, which converts any
    downloaded video that isn't H.264 + AAC/MP3 in mp4. Videos must always play in QuickTime.
  - `proc.rs`: spawn tools, stream lines (lossy UTF-8), cancel/timeout. Each tool gets its own
    process group, and cancel kills the whole tree. Hides console windows on Windows.
  - `binaries.rs` also backs **Settings → Download engines**: `status` (version, bundled vs updated) and
    `update_yt_dlp`.
  - `http.rs`: the one shared reqwest client (connect + read timeouts) and `stream_to_file`, which reports
    progress at most once per percent. Don't build new clients or copy the streaming loop.
- Every long-running command (`probe_url`, `download_item`, `render_slideshow`) takes a `job_id`, which must
  be unique while the job runs; `cancel_job(job_id)` stops it.
- `design/app-icon.svg`: the hand-drawn icon source. To change the icon, edit it, re-render
  `rsvg-convert -w 1024 -h 1024 design/app-icon.svg -o design/app-icon.png`, then run `pnpm icons`. That
  regenerates `src-tauri/icons/` and deletes everything `bundle.icon` doesn't list, so the folder only holds
  the 5 files the installers use. The UI's placeholder logo imports `icons/128x128.png`.
- `scripts/collect-build.mjs`: copies installers into `builds/`. `builds/` and `dist/` are never committed.
- `design/dmg/` + `scripts/dmg.mjs`: the macOS install window. Tauri's own DMG styling skips CI and doesn't stick on
  recent macOS, so don't use `bundle.macOS.dmg`. `pnpm release:local` and the release workflow copy the committed
  `background.tiff` and `DS_Store` into the built .dmg with hdiutil; a bare `pnpm tauri build` gives a plain window.
  To change it, edit `background.svg` (340 pt tall; Finder's title bar takes the other 32) or the positions in
  `dmg.mjs`, then run `pnpm dmg-layout` on a Mac to re-render the art and regenerate the layout (opens Finder).

## Conventions

- **Keep it light.** Small installer and low RAM are product goals. Prefer the standard library and existing
  deps; justify any new dependency. No UI component or state libraries; plain CSS in `App.css`
  (light/dark via `prefers-color-scheme`).
- **Dependency policy (owner's rule):** don't use Vite or anything from its toolchain (Vite, Vitest,
  `@vitejs/*`, Rolldown, Oxc, VoidZero), and don't add tools from Chinese companies or projects
  (e.g. Rspack, Farm). Ask the owner before adding any new npm package or Rust crate.
- Rust errors cross the IPC boundary as `String` (`proc::Result<T>`); use `proc::err` / `map_err`.
- yt-dlp output parsing relies on the `[da]` progress-template prefix and `--print after_move:filepath`.
- **No system access (owner's security rule):** the app only writes downloads to the chosen folder, plus its own
  app-data/cache/temp files (settings, the optional yt-dlp update, short-lived work files it deletes). Never read
  browser data or cookies, the keychain, the clipboard, or other apps' folders, and never run package managers.
  Browser login, clipboard auto-fill and Homebrew installs were removed for this reason. Solve site access with
  public data only (see photos.rs); if something seems to need more, explain the limit instead of adding it.
- Licensing of bundled engines: every build must be shareable. Never ship an FFmpeg configured with
  `--enable-nonfree`. The macOS FFmpeg is compiled from GPG-verified official source by
  `scripts/build-ffmpeg-macos.sh` as **LGPL** (no `--enable-gpl` or `--enable-nonfree`, `--disable-autodetect`,
  system libraries only; the script checks all of this). So on macOS H.264 is encoded with VideoToolbox, not
  x264 (`media::VideoEncoder`). Windows/Linux use GPL-3.0 ffmpeg-static builds. Licenses and source links ship in
  `licenses/` (bundled from `src-tauri/binaries/licenses/`). Running engines as separate programs keeps this
  project MIT.
- The project is **MIT** ([LICENSE](LICENSE)). Third-party credits live in the README's **Credits** section;
  update it when dependencies change.
- Mobile targets are out of scope.

## Releases

Bump `version` in `src-tauri/tauri.conf.json` (and `package.json`). Push a `v*` tag; the GitHub Actions workflow
builds macOS (arm64 + x64), Windows and Linux installers into a draft release. Builds are unsigned.
