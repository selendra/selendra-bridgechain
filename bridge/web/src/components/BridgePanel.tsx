import { useEffect, useMemo, useState } from "react";
import { useWallet } from "../hooks/useWallet";
import {
  type BridgeChain,
  findChain,
  loadChains,
  saveChains,
} from "../lib/chains";
import { bridgeSend, type SendResult } from "../lib/bridge";
import { shortHex } from "../lib/format";

type Stage = "idle" | "approving" | "sending" | "confirming" | "done" | "error";

const STAGE_LABEL: Record<Exclude<Stage, "idle" | "done" | "error">, string> = {
  approving: "Approving token…",
  sending: "Sending (confirm in wallet)…",
  confirming: "Waiting for confirmation…",
};

export function BridgePanel({ onSent }: { onSent?: (submissionId: string) => void }) {
  const wallet = useWallet();
  const [chains, setChains] = useState<BridgeChain[]>(() => loadChains());
  const [showSettings, setShowSettings] = useState(false);

  const sourceChain = findChain(chains, wallet.chainId);

  // Form fields.
  const [gate, setGate] = useState("");
  const [token, setToken] = useState("");
  const [amount, setAmount] = useState("");
  const [chainIdTo, setChainIdTo] = useState<number | "">("");
  const [receiver, setReceiver] = useState("");

  const [stage, setStage] = useState<Stage>("idle");
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<SendResult | null>(null);

  // Prefill gate/token from the registry when the wallet's chain changes.
  useEffect(() => {
    setGate(sourceChain?.gate ?? "");
    setToken(sourceChain?.token ?? "");
  }, [sourceChain?.chainId]);

  // Default the receiver to the connected account, and the destination to the
  // first other configured chain.
  useEffect(() => {
    if (wallet.account && !receiver) setReceiver(wallet.account);
  }, [wallet.account]);

  const destOptions = useMemo(
    () => chains.filter((c) => c.chainId !== wallet.chainId),
    [chains, wallet.chainId],
  );
  useEffect(() => {
    if (chainIdTo === "" && destOptions[0]) setChainIdTo(destOptions[0].chainId);
  }, [destOptions, chainIdTo]);

  const busy = stage === "approving" || stage === "sending" || stage === "confirming";
  const canSubmit =
    !!wallet.account &&
    !!sourceChain &&
    gate.trim() !== "" &&
    token.trim() !== "" &&
    amount.trim() !== "" &&
    chainIdTo !== "" &&
    receiver.trim() !== "" &&
    !busy;

  async function handleBridge() {
    if (!wallet.provider || chainIdTo === "") return;
    setError(null);
    setResult(null);
    setStage("sending");
    try {
      const res = await bridgeSend(
        wallet.provider,
        { gate, token, amount, chainIdTo, receiver },
        (s) => setStage(s),
      );
      setResult(res);
      setStage("done");
      if (res.submissionId) onSent?.(res.submissionId);
    } catch (e) {
      setStage("error");
      const msg = (e as { shortMessage?: string })?.shortMessage;
      setError(msg ?? (e instanceof Error ? e.message : "Bridge transaction failed."));
    }
  }

  function persistChain(chainId: number, patch: Partial<BridgeChain>) {
    const next = chains.map((c) => (c.chainId === chainId ? { ...c, ...patch } : c));
    setChains(next);
    saveChains(next);
  }

  // ---- Not installed / not connected gates ----
  if (!wallet.available) {
    return (
      <div className="bridge-card">
        <h2>Bridge tokens</h2>
        <div className="banner error">
          <strong>No Ethereum wallet detected.</strong>
          <div className="banner-hint">
            Install <a href="https://metamask.io" target="_blank" rel="noreferrer">MetaMask</a>{" "}
            (or another EIP-1193 wallet) and add your local anvil networks (chain id +
            RPC) to bridge from the browser.
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="bridge-card">
      <div className="bridge-head">
        <h2>Bridge tokens</h2>
        {wallet.account ? (
          <div className="wallet-chip" title={wallet.account}>
            <span className="wallet-dot" />
            {shortHex(wallet.account, 6, 4)}
            <span className="wallet-net">
              {sourceChain ? sourceChain.name : `chain ${wallet.chainId ?? "?"}`}
            </span>
          </div>
        ) : (
          <button onClick={wallet.connect} disabled={wallet.connecting}>
            {wallet.connecting ? "Connecting…" : "Connect wallet"}
          </button>
        )}
      </div>

      {wallet.error && <div className="banner error">{wallet.error}</div>}

      {wallet.account && !sourceChain && (
        <div className="banner error">
          <strong>Chain {wallet.chainId} isn't configured.</strong>
          <div className="banner-hint">
            Switch your wallet to a configured network, or add this chain under{" "}
            <button className="link-btn" onClick={() => setShowSettings(true)}>
              network settings
            </button>
            .
          </div>
        </div>
      )}

      {wallet.account && (
        <div className="bridge-form">
          <label>
            From
            <input value={sourceChain ? `${sourceChain.name} (${sourceChain.chainId})` : "—"} readOnly />
          </label>

          <label>
            To chain
            <select
              value={chainIdTo}
              onChange={(e) => setChainIdTo(e.target.value === "" ? "" : Number(e.target.value))}
            >
              {destOptions.length === 0 && <option value="">no other chains</option>}
              {destOptions.map((c) => (
                <option key={c.chainId} value={c.chainId}>
                  {c.name} ({c.chainId})
                </option>
              ))}
            </select>
          </label>

          <label className="wide">
            Token address (source)
            <input
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="0x… ERC-20 on the source chain"
              spellCheck={false}
            />
          </label>

          <label className="wide">
            Gate address (source)
            <input
              value={gate}
              onChange={(e) => setGate(e.target.value)}
              placeholder="0x… deployed Gate on the source chain"
              spellCheck={false}
            />
          </label>

          <label>
            Amount
            <input
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
              placeholder="1.0"
              inputMode="decimal"
            />
          </label>

          <label className="wide">
            Receiver (destination)
            <input
              value={receiver}
              onChange={(e) => setReceiver(e.target.value)}
              placeholder="0x… recipient on the destination chain"
              spellCheck={false}
            />
          </label>

          <div className="bridge-actions">
            <button onClick={handleBridge} disabled={!canSubmit}>
              {busy
                ? STAGE_LABEL[stage as keyof typeof STAGE_LABEL]
                : "Approve & bridge"}
            </button>
            {sourceChain && gate.trim() !== "" && (
              <button
                className="btn-ghost"
                onClick={() => persistChain(sourceChain.chainId, { gate, token })}
                disabled={busy}
                title="Remember this gate/token for this chain"
              >
                Save as default
              </button>
            )}
          </div>
        </div>
      )}

      {stage === "done" && result && (
        <div className="banner success">
          <strong>Bridged.</strong> Validators will sign it; it appears in the
          dashboard once observed.
          <div className="banner-hint">
            submissionId <code>{shortHex(result.submissionId, 10, 8)}</code>
            {" · "}tx <code>{shortHex(result.txHash, 8, 6)}</code>
            {result.approvalTxHash && (
              <> {" · "}approve <code>{shortHex(result.approvalTxHash, 8, 6)}</code></>
            )}
          </div>
        </div>
      )}

      {stage === "error" && error && <div className="banner error">{error}</div>}

      <div className="bridge-settings">
        <button className="link-btn" onClick={() => setShowSettings((s) => !s)}>
          {showSettings ? "▾ Network settings" : "▸ Network settings"}
        </button>
        {showSettings && (
          <table className="net-table">
            <thead>
              <tr>
                <th>Chain id</th>
                <th>Name</th>
                <th>Gate</th>
                <th>Default token</th>
              </tr>
            </thead>
            <tbody>
              {chains.map((c) => (
                <tr key={c.chainId}>
                  <td className="mono">{c.chainId}</td>
                  <td>{c.name}</td>
                  <td>
                    <input
                      className="net-input"
                      value={c.gate ?? ""}
                      placeholder="0x…"
                      spellCheck={false}
                      onChange={(e) => persistChain(c.chainId, { gate: e.target.value })}
                    />
                  </td>
                  <td>
                    <input
                      className="net-input"
                      value={c.token ?? ""}
                      placeholder="0x…"
                      spellCheck={false}
                      onChange={(e) => persistChain(c.chainId, { token: e.target.value })}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
