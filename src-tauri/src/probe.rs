//! Turns a pasted URL into a list of downloadable media.
//!
//! Strategy: direct file links first, then yt-dlp (1,800+ sites), then photo posts read from
//! yt-dlp's own data (see photos.rs), then a plain HTML scan of og: tags and <img>/<video>/<audio>.

use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::binaries::Engine;
use crate::http::client;
use crate::proc::{self, cancelled, err, Cancel, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Video,
    Image,
    Audio,
}

/// How to fetch an item; sent to the UI and handed back verbatim on download.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "via", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Source {
    YtDlp {
        url: String,
        playlist_index: Option<u32>,
        audio_only: bool,
        /// Chosen in the UI from `MediaItem::qualities`; `None` = best H.264.
        #[serde(default)]
        max_height: Option<u32>,
    },
    Direct { url: String, referer: Option<String> },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaItem {
    pub kind: Kind,
    pub title: String,
    /// File type of the saved file, e.g. "mp4", "mp3", "jpg".
    pub format: Option<String>,
    /// Extra info: duration, "cover art"...
    pub detail: Option<String>,
    pub thumbnail: Option<String>,
    /// Video resolutions on offer, best first. Empty when unknown.
    #[serde(default)]
    pub qualities: Vec<Quality>,
    /// Bytes to download, when there's no quality menu (`Quality::size` otherwise). `None` when unknown.
    #[serde(default)]
    pub size: Option<u64>,
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quality {
    /// The short side in pixels ("1080" for 1920x1080 and 1080x1920), like yt-dlp's `res`.
    pub height: u32,
    /// Available as H.264, so it downloads without converting.
    pub compatible: bool,
    /// Estimated bytes to download at this resolution, video and audio together.
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct ProbeResult {
    pub title: String,
    pub items: Vec<MediaItem>,
}

const MAX_ITEMS: usize = 60;
const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "gif", "webp", "avif", "heic", "bmp"];
const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mov", "mkv", "m4v", "avi", "m3u8"];
const AUDIO_EXTS: &[&str] = &["mp3", "m4a", "aac", "ogg", "opus", "wav", "flac"];
/// Upper bound for one engine to inspect a link; past this the UI would look frozen.
const ENGINE_TIMEOUT: Duration = Duration::from_secs(90);
const PAGE_TIMEOUT: Duration = Duration::from_secs(30);

pub async fn probe(engine: &Engine, url: &str, cancel: &Cancel) -> Result<ProbeResult> {
    let mut first_error = None;
    let is_cancelled = || *cancel.borrow();

    // A link straight to a file (photo.jpg, song.mp3, clip.mp4): fetch it directly. yt-dlp
    // only understands video pages and would just report an error for images.
    if kind_from_ext(&ext_of(url)).is_some() {
        match tokio::time::timeout(PAGE_TIMEOUT, probe_html(url)).await {
            Ok(Ok(found)) if !found.items.is_empty() => return Ok(found),
            Ok(Err(e)) => return Err(format!("Couldn't open this file: {e}")),
            Err(_) => return Err("The file took too long to respond".into()),
            Ok(Ok(_)) => {}
        }
    }

    // TikTok photo posts: yt-dlp rejects their /photo/ links outright, so go straight there.
    if let Ok(Some(found)) = crate::photos::tiktok(engine, url, cancel, ENGINE_TIMEOUT).await {
        return Ok(found);
    }

    // Posts that may turn out to be photos keep yt-dlp's pages from this one run, so reading
    // the photos doesn't cost a second run (each one takes seconds just to start).
    let (run, pages) = if crate::photos::may_be_photo_post(url) {
        match crate::photos::saved_pages(engine, url, &YT_DLP_PROBE_ARGS, cancel, ENGINE_TIMEOUT).await {
            Ok((out, pages)) => (Ok(out), pages),
            Err(e) => (Err(e), Vec::new()),
        }
    } else {
        let args = YT_DLP_PROBE_ARGS.into_iter().chain(["--no-playlist", url]);
        (proc::run_with_timeout(&engine.yt_dlp, args, Some(cancel), Some(ENGINE_TIMEOUT), |_| {}).await, Vec::new())
    };
    match run.and_then(|out| read_yt_dlp(&out, url)) {
        Ok(found) if !found.items.is_empty() => return Ok(found),
        Ok(_) => {}
        Err(e) => first_error = Some(e),
    }
    if is_cancelled() {
        return Err("Cancelled".into());
    }

    // Photo posts (Instagram, Facebook): yt-dlp reads them (without a login, for public posts)
    // but only downloads video. Take the photos from the data it fetched.
    if let Some(found) = crate::photos::instagram(url, &pages).or_else(|| crate::photos::facebook(url, &pages)) {
        return Ok(found);
    }

    let html = tokio::select! {
        found = tokio::time::timeout(PAGE_TIMEOUT, probe_html(url)) => {
            found.unwrap_or_else(|_| Err("The page took too long to load".into()))
        }
        _ = cancelled(Some(cancel)) => return Err("Cancelled".into()),
    };
    match html {
        Ok(found) if !found.items.is_empty() => Ok(found),
        // The site only shows this to signed-in users (private accounts, age-restricted posts...).
        // The app never uses the user's own accounts or browser data, so say so plainly.
        _ if first_error.as_deref().is_some_and(is_login_wall) => Err(login_message(url)),
        Ok(_) => Err(first_error.unwrap_or_else(|| "No downloadable media found on this page".into())),
        Err(e) => Err(first_error.unwrap_or(e)),
    }
}

fn is_login_wall(error: &str) -> bool {
    let e = error.to_lowercase();
    ["login", "log in", "logged-in", "sign in", "--cookies", "401", "authentication"].iter().any(|k| e.contains(k))
}

fn login_message(url: &str) -> String {
    let site = site_name(url);
    format!("{site} only shows this to people who are logged in (for example a private account), so it can't be downloaded.")
}

/// "https://www.instagram.com/p/x" → "Instagram".
fn site_name(url: &str) -> String {
    let host = url.split('/').nth(2).unwrap_or_default().trim_start_matches("www.").trim_start_matches("m.");
    let name = host.split('.').next().unwrap_or(host);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) if name == "x" => first.to_uppercase().collect(),
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "This site".into(),
    }
}

