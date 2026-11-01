import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import * as api from "./api";
import type { Kind, MediaItem, ProbeResult, Quality } from "./api";
import Settings from "./Settings";
import logo from "../src-tauri/icons/128x128.png";
import "./App.css";

type Job =
  | { status: "running"; percent: number }
  | { status: "done"; path: string }
  | { status: "error"; error: string };

type EngineState =
  | { status: "setup" }
  | { status: "ready" }
  | { status: "error"; error: string };

const OUT_DIR_KEY = "outDir";
// Left behind by an earlier version's (removed) browser-login setting; cleared on start.
const OLD_KEYS = ["cookieBrowser"];
// macOS draws its traffic lights over the page (titleBarStyle "Overlay"); we only add an empty
// strip there to drag the window by. Windows and Linux keep their native title bar.
const IS_MAC = navigator.userAgent.includes("Mac");
const isUrl = (s: string) => /^https?:\/\/\S+$/i.test(s.trim());
const message = (e: unknown) => (typeof e === "string" ? e : String(e));
const SECTIONS: { kind: Kind; label: string }[] = [
  { kind: "video", label: "Video" },
  { kind: "audio", label: "Audio" },
  { kind: "image", label: "Images" },
];
const qualityLabel = (q: Quality) =>
  `${q.height}p${q.height >= 2160 ? " 4K" : q.height >= 1440 ? " 2K" : ""}${q.compatible ? "" : " · slower"}`;

function stored(key: string) {
  try {
    return localStorage.getItem(key) ?? "";
  } catch {
    return "";
  }
}

function store(key: string, value: string) {
  try {
    if (value) localStorage.setItem(key, value);
    else localStorage.removeItem(key);
  } catch {}
}

