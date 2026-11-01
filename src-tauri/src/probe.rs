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
    pub source: Source,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quality {
    /// The short side in pixels ("1080" for 1920x1080 and 1080x1920), like yt-dlp's `res`.
    pub height: u32,
    /// Available as H.264, so it downloads without converting.
    pub compatible: bool,
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

    match probe_yt_dlp(engine, url, cancel).await {
        Ok(found) if !found.items.is_empty() => return Ok(found),
        Ok(_) => {}
        Err(e) => first_error = Some(e),
    }
    if is_cancelled() {
        return Err("Cancelled".into());
    }

    // Photo posts (Instagram, Facebook): yt-dlp reads them (without a login, for public posts)
    // but only downloads video. Take the photos from the data it fetched.
    if let Ok(Some(found)) = crate::photos::instagram(engine, url, cancel, ENGINE_TIMEOUT).await {
        return Ok(found);
    }
    if let Ok(Some(found)) = crate::photos::facebook(engine, url, cancel, ENGINE_TIMEOUT).await {
        return Ok(found);
    }
    if is_cancelled() {
        return Err("Cancelled".into());
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

async fn probe_yt_dlp(engine: &Engine, url: &str, cancel: &Cancel) -> Result<ProbeResult> {
    let mut args: Vec<String> =
        ["-J", "--no-warnings", "--flat-playlist", "--no-playlist", "--playlist-items", "1:50"].map(String::from).into();
    args.push(url.into());
    let out = proc::run_with_timeout(&engine.yt_dlp, &args, Some(cancel), Some(ENGINE_TIMEOUT), |_| {}).await?;
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

    let source = |audio_only| Source::YtDlp { url: url.into(), playlist_index, audio_only, max_height: None };
    if has_video {
        items.push(MediaItem {
            kind: Kind::Video,
            title: title.clone(),
            format: Some("mp4".into()),
            detail: duration.clone(),
            thumbnail: thumbnail.clone(),
            qualities: qualities(e),
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
                source: Source::Direct { url: thumb, referer: Some(url.into()) },
            });
        }
    }
}

/// Distinct video resolutions, best first, noting which exist as H.264.
fn qualities(e: &Value) -> Vec<Quality> {
    let mut by_height = std::collections::BTreeMap::<u32, bool>::new();
    for f in e["formats"].as_array().into_iter().flatten() {
        let vcodec = f["vcodec"].as_str().unwrap_or_default();
        if vcodec == "none" || f["format_note"] == "storyboard" {
            continue;
        }
        let (Some(w), Some(h)) = (f["width"].as_u64(), f["height"].as_u64()) else { continue };
        let short = w.min(h) as u32;
        if short < 100 {
            continue;
        }
        let h264 = vcodec.starts_with("avc1") || vcodec.starts_with("h264");
        *by_height.entry(short).or_default() |= h264;
    }
    by_height
        .into_iter()
        .rev()
        .map(|(height, compatible)| Quality { height, compatible })
        .collect()
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
