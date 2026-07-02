import { Check } from "lucide-react";
import type { Submission } from "../../lib/api";
import { formatAmount, shortHex } from "../../lib/format";
import { cn } from "../../lib/cn";
import { StatusBadge } from "./StatusBadge";

interface Props {
  submissions: Submission[];
  selectedId: string | null;
  onSelect: (s: Submission) => void;
}

const TH = "px-4 py-3 text-[11px] font-medium uppercase tracking-wide text-muted";

export function SubmissionsTable({ submissions, selectedId, onSelect }: Props) {
  if (submissions.length === 0) {
    return (
      <div className="rounded-xl border border-dashed border-line px-5 py-10 text-center text-[13px] text-muted">
        No submissions match. Once a transfer is locked on the source chain and a
        validator signs it, it appears here.
      </div>
    );
  }

  return (
    <div className="overflow-hidden rounded-xl border border-line">
      <table className="w-full border-collapse text-[13px]">
        <thead className="bg-surface">
          <tr className="border-b border-line text-left">
            <th className={TH}>Submission ID</th>
            <th className={TH}>Route</th>
            <th className={cn(TH, "text-right")}>Nonce</th>
            <th className={cn(TH, "text-right")}>Amount</th>
            <th className={TH}>Receiver</th>
            <th className={cn(TH, "text-right")}>Sigs</th>
            <th className={TH}>Status</th>
          </tr>
        </thead>
        <tbody>
          {submissions.map((s) => {
            const selected = s.submissionId === selectedId;
            return (
              <tr
                key={s.submissionId}
                onClick={() => onSelect(s)}
                className={cn(
                  "cursor-pointer border-b border-line/70 transition-colors last:border-0",
                  selected ? "bg-accent-soft" : "hover:bg-surface",
                )}
              >
                <td
                  className="px-4 py-3 font-mono text-[12px]"
                  title={s.submissionId}
                >
                  {shortHex(s.submissionId)}
                </td>
                <td className="px-4 py-3 tnum text-muted">
                  {s.chainIdFrom} → {s.chainIdTo}
                </td>
                <td className="px-4 py-3 text-right tnum">{s.nonce}</td>
                <td
                  className="px-4 py-3 text-right tnum"
                  title={`${s.amount} (base units)`}
                >
                  {formatAmount(s.amount)}
                </td>
                <td
                  className="px-4 py-3 font-mono text-[12px]"
                  title={s.receiver}
                >
                  {shortHex(s.receiver)}
                </td>
                <td className="px-4 py-3 text-right tnum">
                  <span className="inline-flex items-center justify-end gap-1">
                    {s.signatureCount}
                    {s.meetsThreshold === true && (
                      <Check className="size-3.5 text-success" />
                    )}
                  </span>
                </td>
                <td className="px-4 py-3">
                  <StatusBadge status={s.status} />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