export default function App() {
  const [engine, setEngine] = useState<EngineState>({ status: "setup" });
  const [url, setUrl] = useState("");
  const [probing, setProbing] = useState(false);
  const [probeError, setProbeError] = useState("");
  const [result, setResult] = useState<ProbeResult | null>(null);
  const [jobs, setJobs] = useState<Record<string, Job>>({});
  const [outDir, setOutDir] = useState(() => stored(OUT_DIR_KEY));
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Chosen resolution per video row, keyed by job id.
  const [quality, setQuality] = useState<Record<string, number>>({});
  // Job ids are scoped to a probe so results from an older link never collide.
  const generation = useRef(0);
  // Latest jobs for long-running loops, which would otherwise see a stale snapshot.
  const jobsRef = useRef(jobs);
  jobsRef.current = jobs;
  const probingRef = useRef(false);
  const selectOnMouseUp = useRef(false);
  const readyRef = useRef(false);
  // A link that arrived before the engine was ready; fetched as soon as it is.
  const pendingUrl = useRef("");

  const setJob = (id: string, job: Job) => setJobs((j) => ({ ...j, [id]: job }));
  const probeId = (gen: number) => `probe-${gen}`;
  const jobId = (key: string | number, gen = generation.current) => `${gen}-${key}`;

  const probe = async (link: string) => {
    link = link.trim();
    if (!isUrl(link)) return;
    if (!readyRef.current) {
      pendingUrl.current = link;
      return;
    }
    // A new link replaces the one still being looked up.
    if (probingRef.current) api.cancelJob(probeId(generation.current));
    const gen = ++generation.current;
    probingRef.current = true;
    setProbing(true);
    setProbeError("");
    setResult(null);
    try {
      const found = await api.probeUrl(probeId(gen), link);
      if (generation.current === gen) setResult(found);
    } catch (e) {
      if (generation.current === gen) setProbeError(message(e));
    } finally {
      if (generation.current === gen) {
        probingRef.current = false;
        setProbing(false);
      }
    }
  };

  const startEngine = useCallback(() => {
    setEngine({ status: "setup" });
    api
      .setupEngine()
      .then(() => {
        readyRef.current = true;
        setEngine({ status: "ready" });
        if (pendingUrl.current) probe(pendingUrl.current);
      })
      .catch((e) => setEngine({ status: "error", error: message(e) }));
  }, []);

  useEffect(() => {
    const unlisten = [
      api.onJobProgress((id, percent) =>
        setJobs((j) => (j[id]?.status === "running" ? { ...j, [id]: { status: "running", percent } } : j)),
      ),
    ];
    OLD_KEYS.forEach((key) => store(key, ""));
    startEngine();
    if (!outDir) api.defaultOutDir().then(setOutDir).catch(() => {});
    return () => unlisten.forEach((p) => p.then((off) => off()));
  }, []);

  const download = async (item: MediaItem, index: number) => {
    const id = jobId(index);
    if (jobsRef.current[id]?.status === "running") return;
    // Apply the resolution picked in the row's quality menu (highest by default).
    const height = quality[id] ?? item.qualities[0]?.height;
    const source = item.source.via === "ytDlp" && height ? { ...item.source, maxHeight: height } : item.source;
    const running: Job = { status: "running", percent: 0 };
    jobsRef.current = { ...jobsRef.current, [id]: running };
    setJob(id, running);
    try {
      setJob(id, { status: "done", path: await api.downloadItem(id, { ...item, source }, outDir) });
    } catch (e) {
      setJob(id, { status: "error", error: message(e) });
    }
  };

  const render = async () => {
    if (!result) return;
    const id = jobId("render");
    if (jobsRef.current[id]?.status === "running") return;
    const images = result.items.filter((i) => i.kind === "image");
    const audio = result.items.find((i) => i.kind === "audio")!;
    const running: Job = { status: "running", percent: 0 };
    jobsRef.current = { ...jobsRef.current, [id]: running };
    setJob(id, running);
    try {
      setJob(id, { status: "done", path: await api.renderSlideshow(id, images, audio, result.title, outDir) });
    } catch (e) {
      setJob(id, { status: "error", error: message(e) });
    }
  };

  const closeSettings = () => setSettingsOpen(false);

  const chooseFolder = async () => {
    const dir = await open({ directory: true, defaultPath: outDir || undefined });
    if (typeof dir === "string") {
      setOutDir(dir);
      store(OUT_DIR_KEY, dir);
    }
  };


  const items = result?.items ?? [];
  const imageCount = items.filter((i) => i.kind === "image").length;
  const canRender =
    imageCount > 0 && items.some((i) => i.kind === "audio") && !items.some((i) => i.kind === "video");

  return (
    <main className={IS_MAC ? "mac" : undefined}>
      {IS_MAC && <div className="drag-strip" data-tauri-drag-region />}

      {settingsOpen ? (
        <Settings
          outDir={outDir}
          onChooseFolder={chooseFolder}
          onClose={closeSettings}
        />
      ) : (
        <>
          <button className="gear" title="Settings" aria-label="Settings" onClick={() => setSettingsOpen(true)}>
            <GearIcon />
          </button>

          <form
            className="search"
            onSubmit={(e) => {
              e.preventDefault();
              probe(url);
            }}
          >
            <input
              autoFocus
              spellCheck={false}
              placeholder="Paste a link to any video, image or audio"
              value={url}
              // Clicking the box selects the whole link, ready to be replaced. The mouseup that
              // follows a focusing click would otherwise collapse the selection again.
              onFocus={(e) => {
                e.currentTarget.select();
                selectOnMouseUp.current = true;
              }}
              onMouseUp={(e) => {
                if (selectOnMouseUp.current) {
                  e.preventDefault();
                  selectOnMouseUp.current = false;
                }
              }}
              onChange={(e) => setUrl(e.target.value)}
              onPaste={(e) => {
                const text = e.clipboardData.getData("text").trim();
                if (isUrl(text)) {
                  e.preventDefault();
                  setUrl(text);
                  probe(text);
                }
              }}
            />
            {probing && (
              <div className="inline-status">
                <span className="spinner" />
                <button type="button" className="link" onClick={() => api.cancelJob(probeId(generation.current))}>
                  Cancel
                </button>
              </div>
            )}
          </form>

          <div className={`content ${result ? "" : "centered"}`}>
            {engine.status === "setup" ? (
              <p className="note">Starting…</p>
            ) : engine.status === "error" ? (
              <p className="note error">
                {engine.error}
                <br />
                <button className="link" onClick={startEngine}>
                  Try again
                </button>
              </p>
            ) : probing ? (
              <p className="note">Looking for media…</p>
            ) : probeError ? (
              <p className="note error">
                {probeError}
              </p>
            ) : result ? (
              <div className="results">
                <h1 title={result.title}>{result.title}</h1>

                {canRender && (
                  <section>
                    <SectionHead kind="render" label="Make a video" />
                    <Row
                      thumb={items.find((i) => i.kind === "image")?.thumbnail ?? null}
                      title={`${imageCount} image${imageCount > 1 ? "s" : ""} + audio`}
                      format="mp4"
                      detail="slideshow"
                      action="Render"
                      job={jobs[jobId("render")]}
                      onStart={render}
                      onCancel={() => api.cancelJob(jobId("render"))}
                    />
                  </section>
                )}

                {SECTIONS.map(({ kind, label }) => {
                  const rows = items.map((item, i) => ({ item, i })).filter(({ item }) => item.kind === kind);
                  if (rows.length === 0) return null;
                  return (
                    <section key={kind}>
                      <SectionHead kind={kind} label={label} count={rows.length} />
                      {rows.map(({ item, i }) => {
                        const id = jobId(i);
                        return (
                          <Row
                            key={id}
                            thumb={item.thumbnail}
                            title={item.title}
                            format={item.format}
                            detail={item.detail}
                            qualities={item.qualities}
                            quality={quality[id] ?? item.qualities[0]?.height}
                            onQuality={(h) => setQuality((q) => ({ ...q, [id]: h }))}
                            action="Download"
                            job={jobs[id]}
                            onStart={() => download(item, i)}
                            onCancel={() => api.cancelJob(id)}
                          />
                        );
                      })}
                    </section>
                  );
                })}
              </div>
            ) : (
              <div className="placeholder">
                <img src={logo} alt="" />
                <p>Nothing to preview</p>
              </div>
            )}
          </div>

        </>
      )}
    </main>
  );
}

function GearIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true">
      <path d="M12 15.2a3.2 3.2 0 1 0 0-6.4 3.2 3.2 0 0 0 0 6.4Z" />
      <path d="M19.4 13.5a7.6 7.6 0 0 0 0-3l2-1.6-2-3.4-2.4 1a7.4 7.4 0 0 0-2.6-1.5L14 2.5h-4l-.4 2.5A7.4 7.4 0 0 0 7 6.5l-2.4-1-2 3.4 2 1.6a7.6 7.6 0 0 0 0 3l-2 1.6 2 3.4 2.4-1a7.4 7.4 0 0 0 2.6 1.5l.4 2.5h4l.4-2.5a7.4 7.4 0 0 0 2.6-1.5l2.4 1 2-3.4Z" />
    </svg>
  );
}

function SectionHead({ kind, label, count }: { kind: Kind | "render"; label: string; count?: number }) {
  return (
    <div className={`section-head ${kind}`}>
      <span className="dot" />
      <span className="section-label">{label}</span>
      {count !== undefined && count > 1 && <span className="count">{count}</span>}
    </div>
  );
}

interface RowProps {
  thumb: string | null;
  title: string;
  format: string | null;
  detail: string | null;
  qualities?: Quality[];
  quality?: number;
  onQuality?: (height: number) => void;
  action: string;
  job?: Job;
  onStart: () => void;
  onCancel: () => void;
}

function Row({ thumb, title, format, detail, qualities = [], quality, onQuality, action, job, onStart, onCancel }: RowProps) {
  const running = job?.status === "running";
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
            {detail && <span className="detail">{detail}</span>}
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
