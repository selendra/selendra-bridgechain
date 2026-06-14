import type { Submission } from "../lib/api";
import { formatAmount, receiverAddress, shortHex } from "../lib/format";
import { StatusBadge } from "./StatusBadge";

interface Props {
  submission: Submission;
  onClose: () => void;
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="detail-row">
      <span className="detail-label">{label}</span>
      <span className={mono ? "detail-value mono" : "detail-value"}>{value}</span>
    </div>
  );
}

/** Slide-in panel with the full record + every collected signature. */
export function SubmissionDetail({ submission: s, onClose }: Props) {
  return (
    <aside className="detail" role="dialog" aria-label="Submission detail">
      <header className="detail-head">
        <div>
          <StatusBadge status={s.status} />
          <h2>Submission</h2>
        </div>
        <button className="btn-ghost" onClick={onClose} aria-label="Close">
          ✕
        </button>
      </header>

      <div className="detail-body">
        <Row label="Submission ID" value={s.submissionId} mono />
        <Row label="deBridge ID" value={s.debridgeId} mono />
        <Row label="Route" value={`${s.chainIdFrom} → ${s.chainIdTo}`} />
        <Row label="Nonce" value={s.nonce.toString()} />
        <Row label="Amount" value={`${formatAmount(s.amount)}  (${s.amount})`} />
        <Row label="Receiver" value={receiverAddress(s.receiver)} mono />
        <Row label="Native sender" value={s.nativeSender} mono />
        <Row
          label="Auto params"
          value={s.autoParams === "0x" ? "0x (none)" : s.autoParams}
          mono
        />
        <Row
          label="Meets threshold"
          value={s.meetsThreshold == null ? "unknown" : s.meetsThreshold ? "yes" : "no"}
        />
        <Row
          label="Executed on dest"
          value={s.executed == null ? "unknown (no dest RPC)" : s.executed ? "yes" : "no"}
        />

        <h3>
          Signatures <span className="count-pill">{s.signatureCount}</span>
        </h3>
        {s.signatures.length === 0 ? (
          <div className="empty small">No signatures collected yet.</div>
        ) : (
          <ul className="sig-list">
            {s.signatures.map((sig) => (
              <li key={sig.signature}>
                <span className="mono signer" title={sig.signer}>
                  {shortHex(sig.signer, 10, 6)}
                </span>
                <span className="mono sig" title={sig.signature}>
                  {shortHex(sig.signature, 10, 8)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </aside>
  );
}
