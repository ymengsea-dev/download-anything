//! The download engines (yt-dlp and FFmpeg) ship inside the app as Tauri sidecars
//! (`bundle.externalBin`, fetched and checksum-verified at build time by `scripts/engines.mjs`).
//! The app never downloads dependencies by itself. The one exception is the "Update yt-dlp"
//! button in Settings, which only runs when clicked and verifies the download against
//! yt-dlp's official SHA-256 checksums before using it.
//!
//! On macOS yt-dlp ships as its official folder build (a zip in the app's resources, unpacked once
//! into the app's data folder). The single-file build unpacks itself to a new temp folder on every
//! run, and macOS checks every library in a new location, which adds 7 seconds to each run.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::http;
use crate::proc::{self, err, Result};

const YT_DLP_LATEST: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
const MAC_ZIP: &str = "yt-dlp-macos.zip";
const MAC_EXE: &str = "yt-dlp_macos";
/// Inside an unpacked folder: the SHA-256 of the zip it came from.
const MAC_ZIP_HASH: &str = "zip.sha256";
/// A new copy's first run is slow (macOS checks its libraries), and later ones aren't.
const FIRST_RUN_TIMEOUT: Duration = Duration::from_secs(120);
/// `unpack` is one at a time. The app only calls it once per job, but tests locate in parallel.
static UNPACKING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tool {
    YtDlp,
    Ffmpeg,
}

impl Tool {
    fn name(self) -> &'static str {
        match self {
            Tool::YtDlp => "yt-dlp",
            Tool::Ffmpeg => "ffmpeg",
        }
    }

    fn file_name(self) -> String {
        format!("{}{}", self.name(), std::env::consts::EXE_SUFFIX)
    }

    fn version_flag(self) -> &'static str {
        match self {
            Tool::Ffmpeg => "-version",
            Tool::YtDlp => "--version",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Engine {
    pub yt_dlp: PathBuf,
    pub ffmpeg: PathBuf,
}

/// Finds the bundled engines. `updates_dir` may hold a newer yt-dlp from "Update yt-dlp",
/// which is preferred over the bundled one. `engines_dir` is where macOS unpacks the bundled yt-dlp.
pub async fn locate(updates_dir: &Path, engines_dir: &Path) -> Result<Engine> {
    let bundled = bundled_dir()?;
    let find = |tool: Tool| -> Result<PathBuf> {
        let path = bundled.join(tool.file_name());
        if path.is_file() {
            Ok(path)
        } else {
            Err(format!(
                "The app's {} is missing (looked in {}). Reinstall the app; when developing, run `pnpm engines`.",
                tool.name(),
                bundled.display()
            ))
        }
    };
    let yt_dlp = match updated_yt_dlp(updates_dir) {
        Some(updated) => updated,
        None if cfg!(target_os = "macos") => unpack_bundled(&bundled, engines_dir).await?,
        None => find(Tool::YtDlp)?,
    };
    Ok(Engine { yt_dlp, ffmpeg: find(Tool::Ffmpeg)? })
}

/// The yt-dlp from "Update yt-dlp", if any. On macOS, older copies are removed here, at startup,
/// because a download could still have been using them when the update finished.
fn updated_yt_dlp(updates_dir: &Path) -> Option<PathBuf> {
    if !cfg!(target_os = "macos") {
        let updated = updates_dir.join(Tool::YtDlp.file_name());
        return updated.is_file().then_some(updated);
    }
    let newest = std::fs::read_dir(updates_dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().join(MAC_EXE).is_file())
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())?
        .path();
    remove_others(updates_dir, &newest);
    Some(newest.join(MAC_EXE))
}

async fn unpack_bundled(bundled: &Path, engines_dir: &Path) -> Result<PathBuf> {
    // Next to the executable in `tauri dev`, in Contents/Resources in the installed app.
    let zip = [bundled.join(MAC_ZIP), bundled.join("../Resources").join(MAC_ZIP)]
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| format!("The app's {MAC_ZIP} is missing. Reinstall the app; when developing, run `pnpm engines`."))?;
    let hash = std::fs::read_to_string(format!("{}.sha256", zip.display())).map_err(err)?;
    let exe = unpack(&zip, hash.trim(), engines_dir).await?;
    if let Some(current) = exe.parent() {
        remove_others(engines_dir, current);
    }
    Ok(exe)
}

