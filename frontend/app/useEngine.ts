import { useCallback, useEffect, useState } from "react";
import * as api from "../shared/api";

export type EngineState =
  | { status: "setup" }
  | { status: "ready" }
  | { status: "error"; error: string };

/** Finds the bundled engines on start; `start` retries after an error. */
export function useEngine() {
  const [state, setState] = useState<EngineState>({ status: "setup" });

  const start = useCallback(() => {
    setState({ status: "setup" });
    api
      .setupEngine()
      .then(() => setState({ status: "ready" }))
      .catch((e) => setState({ status: "error", error: api.message(e) }));
  }, []);

  useEffect(start, []);

  return { state, start };
}
