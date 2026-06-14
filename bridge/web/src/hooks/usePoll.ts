import { useCallback, useEffect, useRef, useState } from "react";

export interface PollState<T> {
  data: T | null;
  error: string | null;
  /** true only on the very first load (no data yet); refreshes are silent. */
  loading: boolean;
  /** true while any fetch (including a poll tick) is in flight. */
  refreshing: boolean;
  /** force an immediate refetch. */
  refetch: () => void;
}

/**
 * Poll an async fetcher on an interval, cancelling in-flight requests on
 * unmount or when the fetcher identity changes. The fetcher receives an
 * AbortSignal so a stale request can't clobber fresher data.
 *
 * Pass `intervalMs = 0` to fetch once and not poll.
 */
export function usePoll<T>(
  fetcher: (signal: AbortSignal) => Promise<T>,
  intervalMs: number,
  deps: ReadonlyArray<unknown> = [],
): PollState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [tick, setTick] = useState(0);
  const refetch = useCallback(() => setTick((t) => t + 1), []);

  // Hold the latest fetcher in a ref so the interval doesn't need it in deps.
  const fetcherRef = useRef(fetcher);
  fetcherRef.current = fetcher;

  useEffect(() => {
    let cancelled = false;
    const ctrl = new AbortController();

    const run = async () => {
      setRefreshing(true);
      try {
        const next = await fetcherRef.current(ctrl.signal);
        if (!cancelled) {
          setData(next);
          setError(null);
        }
      } catch (e) {
        if (cancelled || (e instanceof DOMException && e.name === "AbortError")) return;
        setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!cancelled) {
          setLoading(false);
          setRefreshing(false);
        }
      }
    };

    run();
    const id =
      intervalMs > 0 ? window.setInterval(run, intervalMs) : undefined;
    return () => {
      cancelled = true;
      ctrl.abort();
      if (id !== undefined) window.clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [intervalMs, tick, ...deps]);

  return { data, error, loading, refreshing, refetch };
}
