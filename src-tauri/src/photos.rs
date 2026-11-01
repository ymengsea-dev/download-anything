//! Photo posts that yt-dlp can read but won't download (it only handles video).
//!
//! For public posts, yt-dlp already fetches the data the site serves to logged-out visitors,
//! and that data includes the photos. We have yt-dlp save those responses (`--write-pages`)
//! and read the photo URLs from them. So public photo posts work without a login or any extra
//! tool, and keep working as yt-dlp keeps up with the sites. One small reader per site:
//! - Instagram: the post's JSON (`image_versions2` / `carousel_media`)
//! - TikTok photo mode: the page's `__UNIVERSAL_DATA_FOR_REHYDRATION__` (`imagePost`), plus the
//!   post's sound, which yt-dlp itself offers
//! - Facebook: the page's JSON blocks (`all_subattachments` for posts, the `fbid` photo otherwise)

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use crate::binaries::Engine;
use crate::probe::{clean_title, ext_of, Kind, MediaItem, ProbeResult, Source};
use crate::proc::{self, Cancel, Result};

static INSTAGRAM_POST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"instagram\.com/(?:[^/?#]+/)?(?:p|reels?|tv)/([A-Za-z0-9_-]+)").unwrap());
static TIKTOK_PHOTO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(https?://(?:www\.|m\.)?tiktok\.com/@[^/?#]+)/photo/(\d+)").unwrap());
static FACEBOOK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://(?:[a-z0-9-]+\.)?(?:facebook\.com|fb\.com|fb\.watch)/").unwrap());
static FBID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[?&]fbid=(\d+)").unwrap());
static REHYDRATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?s)<script id="__UNIVERSAL_DATA_FOR_REHYDRATION__"[^>]*>(.*?)</script>"#).unwrap()
});
static JSON_SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?s)<script type="application/json"[^>]*>(.*?)</script>"#).unwrap());
static OG_TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<meta property="og:title" content="([^"]*)""#).unwrap());

/// Runs yt-dlp on `url` with `--write-pages` (plus `extra_args`) in a temporary folder and
/// returns its stdout and the text of every page it saved. A failing exit (e.g. "There is no
/// video in this post") is expected; the saved pages are what we want.
async fn saved_pages(
    engine: &Engine,
    url: &str,
    extra_args: &[&str],
    cancel: &Cancel,
    timeout: std::time::Duration,
) -> Result<(String, Vec<String>)> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let dir = std::env::temp_dir().join(format!("download-anything-pages-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(proc::err)?;

    let mut cmd = proc::command(&engine.yt_dlp);
    cmd.current_dir(&dir).args(["--write-pages", "--no-warnings", "--no-playlist"]).args(extra_args).arg(url);
    let run = proc::run_command(cmd, Some(cancel), Some(timeout), |_| {}).await;
    let pages = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "dump"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    Ok((run?.stdout, pages))
}

fn photo_item(title: String, url: String, size: (u64, u64), referer: &str) -> MediaItem {
    MediaItem {
        kind: Kind::Image,
        title,
        format: Some(match ext_of(&url).as_str() {
            "" | "jpeg" => "jpg".into(),
            ext => ext.into(),
        }),
        detail: (size.0 > 0).then(|| format!("{}×{}", size.0, size.1)),
        thumbnail: Some(url.clone()),
        qualities: Vec::new(),
        source: Source::Direct { url, referer: Some(referer.into()) },
    }
}

/// "Title (2/5)" when a post has several photos.
fn numbered(title: &str, i: usize, count: usize) -> String {
    if count > 1 { format!("{title} ({}/{count})", i + 1) } else { title.to_string() }
}

// ---------- Instagram ----------

/// Photos of a public Instagram post, or `None` if the link isn't an Instagram post or
/// nothing was found.
pub async fn instagram(engine: &Engine, url: &str, cancel: &Cancel, timeout: std::time::Duration) -> Result<Option<ProbeResult>> {
    let Some(code) = INSTAGRAM_POST.captures(url).map(|c| c[1].to_string()) else {
        return Ok(None);
    };
    let (_, pages) = saved_pages(engine, url, &["--skip-download"], cancel, timeout).await?;
    let pages: Vec<Value> = pages.iter().filter_map(|text| serde_json::from_str(text).ok()).collect();
    Ok(pages.iter().find_map(|page| find_post(page, &code)).and_then(|post| {
        let (title, items) = post_items(post);
        (!items.is_empty()).then(|| ProbeResult { title, items })
    }))
}

/// The post object with this shortcode that carries images (searched depth-first, so the
/// post itself wins over its appearances in the author's timeline).
fn find_post<'a>(v: &'a Value, code: &str) -> Option<&'a Value> {
    match v {
        Value::Object(map) => {
            let has_media = map.contains_key("image_versions2") || map.contains_key("carousel_media");
            if map.get("code").and_then(Value::as_str) == Some(code) && has_media {
                return Some(v);
            }
            map.values().find_map(|child| find_post(child, code))
        }
        Value::Array(list) => list.iter().find_map(|child| find_post(child, code)),
        _ => None,
    }
}