/// Unpacks `zip` (whose SHA-256 is `hash`) to `parent/<hash>` unless that's already there, and
/// returns its yt-dlp. The new copy has to run before it's moved into place, which also gets its
/// slow first run out of the way, so the first fetch is fast too.
async fn unpack(zip: &Path, hash: &str, parent: &Path) -> Result<PathBuf> {
    let _one_at_a_time = UNPACKING.lock().await;
    let dest = parent.join(hash);
    if dest.join(MAC_EXE).is_file() {
        return Ok(dest.join(MAC_EXE));
    }
    let part = parent.join(format!("{hash}.part"));
    let _ = std::fs::remove_dir_all(&part);
    std::fs::create_dir_all(&part).map_err(err)?;
    let result = async {
        let args = [Path::new("-x"), Path::new("-k"), zip, &part];
        let out = proc::run(Path::new("/usr/bin/ditto"), args, None, |_| {}).await?;
        if !out.success {
            return Err(out.error_message());
        }
        let out = proc::run_with_timeout(&part.join(MAC_EXE), ["--version"], None, Some(FIRST_RUN_TIMEOUT), |_| {}).await?;
        if !out.success {
            return Err(out.error_message());
        }
        std::fs::write(part.join(MAC_ZIP_HASH), hash).map_err(err)?;
        std::fs::rename(&part, &dest).map_err(err)
    }
    .await;
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&part);
    }
    result?;
    Ok(dest.join(MAC_EXE))
}

/// Removes everything in `dir` except `keep`.
fn remove_others(dir: &Path, keep: &Path) {
    for path in std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p != keep) {
        let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
    }
}

/// Sidecars sit next to the app's executable (Contents/MacOS on macOS). Test binaries run from
/// target/<profile>/deps, one level below where `tauri dev`/`cargo build` put them.
fn bundled_dir() -> Result<PathBuf> {
    let exe = std::env::current_exe().map_err(err)?;
    let dir = exe.parent().ok_or("can't locate the app folder")?.to_path_buf();
    Ok(if dir.ends_with("deps") { dir.parent().unwrap_or(&dir).to_path_buf() } else { dir })
}

/// One row of the settings screen.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub tool: Tool,
    pub path: PathBuf,
    pub version: Option<String>,
    /// A yt-dlp installed with "Update yt-dlp" rather than the one bundled with the app.
    pub updated: bool,
}

pub async fn status(engine: &Engine, updates_dir: &Path) -> Vec<ToolStatus> {
    let mut rows = Vec::new();
    for (tool, path) in [(Tool::YtDlp, &engine.yt_dlp), (Tool::Ffmpeg, &engine.ffmpeg)] {
        rows.push(ToolStatus {
            tool,
            path: path.clone(),
            version: version_of(tool, path).await,
            updated: path.starts_with(updates_dir),
        });
    }
    rows
}

async fn version_of(tool: Tool, path: &Path) -> Option<String> {
    let out = proc::run_with_timeout(path, [tool.version_flag()], None, Some(std::time::Duration::from_secs(15)), |_| {})
        .await
        .ok()?;
    let first = out.stdout.lines().find(|l| !l.trim().is_empty())?.trim();
    Some(match tool {
        // "ffmpeg version 6.1.1-… Copyright (c) 2000-2023 the FFmpeg developers"
        Tool::Ffmpeg => first.split_whitespace().nth(2)?.to_string(),
        Tool::YtDlp => first.to_string(),
    })
}

/// What "Update yt-dlp" did.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "result", rename_all = "camelCase")]
pub enum UpdateOutcome {
    AlreadyLatest,
    Updated { path: PathBuf },
}