// ---------- yt-dlp ----------

const YT_DLP_PROBE_ARGS: [&str; 5] = ["-J", "--no-warnings", "--flat-playlist", "--playlist-items", "1:50"];

fn read_yt_dlp(out: &proc::Output, url: &str) -> Result<ProbeResult> {
    if !out.success {
        return Err(out.error_message());
    }
    let info: Value = serde_json::from_str(&out.stdout).map_err(err)?;
    // yt-dlp's generic fallback "accepts" any direct file link, images included, but knows
    // nothing about it (ext "unknown_video", no codecs). The content-type check in probe_html
    // classifies those properly.
    if info["direct"] == true {
        return Ok(ProbeResult { title: String::new(), items: Vec::new() });
    }

    let mut items = Vec::new();
    match info["entries"].as_array() {
        Some(entries) => {
            for (i, entry) in entries.iter().enumerate() {
                // Flat entries carry their own URL; resolved ones share the parent URL.
                let (src_url, index) = match entry["_type"].as_str() {
                    Some("url") => (entry["url"].as_str().unwrap_or(url).to_string(), None),
                    _ => {
                        let idx = entry["playlist_index"].as_u64().unwrap_or(i as u64 + 1);
                        (url.to_string(), Some(idx as u32))
                    }
                };
                push_yt_dlp_items(&mut items, entry, &src_url, index);
            }
        }
        None => push_yt_dlp_items(&mut items, &info, url, None),
    }
    items.truncate(MAX_ITEMS);
    Ok(ProbeResult { title: title_of(&info).unwrap_or_else(|| url.into()), items })
}

