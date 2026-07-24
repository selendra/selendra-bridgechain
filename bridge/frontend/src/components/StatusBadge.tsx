import type { SubmissionStatus } from "../api/types";

const LABEL: Record<SubmissionStatus, string> = {
  PENDING: "Pending",
  READY: "Ready",
  EXECUTED: "Executed",
  CANCELLED: "Cancelled",
  UNKNOWN: "Unknown",
};

export function StatusBadge({ status }: { status: SubmissionStatus }) {
  return <span className={`badge badge--${status.toLowerCase()}`}>{LABEL[status]}</span>;
}

const REFUND_LABEL: Record<string, string> = {
  eligible: "Stuck — refund eligible",
  cancelled: "Cancelling — refund pending",
  refunded: "Refunded",
};

/**
 * Shown alongside StatusBadge for a transfer in the refund lifecycle.
 *
 * 'cancelled' is the one worth reading carefully: the transfer has been burned
 * on the destination chain so it can never be delivered, and the source-chain
 * repayment is in flight. Funds are safe but have not landed anywhere yet.
 */
export function RefundBadge({ refundStatus }: { refundStatus: string }) {
  if (refundStatus === "none") return null;
  const label = REFUND_LABEL[refundStatus] ?? refundStatus;
  return <span className={`badge badge--refund-${refundStatus}`}>{label}</span>;
}