/// Downloads the latest official yt-dlp for this OS into `updates_dir`, but only after checking
/// it against the SHA2-256SUMS file published with the release. Runs only when the user asks.
pub async fn update_yt_dlp(current: &Path, updates_dir: &Path, on_progress: impl Fn(f64)) -> Result<UpdateOutcome> {
    let asset = yt_dlp_asset()?;
    let sums = http::client()
        .get(format!("{YT_DLP_LATEST}/SHA2-256SUMS"))
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(err)?
        .text()
        .await
        .map_err(err)?;
    let expected = expected_hash(&sums, asset).ok_or_else(|| format!("{asset} isn't listed in yt-dlp's checksums"))?;
    if current_hash(current).ok().as_deref() == Some(expected.as_str()) {
        return Ok(UpdateOutcome::AlreadyLatest);
    }

    std::fs::create_dir_all(updates_dir).map_err(err)?;
    let final_path = updates_dir.join(Tool::YtDlp.file_name());
    let part = updates_dir.join(format!("{}.download", Tool::YtDlp.file_name()));
    let result = async {
        http::fetch(&format!("{YT_DLP_LATEST}/{asset}"), &part, &on_progress).await?;
        let actual = sha256_file(&part)?;
        if actual != expected {
            return Err(format!("The downloaded yt-dlp didn't match its official checksum, so it wasn't used.\nexpected {expected}\ngot {actual}"));
        }
        if cfg!(target_os = "macos") {
            return unpack(&part, &actual, updates_dir).await;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&part, std::fs::Permissions::from_mode(0o755)).map_err(err)?;
        }
        std::fs::rename(&part, &final_path).map_err(err)?;
        Ok(final_path)
    }
    .await;
    let _ = std::fs::remove_file(&part);
    Ok(UpdateOutcome::Updated { path: result? })
}

/// What to compare with the official checksum: on macOS the zip this copy came from.
fn current_hash(current: &Path) -> Result<String> {
    if !cfg!(target_os = "macos") {
        return sha256_file(current);
    }
    let folder = current.parent().ok_or("yt-dlp has no folder")?;
    std::fs::read_to_string(folder.join(MAC_ZIP_HASH)).map(|h| h.trim().to_string()).map_err(err)
}

fn yt_dlp_asset() -> Result<&'static str> {
    use std::env::consts::{ARCH, OS};
    Ok(match (OS, ARCH) {
        ("windows", _) => "yt-dlp.exe",
        ("macos", _) => "yt-dlp_macos.zip",
        ("linux", "aarch64") => "yt-dlp_linux_aarch64",
        ("linux", _) => "yt-dlp_linux",
        _ => return Err(format!("yt-dlp has no build for {OS}")),
    })
}

/// The hash for `asset` from a `sha256sum`-style file ("<hex>  <name>" per line).
fn expected_hash(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == asset).then(|| hash.to_lowercase())
    })
}

fn sha256_file(path: &Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(err)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf).map_err(err)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_lines_are_matched_exactly() {
        let sums = "aa11  yt-dlp\nbb22  yt-dlp_macos\ncc33  yt-dlp_macos.zip\nDD44 *yt-dlp.exe\n";
        assert_eq!(expected_hash(sums, "yt-dlp_macos").as_deref(), Some("bb22"));
        assert_eq!(expected_hash(sums, "yt-dlp.exe").as_deref(), Some("dd44"));
        assert_eq!(expected_hash(sums, "yt-dlp_linux"), None);
    }

    #[tokio::test]
    async fn bundled_engines_are_found() {
        let engines = std::env::temp_dir().join("download-anything-test-engines");
        let engine = locate(Path::new("/nonexistent"), &engines).await.expect("run `pnpm engines` first");
        assert!(engine.yt_dlp.is_file() && engine.ffmpeg.is_file());
    }

    /// Network (downloads yt-dlp): an update installs, runs, and is preferred from then on.
    #[tokio::test]
    #[ignore]
    async fn update_installs_and_is_preferred() {
        let dir = std::env::temp_dir().join("download-anything-update-install");
        let _ = std::fs::remove_dir_all(&dir);
        let UpdateOutcome::Updated { path } = update_yt_dlp(Path::new("/nonexistent/yt-dlp"), &dir, |_| {}).await.unwrap() else {
            panic!("a missing yt-dlp can't be the latest");
        };
        let out = proc::run(&path, ["--version"], None, |_| {}).await.unwrap();
        assert!(out.success);
        let engine = locate(&dir, Path::new("/nonexistent")).await.unwrap();
        assert_eq!(engine.yt_dlp, path);
        assert!(matches!(update_yt_dlp(&path, &dir, |_| {}).await.unwrap(), UpdateOutcome::AlreadyLatest));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Network (checksum list only when already current): `cargo test update_check -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn update_check_against_official_checksums() {
        let engines = std::env::temp_dir().join("download-anything-test-engines");
        let engine = locate(Path::new("/nonexistent"), &engines).await.unwrap();
        let dir = std::env::temp_dir().join("download-anything-update-check");
        let outcome = update_yt_dlp(&engine.yt_dlp, &dir, |_| {}).await.unwrap();
        println!("{outcome:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
