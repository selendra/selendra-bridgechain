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

/**
 * Shown alongside StatusBadge for a transfer the indexer has flagged past the
 * refund timeout and still unclaimed. Informational only — no on-chain refund
 * mechanism exists yet, so this never implies funds have moved.
 */
export function RefundBadge({ refundStatus }: { refundStatus: string }) {
  if (refundStatus === "none") return null;
  const label = refundStatus === "refunded" ? "Refunded" : "Stuck — refund eligible";
  return <span className={`badge badge--refund-${refundStatus}`}>{label}</span>;
}