fn push_yt_dlp_items(items: &mut Vec<MediaItem>, e: &Value, url: &str, playlist_index: Option<u32>) {
    let title = title_of(e).unwrap_or_else(|| "Untitled".into());
    let thumbnail = e["thumbnail"]
        .as_str()
        .or_else(|| e["thumbnails"].as_array()?.last()?["url"].as_str())
        .map(String::from);
    let duration = e["duration"].as_f64().map(fmt_duration);
    let (has_video, has_audio) = stream_kinds(e);
    let qualities = qualities(e);

    let source = |audio_only| Source::YtDlp { url: url.into(), playlist_index, audio_only, max_height: None };
    if has_video {
        items.push(MediaItem {
            kind: Kind::Video,
            title: title.clone(),
            format: Some("mp4".into()),
            detail: duration.clone(),
            thumbnail: thumbnail.clone(),
            qualities: qualities.clone(),
            size: if qualities.is_empty() { video_size(e) } else { None },
            source: source(false),
        });
    }
    if has_audio {
        items.push(MediaItem {
            kind: Kind::Audio,
            title: title.clone(),
            format: Some("mp3".into()),
            detail: duration,
            thumbnail: thumbnail.clone(),
            qualities: Vec::new(),
            size: audio_size(e),
            source: source(true),
        });
    }
    // For audio-only pages, offer the cover art too so it can be rendered into a video.
    if !has_video {
        if let Some(thumb) = thumbnail {
            items.push(MediaItem {
                kind: Kind::Image,
                title: format!("{title} (cover)"),
                format: format_for(Kind::Image, &ext_of(&thumb)),
                detail: Some("cover art".into()),
                thumbnail: Some(thumb.clone()),
                qualities: Vec::new(),
                size: None,
                source: Source::Direct { url: thumb, referer: Some(url.into()) },
            });
        }
    }
}

/// Distinct video resolutions, best first, noting which exist as H.264 and how much each downloads.
fn qualities(e: &Value) -> Vec<Quality> {
    let mut by_height = std::collections::BTreeMap::<u32, Vec<&Value>>::new();
    for f in formats_of(e) {
        let vcodec = f["vcodec"].as_str().unwrap_or_default();
        if vcodec == "none" || f["format_note"] == "storyboard" {
            continue;
        }
        let (Some(w), Some(h)) = (f["width"].as_u64(), f["height"].as_u64()) else { continue };
        let short = w.min(h) as u32;
        if short < 100 {
            continue;
        }
        by_height.entry(short).or_default().push(f);
    }
    let audio = best_audio_for_video(e);
    by_height
        .into_iter()
        .rev()
        .filter_map(|(height, formats)| {
            let picked = pick_video(&formats)?;
            Some(Quality { height, compatible: is_h264(picked), size: with_audio(picked, audio, e) })
        })
        .collect()
}

// Sizes: the formats download::yt_dlp's `-S` sort would pick. yt-dlp lists formats worst to best,
// so among equals the last one wins.

fn formats_of(e: &Value) -> impl Iterator<Item = &Value> {
    e["formats"].as_array().into_iter().flatten()
}

fn is_h264(f: &Value) -> bool {
    f["vcodec"].as_str().is_some_and(|v| v.starts_with("avc1") || v.starts_with("h264"))
}

fn is_aac(f: &Value) -> bool {
    f["acodec"].as_str().is_some_and(|a| a.starts_with("mp4a") || a.starts_with("aac"))
}

fn is_audio_only(f: &Value) -> bool {
    f["vcodec"] == "none" && f["acodec"] != "none"
}

/// `vcodec:h264,acodec:aac`: H.264 first, then a format that already has AAC sound.
fn pick_video<'a>(formats: &[&'a Value]) -> Option<&'a Value> {
    formats.iter().copied().max_by_key(|f| (is_h264(f), is_aac(f)))
}

fn best_audio_for_video(e: &Value) -> Option<&Value> {
    formats_of(e).filter(|f| is_audio_only(f)).max_by_key(|f| is_aac(f))
}

/// A video-only format needs the separate audio too. A missing acodec means it's unknown, usually
/// because the sound is already inside.
fn with_audio(video: &Value, audio: Option<&Value>, e: &Value) -> Option<u64> {
    let duration = e["duration"].as_f64();
    let extra = if video["acodec"] == "none" { audio.and_then(|a| format_size(a, duration)) } else { None };
    Some(format_size(video, duration)? + extra.unwrap_or(0))
}

/// Size of one format as the site states it, else estimated from its bitrate (kbit/s).
fn format_size(f: &Value, duration: Option<f64>) -> Option<u64> {
    f["filesize"]
        .as_f64()
        .or_else(|| f["filesize_approx"].as_f64())
        .or_else(|| Some(f["tbr"].as_f64()? * 125.0 * duration?))
        .map(|bytes| bytes as u64)
        .filter(|bytes| *bytes > 0)
}

