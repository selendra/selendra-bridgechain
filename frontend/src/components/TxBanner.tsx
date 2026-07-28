import { Check, Spinner, Warn } from "./icons";
import { shortHex } from "../data/format";

export type TxState =
  | { kind: "idle" }
  | { kind: "pending"; label: string; hash?: string }
  | { kind: "done"; label?: string; hash?: string }
  | { kind: "error"; message: string };

/** Inline pending / success / error status with the tx hash when known. */
export function TxBanner({ tx }: { tx: TxState }) {
  if (tx.kind === "idle") return null;

  if (tx.kind === "pending") {
    return (
      <div className="txbar txbar--pending">
        <Spinner size={16} />
        <span>{tx.label}</span>
        {tx.hash && <span className="txbar__hash">{shortHex(tx.hash, 10, 8)}</span>}
      </div>
    );
  }
  if (tx.kind === "done") {
    return (
      <div className="txbar txbar--done">
        <Check size={16} />
        <span>{tx.label ?? "Confirmed"}</span>
        {tx.hash && <span className="txbar__hash">{shortHex(tx.hash, 10, 8)}</span>}
      </div>
    );
  }
  return (
    <div className="txbar txbar--error">
      <Warn size={16} />
      <span>{tx.message}</span>
    </div>
  );
}
