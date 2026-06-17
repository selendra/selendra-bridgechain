import { useEffect, useMemo, useState } from "react";
import { useWallet } from "../hooks/useWallet";
import {
  type BridgeChain,
  findChain,
  loadChains,
  saveChains,
} from "../lib/chains";
import {
  bridgeSend,
  formatTokenAmount,
  mintTokens,
  readBalanceAt,
  readToken,
  type SendResult,
  type TokenMeta,
} from "../lib/bridge";
import { shortHex } from "../lib/format";

const FAUCET_AMOUNT = "1000";

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
  const [meta, setMeta] = useState<TokenMeta | null>(null);
  const [minting, setMinting] = useState(false);
  const [destNote, setDestNote] = useState<string | null>(null);

  // Load the connected account's balance/symbol for the chosen token.
  // Prefer the configured chain RPC over the wallet provider: MetaMask serves
  // stale/cached ERC-20 balances on local chains, which made this read wrong.
  const tokenAddr = token.trim();
  async function refreshBalance() {
    if (!wallet.account || tokenAddr === "") {
      setMeta(null);
      return;
    }
    try {
      if (sourceChain?.rpcUrl) {
        setMeta(await readBalanceAt(sourceChain.rpcUrl, tokenAddr, wallet.account));
      } else if (wallet.provider) {
        setMeta(await readToken(wallet.provider, tokenAddr, wallet.account));
      } else {
        setMeta(null);
      }
    } catch {
      setMeta(null); // not a valid token address (yet)
    }
  }
  useEffect(() => {
    void refreshBalance();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tokenAddr, wallet.account, wallet.chainId]);

  async function handleMint() {
    if (!wallet.provider || !wallet.account || tokenAddr === "") return;
    setError(null);
    setMinting(true);
    try {
      await mintTokens(wallet.provider, tokenAddr, wallet.account, FAUCET_AMOUNT);
      await refreshBalance();
    } catch (e) {
      const msg = (e as { shortMessage?: string })?.shortMessage;
      setError(msg ?? (e instanceof Error ? e.message : "Mint failed (is this the local TestToken?)."));
    } finally {
      setMinting(false);
    }
  }

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
  // Keep the destination valid: never equal to the source, always a configured
  // chain. Re-runs when the source network changes (e.g. user switches "From").
  useEffect(() => {
    const valid = chainIdTo !== "" && destOptions.some((c) => c.chainId === chainIdTo);
    if (!valid) setChainIdTo(destOptions[0]?.chainId ?? "");
  }, [destOptions, chainIdTo]);

  const sameChain = chainIdTo !== "" && chainIdTo === wallet.chainId;
  const busy = stage === "approving" || stage === "sending" || stage === "confirming";
  const canSubmit =
    !!wallet.account &&
    !!sourceChain &&
    gate.trim() !== "" &&
    token.trim() !== "" &&
    amount.trim() !== "" &&
    chainIdTo !== "" &&
    !sameChain &&
    receiver.trim() !== "" &&
    !busy;

  async function handleBridge() {
    if (!wallet.provider || chainIdTo === "") return;
    if (sameChain) {
      setError("Source and destination must differ — pick a different destination chain.");
      return;
    }
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
      void refreshBalance();
      void trackDestCredit(receiver, Number(chainIdTo));
      if (res.submissionId) onSent?.(res.submissionId);
    } catch (e) {
      setStage("error");
      const msg = (e as { shortMessage?: string })?.shortMessage;
      setError(msg ?? (e instanceof Error ? e.message : "Bridge transaction failed."));
    }
  }

  // After a send, read the receiver's balance straight from the DESTINATION
  // chain's RPC (not via MetaMask, which caches stale balances on local chains)
  // and poll until the keeper's claim credits it.
  async function trackDestCredit(receiverAddr: string, destChainId: number) {
    const dest = findChain(chains, destChainId);
    if (!dest?.rpcUrl || !dest.token) {
      setDestNote(null);
      return;
    }
    setDestNote(`Waiting for the keeper to credit ${dest.name}…`);
    let last: bigint | null = null;
    for (let i = 0; i < 30; i++) {
      try {
        const m = await readBalanceAt(dest.rpcUrl, dest.token, receiverAddr);
        if (last != null && m.balance > last) {
          setDestNote(
            `Receiver now holds ${formatTokenAmount(m.balance, m.decimals)} ${m.symbol} on ${dest.name} (read from chain RPC).`,
          );
          return;
        }
        last = m.balance;
        if (i === 0) {
          setDestNote(
            `Receiver holds ${formatTokenAmount(m.balance, m.decimals)} ${m.symbol} on ${dest.name}; waiting for the +${amount} credit…`,
          );
        }
      } catch {
        /* keep polling */
      }
      await new Promise((r) => setTimeout(r, 1000));
    }
    setDestNote(
      `Sent. If your wallet shows no change on ${dest.name}, switch MetaMask to that network and refresh — it caches balances on local chains.`,
    );
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
            From (your wallet network)
            <select
              value={wallet.chainId ?? ""}
              onChange={(e) => wallet.switchChain(Number(e.target.value))}
            >
              {!sourceChain && (
                <option value={wallet.chainId ?? ""}>
                  chain {wallet.chainId ?? "?"} (not configured)
                </option>
              )}
              {chains.map((c) => (
                <option key={c.chainId} value={c.chainId}>
                  {c.name} ({c.chainId})
                </option>
              ))}
            </select>
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
            <span className="label-row">
              Amount
              {meta && (
                <span className="balance">
                  balance {formatTokenAmount(meta.balance, meta.decimals)} {meta.symbol}
                  {meta.balance > 0n && (
                    <button
                      type="button"
                      className="link-btn"
                      onClick={() =>
                        setAmount(formatTokenAmount(meta.balance, meta.decimals))
                      }
                    >
                      max
                    </button>
                  )}
                </span>
              )}
            </span>
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

          {sameChain && (
            <p className="zero-hint">
              Destination equals source ({wallet.chainId}). Pick a different “To”
              chain — a same-chain transfer is never claimed by the keeper and
              would stay <b>Ready</b> forever.
            </p>
          )}

          {meta && meta.balance === 0n && (
            <p className="zero-hint">
              This account holds 0 {meta.symbol} on{" "}
              {sourceChain?.name ?? `chain ${wallet.chainId}`}. Switch the “From”
              network to where your tokens are, or click <b>Mint test tokens</b>{" "}
              (local TestToken only).
            </p>
          )}

          <div className="bridge-actions">
            <button onClick={handleBridge} disabled={!canSubmit}>
              {busy
                ? STAGE_LABEL[stage as keyof typeof STAGE_LABEL]
                : "Approve & bridge"}
            </button>
            <button
              className="btn-ghost"
              onClick={handleMint}
              disabled={minting || busy || tokenAddr === ""}
              title={`Mint ${FAUCET_AMOUNT} test tokens to your account`}
            >
              {minting ? "Minting…" : `Mint test tokens`}
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
          {destNote && <div className="banner-hint">{destNote}</div>}
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
