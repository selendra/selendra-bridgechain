import type { SubmissionStatus } from "../lib/api";

const LABELS: Record<SubmissionStatus, string> = {
  PENDING: "Pending",
  READY: "Ready",
  EXECUTED: "Executed",
  UNKNOWN: "Unknown",
};

/** Colored pill for a submission's lifecycle status. */
export function StatusBadge({ status }: { status: SubmissionStatus }) {
  return (
    <span className={`badge badge-${status.toLowerCase()}`}>{LABELS[status]}</span>
  );
}
