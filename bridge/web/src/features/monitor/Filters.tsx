import { X } from "lucide-react";
import type { SubmissionFilter } from "../../lib/api";
import { Button } from "../../components/ui/button";
import { FieldLabel, Input, Select } from "../../components/ui/input";

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
  const dirty = JSON.stringify(values) !== JSON.stringify(EMPTY_FILTERS);

  return (
    <section className="flex flex-wrap items-end gap-3">
      <FieldLabel className="w-28">
        From chain
        <Input
          type="number"
          inputMode="numeric"
          placeholder="any"
          value={values.chainIdFrom}
          onChange={(e) => set({ chainIdFrom: e.target.value })}
        />
      </FieldLabel>
      <FieldLabel className="w-28">
        To chain
        <Input
          type="number"
          inputMode="numeric"
          placeholder="any"
          value={values.chainIdTo}
          onChange={(e) => set({ chainIdTo: e.target.value })}
        />
      </FieldLabel>
      <FieldLabel className="w-28">
        Min signatures
        <Input
          type="number"
          inputMode="numeric"
          min={0}
          placeholder="0"
          value={values.minSignatures}
          onChange={(e) => set({ minSignatures: e.target.value })}
        />
      </FieldLabel>
      <FieldLabel className="w-36">
        Readiness
        <Select
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
        </Select>
      </FieldLabel>
      <Button
        variant="ghost"
        size="sm"
        disabled={!dirty}
        onClick={() => onChange(EMPTY_FILTERS)}
      >
        <X />
        Clear
      </Button>
    </section>
  );
}
