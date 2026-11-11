import type { Tool, ToolStatus } from "../../shared/api";

const TOOLS: Record<Tool, { name: string; purpose: string }> = {
  "yt-dlp": { name: "yt-dlp", purpose: "Videos, audio and photos from 1,800+ sites" },
  ffmpeg: { name: "FFmpeg", purpose: "Joins, converts and renders video" },
};

/** "Update yt-dlp": idle → downloading (verified before use) → a result line. */
export type Update = { step: "running"; percent: number } | { step: "done"; text: string } | { step: "error"; error: string };

interface EngineRowProps {
  status: ToolStatus;
  update?: Update | null;
  onUpdate?: () => void;
}

export function EngineRow({ status, update, onUpdate }: EngineRowProps) {
  const info = TOOLS[status.tool];

  let line: React.ReactNode;
  if (update?.step === "running") {
    line = `Downloading${update.percent >= 0 ? ` ${Math.round(update.percent)}%` : ""}…`;
  } else if (update?.step === "error") {
    line = <span className="error" title={update.error}>{update.error}</span>;
  } else {
    line = (
      <>
        <span className="ok">{status.updated ? "Updated" : "Built in"}</span>
        {status.version && ` · ${status.version}`}
        {update?.step === "done" && ` · ${update.text}`}
      </>
    );
  }

  return (
    <div className="row engine" title={status.path}>
      <div className="info">
        <div className="title">{info.name}</div>
        <div className="detail">{info.purpose}</div>
        <div className="detail engine-state">{line}</div>
      </div>
      {onUpdate &&
        (update?.step === "running" ? (
          <span className="spinner" />
        ) : (
          <button className="secondary" onClick={onUpdate}>
            Update
          </button>
        ))}
    </div>
  );
}
