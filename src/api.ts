// Typed wrappers around the Rust commands in src-tauri/src/lib.rs.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export type Kind = "video" | "image" | "audio";

export type Source =
  | { via: "ytDlp"; url: string; playlistIndex: number | null; audioOnly: boolean; maxHeight?: number | null }
  | { via: "direct"; url: string; referer: string | null };

export interface Quality {
  /** Short side in pixels: 1080 for both 1920x1080 and 1080x1920. */
  height: number;
  /** Offered as H.264, so no conversion is needed. */
  compatible: boolean;
}

export interface MediaItem {
  kind: Kind;
  title: string;
  /** File type of the saved file: "mp4", "mp3", "jpg"... */
  format: string | null;
  /** Extra info such as the duration. */
  detail: string | null;
  thumbnail: string | null;
  /** Video resolutions on offer, best first. */
  qualities: Quality[];
  source: Source;
}

export interface ProbeResult {
  title: string;
  items: MediaItem[];
}

export interface Engine {
  ytDlp: string;
  ffmpeg: string;
}

export type Tool = "yt-dlp" | "ffmpeg";

export const setupEngine = () => invoke<Engine>("setup_engine");

export interface ToolStatus {
  tool: Tool;
  path: string;
  version: string | null;
  /** A yt-dlp installed with "Update yt-dlp" instead of the copy bundled with the app. */
  updated: boolean;
}

export const engineStatus = () => invoke<ToolStatus[]>("engine_status");

export type UpdateOutcome = { result: "alreadyLatest" } | { result: "updated"; path: string };

/** Fetches the latest official yt-dlp, verified against its SHA-256 list. Only on request. */
export const updateYtDlp = () => invoke<UpdateOutcome>("update_yt_dlp");

/** Download progress of updateYtDlp: 0-100, or -1 when unknown. */
export const onUpdateProgress = (cb: (percent: number) => void) =>
  listen<number>("update-progress", (e) => cb(e.payload));

export const defaultOutDir = () => invoke<string>("default_out_dir");

export const probeUrl = (jobId: string, url: string) => invoke<ProbeResult>("probe_url", { jobId, url });

export const downloadItem = (jobId: string, item: MediaItem, outDir: string) =>
  invoke<string>("download_item", { jobId, item, outDir });

export const renderSlideshow = (
  jobId: string,
  images: MediaItem[],
  audio: MediaItem,
  title: string,
  outDir: string,
) => invoke<string>("render_slideshow", { jobId, images, audio, title, outDir });

export const cancelJob = (jobId: string) => invoke<void>("cancel_job", { jobId });

/** One overall percent (0-100, only moves forward), or -1 when it can't be measured. */
export const onJobProgress = (cb: (jobId: string, percent: number) => void) =>
  listen<{ jobId: string; percent: number }>("job-progress", (e) => cb(e.payload.jobId, e.payload.percent));