/// For a video without a quality menu (`-f bv*+ba/b`).
fn video_size(e: &Value) -> Option<u64> {
    let videos: Vec<&Value> = formats_of(e).filter(|f| f["vcodec"] != "none").collect();
    match pick_video(&videos) {
        Some(video) => with_audio(video, best_audio_for_video(e), e),
        None => format_size(e, e["duration"].as_f64()),
    }
}

/// For the audio row (`-f ba/b`). This is what gets downloaded; the saved mp3 can differ.
pub(crate) fn audio_size(e: &Value) -> Option<u64> {
    let audio = formats_of(e).filter(|f| is_audio_only(f)).last().or_else(|| formats_of(e).last());
    format_size(audio.unwrap_or(e), e["duration"].as_f64())
}

/// The file type shown for an item. Videos are always saved as mp4 (see media::make_playable).
fn format_for(kind: Kind, ext: &str) -> Option<String> {
    match (kind, ext) {
        (Kind::Video, _) => Some("mp4".into()),
        (_, "") => None,
        (_, "jpeg") => Some("jpg".into()),
        (_, ext) => Some(ext.to_lowercase()),
    }
}

/// (has video, has audio) for a yt-dlp info dict.
fn stream_kinds(e: &Value) -> (bool, bool) {
    if e["_type"] == "url" {
        // Unresolved playlist entry: assume a normal video unless it's a known audio site.
        let ie = e["ie_key"].as_str().unwrap_or_default();
        let audio_site = ["Soundcloud", "Bandcamp", "Mixcloud", "Audiomack"]
            .iter()
            .any(|s| ie.starts_with(s));
        return (!audio_site, true);
    }
    // Codec fields are often missing (e.g. HLS); fall back to dimensions or extension.
    // The generic extractor also "succeeds" on direct image links, which we handle elsewhere.
    let ext_kind = |f: &Value| kind_from_ext(f["ext"].as_str().unwrap_or_default());
    let is_video = |f: &Value| match f["vcodec"].as_str() {
        _ if ext_kind(f) == Some(Kind::Image) => false,
        Some("none") => false,
        Some(_) => true,
        None => f["height"].is_u64() || ext_kind(f) == Some(Kind::Video),
    };
    let is_audio = |f: &Value| f["acodec"].as_str() != Some("none") && ext_kind(f) != Some(Kind::Image);
    match e["formats"].as_array() {
        Some(formats) if !formats.is_empty() => {
            (formats.iter().any(is_video), formats.iter().any(is_audio))
        }
        _ => (is_video(e), is_audio(e)),
    }
}

fn title_of(e: &Value) -> Option<String> {
    let names: Vec<&str> = ["uploader", "channel", "creator"].iter().filter_map(|k| e[k].as_str()).collect();
    ["title", "fulltitle", "id"]
        .iter()
        .find_map(|k| e[k].as_str())
        .map(|s| clean_title(s, &names))
        .filter(|s| !s.is_empty())
}

static STAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[\d.,]+\s*[kmb]?\s+(views?|reactions?|likes?|comments?|shares?|plays?|followers?|subscribers?|reposts?)$")
        .unwrap()
});
const SITE_NAMES: &[&str] = &["facebook", "instagram", "tiktok", "youtube", "x", "twitter", "threads", "reddit", "vimeo"];

/// Strips the noise sites wrap around a post's caption, e.g.
/// "393K views · 16K reactions | Hug me like this when we meet. | 𝗠𝗜𝗟𝗞𝗦𝗛𝗔𝗞𝗘"
/// becomes "Hug me like this when we meet.". `names` are the uploader/channel names, which are
/// dropped when they appear as their own segment.
pub fn clean_title(raw: &str, names: &[&str]) -> String {
    let names: Vec<String> = names.iter().map(|n| plain(n)).collect();
    let segments: Vec<&str> = raw
        .split(" | ")
        .map(str::trim)
        .filter(|seg| !seg.is_empty())
        // "393K views · 16K reactions"
        .filter(|seg| !seg.split(['·', '•', ',']).all(|part| STAT.is_match(part.trim())))
        // "𝗠𝗜𝗟𝗞𝗦𝗛𝗔𝗞𝗘", "Facebook"
        .filter(|seg| {
            let p = plain(seg);
            !names.contains(&p) && !SITE_NAMES.contains(&p.as_str())
        })
        .collect();
    match segments.as_slice() {
        [] => raw.trim().to_string(),
        [one] => one.to_string(),
        // Still several parts: the caption is usually the longest.
        many => many.iter().max_by_key(|s| s.chars().count()).unwrap().to_string(),
    }
}

