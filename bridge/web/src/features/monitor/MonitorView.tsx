import { useMemo, useState } from "react";
import type { Submission } from "../../lib/api";
import { useStats, useSubmissions } from "../../hooks/queries";
import { Banner } from "../../components/ui/banner";
import { StatsBar } from "./StatsBar";
import { EMPTY_FILTERS, Filters, toFilter, type FilterValues } from "./Filters";
import { SubmissionsTable } from "./SubmissionsTable";
import { SubmissionDetail } from "./SubmissionDetail";

interface Props {
  selected: Submission | null;
  onSelect: (s: Submission | null) => void;
}

export function MonitorView({ selected, onSelect }: Props) {
  const [filters, setFilters] = useState<FilterValues>(EMPTY_FILTERS);
  const gqlFilter = useMemo(() => toFilter(filters), [filters]);

  const stats = useStats();
  const subs = useSubmissions(gqlFilter);

  const thresholdKnown = stats.data?.threshold != null;
  const apiDown = stats.isError && stats.data == null;

  // Keep the open detail panel in sync with freshly polled list data.
  const selectedLive =
    selected && subs.data
      ? subs.data.find((s) => s.submissionId === selected.submissionId) ?? selected
      : selected;

  return (
    <>
      {apiDown ? (
        <Banner tone="error">
          <strong>Can't reach the GraphQL API.</strong>{" "}
          {stats.error instanceof Error ? stats.error.message : ""}
          <div className="mt-1.5 text-[12.5px] text-muted">
            Start it, e.g.{" "}
            <code className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[12px]">
              graphql-api --dir bridge/sig-store-data --threshold 2
            </code>{" "}
            (or{" "}
            <code className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[12px]">
              --store-url http://127.0.0.1:8080
            </code>
            ), then this page reconnects on the next poll.
          </div>
        </Banner>
      ) : (
        stats.data && <StatsBar stats={stats.data} />
      )}

      <main className="flex flex-col items-start gap-4 lg:flex-row">
        <div className="min-w-0 flex-1 space-y-3.5">
          <Filters
            values={filters}
            onChange={setFilters}
            thresholdKnown={thresholdKnown}
          />
          {subs.isError && subs.data == null ? (
            <Banner tone="error">
              {subs.error instanceof Error ? subs.error.message : "Query failed."}
            </Banner>
          ) : subs.isLoading ? (
            <div className="rounded-xl border border-dashed border-line px-5 py-10 text-center text-muted">
              Loading submissions…
            </div>
          ) : (
            <SubmissionsTable
              submissions={subs.data ?? []}
              selectedId={selectedLive?.submissionId ?? null}
              onSelect={onSelect}
            />
          )}
        </div>

        {selectedLive && (
          <SubmissionDetail
            submission={selectedLive}
            onClose={() => onSelect(null)}
          />
        )}
      </main>
    </>
  );
}
