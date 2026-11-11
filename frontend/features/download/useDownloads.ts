import { useEffect, useRef, useState } from "react";
import * as api from "../../shared/api";
import type { MediaItem, ProbeResult } from "../../shared/api";

export type Job =
  | { status: "running"; percent: number }
  | { status: "done"; path: string }
  | { status: "error"; error: string };

export const isUrl = (s: string) => /^https?:\/\/\S+$/i.test(s.trim());

/** The link, what it was found to hold, and every download/render job for it. Lives in App so it
 *  survives a trip to the settings screen. Links that arrive before `ready` are fetched once it is. */
export function useDownloads(outDir: string, ready: boolean) {
  const [url, setUrl] = useState("");
  const [probing, setProbing] = useState(false);
  const [probeError, setProbeError] = useState("");
  const [result, setResult] = useState<ProbeResult | null>(null);
  const [jobs, setJobs] = useState<Record<string, Job>>({});
  // Chosen resolution per video row, keyed by job id.
  const [quality, setQuality] = useState<Record<string, number>>({});
  // Job ids are scoped to a probe so results from an older link never collide.
  const generation = useRef(0);
  // Latest jobs for long-running loops, which would otherwise see a stale snapshot.
  const jobsRef = useRef(jobs);
  jobsRef.current = jobs;
  const probingRef = useRef(false);
  const readyRef = useRef(ready);
  readyRef.current = ready;
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
      if (generation.current === gen) setProbeError(api.message(e));
    } finally {
      if (generation.current === gen) {
        probingRef.current = false;
        setProbing(false);
      }
    }
  };

  useEffect(() => {
    if (ready && pendingUrl.current) {
      probe(pendingUrl.current);
      pendingUrl.current = "";
    }
  }, [ready]);

  useEffect(() => {
    const unlisten = api.onJobProgress((id, percent) =>
      setJobs((j) => (j[id]?.status === "running" ? { ...j, [id]: { status: "running", percent } } : j)),
    );
    return () => {
      unlisten.then((off) => off());
    };
  }, []);

  /** Marks `id` running and stores what `work` returns; a second start while it runs is ignored. */
  const runJob = async (id: string, work: () => Promise<string>) => {
    if (jobsRef.current[id]?.status === "running") return;
    const running: Job = { status: "running", percent: 0 };
    jobsRef.current = { ...jobsRef.current, [id]: running };
    setJob(id, running);
    try {
      setJob(id, { status: "done", path: await work() });
    } catch (e) {
      setJob(id, { status: "error", error: api.message(e) });
    }
  };

  const download = (item: MediaItem, index: number) => {
    const id = jobId(index);
    // Apply the resolution picked in the row's quality menu (highest by default).
    const height = quality[id] ?? item.qualities[0]?.height;
    const source = item.source.via === "ytDlp" && height ? { ...item.source, maxHeight: height } : item.source;
    return runJob(id, () => api.downloadItem(id, { ...item, source }, outDir));
  };

  const render = () => {
    if (!result) return;
    const id = jobId("render");
    const images = result.items.filter((i) => i.kind === "image");
    const audio = result.items.find((i) => i.kind === "audio")!;
    return runJob(id, () => api.renderSlideshow(id, images, audio, result.title, outDir));
  };

  const chooseQuality = (id: string, height: number) => setQuality((q) => ({ ...q, [id]: height }));
  const cancelProbe = () => api.cancelJob(probeId(generation.current));

  return {
    url,
    setUrl,
    probing,
    probeError,
    result,
    jobs,
    quality,
    jobId,
    probe,
    cancelProbe,
    download,
    render,
    chooseQuality,
  };
}

export type Downloads = ReturnType<typeof useDownloads>;
