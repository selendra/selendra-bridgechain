import { useEffect, type ReactNode } from "react";
import { StatusBadge } from "./StatusBadge";
import { Glyph, ArrowRight } from "./icons";
import { chainViz, formatUnits, shortHex } from "../data/format";
import { fetchSubmission } from "../api/client";
import { usePoll } from "../api/hooks";
import type { Chain, Submission } from "../api/types";

interface Props {
  submissionId: string;
  chains: Chain[];
  onClose: () => void;
}

function Row({ label, children, mono }: { label: string; children: ReactNode; mono?: boolean }) {
  return (
    <div className="detail__row">
      <span className="detail__label">{label}</span>
      <span className={`detail__value${mono ? " detail__value--mono" : ""}`}>{children}</span>
    </div>
  );
}

function chainName(chains: Chain[], id: number) {
  return chains.find((c) => c.chainId === id)?.name ?? `Chain ${id}`;
}

export function SubmissionDetail({ submissionId, chains, onClose }: Props) {
  const { data, error, loading } = usePoll<Submission | null>(
    () => fetchSubmission(submissionId),
    [submissionId],
    6000
  );

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const from = data ? chainViz(data.chainIdFrom, chainName(chains, data.chainIdFrom)) : null;
  const to = data ? chainViz(data.chainIdTo, chainName(chains, data.chainIdTo)) : null;

  return (
    <div className="drawer-scrim" onClick={onClose}>
      <aside className="drawer" onClick={(e) => e.stopPropagation()} role="dialog" aria-label="Submission detail">
        <header className="drawer__head">
          <h2>Transfer</h2>
          <button type="button" className="drawer__close" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

        {loading && !data && <div className="drawer__body">Loading…</div>}
        {error && !data && <div className="drawer__body notice notice--error">Couldn’t load: {error}</div>}

        {data && (
          <div className="drawer__body">
            <div className="detail__route">
              {from && (
                <span className="chaincell">
                  <Glyph gradient={from.gradient} size={22} /> {chainName(chains, data.chainIdFrom)}
                </span>
              )}
              <ArrowRight size={16} />
              {to && (
                <span className="chaincell">
                  <Glyph gradient={to.gradient} size={22} /> {chainName(chains, data.chainIdTo)}
                </span>
              )}
              <StatusBadge status={data.status} />
            </div>

            <Row label="Amount">{formatUnits(data.amount, 18)} </Row>
            <Row label="Nonce">{data.nonce}</Row>
            <Row label="Receiver" mono>
              {data.receiver}
            </Row>
            {data.nativeSender && (
              <Row label="Sender" mono>
                {data.nativeSender}
              </Row>
            )}
            <Row label="Submission ID" mono>
              {data.submissionId}
            </Row>
            {data.debridgeId && (
              <Row label="deBridge ID" mono>
                {shortHex(data.debridgeId, 12, 8)}
              </Row>
            )}

            <div className="detail__sigs">
              <div className="detail__sigs-head">
                <span>Signatures</span>
                <span className="sig-count">
                  {data.signatureCount}
                  {data.meetsThreshold != null && (
                    <span className={`sig-count__badge${data.meetsThreshold ? " ok" : ""}`}>
                      {data.meetsThreshold ? "threshold met" : "below threshold"}
                    </span>
                  )}
                </span>
              </div>
              {data.signatures.length === 0 && <div className="detail__empty">No signatures yet.</div>}
              {data.signatures.map((sig, i) => (
                <div className="sig-item" key={sig.signer + i}>
                  <span className="sig-item__idx">{i + 1}</span>
                  <span className="sig-item__signer" title={sig.signer}>
                    {shortHex(sig.signer, 10, 8)}
                  </span>
                  {sig.signature && <span className="sig-item__sig">{shortHex(sig.signature, 10, 6)}</span>}
                </div>
              ))}
            </div>
          </div>
        )}
      </aside>
    </div>
  );
}
