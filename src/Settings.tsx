import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import * as api from "./api";
import type { Tool, ToolStatus } from "./api";

const TOOLS: Record<Tool, { name: string; purpose: string }> = {
  "yt-dlp": { name: "yt-dlp", purpose: "Videos, audio and photos from 1,800+ sites" },
  ffmpeg: { name: "FFmpeg", purpose: "Joins, converts and renders video" },
};

/** "Update yt-dlp": idle → downloading (verified before use) → a result line. */
type Update = { step: "running"; percent: number } | { step: "done"; text: string } | { step: "error"; error: string };

const message = (e: unknown) => (typeof e === "string" ? e : String(e));
const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

interface Props {
  outDir: string;
  onChooseFolder: () => void;
  onClose: () => void;
}

export default function Settings({ outDir, onChooseFolder, onClose }: Props) {
  const [tools, setTools] = useState<ToolStatus[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);

  const loadTools = () => api.engineStatus().then(setTools).catch((e) => setLoadError(message(e)));

  useEffect(() => {
    loadTools();
    const off = api.onUpdateProgress((percent) =>
      setUpdate((u) => (u?.step === "running" ? { step: "running", percent } : u)),
    );
    return () => {
      off.then((stop) => stop());
    };
  }, []);

  // Only runs when clicked; the backend checks the download against yt-dlp's official checksums.
  const updateYtDlp = async () => {
    setUpdate({ step: "running", percent: -1 });
    try {
      const outcome = await api.updateYtDlp();
      await loadTools();
      setUpdate({ step: "done", text: outcome.result === "updated" ? "Updated and verified" : "Already up to date" });
    } catch (e) {
      setUpdate({ step: "error", error: message(e) });
    }
  };

  return (
    <div className="settings">
      <div className="settings-head">
        <h1>Settings</h1>
        <button className="secondary" onClick={onClose}>
          Done
        </button>
      </div>

      <section>
        <div className="settings-label">Save to</div>
        <div className="save">
          <FolderIcon />
          <button
            className="save-target"
            title="Open this folder"
            onClick={() => outDir && revealItemInDir(outDir).catch(() => {})}
          >
            <span className="save-name">{outDir ? folderName(outDir) : "Downloads"}</span>
            <span className="save-path">{outDir}</span>
          </button>
          <button className="secondary" onClick={onChooseFolder}>
            Change
          </button>
        </div>
      </section>

      <section>
        <div className="settings-label">Download engines</div>
        {loadError && <p className="note error">{loadError}</p>}
        {!tools && !loadError && <p className="note">Checking…</p>}
        {tools?.map((t) => (
          <EngineRow
            key={t.tool}
            status={t}
            update={t.tool === "yt-dlp" ? update : undefined}
            onUpdate={t.tool === "yt-dlp" ? updateYtDlp : undefined}
          />
        ))}
        <p className="note small">
          Built into the app; nothing is downloaded unless you press Update. yt-dlp updates are checked against
          yt-dlp's official checksums before they're used.
        </p>
      </section>
    </div>
  );
}

interface EngineRowProps {
  status: ToolStatus;
  update?: Update | null;
  onUpdate?: () => void;
}

function EngineRow({ status, update, onUpdate }: EngineRowProps) {
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

export function FolderIcon() {
  return (
    <svg className="folder-icon" viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 7.5A2.5 2.5 0 0 1 5.5 5h3.6l2 2h7.4A2.5 2.5 0 0 1 21 9.5v7a2.5 2.5 0 0 1-2.5 2.5h-13A2.5 2.5 0 0 1 3 16.5z" />
    </svg>
  );
}