/// The post's title and one item per photo.
fn post_items(post: &Value) -> (String, Vec<MediaItem>) {
    let photos: Vec<(String, u64, u64)> = match post["carousel_media"].as_array().filter(|c| !c.is_empty()) {
        // Carousel: one entry per slide; video slides are left to yt-dlp.
        Some(slides) => slides.iter().filter(|s| !has_video(s)).filter_map(best_image).collect(),
        None if !has_video(post) => best_image(post).into_iter().collect(),
        None => Vec::new(),
    };

    let caption = post["caption"]["text"].as_str().map(|t| clean_title(t.lines().next().unwrap_or(t), &[]));
    let title = caption
        .filter(|t| !t.is_empty())
        .or_else(|| post["user"]["username"].as_str().map(|u| format!("Photo by {u}")))
        .unwrap_or_else(|| "Instagram photo".into());
    let count = photos.len();

    let items = photos
        .into_iter()
        .enumerate()
        .map(|(i, (url, w, h))| photo_item(numbered(&title, i, count), url, (w, h), "https://www.instagram.com/"))
        .collect();
    (title, items)
}

fn has_video(media: &Value) -> bool {
    media["video_versions"].as_array().is_some_and(|v| !v.is_empty())
}

/// The full-size image: (url, width, height).
///
/// Candidates come largest first. The logged-out data often leaves out their sizes (they're
/// encoded in the URL instead), so pick the widest when sizes are given and the first otherwise;
/// never "max of all zeros", which would be the 150px thumbnail at the end. `display_uri` is a
/// reduced copy (480px), used only as a last resort.
fn best_image(media: &Value) -> Option<(String, u64, u64)> {
    let candidates: Vec<(String, u64, u64)> = media["image_versions2"]["candidates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| Some((c["url"].as_str()?.to_string(), c["width"].as_u64().unwrap_or(0), c["height"].as_u64().unwrap_or(0))))
        .collect();
    let original = (media["original_width"].as_u64().unwrap_or(0), media["original_height"].as_u64().unwrap_or(0));
    let best = if candidates.iter().any(|(_, w, _)| *w > 0) {
        candidates.into_iter().max_by_key(|(_, w, _)| *w)
    } else {
        candidates.into_iter().next().map(|(url, _, _)| (url, original.0, original.1))
    };
    best.or_else(|| media["display_uri"].as_str().map(|u| (u.to_string(), 0, 0)))
}

// ---------- TikTok photo mode ----------

/// TikTok photo posts (`/@user/photo/<id>`). yt-dlp only accepts them in their `/video/<id>`
/// form (same id), where it offers the post's sound; the photos come from the page it fetched.
pub async fn tiktok(engine: &Engine, url: &str, cancel: &Cancel, timeout: std::time::Duration) -> Result<Option<ProbeResult>> {
    let Some(c) = TIKTOK_PHOTO.captures(url) else {
        return Ok(None);
    };
    let video_url = format!("{}/video/{}", &c[1], &c[2]);
    let (stdout, pages) = saved_pages(engine, &video_url, &["-J"], cancel, timeout).await?;
    let Some(post) = pages.iter().find_map(|page| tiktok_post(page)) else {
        return Ok(None);
    };
    let title = post["desc"]
        .as_str()
        .map(|d| clean_title(d, &[]))
        .filter(|t| !t.is_empty())
        .or_else(|| post["author"]["uniqueId"].as_str().map(|u| format!("Photos by @{u}")))
        .unwrap_or_else(|| "TikTok photos".into());

    let images: Vec<(String, u64, u64)> = post["imagePost"]["images"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|img| {
            let url = img["imageURL"]["urlList"].as_array()?.first()?.as_str()?.to_string();
            Some((url, img["imageWidth"].as_u64().unwrap_or(0), img["imageHeight"].as_u64().unwrap_or(0)))
        })
        .collect();
    let count = images.len();
    let mut items: Vec<MediaItem> = images
        .into_iter()
        .enumerate()
        .map(|(i, (url, w, h))| photo_item(numbered(&title, i, count), url, (w, h), "https://www.tiktok.com/"))
        .collect();

    // The sound, as yt-dlp lists it for the /video/ form (an audio-only format).
    let info: Value = serde_json::from_str(&stdout).unwrap_or(Value::Null);
    let has_sound = info["formats"].as_array().is_some_and(|f| f.iter().any(|f| f["acodec"].as_str().is_some_and(|a| a != "none")));
    if has_sound {
        let music = &post["music"];
        let duration = music["duration"].as_f64().or_else(|| info["duration"].as_f64());
        items.push(MediaItem {
            kind: Kind::Audio,
            title: music["title"].as_str().filter(|t| !t.is_empty()).map(String::from).unwrap_or_else(|| title.clone()),
            format: Some("mp3".into()),
            detail: duration.map(crate::probe::fmt_duration),
            thumbnail: music["coverLarge"].as_str().map(String::from),
            qualities: Vec::new(),
            source: Source::YtDlp { url: video_url, playlist_index: None, audio_only: true, max_height: None },
        });
    }
    Ok((count > 0).then(|| ProbeResult { title, items }))
}

/// The post from a TikTok page's rehydration data, if it's a photo post.
fn tiktok_post(page: &str) -> Option<Value> {
    let data: Value = serde_json::from_str(&REHYDRATION.captures(page)?[1]).ok()?;
    let post = data["__DEFAULT_SCOPE__"]["webapp.video-detail"]["itemInfo"]["itemStruct"].clone();
    post["imagePost"]["images"].is_array().then_some(post)
}

// ---------- Facebook ----------

/// Photos of a public Facebook post (several photos) or a single photo page (`?fbid=`).
pub async fn facebook(engine: &Engine, url: &str, cancel: &Cancel, timeout: std::time::Duration) -> Result<Option<ProbeResult>> {
    if !FACEBOOK.is_match(url) {
        return Ok(None);
    }
    let (_, pages) = saved_pages(engine, url, &["--skip-download"], cancel, timeout).await?;
    let blocks: Vec<Value> = pages
        .iter()
        .flat_map(|page| JSON_SCRIPT.captures_iter(page).filter_map(|c| serde_json::from_str(&c[1]).ok()))
        .collect();
    let photos = facebook_photos(&blocks, FBID.captures(url).map(|c| c[1].to_string()).as_deref());
    if photos.is_empty() {
        return Ok(None);
    }
    let title = pages
        .iter()
        .find_map(|page| OG_TITLE.captures(page).map(|c| clean_title(&crate::probe::decode_entities(&c[1]), &[])))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "Facebook photo".into());
    let count = photos.len();
    let items = photos
        .into_iter()
        .enumerate()
        .map(|(i, (url, w, h))| photo_item(numbered(&title, i, count), url, (w, h), "https://www.facebook.com/"))
        .collect();
    Ok(Some(ProbeResult { title, items }))
}

