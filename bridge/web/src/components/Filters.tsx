import type { SubmissionFilter } from "../lib/api";

export type ReadyFilter = "any" | "ready" | "pending";

export interface FilterValues {
  chainIdFrom: string;
  chainIdTo: string;
  minSignatures: string;
  ready: ReadyFilter;
}

export const EMPTY_FILTERS: FilterValues = {
  chainIdFrom: "",
  chainIdTo: "",
  minSignatures: "",
  ready: "any",
};

/** Translate the form's string fields into the GraphQL SubmissionFilter. */
export function toFilter(v: FilterValues): SubmissionFilter | undefined {
  const f: SubmissionFilter = {};
  const from = parseInt(v.chainIdFrom, 10);
  const to = parseInt(v.chainIdTo, 10);
  const min = parseInt(v.minSignatures, 10);
  if (Number.isFinite(from)) f.chainIdFrom = from;
  if (Number.isFinite(to)) f.chainIdTo = to;
  if (Number.isFinite(min)) f.minSignatures = min;
  if (v.ready === "ready") f.ready = true;
  if (v.ready === "pending") f.ready = false;
  return Object.keys(f).length ? f : undefined;
}

interface Props {
  values: FilterValues;
  onChange: (next: FilterValues) => void;
  thresholdKnown: boolean;
}

export function Filters({ values, onChange, thresholdKnown }: Props) {
  const set = (patch: Partial<FilterValues>) => onChange({ ...values, ...patch });

  return (
    <section className="filters">
      <label>
        From chain
        <input
          type="number"
          inputMode="numeric"
          placeholder="any"
          value={values.chainIdFrom}
          onChange={(e) => set({ chainIdFrom: e.target.value })}
        />
      </label>
      <label>
        To chain
        <input
          type="number"
          inputMode="numeric"
          placeholder="any"
          value={values.chainIdTo}
          onChange={(e) => set({ chainIdTo: e.target.value })}
        />
      </label>
      <label>
        Min signatures
        <input
          type="number"
          inputMode="numeric"
          min={0}
          placeholder="0"
          value={values.minSignatures}
          onChange={(e) => set({ minSignatures: e.target.value })}
        />
      </label>
      <label>
        Readiness
        <select
          value={values.ready}
          onChange={(e) => set({ ready: e.target.value as ReadyFilter })}
          disabled={!thresholdKnown}
          title={
            thresholdKnown
              ? undefined
              : "the API was started without --threshold, so readiness can't be filtered"
          }
        >
          <option value="any">Any</option>
          <option value="ready">Ready only</option>
          <option value="pending">Pending only</option>
        </select>
      </label>
      <button
        type="button"
        className="btn-ghost"
        onClick={() => onChange(EMPTY_FILTERS)}
      >
        Clear
      </button>
    </section>
  );
}
