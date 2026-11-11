import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import * as api from "../shared/api";
import { GearIcon } from "../shared/icons";
import { store, stored } from "../shared/storage";
import DownloadView from "../features/download/DownloadView";
import { useDownloads } from "../features/download/useDownloads";
import SettingsView from "../features/settings/SettingsView";
import { useEngine } from "./useEngine";

const OUT_DIR_KEY = "outDir";
// Left behind by an earlier version's (removed) browser-login setting; cleared on start.
const OLD_KEYS = ["cookieBrowser"];
// macOS draws its traffic lights over the page (titleBarStyle "Overlay"); we only add an empty
// strip there to drag the window by. Windows and Linux keep their native title bar.
const IS_MAC = navigator.userAgent.includes("Mac");

export default function App() {
  const engine = useEngine();
  const [outDir, setOutDir] = useState(() => stored(OUT_DIR_KEY));
  const [settingsOpen, setSettingsOpen] = useState(false);
  const downloads = useDownloads(outDir, engine.state.status === "ready");

  useEffect(() => {
    OLD_KEYS.forEach((key) => store(key, ""));
    if (!outDir) api.defaultOutDir().then(setOutDir).catch(() => {});
  }, []);

  const chooseFolder = async () => {
    const dir = await open({ directory: true, defaultPath: outDir || undefined });
    if (typeof dir === "string") {
      setOutDir(dir);
      store(OUT_DIR_KEY, dir);
    }
  };

  return (
    <main className={IS_MAC ? "mac" : undefined}>
      {IS_MAC && <div className="drag-strip" data-tauri-drag-region />}

      {settingsOpen ? (
        <SettingsView outDir={outDir} onChooseFolder={chooseFolder} onClose={() => setSettingsOpen(false)} />
      ) : (
        <>
          <button className="gear" title="Settings" aria-label="Settings" onClick={() => setSettingsOpen(true)}>
            <GearIcon />
          </button>
          <DownloadView engine={engine.state} onRetryEngine={engine.start} downloads={downloads} />
        </>
      )}
    </main>
  );
}
