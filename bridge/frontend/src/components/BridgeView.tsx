import { useCallback, useEffect, useMemo, useState } from "react";
import { Dropdown, type DropdownOption } from "./Dropdown";
import { TxBanner, type TxState } from "./TxBanner";
import { ArrowRight, Glyph, Help } from "./icons";
import { chainViz, formatUnits, formatUnitsRaw, isAddress, parseUnits } from "../data/format";
import { errMsg, readAllowance, readBalance, readDecimals, sendApprove, sendBridge, waitReceipt } from "../wallet/eth";
import type { Chain, SubmissionFilter } from "../api/types";
import type { WalletState } from "../wallet/useWallet";

interface Props {
  chains: Chain[];
  wallet: WalletState;
  /** Jump to the Explorer filtered to this corridor after a send. */
  onReview: (filter: SubmissionFilter) => void;
}

export function BridgeView({ chains, wallet, onReview }: Props) {
  const fromChainId = wallet.chainId; // sends execute on the connected chain
  const fromReg = useMemo(
    () => chains.find((c) => c.chainId === fromChainId) ?? null,
    [chains, fromChainId]
  );

  const [toChainId, setToChainId] = useState<number | null>(null);
  const [token, setToken] = useState("");
  const [gate, setGate] = useState("");
  const [amount, setAmount] = useState("");
  const [receiver, setReceiver] = useState("");
  const [tx, setTx] = useState<TxState>({ kind: "idle" });
  const [decimals, setDecimals] = useState(18);
  const [balance, setBalance] = useState<bigint | null>(null);
  const [allowance, setAllowance] = useState<bigint | null>(null);

  // Default destination = first chain that isn't the source.
  useEffect(() => {
    if (toChainId != null && toChainId !== fromChainId) return;
    const other = chains.find((c) => c.chainId !== fromChainId);
    if (other) setToChainId(other.chainId);
  }, [chains, fromChainId, toChainId]);

  // Prefill token/gate from the registry for the source chain, when it pins them.
  useEffect(() => {
    if (fromReg?.token && !token) setToken(fromReg.token);
    if (fromReg?.gate && !gate) setGate(fromReg.gate);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fromReg?.chainId]);

  // Default receiver = the connected account.
  useEffect(() => {
    if (!receiver && wallet.address) setReceiver(wallet.address);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wallet.address]);

  const tokenOk = isAddress(token);
  const gateOk = isAddress(gate);
  const receiverOk = isAddress(receiver);

  // On-chain reads (decimals/balance/allowance) against the connected chain.
  const refreshOnchain = useCallback(async () => {
    if (!wallet.address || !tokenOk) {
      setBalance(null);
      setAllowance(null);
      return;
    }
    try {
      const dec = await readDecimals(wallet.request, token).catch(() => 18);
      setDecimals(Number.isFinite(dec) && dec > 0 && dec <= 36 ? dec : 18);
      const [b, a] = await Promise.all([
        readBalance(wallet.request, token, wallet.address),
        gateOk ? readAllowance(wallet.request, token, wallet.address, gate) : Promise.resolve(0n),
      ]);
      setBalance(b);
      setAllowance(a);
    } catch {
      setBalance(null);
      setAllowance(null);
    }
  }, [wallet.address, wallet.request, token, gate, tokenOk, gateOk]);

  useEffect(() => {
    refreshOnchain();
  }, [refreshOnchain]);

  const amountBase = tokenOk ? parseUnits(amount, decimals) : 0n;
  const needsApprove = allowance != null && amountBase > 0n && gateOk && allowance < amountBase;
  const insufficient = balance != null && amountBase > balance;
  const busy = tx.kind === "pending";

  const doApprove = async () => {
    if (!wallet.address) return;
    setTx({ kind: "pending", label: "Approving token…" });
    try {
      const hash = await sendApprove(wallet.request, wallet.address, token, gate, amountBase);
      setTx({ kind: "pending", label: "Confirming approval…", hash });
      await waitReceipt(wallet.request, hash);
      await refreshOnchain();
      setTx({ kind: "idle" });
    } catch (e) {
      setTx({ kind: "error", message: errMsg(e) });
    }
  };

  const doSend = async () => {
    if (!wallet.address || toChainId == null) return;
    setTx({ kind: "pending", label: "Locking + emitting…" });
    try {
      const hash = await sendBridge(wallet.request, wallet.address, gate, token, amountBase, toChainId, receiver, "0x");
      setTx({ kind: "pending", label: "Confirming send…", hash });
      const r = await waitReceipt(wallet.request, hash);
      if (!r.success) throw new Error("Send reverted on-chain");
      setTx({ kind: "done", label: "Locked — validators will sign it", hash });
      setAmount("");
      await refreshOnchain();
    } catch (e) {
      setTx({ kind: "error", message: errMsg(e) });
    }
  };

  const toOptions: DropdownOption[] = chains
    .filter((c) => c.chainId !== fromChainId)
    .map((c) => {
      const v = chainViz(c.chainId, c.name);
      return { value: String(c.chainId), label: c.name, glyph: <Glyph gradient={v.gradient} size={20} /> };
    });

  const fromViz = chainViz(fromChainId ?? 0, fromReg?.name);
  const fromName = fromReg?.name ?? (fromChainId ? `Chain ${fromChainId}` : "—");

  // Primary button
  let button: { label: string; onClick?: () => void; disabled?: boolean };
  if (!wallet.address) button = { label: "Connect Wallet", onClick: () => wallet.connect() };
  else if (fromChainId == null) button = { label: "Unknown network", disabled: true };
  else if (!tokenOk) button = { label: "Enter a token address", disabled: true };
  else if (!gateOk) button = { label: "Enter the Gate address", disabled: true };
  else if (toChainId == null) button = { label: "Pick a destination", disabled: true };
  else if (!receiverOk) button = { label: "Enter a valid receiver", disabled: true };
  else if (amountBase <= 0n) button = { label: "Enter an amount", disabled: true };
  else if (insufficient) button = { label: "Insufficient balance", disabled: true };
  else if (needsApprove) button = { label: "Approve token", onClick: doApprove };
  else button = { label: "Bridge", onClick: doSend };
  if (busy) button = { label: tx.label, disabled: true };

  return (
    <section className="card">
      <div className="card__head">
        <div>
          <h2 className="card__title">Bridge</h2>
          <p className="card__subtitle">Lock on the source chain; a threshold of validators signs the claim.</p>
        </div>
      </div>

      {/* route: from (wallet) -> to (picker) */}
      <div className="bridge-route">
        <div className="bridge-route__node">
          <span className="bridge-route__label">From</span>
          <span className="chaincell">
            <Glyph gradient={fromViz.gradient} size={22} /> {fromName}
          </span>
          <span className="bridge-route__hint">connected wallet</span>
        </div>
        <ArrowRight size={18} className="bridge-route__arrow" />
        <div className="bridge-route__node">
          <span className="bridge-route__label">To</span>
          <Dropdown
            variant="chain"
            value={toChainId != null ? String(toChainId) : ""}
            options={toOptions}
            onChange={(v) => setToChainId(Number(v))}
          />
        </div>
      </div>

      <div className="fields">
        <label className="field">
          <span className="field__label">Token (ERC-20 on {fromName})</span>
          <input
            className={`field__input mono${token && !tokenOk ? " field__input--bad" : ""}`}
            placeholder="0x…"
            value={token}
            onChange={(e) => setToken(e.target.value.trim())}
          />
        </label>

        <label className="field">
          <span className="field__label">Gate contract</span>
          <input
            className={`field__input mono${gate && !gateOk ? " field__input--bad" : ""}`}
            placeholder="0x…"
            value={gate}
            onChange={(e) => setGate(e.target.value.trim())}
          />
        </label>

        <div className="field">
          <span className="field__label">
            Amount
            {balance != null && (
              <button
                type="button"
                className="field__max"
                onClick={() => setAmount(formatUnitsRaw(balance, decimals))}
              >
                Max {formatUnits(balance, decimals)}
              </button>
            )}
          </span>
          <input
            className="field__input"
            inputMode="decimal"
            placeholder="0.0"
            value={amount}
            onChange={(e) => {
              const v = e.target.value;
              if (v === "" || /^\d*\.?\d*$/.test(v)) setAmount(v);
            }}
          />
        </div>

        <label className="field">
          <span className="field__label">Receiver (destination address)</span>
          <input
            className={`field__input mono${receiver && !receiverOk ? " field__input--bad" : ""}`}
            placeholder="0x…"
            value={receiver}
            onChange={(e) => setReceiver(e.target.value.trim())}
          />
        </label>
      </div>

      <dl className="summary">
        <div className="summary__row">
          <dt>
            Locked on send <Help size={14} />
          </dt>
          <dd>{amountBase > 0n ? formatUnits(amountBase, decimals) : "—"}</dd>
        </div>
        <div className="summary__row">
          <dt>
            Signature threshold <Help size={14} />
          </dt>
          <dd>validator quorum</dd>
        </div>
      </dl>

      <TxBanner tx={tx} />

      {tx.kind === "done" && toChainId != null && fromChainId != null && (
        <button
          type="button"
          className="ghost-btn ghost-btn--full"
          onClick={() => onReview({ chainIdFrom: fromChainId, chainIdTo: toChainId })}
        >
          Track this transfer in the Explorer <ArrowRight size={14} />
        </button>
      )}

      <button type="button" className="review-btn" disabled={button.disabled || !button.onClick} onClick={button.onClick}>
        {button.label}
      </button>
    </section>
  );
}
