import { useCallback, useEffect, useRef, useState } from "react";

/** Debounce a rapidly-changing value (e.g. an amount input driving a quote). */
export function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setV(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return v;
}

export interface PollResult<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  /** Manually re-run the fetch now. */
  refetch: () => void;
}

/**
 * Poll an async fetcher on an interval, keeping the last good data on error.
 * `deps` re-arms the poll (e.g. a filter change). The tab is polite: it skips
 * refetches while the document is hidden.
 */
export function usePoll<T>(fn: () => Promise<T>, deps: unknown[], intervalMs = 4000): PollResult<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const fnRef = useRef(fn);
  fnRef.current = fn;

  const run = useCallback(async () => {
    try {
      const d = await fnRef.current();
      setData(d);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let alive = true;
    setLoading(true);
    run();
    const id = setInterval(() => {
      if (alive && document.visibilityState !== "hidden") run();
    }, intervalMs);
    return () => {
      alive = false;
      clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  return { data, error, loading, refetch: run };
}