/// A post's photos (`all_subattachments`, in order, each once), or else the photo whose id is `fbid`.
fn facebook_photos(blocks: &[Value], fbid: Option<&str>) -> Vec<(String, u64, u64)> {
    let image = |media: &Value| -> Option<(String, u64, u64)> {
        let img = [&media["viewer_image"], &media["image"]].into_iter().find(|i| i["uri"].is_string())?;
        Some((img["uri"].as_str()?.to_string(), img["width"].as_u64().unwrap_or(0), img["height"].as_u64().unwrap_or(0)))
    };
    let mut seen = std::collections::HashSet::new();
    let mut photos = Vec::new();
    for block in blocks {
        visit(block, &mut |obj| {
            for node in obj.get("all_subattachments").and_then(|a| a["nodes"].as_array()).into_iter().flatten() {
                let media = &node["media"];
                if let (Some(id), Some(photo)) = (media["id"].as_str(), image(media)) {
                    if seen.insert(id.to_string()) {
                        photos.push(photo);
                    }
                }
            }
        });
    }
    if photos.is_empty() {
        if let Some(fbid) = fbid {
            for block in blocks {
                visit(block, &mut |obj| {
                    if photos.is_empty() && obj.get("id").and_then(Value::as_str) == Some(fbid) {
                        if let Some(photo) = image(&Value::Object(obj.clone())) {
                            photos.push(photo);
                        }
                    }
                });
            }
        }
    }
    photos
}