/// Lower-case ASCII form for comparing names, folding the "fancy" bold/italic/script letters
/// (Unicode Mathematical Alphanumeric Symbols) people use in page names: 𝗠𝗜𝗟𝗞 → milk.
fn plain(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c as u32 {
            // 13 letter styles of 52 letters each (A-Z, a-z), starting at 𝐀.
            cp @ 0x1D400..=0x1D6A3 => {
                let i = (cp - 0x1D400) % 52;
                char::from(if i < 26 { b'a' + i as u8 } else { b'a' + (i - 26) as u8 })
            }
            // 5 digit styles of 10 digits each.
            cp @ 0x1D7CE..=0x1D7FF => char::from(b'0' + ((cp - 0x1D7CE) % 10) as u8),
            _ => c.to_ascii_lowercase(),
        })
        .collect()
}

pub(crate) fn fmt_duration(secs: f64) -> String {
    let s = secs.round() as u64;
    match s / 3600 {
        0 => format!("{}:{:02}", s / 60, s % 60),
        h => format!("{h}:{:02}:{:02}", s / 60 % 60, s % 60),
    }
}

// ---------- plain HTML / direct link ----------

static META_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<meta\s[^>]*>").unwrap());
static MEDIA_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<(img|video|audio|source)\s[^>]*>").unwrap());
static ATTR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)([a-zA-Z:_-]+)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap()
});
static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());

async fn probe_html(url: &str) -> Result<ProbeResult> {
    let resp = client()
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(err)?;
    let page = resp.url().clone();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();

    // A direct link to a media file. Some servers label every file as generic binary data;
    // then the extension in the link decides.
    let direct_kind = match content_type.split('/').next() {
        Some("image") => Some(Kind::Image),
        Some("video") => Some(Kind::Video),
        Some("audio") => Some(Kind::Audio),
        _ if content_type.is_empty() || content_type.contains("octet-stream") => kind_from_ext(&ext_of(page.as_str())),
        _ => None,
    };
    if let Some(kind) = direct_kind {
        let name = file_name_of(page.as_str());
        return Ok(ProbeResult {
            title: name.clone(),
            items: vec![MediaItem {
                kind,
                title: name,
                format: crate::download::ext_for_mime(&content_type)
                    .map(String::from)
                    .or_else(|| format_for(kind, &ext_of(page.as_str()))),
                detail: None,
                thumbnail: (kind == Kind::Image).then(|| page.to_string()),
                qualities: Vec::new(),
                size: resp.content_length().filter(|bytes| *bytes > 0),
                source: Source::Direct { url: page.to_string(), referer: None },
            }],
        });
    }
    if !content_type.contains("html") {
        return Err(format!("Unsupported content type: {content_type}"));
    }

    let html = resp.text().await.map_err(err)?;
    let mut found: Vec<(Kind, String)> = Vec::new();

    for tag in META_TAG.find_iter(&html) {
        let attrs = attrs_of(tag.as_str());
        let key = attr(&attrs, "property").or_else(|| attr(&attrs, "name")).unwrap_or_default();
        let Some(content) = attr(&attrs, "content") else { continue };
        let kind = match key.to_lowercase().as_str() {
            "og:image" | "og:image:url" | "og:image:secure_url" | "twitter:image" => Kind::Image,
            "og:video" | "og:video:url" | "og:video:secure_url" | "twitter:player:stream" => Kind::Video,
            "og:audio" | "og:audio:url" | "og:audio:secure_url" => Kind::Audio,
            _ => continue,
        };
        found.push((kind, content));
    }

    for cap in MEDIA_TAG.captures_iter(&html) {
        let tag = cap[1].to_lowercase();
        let attrs = attrs_of(&cap[0]);
        let Some(src) = media_src(&attrs) else { continue };
        let kind = match tag.as_str() {
            "img" => Kind::Image,
            "video" => Kind::Video,
            "audio" => Kind::Audio,
            _ => match attr(&attrs, "type") {
                Some(t) if t.starts_with("audio/") => Kind::Audio,
                Some(t) if t.starts_with("video/") => Kind::Video,
                _ => kind_from_ext(&ext_of(&src)).unwrap_or(Kind::Video),
            },
        };
        found.push((kind, src));
    }

    let mut seen = HashSet::new();
    let items = found
        .into_iter()
        .filter_map(|(kind, src)| {
            let src = decode_entities(&src);
            if src.starts_with("data:") || src.ends_with(".svg") || src.ends_with(".ico") {
                return None;
            }
            let abs = page.join(&src).ok()?.to_string();
            seen.insert(abs.clone()).then_some((kind, abs))
        })
        .take(MAX_ITEMS)
        .map(|(kind, abs)| MediaItem {
            kind,
            title: file_name_of(&abs),
            format: format_for(kind, &ext_of(&abs)),
            detail: None,
            thumbnail: (kind == Kind::Image).then(|| abs.clone()),
            qualities: Vec::new(),
            size: None,
            source: Source::Direct { url: abs, referer: Some(page.to_string()) },
        })
        .collect();

    let title = TITLE
        .captures(&html)
        .map(|c| clean_title(&decode_entities(c[1].trim()), &[]))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| page.host_str().unwrap_or(url).to_string());
    Ok(ProbeResult { title, items })
}

