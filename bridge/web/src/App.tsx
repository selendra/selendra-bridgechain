import { useCallback, useMemo, useState } from "react";
import {
  fetchStats,
  fetchSubmissions,
  type Submission,
} from "./lib/api";
import { usePoll } from "./hooks/usePoll";
import { StatsBar } from "./components/StatsBar";
import {
  EMPTY_FILTERS,
  Filters,
  toFilter,
  type FilterValues,
} from "./components/Filters";
import { SubmissionsTable } from "./components/SubmissionsTable";
import { SubmissionDetail } from "./components/SubmissionDetail";
import { Lookup } from "./components/Lookup";
import { BridgePanel } from "./components/BridgePanel";

const POLL_MS = 4000;

type View = "monitor" | "bridge";

export function App() {
  const [view, setView] = useState<View>("monitor");
  const [filters, setFilters] = useState<FilterValues>(EMPTY_FILTERS);
  const [selected, setSelected] = useState<Submission | null>(null);

  // Memoize the GraphQL filter so the poll effect only restarts when a field
  // actually changes (not on every keystroke that leaves it equivalent).
  const gqlFilter = useMemo(() => toFilter(filters), [filters]);
  const filterKey = JSON.stringify(gqlFilter ?? null);

  const stats = usePoll(fetchStats, POLL_MS, []);
  const subs = usePoll(
    useCallback((signal) => fetchSubmissions(gqlFilter, signal), [filterKey]),
    POLL_MS,
    [filterKey],
  );

  const thresholdKnown = stats.data?.threshold != null;
  const apiDown = stats.error && stats.data == null;

  // Keep the open detail panel in sync with freshly polled list data.
  const selectedLive =
    selected && subs.data
      ? subs.data.find((s) => s.submissionId === selected.submissionId) ?? selected
      : selected;

  return (
    <div className="app">
      <header className="topbar">
        <div className="brand">
          <span className="logo">⇌</span>
          <div>
            <h1>Bridge Dashboard</h1>
            <p>EVM ↔ EVM transfers, signatures &amp; on-chain status</p>
          </div>
        </div>
        <div className="topbar-right">
          <nav className="tabs">
            <button
              className={`tab ${view === "monitor" ? "active" : ""}`}
              onClick={() => setView("monitor")}
            >
              Monitor
            </button>
            <button
              className={`tab ${view === "bridge" ? "active" : ""}`}
              onClick={() => setView("bridge")}
            >
              Bridge
            </button>
          </nav>
          {view === "monitor" && <Lookup onFound={setSelected} />}
          <span
            className={`live-dot ${subs.refreshing || stats.refreshing ? "pulsing" : ""}`}
            title={`auto-refresh every ${POLL_MS / 1000}s`}
          />
        </div>
      </header>

      {view === "bridge" && (
        <BridgePanel
          onSent={() => {
            setView("monitor");
          }}
        />
      )}

      {view === "monitor" && (
        <>
          {apiDown ? (
            <div className="banner error">
              <strong>Can't reach the GraphQL API.</strong> {stats.error}
              <div className="banner-hint">
                Start it, e.g.{" "}
                <code>
                  graphql-api --dir bridge/sig-store-data --threshold 2
                </code>{" "}
                (or <code>--store-url http://127.0.0.1:8080</code>), then this
                page reconnects on the next poll.
              </div>
            </div>
          ) : (
            stats.data && <StatsBar stats={stats.data} />
          )}

          <main className="content">
            <div className="list-pane">
              <Filters
                values={filters}
                onChange={setFilters}
                thresholdKnown={thresholdKnown}
              />
              {subs.error && subs.data == null ? (
                <div className="banner error">{subs.error}</div>
              ) : subs.loading ? (
                <div className="empty">Loading submissions…</div>
              ) : (
                <SubmissionsTable
                  submissions={subs.data ?? []}
                  selectedId={selectedLive?.submissionId ?? null}
                  onSelect={setSelected}
                />
              )}
            </div>

            {selectedLive && (
              <SubmissionDetail
                submission={selectedLive}
                onClose={() => setSelected(null)}
              />
            )}
          </main>
        </>
      )}
    </div>
  );
}
