import type { SubmissionStatus } from "../api/types";

const LABEL: Record<SubmissionStatus, string> = {
  PENDING: "Pending",
  READY: "Ready",
  EXECUTED: "Executed",
  UNKNOWN: "Unknown",
};

export function StatusBadge({ status }: { status: SubmissionStatus }) {
  return <span className={`badge badge--${status.toLowerCase()}`}>{LABEL[status]}</span>;
}