fn attrs_of(tag: &str) -> Vec<(String, String)> {
    ATTR.captures_iter(tag)
        .map(|c| {
            let value = c.get(2).or_else(|| c.get(3)).map_or("", |m| m.as_str());
            (c[1].to_lowercase(), value.to_string())
        })
        .collect()
}

/// The element's real media URL. Lazy-loading pages put it in a data-* attribute and a
/// placeholder (often a data: URI) in src.
fn media_src(attrs: &[(String, String)]) -> Option<String> {
    ["data-src", "data-lazy-src", "data-original", "src"]
        .iter()
        .filter_map(|name| attr(attrs, name))
        .find(|v| !v.trim_start().starts_with("data:"))
}

fn attr(attrs: &[(String, String)], name: &str) -> Option<String> {
    attrs.iter().find(|(k, v)| k == name && !v.is_empty()).map(|(_, v)| v.clone())
}

static NUMERIC_ENTITY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"&#(x[0-9a-fA-F]+|[0-9]+);").unwrap());

/// Decodes HTML entities: numeric ones (`&#x16981;`, `&#39;`, which some sites use for every
/// non-ASCII character) and the common named ones.
pub(crate) fn decode_entities(s: &str) -> String {
    let numeric = NUMERIC_ENTITY.replace_all(s, |c: &regex::Captures| {
        let code = &c[1];
        let n = match code.strip_prefix('x') {
            Some(hex) => u32::from_str_radix(hex, 16).ok(),
            None => code.parse().ok(),
        };
        n.and_then(char::from_u32).map(String::from).unwrap_or_else(|| c[0].to_string())
    });
    numeric
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&") // last, so "&amp;lt;" stays "&lt;"
}

pub fn kind_from_ext(ext: &str) -> Option<Kind> {
    let ext = ext.to_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        Some(Kind::Image)
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some(Kind::Video)
    } else if AUDIO_EXTS.contains(&ext.as_str()) {
        Some(Kind::Audio)
    } else {
        None
    }
}

/// Last path segment of a URL, without query string.
pub fn file_name_of(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.trim_end_matches('/')
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty() && !s.contains(':'))
        .unwrap_or("download")
        .to_string()
}

