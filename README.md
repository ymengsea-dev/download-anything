<p align="center"><img src="design/icon/app-icon.png" width="160" alt="Download Anything logo: a photo, a play button and a music note dropping into an open cardboard box"></p>

# Download Anything

A tiny desktop app for macOS, Windows and Linux. Paste a link and download the video, images or audio on that page.
If a page has images plus audio (for example a TikTok or Instagram photo slideshow, or a song with cover art),
it can render them into one mp4.

**[What it does →](docs/FEATURES.md)**

Built with Tauri 2 (Rust) and React, bundled with esbuild. The app is **standalone**: its download engines
ship inside it, so it never downloads dependencies on its own.

| Engine | Used for | Bundled version |
|---|---|---|
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) | video, audio and photos from 1,800+ sites | pinned in [scripts/engines.mjs](scripts/engines.mjs) |
| [FFmpeg](https://ffmpeg.org) | merging streams, mp3, conversion, slideshows | [ffmpeg-static](https://github.com/eugeneware/ffmpeg-static) build, pinned likewise |

`pnpm engines` fetches exactly those versions for the target platform and checks each file against a pinned
SHA-256 before using it; the files aren't committed. Sites change often, so Settings has an **Update yt-dlp**
button. It downloads the latest official release only when clicked, and only after checking it against yt-dlp's
published SHA-256 list.

Public Instagram photo posts are read from the data yt-dlp already fetches. When no engine recognises a link, the
app reads the page's `og:` tags and its `<img>`, `<video>` and `<audio>` elements, and handles direct links to files.

## Develop

Prerequisites: [Rust](https://rustup.rs), Node 22+, pnpm, and the
[Tauri system dependencies](https://tauri.app/start/prerequisites/) for your OS.

```sh
pnpm install
pnpm engines        # fetch + verify the bundled engines (also runs automatically before dev/build)
pnpm tauri dev      # run the app
pnpm tauri build    # build an installer for this OS
pnpm release:local  # build, then copy the installer into builds/v<version>/
```

`tauri build` writes its output to `backend/target/release/bundle/`. `pnpm release:local` (or
`pnpm collect-build` after a build) copies just the installer for the current version (the `.dmg` on macOS)
into a folder named after the version:

```text
builds/
  v0.1.0/
    Download Anything_0.1.0_aarch64.dmg
  v0.1.1/
    Download Anything_0.1.1_aarch64.dmg
```

`builds/` is in `.gitignore` and is never pushed. To make a new version, bump `version` in
`backend/tauri.conf.json` (and `package.json`). Rebuilding the same version replaces its installer.

Tests (from `backend/`):

```sh
cargo test                     # unit tests
cargo test -- --include-ignored  # plus end-to-end tests that hit the network and run yt-dlp/ffmpeg
```

## Layout

```text
design/icon/         app-icon.svg (hand-drawn icon source) and app-icon.png (its 1024 px render)
design/dmg/          macOS install window: background.svg, its render, and the saved Finder layout
scripts/
  esbuild.mjs        bundles frontend/ into dist/ (`pnpm dev` = dev server on :1420 with live reload)
  collect-build.mjs  copies the installer into builds/v<version>/
  dmg.mjs            adds the install window to the built .dmg
frontend/            React UI; index.html is the page shell (loads main.js + main.css)
  app/               window shell (App.tsx) and engine start-up
  features/download/ main screen: link box, results, download/render jobs
  features/settings/ settings screen: save folder, engines, "Update yt-dlp"
  shared/            typed command wrappers (api.ts), icons, storage
  styles/base.css    colour tokens and styles both screens share
backend/src/         Rust
  lib.rs             module list + app start-up
  commands.rs        Tauri commands
  jobs.rs            job tracking, cancel, progress events
  engines/           binaries.rs (bundled yt-dlp / ffmpeg, verified "Update yt-dlp"), proc.rs (run tools + cancel)
  sources/           probe.rs (URL -> media: direct file -> yt-dlp -> photos.rs -> HTML scan), photos.rs
  output/            download.rs (one item, with progress), render.rs (images + audio -> mp4), media.rs
  http.rs            shared HTTP client + streamed downloads
```

## Releases

Push a `v*` tag. [.github/workflows/release.yml](.github/workflows/release.yml) builds `.dmg` (Apple Silicon + Intel),
`.msi`/`.exe` and `.AppImage`/`.deb` files and attaches them to a draft GitHub release.
The builds are not code-signed. macOS users right-click the app and choose Open the first time.

## Credits

Download Anything is built on these open-source projects. Thank you to their authors and contributors.

### Download engines

yt-dlp and FFmpeg are bundled, unmodified, as separate programs inside the app. They are not stored in
this repo. Their licenses and source links ship with the app in `licenses/`.

| Project | What it does here | License |
|---|---|---|
| [yt-dlp](https://github.com/yt-dlp/yt-dlp) | Downloads video and audio from 1,800+ sites | Unlicense |
| [FFmpeg](https://ffmpeg.org) | Merges streams, converts to mp3, renders slideshows | LGPL-2.1+ / GPL (builds used here: GPL-3.0) |
| [ffmpeg-static](https://github.com/eugeneware/ffmpeg-static) | Ready-made FFmpeg builds for every OS | GPL-3.0 |

### App framework and UI

| Project | License |
|---|---|
| [Tauri](https://github.com/tauri-apps/tauri) and its [opener and dialog plugins](https://github.com/tauri-apps/plugins-workspace) | MIT OR Apache-2.0 |
| [React](https://github.com/facebook/react) | MIT |
| [tokio](https://github.com/tokio-rs/tokio) | MIT |
| [reqwest](https://github.com/seanmonstar/reqwest) | MIT OR Apache-2.0 |
| [serde](https://github.com/serde-rs/serde) / [serde_json](https://github.com/serde-rs/json) | MIT OR Apache-2.0 |
| [regex](https://github.com/rust-lang/regex) | MIT OR Apache-2.0 |
| [flate2](https://github.com/rust-lang/flate2-rs) | MIT OR Apache-2.0 |
| [futures-rs](https://github.com/rust-lang/futures-rs) | MIT OR Apache-2.0 |

### Build tools

| Project | License |
|---|---|
| [esbuild](https://github.com/evanw/esbuild) | MIT |
| [TypeScript](https://github.com/microsoft/TypeScript) | Apache-2.0 |
| [pnpm](https://github.com/pnpm/pnpm) | MIT |
| [Rust](https://github.com/rust-lang/rust) | MIT OR Apache-2.0 |

These Rust crates bring in a few hundred smaller ones, almost all MIT or Apache-2.0. To list them with their licenses,
run `cargo metadata --format-version 1` in `backend/`.

## License

[MIT](LICENSE) © 2026 ymengsea

The MIT license covers this project's own code. The bundled engines keep their own licenses (FFmpeg: GPL-3.0,
yt-dlp: Unlicense). They run as separate programs, so the app itself stays MIT.

**Sharing builds:** the macOS FFmpeg build currently pinned is configured as "nonfree", so Mac builds are for
personal use only until it's replaced; `pnpm engines --release` (used by the release workflow) refuses them.
Windows and Linux builds may be shared.