/// Calls `f` on every JSON object inside `v`.
fn visit(v: &Value, f: &mut impl FnMut(&serde_json::Map<String, Value>)) {
    match v {
        Value::Object(map) => {
            f(map);
            map.values().for_each(|child| visit(child, f));
        }
        Value::Array(list) => list.iter().for_each(|child| visit(child, f)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_single_photos_and_carousels() {
        let page: Value = serde_json::from_str(
            r#"{"data":{"xig_polaris_media":{"if_not_gated_logged_out":{
                "code":"ABC","media_type":8,"caption":{"text":"Sunset walk\nmore"},
                "user":{"username":"someone","timeline":[{"code":"ABC","display_uri":"https://x/timeline.jpg"}]},
                "carousel_media":[
                  {"image_versions2":{"candidates":[{"url":"https://x/a_small.jpg","width":320,"height":400},
                                                    {"url":"https://x/a_big.jpg?sig=1","width":1080,"height":1350}]}},
                  {"video_versions":[{"url":"https://x/v.mp4"}],"image_versions2":{"candidates":[{"url":"https://x/v.jpg","width":1080}]}},
                  {"image_versions2":{"candidates":[{"url":"https://x/b.webp","width":1080,"height":1080}]}}
                ]}}}}"#,
        )
        .unwrap();
        let post = find_post(&page, "ABC").unwrap();
        let (_, items) = post_items(post);
        let summary: Vec<_> = items.iter().map(|i| (i.title.as_str(), i.format.as_deref().unwrap())).collect();
        assert_eq!(summary, [("Sunset walk (1/2)", "jpg"), ("Sunset walk (2/2)", "webp")]);
        assert!(matches!(&items[0].source, Source::Direct { url, .. } if url == "https://x/a_big.jpg?sig=1"));

        // Sizes missing (as in logged-out data): the first candidate is the full-size one.
        let no_sizes: Value = serde_json::from_str(
            r#"{"original_width":949,"original_height":1266,"display_uri":"https://x/480.jpg",
                "image_versions2":{"candidates":[{"url":"https://x/full.jpg"},{"url":"https://x/640.jpg"},{"url":"https://x/150.jpg"}]}}"#,
        )
        .unwrap();
        assert_eq!(best_image(&no_sizes), Some(("https://x/full.jpg".into(), 949, 1266)));

        assert_eq!(TIKTOK_PHOTO.captures("https://www.tiktok.com/@yinzijqn/photo/7670832948664372488?is_from_webapp=1").map(|c| format!("{}/video/{}", &c[1], &c[2])).unwrap(), "https://www.tiktok.com/@yinzijqn/video/7670832948664372488");

        assert_eq!(INSTAGRAM_POST.captures("https://www.instagram.com/p/DfL9wRmhxaC/?img_index=1").unwrap()[1].to_string(), "DfL9wRmhxaC");
        assert_eq!(INSTAGRAM_POST.captures("https://instagram.com/someone/reel/Xy_z-1/").unwrap()[1].to_string(), "Xy_z-1");
    }

    #[test]
    fn reads_tiktok_and_facebook_pages() {
        let tiktok = r#"<html><script id="__UNIVERSAL_DATA_FOR_REHYDRATION__" type="application/json">{"__DEFAULT_SCOPE__":{"webapp.video-detail":{"itemInfo":{"itemStruct":{"desc":"","imagePost":{"images":[{"imageURL":{"urlList":["https://p/a.jpeg?x=1","https://p/a2.jpeg"]},"imageWidth":1080,"imageHeight":1920}]}}}}}}</script></html>"#;
        let post = tiktok_post(tiktok).unwrap();
        assert_eq!(post["imagePost"]["images"][0]["imageWidth"], 1080);
        assert!(tiktok_post("<html>no data</html>").is_none());

        let blocks: Vec<Value> = vec![serde_json::from_str(
            r#"{"x":{"all_subattachments":{"count":3,"nodes":[
                {"media":{"__typename":"Photo","id":"1","viewer_image":{"uri":"https://fb/1.jpg","width":736,"height":981}}},
                {"media":{"__typename":"Photo","id":"2","viewer_image":{"uri":"https://fb/2.jpg","width":735,"height":972}}}]}},
               "y":{"all_subattachments":{"nodes":[{"media":{"id":"1","viewer_image":{"uri":"https://fb/1.jpg"}}}]}}}"#,
        )
        .unwrap()];
        let photos = facebook_photos(&blocks, None);
        assert_eq!(photos, [("https://fb/1.jpg".into(), 736, 981), ("https://fb/2.jpg".into(), 735, 972)]);

        let single: Vec<Value> = vec![serde_json::from_str(
            r#"{"a":{"id":"999","image":{"uri":"https://fb/thumb.jpg","width":100}},"b":{"id":"123","__typename":"Photo","image":{"uri":"https://fb/full.jpg","width":1320,"height":1306}}}"#,
        )
        .unwrap()];
        assert_eq!(facebook_photos(&single, Some("123")), [("https://fb/full.jpg".into(), 1320, 1306)]);
    }
}