pub fn ext_of(url: &str) -> String {
    file_name_of(url)
        .rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lose_stats_and_page_names() {
        let fb = "393K views · 16K reactions | Hug me like this when we meet. | 𝗠𝗜𝗟𝗞𝗦𝗛𝗔𝗞𝗘";
        assert_eq!(clean_title(fb, &[]), "Hug me like this when we meet.");
        assert_eq!(clean_title(fb, &["Milkshake"]), "Hug me like this when we meet.");
        assert_eq!(clean_title("1.2M views | A short clip | Facebook", &[]), "A short clip");
        assert_eq!(clean_title("Video by timvatruc", &[]), "Video by timvatruc");
        assert_eq!(clean_title("Rick Astley - Never Gonna Give You Up", &["Rick Astley"]), "Rick Astley - Never Gonna Give You Up");
        assert_eq!(plain("𝗠𝗜𝗟𝗞𝗦𝗛𝗔𝗞𝗘 𝟮𝟬𝟮𝟲"), "milkshake 2026");
    }

    #[test]
    fn sizes_match_the_formats_a_download_picks() {
        let e: Value = serde_json::from_str(
            r#"{"duration":10,"formats":[
                {"format_id":"sb","vcodec":"none","acodec":"none","format_note":"storyboard","width":48,"height":27},
                {"format_id":"a-aac","vcodec":"none","acodec":"mp4a.40.2","filesize":100},
                {"format_id":"a-opus","vcodec":"none","acodec":"opus","filesize":150},
                {"format_id":"combined-360","vcodec":"avc1.42","acodec":"mp4a.40.2","width":640,"height":360,"filesize_approx":700},
                {"format_id":"h264-360","vcodec":"avc1.4d","acodec":"none","width":640,"height":360,"filesize":500},
                {"format_id":"vp9-720","vcodec":"vp9","acodec":"none","width":1280,"height":720,"filesize":2000},
                {"format_id":"h264-720","vcodec":"avc1.64","acodec":"none","width":1280,"height":720,"filesize":3000},
                {"format_id":"hls-1080","vcodec":"avc1.64","acodec":"none","width":1920,"height":1080,"tbr":800},
                {"format_id":"av1-2160","vcodec":"av01","acodec":"none","width":3840,"height":2160},
                {"format_id":"vertical","vcodec":"vp9","acodec":"none","width":480,"height":854,"filesize":900}
            ]}"#,
        )
        .unwrap();
        let q = |height, compatible, size| Quality { height, compatible, size };
        assert_eq!(
            qualities(&e),
            [
                q(2160, false, None),       // the site gives no size or bitrate
                q(1080, true, Some(1_000_100)), // 800 kbit/s for 10 s, plus the AAC audio
                q(720, true, Some(3100)),   // H.264 even though VP9 is smaller
                q(480, false, Some(1000)),  // the short side of a vertical video
                q(360, true, Some(700)),    // already has sound: no separate audio
            ]
        );
        assert_eq!(audio_size(&e), Some(150), "ba: the best audio, not the AAC");
        assert_eq!(video_size(&serde_json::json!({"duration":2,"tbr":8})), Some(2000), "no format list");
    }

    #[test]
    fn login_walls_get_a_helpful_message() {
        let ig = "https://www.instagram.com/p/DfL9wRmhxaC/";
        assert!(is_login_wall("HTTP redirect to login page (https://www.instagram.com/accounts/login/)"));
        assert!(!is_login_wall("[Instagram] DfL9wRmhxaC: There is no video in this post"));
        assert!(login_message(ig).starts_with("Instagram only shows this to people who are logged in"));
    }

    #[test]
    fn decodes_numeric_and_named_entities() {
        assert_eq!(decode_entities("&#x1d400;&#x1d40f; &#39;hi&#39; &amp;lt; caf&#233;"), "𝐀𝐏 'hi' &lt; café");
    }

    #[test]
    fn lazy_images_use_data_src() {
        let lazy = attrs_of(r#"<img src="data:image/gif;base64,R0lGOD" data-src="/photos/1.jpg">"#);
        assert_eq!(media_src(&lazy).as_deref(), Some("/photos/1.jpg"));
        let plain = attrs_of(r#"<img src="/a.png" alt="x">"#);
        assert_eq!(media_src(&plain).as_deref(), Some("/a.png"));
        let placeholder_only = attrs_of(r#"<img src="data:image/gif;base64,R0lGOD">"#);
        assert_eq!(media_src(&placeholder_only), None);
    }
}
