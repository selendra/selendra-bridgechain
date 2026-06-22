import type { Submission } from "../lib/api";
import { formatAmount, shortHex } from "../lib/format";
import { StatusBadge } from "./StatusBadge";

interface Props {
  submissions: Submission[];
  selectedId: string | null;
  onSelect: (s: Submission) => void;
}

export function SubmissionsTable({ submissions, selectedId, onSelect }: Props) {
  if (submissions.length === 0) {
    return (
      <div className="empty">
        No submissions match. Once a transfer is locked on the source chain and a
        validator signs it, it appears here.
      </div>
    );
  }

  return (
    <div className="table-wrap">
      <table className="subs-table">
        <thead>
          <tr>
            <th>Submission ID</th>
            <th>Route</th>
            <th className="num">Nonce</th>
            <th className="num">Amount</th>
            <th>Receiver</th>
            <th className="num">Sigs</th>
            <th>Status</th>
          </tr>
        </thead>
        <tbody>
          {submissions.map((s) => (
            <tr
              key={s.submissionId}
              className={s.submissionId === selectedId ? "selected" : undefined}
              onClick={() => onSelect(s)}
            >
              <td className="mono" title={s.submissionId}>
                {shortHex(s.submissionId)}
              </td>
              <td className="route">
                {s.chainIdFrom} → {s.chainIdTo}
              </td>
              <td className="num">{s.nonce}</td>
              <td className="num" title={`${s.amount} (base units)`}>
                {formatAmount(s.amount)}
              </td>
              <td className="mono" title={s.receiver}>
                {shortHex(s.receiver)}
              </td>
              <td className="num">
                {s.signatureCount}
                {s.meetsThreshold === true && <span className="check"> ✓</span>}
              </td>
              <td>
                <StatusBadge status={s.status} />
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
