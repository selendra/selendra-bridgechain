import { X } from "lucide-react";
import type { Submission } from "../../lib/api";
import { formatAmount, receiverAddress, shortHex } from "../../lib/format";
import { cn } from "../../lib/cn";
import { Button } from "../../components/ui/button";
import { StatusBadge } from "./StatusBadge";

interface Props {
  submission: Submission;
  onClose: () => void;
}

function Row({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex flex-col gap-0.5 border-b border-line/70 py-2">
      <span className="text-[11px] uppercase tracking-wide text-muted">
        {label}
      </span>
      <span className={cn("break-all text-fg/90", mono && "font-mono text-[12px]")}>
        {value}
      </span>
    </div>
  );
}

/** Sticky side panel with the full record + every collected signature. */
export function SubmissionDetail({ submission: s, onClose }: Props) {
  return (
    <aside
      role="dialog"
      aria-label="Submission detail"
      className="glass sticky top-4 max-h-[calc(100vh-2rem)] w-full shrink-0 overflow-auto rounded-xl border border-line lg:w-[380px]"
    >
      <header className="flex items-start justify-between border-b border-line p-4">
        <div className="flex flex-col gap-2">
          <StatusBadge status={s.status} />
          <h2 className="text-base font-semibold">Submission</h2>
        </div>
        <Button variant="ghost" size="icon" onClick={onClose} aria-label="Close">
          <X />
        </Button>
      </header>

      <div className="p-4">
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
          value={
            s.meetsThreshold == null ? "unknown" : s.meetsThreshold ? "yes" : "no"
          }
        />
        <Row
          label="Executed on dest"
          value={
            s.executed == null ? "unknown (no dest RPC)" : s.executed ? "yes" : "no"
          }
        />

        <h3 className="mb-2 mt-5 flex items-center gap-2 text-[13px] font-semibold">
          Signatures
          <span className="rounded-full bg-surface-2 px-2 py-0.5 text-[11px] font-semibold text-muted">
            {s.signatureCount}
          </span>
        </h3>
        {s.signatures.length === 0 ? (
          <div className="rounded-lg border border-dashed border-line p-3 text-center text-[12.5px] text-muted">
            No signatures collected yet.
          </div>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {s.signatures.map((sig) => (
              <li
                key={sig.signature}
                className="flex flex-col gap-0.5 rounded-lg border border-line bg-surface-2 px-3 py-2"
              >
                <span
                  className="font-mono text-[12px] text-accent"
                  title={sig.signer}
                >
                  {shortHex(sig.signer, 10, 6)}
                </span>
                <span
                  className="font-mono text-[12px] text-faint"
                  title={sig.signature}
                >
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
