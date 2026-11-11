import { useRef } from "react";
import * as api from "../../shared/api";
import type { Kind } from "../../shared/api";
import type { EngineState } from "../../app/useEngine";
import logo from "../../../backend/icons/128x128.png";
import { formatSize } from "./format";
import { Row } from "./Row";
import { SectionHead } from "./SectionHead";
import { isUrl, type Downloads } from "./useDownloads";
import "./download.css";

const SECTIONS: { kind: Kind; label: string }[] = [
  { kind: "video", label: "Video" },
  { kind: "audio", label: "Audio" },
  { kind: "image", label: "Images" },
];

interface Props {
  engine: EngineState;
  onRetryEngine: () => void;
  downloads: Downloads;
}

/** The main screen: the link box, then what the link holds. */
export default function DownloadView({ engine, onRetryEngine, downloads: d }: Props) {
  const selectOnMouseUp = useRef(false);

  const items = d.result?.items ?? [];
  const imageCount = items.filter((i) => i.kind === "image").length;
  const canRender =
    imageCount > 0 && items.some((i) => i.kind === "audio") && !items.some((i) => i.kind === "video");

  return (
    <>
      <form
        className="search"
        onSubmit={(e) => {
          e.preventDefault();
          d.probe(d.url);
        }}
      >
        <input
          autoFocus
          spellCheck={false}
          placeholder="Paste a link to any video, image or audio"
          value={d.url}
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
          onChange={(e) => d.setUrl(e.target.value)}
          onPaste={(e) => {
            const text = e.clipboardData.getData("text").trim();
            if (isUrl(text)) {
              e.preventDefault();
              d.setUrl(text);
              d.probe(text);
            }
          }}
        />
        {d.probing && (
          <div className="inline-status">
            <span className="spinner" />
            <button type="button" className="link" onClick={d.cancelProbe}>
              Cancel
            </button>
          </div>
        )}
      </form>

      <div className={`content ${d.result ? "" : "centered"}`}>
        {engine.status === "setup" ? (
          <p className="note">Starting…</p>
        ) : engine.status === "error" ? (
          <p className="note error">
            {engine.error}
            <br />
            <button className="link" onClick={onRetryEngine}>
              Try again
            </button>
          </p>
        ) : d.probing ? (
          <p className="note">Looking for media…</p>
        ) : d.probeError ? (
          <p className="note error">
            {d.probeError}
          </p>
        ) : d.result ? (
          <div className="results">
            <h1 title={d.result.title}>{d.result.title}</h1>

            {canRender && (
              <section>
                <SectionHead kind="render" label="Make a video" />
                <Row
                  thumb={items.find((i) => i.kind === "image")?.thumbnail ?? null}
                  title={`${imageCount} image${imageCount > 1 ? "s" : ""} + audio`}
                  format="mp4"
                  detail="slideshow"
                  action="Render"
                  job={d.jobs[d.jobId("render")]}
                  onStart={d.render}
                  onCancel={() => api.cancelJob(d.jobId("render"))}
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
                    const id = d.jobId(i);
                    return (
                      <Row
                        key={id}
                        thumb={item.thumbnail}
                        title={item.title}
                        format={item.format}
                        detail={item.detail}
                        // Sizes from the engine are estimates; a direct link's is exact.
                        size={item.size ? `${item.source.via === "ytDlp" ? "~" : ""}${formatSize(item.size)}` : undefined}
                        qualities={item.qualities}
                        quality={d.quality[id] ?? item.qualities[0]?.height}
                        onQuality={(h) => d.chooseQuality(id, h)}
                        action="Download"
                        job={d.jobs[id]}
                        onStart={() => d.download(item, i)}
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
  );
}
