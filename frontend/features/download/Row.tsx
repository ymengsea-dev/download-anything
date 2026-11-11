import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { Quality } from "../../shared/api";
import { qualityLabel } from "./format";
import type { Job } from "./useDownloads";

interface RowProps {
  thumb: string | null;
  title: string;
  format: string | null;
  detail: string | null;
  size?: string;
  qualities?: Quality[];
  quality?: number;
  onQuality?: (height: number) => void;
  action: string;
  job?: Job;
  onStart: () => void;
  onCancel: () => void;
}

export function Row({ thumb, title, format, detail, size, qualities = [], quality, onQuality, action, job, onStart, onCancel }: RowProps) {
  const running = job?.status === "running";
  // The quality menu takes the whole line, so a video's length sits on its thumbnail instead.
  const badge = qualities.length > 0 ? detail : null;
  return (
    <div className="row">
      <div className="thumb">
        {thumb && (
          <img
            src={thumb}
            alt=""
            loading="lazy"
            referrerPolicy="no-referrer"
            onError={(e) => (e.currentTarget.style.display = "none")}
          />
        )}
        {badge && <span className="thumb-badge">{badge}</span>}
      </div>
      <div className="info">
        <div className="title" title={title}>
          {title}
        </div>
        {job?.status === "running" ? (
          <div className="progress">
            <div className={`bar ${job.percent < 0 ? "indeterminate" : ""}`}>
              <div style={{ width: `${Math.max(job.percent, 0)}%` }} />
            </div>
            <span className="progress-label">
              Downloading{job.percent >= 0 && ` ${Math.round(job.percent)}%`}
            </span>
          </div>
        ) : job?.status === "error" ? (
          <div className="detail error" title={job.error}>{job.error}</div>
        ) : (
          <div className="meta">
            {format && <span className="format-chip">{format}</span>}
            {qualities.length > 0 && (
              <select
                className="quality"
                value={quality}
                disabled={running}
                onChange={(e) => onQuality?.(Number(e.target.value))}
                title="Resolution"
              >
                {qualities.map((q) => (
                  <option key={q.height} value={q.height}>
                    {qualityLabel(q)}
                  </option>
                ))}
              </select>
            )}
            {detail && !badge && <span className="detail">{detail}</span>}
            {size && <span className="detail">{size}</span>}
          </div>
        )}
      </div>
      {running ? (
        <button className="secondary" onClick={onCancel}>Cancel</button>
      ) : job?.status === "done" ? (
        <button className="secondary" onClick={() => revealItemInDir(job.path)}>Show</button>
      ) : (
        <button onClick={onStart}>{job?.status === "error" ? "Retry" : action}</button>
      )}
    </div>
  );
}
