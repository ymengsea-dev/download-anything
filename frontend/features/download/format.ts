import type { Quality } from "../../shared/api";

export const formatSize = (bytes: number) => {
  const mb = bytes / 1e6;
  if (mb < 1) return `${Math.max(Math.round(bytes / 1e3), 1)} KB`;
  if (mb < 1000) return `${mb < 100 ? mb.toFixed(1) : Math.round(mb)} MB`;
  return `${(mb / 1000).toFixed(1)} GB`;
};

export const qualityLabel = (q: Quality) =>
  `${q.height === 2160 ? "4K" : q.height === 1440 ? "2K" : `${q.height}p`}` +
  `${q.size ? ` · ~${formatSize(q.size)}` : ""}${q.compatible ? "" : " · slower"}`;
