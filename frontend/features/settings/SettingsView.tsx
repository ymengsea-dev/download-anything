import { useEffect, useState } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import * as api from "../../shared/api";
import type { ToolStatus } from "../../shared/api";
import { FolderIcon } from "../../shared/icons";
import { EngineRow, type Update } from "./EngineRow";
import "./settings.css";

const folderName = (path: string) => path.split(/[\\/]/).filter(Boolean).pop() ?? path;

interface Props {
  outDir: string;
  onChooseFolder: () => void;
  onClose: () => void;
}

export default function SettingsView({ outDir, onChooseFolder, onClose }: Props) {
  const [tools, setTools] = useState<ToolStatus[] | null>(null);
  const [loadError, setLoadError] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);

  const loadTools = () => api.engineStatus().then(setTools).catch((e) => setLoadError(api.message(e)));

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
      setUpdate({ step: "error", error: api.message(e) });
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
