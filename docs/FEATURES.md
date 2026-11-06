# What Download Anything does

Download Anything is a small desktop app for macOS, Windows and Linux. Paste a link and it finds the
videos, images and audio on that page so you can save them. When a page has images plus audio, it can
also turn them into one video.

## Using it

1. **Paste a link** to a video, an image, a song or a page into the box at the top. The app starts looking
   as soon as you paste. Clicking the box selects the whole link, so pasting replaces it.
   For a link you typed, press **Enter**. While it's looking, **Cancel** in the box stops it.
2. **Pick what to save.** Results are grouped into **Video**, **Audio** and **Images** sections. Each row shows a
   preview, the file type it will be saved as (MP4, MP3, JPG…), the length, the download size when the site
   gives enough to tell, and, for videos, a quality menu.
3. **Press Download** on a row. One progress bar shows how far along it is. **Cancel** stops it.
4. **Press Show** when it finishes to open the folder with the file selected.

## What you get

| You paste | You get |
|---|---|
| A video page (YouTube, X/Twitter, Instagram, TikTok, Vimeo, Reddit and 1,800+ more) | The video as **mp4** (H.264 + AAC, so it plays in QuickTime, Windows and browsers), plus the audio alone as **mp3** |
| A music or podcast page (SoundCloud, Bandcamp…) | The audio as **mp3**, plus the cover art as an image |
| A playlist or a post with several videos | One row per video (up to 50) |
| A public Instagram photo post or carousel | Every photo at full size, with no login and nothing extra to install |
| A TikTok photo post | Every photo plus the post's sound as **mp3**, so you can also **Make a video** from them |
| A public Facebook post with photos, or a single Facebook photo | The photos, full size (posts: the ones Facebook shows without logging in) |
| A page with images | The images on that page, in their original format |
| Any other web page | The images, videos and audio files used on that page |
| A direct link to a file (`.jpg`, `.mp4`, `.mp3`…) | That file |

**Choosing a resolution.** Each video has a quality menu listing the resolutions the site offers, with the
highest selected, and the estimated download size next to each one (`~` marks an estimate). Every video is saved as H.264 + AAC mp4, which plays in QuickTime, on Windows and in browsers.
Sites often offer their sharpest versions only in newer formats (VP9, AV1) that QuickTime can't open. Those
qualities are marked **slower**: the app downloads them and then converts them. That's all part of the one
"Downloading" bar. On a Mac the conversion uses the built-in hardware encoder, so it's quick.
Pick a quality without "slower" to skip the conversion. The size is what gets downloaded, so a converted video
or an mp3 can end up a different size on disk.

## Turning images + audio into a video

Some posts are photo slideshows with music, for example TikTok or Instagram photo posts. For a song page,
the cover art and the track work the same way. When the app finds **images and audio but no video**,
it shows a **Make a video** row at the top. Press **Render** and the app:

- downloads all the images and the audio,
- shows each image for an equal share of the audio's length,
- fits every image into the same frame, adding black bars where needed. The frame matches the first
  image's shape, up to 1920 px on its longest side,
- saves the result as `<title> (slideshow).mp4`, which plays almost anywhere.

## Where files go

Files are saved to **`Downloads/Download Anything`**. To pick another folder, open **Settings** (the gear in the
top-right corner) and press **Change** under *Save to*. The app remembers your choice. Clicking the folder name
opens it.

If a file with the same name already exists, the app adds a number, for example `photo (2).jpg`,
instead of overwriting it.

## Built-in download engines

Everything the app needs ships inside it, so there's nothing to install and the app never downloads extra
programs on its own. On a Mac, the first launch takes a few seconds longer ("Starting…") while the app unpacks
yt-dlp into its own data folder, which keeps every later fetch fast:

| Tool | What it's for |
|---|---|
| **yt-dlp** | Videos, audio and photos from most sites |
| **FFmpeg** | Joining video and audio, making mp3s, converting, rendering slideshows |

**Settings → Download engines** shows their versions. Websites change often, and when one stops working, press
**Update** next to yt-dlp. The app then downloads the latest official yt-dlp and checks it against yt-dlp's
published checksums before using it. Nothing is downloaded unless you press it.

## Private posts

The app only downloads what anyone can see without logging in. It never uses your accounts, your browser's
saved logins, or anything else on your computer. The only places it writes are the folder you choose under
**Save to** and its own private settings folder. Private accounts and posts that need a login can't be downloaded.

## What isn't possible without logging in

The app only uses what anyone can see without an account, so a few things can't be downloaded:

- **Music on Instagram photo posts.** Instagram only sends a photo's music to its phone app and to logged-in
  users. That's also why the photo is silent in a browser on a Mac. You get the photo.
- **TikTok sound pages** (`tiktok.com/music/…`). TikTok loads these through its private app interface. Paste the
  link of a video that uses the sound instead, and you'll get the sound as **mp3**.
- **Photos Facebook hides behind "+ more"** in a post, when it doesn't show them to logged-out visitors.

## When something goes wrong

| Message | What to do |
|---|---|
| "Setting up the download engine…" doesn't finish | Check your internet connection, then press **Retry** |
| A site error such as "Video unavailable" or "Private video" | The media is private, removed, region-locked or needs a login. The app can't get it |
| "No downloadable media found on this page" | The page has nothing the app can recognise, or loads its media in a way it can't see |
| "… only shows this to people who are logged in" | The post is private or needs an account, so the app can't get it |
| A row shows an error after Download | Press **Retry**. If a site recently changed, restarting the app lets yt-dlp update itself |

## Please use it responsibly

Only download media you have the right to save. Many sites' terms of service limit downloading,
and most content is protected by copyright.
