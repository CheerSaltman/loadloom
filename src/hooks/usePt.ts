import { useCallback, useEffect, useRef, useState } from "react";
import {
  commands,
  describeCoreError,
  type HistoryPoint,
  type PtSnapshot,
  type PtStartRequest,
} from "@/bindings";

export function usePt() {
  const [snapshot, setSnapshot] = useState<PtSnapshot | null>(null);
  const [history, setHistory] = useState<HistoryPoint[]>([]);
  const [error, setError] = useState<string | null>(null);
  const lastSeq = useRef(-1);

  useEffect(() => {
    let disposed = false;
    void (async () => {
      try {
        const initial = await commands.getPtSnapshot();
        if (disposed) return;
        setSnapshot(initial);
        lastSeq.current = initial.seq;
        await commands.subscribePt((frame) => {
          if (frame.phase === "starting" || frame.phase === "metadata") {
            lastSeq.current = -1;
          } else if (frame.phase === "downloading" && frame.seq <= lastSeq.current) {
            return;
          }
          if (frame.phase === "downloading") lastSeq.current = frame.seq;
          setSnapshot(frame);
          if (frame.phase === "downloading") {
            setHistory((previous) => [
              ...previous.slice(-119),
              { speedBps: frame.speedBps, latencyMs: 0, jitterMs: 0 },
            ]);
          }
        });
      } catch (cause) {
        if (!disposed) setError(describeCoreError(cause));
      }
    })();
    return () => {
      disposed = true;
    };
  }, []);

  const start = useCallback(async (request: PtStartRequest) => {
    setError(null);
    setHistory([]);
    try {
      await commands.startPt(request);
    } catch (cause) {
      setError(describeCoreError(cause));
    }
  }, []);

  const stop = useCallback(async () => {
    setError(null);
    try {
      await commands.stopPt();
    } catch (cause) {
      setError(describeCoreError(cause));
    }
  }, []);

  return { snapshot, history, error, start, stop };
}
