import { useEffect, useMemo, useState } from "react";
import {
  ChevronDown,
  ChevronRight,
  Coins,
  Plus,
  Trash2,
  Wallet,
} from "lucide-react";
import { useWallet } from "../../hooks/useWallet";
import { useChains } from "../../hooks/queries";
import {
  blankChain,
  type BridgeChain,
  findChain,
  fromRemote,
  loadChains,
  mergeChains,
  saveChains,
} from "../../lib/chains";
import {
  bridgeSend,
  formatTokenAmount,
  mintTokens,
  readBalanceAt,
  readToken,
  type SendResult,
  type TokenMeta,
} from "../../lib/bridge";
import { shortHex } from "../../lib/format";
import { Card } from "../../components/ui/card";
import { Button } from "../../components/ui/button";
import { Banner } from "../../components/ui/banner";
import { FieldLabel, Input, Select } from "../../components/ui/input";

const FAUCET_AMOUNT = "1000";

type Stage = "idle" | "approving" | "sending" | "confirming" | "done" | "error";

const STAGE_LABEL: Record<Exclude<Stage, "idle" | "done" | "error">, string> = {
  approving: "Approving token…",
  sending: "Sending (confirm in wallet)…",
  confirming: "Waiting for confirmation…",
};

export function BridgeView({
  onSent,
}: {
  onSent?: (submissionId: string) => void;
}) {
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
      setError(
        msg ??
          (e instanceof Error
            ? e.message
            : "Mint failed (is this the local TestToken?)."),
      );
    } finally {
      setMinting(false);
    }
  }

  // Prefill gate/token from the registry when the wallet's chain changes.
  useEffect(() => {
    setGate(sourceChain?.gate ?? "");
    setToken(sourceChain?.token ?? "");
  }, [sourceChain?.chainId]);

  // Default the receiver to the connected account.
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
    const valid =
      chainIdTo !== "" && destOptions.some((c) => c.chainId === chainIdTo);
    if (!valid) setChainIdTo(destOptions[0]?.chainId ?? "");
  }, [destOptions, chainIdTo]);

  const sameChain = chainIdTo !== "" && chainIdTo === wallet.chainId;
  const busy =
    stage === "approving" || stage === "sending" || stage === "confirming";
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
      setError(
        "Source and destination must differ — pick a different destination chain.",
      );
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
      setError(
        msg ?? (e instanceof Error ? e.message : "Bridge transaction failed."),
      );
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
    const next = chains.map((c) =>
      c.chainId === chainId ? { ...c, ...patch } : c,
    );
    setChains(next);
    saveChains(next);
  }

  function commitChains(next: BridgeChain[]) {
    setChains(next);
    saveChains(next);
  }

  // Edit a settings row by index (chainId itself is editable, so we can't key by
  // it). Number-typed fields (chainId) are coerced; blanks become 0.
  function editChain(index: number, patch: Partial<BridgeChain>) {
    commitChains(chains.map((c, i) => (i === index ? { ...c, ...patch } : c)));
  }
  function addChainRow() {
    commitChains([...chains, blankChain()]);
    setShowSettings(true);
  }
  function removeChainRow(index: number) {
    commitChains(chains.filter((_, i) => i !== index));
  }

  // Fold the backend-advertised registry into the local one: new chains the user
  // hasn't seen are appended; local edits are never overwritten. Runs whenever
  // the fetched list changes; mergeChains returns the same ref when there's
  // nothing to add, so this can't loop.
  const remote = useChains();
  useEffect(() => {
    const data = remote.data;
    if (!data?.length) return;
    setChains((local) => {
      const merged = mergeChains(local, data.map(fromRemote));
      if (merged === local) return local;
      saveChains(merged);
      return merged;
    });
  }, [remote.data]);

  // Duplicate chainIds break findChain(); surface it in the settings panel.
  const dupChainIds = useMemo(() => {
    const seen = new Set<number>();
    const dups = new Set<number>();
    for (const c of chains) {
      if (c.chainId && seen.has(c.chainId)) dups.add(c.chainId);
      seen.add(c.chainId);
    }
    return dups;
  }, [chains]);

  // ---- Not installed gate ----
  if (!wallet.available) {
    return (
      <Card className="mx-auto mt-5 max-w-[720px] p-6">
        <h2 className="mb-3 text-lg font-semibold">Bridge tokens</h2>
        <Banner tone="error">
          <strong>No Ethereum wallet detected.</strong>
          <div className="mt-1.5 text-[12.5px] text-muted">
            Install{" "}
            <a
              href="https://metamask.io"
              target="_blank"
              rel="noreferrer"
              className="text-accent hover:underline"
            >
              MetaMask
            </a>{" "}
            (or another EIP-1193 wallet) and add your local anvil networks (chain
            id + RPC) to bridge from the browser.
          </div>
        </Banner>
      </Card>
    );
  }

  return (
    <Card className="mx-auto mt-5 max-w-[720px] p-6">
      <div className="mb-2 flex items-center justify-between gap-3">
        <h2 className="text-lg font-semibold">Bridge tokens</h2>
        {wallet.account ? (
          <div
            className="inline-flex items-center gap-2 rounded-full border border-line bg-surface px-3 py-1.5 font-mono text-[12px]"
            title={wallet.account}
          >
            <span className="size-2 rounded-full bg-success" />
            {shortHex(wallet.account, 6, 4)}
            <span className="border-l border-line pl-2 font-sans text-muted">
              {sourceChain ? sourceChain.name : `chain ${wallet.chainId ?? "?"}`}
            </span>
          </div>
        ) : (
          <Button onClick={wallet.connect} disabled={wallet.connecting}>
            <Wallet />
            {wallet.connecting ? "Connecting…" : "Connect wallet"}
          </Button>
        )}
      </div>

      {wallet.error && (
        <Banner tone="error" className="mt-3">
          {wallet.error}
        </Banner>
      )}

      {wallet.account && !sourceChain && (
        <Banner tone="error" className="mt-3">
          <strong>Chain {wallet.chainId} isn't configured.</strong>
          <div className="mt-1.5 text-[12.5px] text-muted">
            Switch your wallet to a configured network, or add this chain under{" "}
            <button
              className="text-accent hover:underline"
              onClick={() => setShowSettings(true)}
            >
              network settings
            </button>
            .
          </div>
        </Banner>
      )}

      {wallet.account && (
        <div className="mt-4 grid grid-cols-1 gap-3.5 sm:grid-cols-2">
          <FieldLabel>
            From (your wallet network)
            <Select
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
            </Select>
          </FieldLabel>

          <FieldLabel>
            To chain
            <Select
              value={chainIdTo}
              onChange={(e) =>
                setChainIdTo(e.target.value === "" ? "" : Number(e.target.value))
              }
            >
              {destOptions.length === 0 && (
                <option value="">no other chains</option>
              )}
              {destOptions.map((c) => (
                <option key={c.chainId} value={c.chainId}>
                  {c.name} ({c.chainId})
                </option>
              ))}
            </Select>
          </FieldLabel>

          <FieldLabel className="sm:col-span-2">
            Token address (source)
            <Input
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="0x… ERC-20 on the source chain"
              spellCheck={false}
              className="font-mono"
            />
          </FieldLabel>

          <FieldLabel className="sm:col-span-2">
            Gate address (source)
            <Input
              value={gate}
              onChange={(e) => setGate(e.target.value)}
              placeholder="0x… deployed Gate on the source chain"
              spellCheck={false}
              className="font-mono"
            />
          </FieldLabel>

          <FieldLabel>
            <span className="flex items-baseline justify-between gap-2">
              Amount
              {meta && (
                <span className="inline-flex items-baseline gap-1.5 text-[11px] normal-case tracking-normal text-muted">
                  balance {formatTokenAmount(meta.balance, meta.decimals)}{" "}
                  {meta.symbol}
                  {meta.balance > 0n && (
                    <button
                      type="button"
                      className="text-accent hover:underline"
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
            <Input
              value={amount}
              onChange={(e) => setAmount(e.target.value)}
              placeholder="1.0"
              inputMode="decimal"
              className="font-mono"
            />
          </FieldLabel>

          <FieldLabel className="sm:col-span-2">
            Receiver (destination)
            <Input
              value={receiver}
              onChange={(e) => setReceiver(e.target.value)}
              placeholder="0x… recipient on the destination chain"
              spellCheck={false}
              className="font-mono"
            />
          </FieldLabel>

          {sameChain && (
            <p className="text-[12.5px] text-warning sm:col-span-2">
              Destination equals source ({wallet.chainId}). Pick a different “To”
              chain — a same-chain transfer is never claimed by the keeper and
              would stay <b>Ready</b> forever.
            </p>
          )}

          {meta && meta.balance === 0n && (
            <p className="text-[12.5px] text-warning sm:col-span-2">
              This account holds 0 {meta.symbol} on{" "}
              {sourceChain?.name ?? `chain ${wallet.chainId}`}. Switch the “From”
              network to where your tokens are, or click <b>Mint test tokens</b>{" "}
              (local TestToken only).
            </p>
          )}

          <div className="flex flex-wrap gap-2.5 pt-1 sm:col-span-2">
            <Button onClick={handleBridge} disabled={!canSubmit} size="lg">
              {busy
                ? STAGE_LABEL[stage as keyof typeof STAGE_LABEL]
                : "Approve & bridge"}
            </Button>
            <Button
              variant="ghost"
              size="lg"
              onClick={handleMint}
              disabled={minting || busy || tokenAddr === ""}
              title={`Mint ${FAUCET_AMOUNT} test tokens to your account`}
            >
              <Coins />
              {minting ? "Minting…" : "Mint test tokens"}
            </Button>
            {sourceChain && gate.trim() !== "" && (
              <Button
                variant="ghost"
                size="lg"
                onClick={() =>
                  persistChain(sourceChain.chainId, { gate, token })
                }
                disabled={busy}
                title="Remember this gate/token for this chain"
              >
                Save as default
              </Button>
            )}
          </div>
        </div>
      )}

      {stage === "done" && result && (
        <Banner tone="success" className="mt-4">
          <strong>Bridged.</strong> Validators will sign it; it appears in the
          dashboard once observed.
          <div className="mt-1.5 text-[12.5px] text-muted">
            submissionId{" "}
            <code className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[12px]">
              {shortHex(result.submissionId, 10, 8)}
            </code>
            {" · "}tx{" "}
            <code className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[12px]">
              {shortHex(result.txHash, 8, 6)}
            </code>
            {result.approvalTxHash && (
              <>
                {" · "}approve{" "}
                <code className="rounded bg-surface-2 px-1.5 py-0.5 font-mono text-[12px]">
                  {shortHex(result.approvalTxHash, 8, 6)}
                </code>
              </>
            )}
          </div>
          {destNote && (
            <div className="mt-1.5 text-[12.5px] text-muted">{destNote}</div>
          )}
        </Banner>
      )}

      {stage === "error" && error && (
        <Banner tone="error" className="mt-4">
          {error}
        </Banner>
      )}

      <div className="mt-5 border-t border-line pt-4">
        <button
          className="inline-flex items-center gap-1 text-[13px] text-accent hover:underline"
          onClick={() => setShowSettings((s) => !s)}
        >
          {showSettings ? (
            <ChevronDown className="size-4" />
          ) : (
            <ChevronRight className="size-4" />
          )}
          Network settings
        </button>
        {showSettings && (
          <div className="mt-3">
            <p className="mb-2 text-[12px] text-muted">
              Add, remove, or edit the networks you can bridge between. Stored in
              this browser; new chains advertised by the backend are merged in
              automatically.
            </p>
            <div className="overflow-x-auto">
              <table className="w-full border-collapse text-[12.5px]">
                <thead>
                  <tr className="text-left text-[11px] uppercase tracking-wide text-muted">
                    <th className="px-1.5 py-1.5 font-medium">Chain id</th>
                    <th className="px-1.5 py-1.5 font-medium">Name</th>
                    <th className="px-1.5 py-1.5 font-medium">RPC URL</th>
                    <th className="px-1.5 py-1.5 font-medium">Gate</th>
                    <th className="px-1.5 py-1.5 font-medium">Default token</th>
                    <th className="px-1.5 py-1.5" />
                  </tr>
                </thead>
                <tbody>
                  {chains.map((c, i) => (
                    <tr key={i}>
                      <td className="px-1.5 py-1.5">
                        <Input
                          className="h-8 w-20 font-mono text-[11.5px]"
                          value={c.chainId || ""}
                          placeholder="1337"
                          inputMode="numeric"
                          spellCheck={false}
                          onChange={(e) =>
                            editChain(i, {
                              chainId: Number(e.target.value.trim()) || 0,
                            })
                          }
                        />
                      </td>
                      <td className="px-1.5 py-1.5">
                        <Input
                          className="h-8 w-28 text-[11.5px]"
                          value={c.name}
                          placeholder="My chain"
                          onChange={(e) => editChain(i, { name: e.target.value })}
                        />
                      </td>
                      <td className="px-1.5 py-1.5">
                        <Input
                          className="h-8 w-40 font-mono text-[11.5px]"
                          value={c.rpcUrl ?? ""}
                          placeholder="http://127.0.0.1:8545"
                          spellCheck={false}
                          onChange={(e) =>
                            editChain(i, { rpcUrl: e.target.value || undefined })
                          }
                        />
                      </td>
                      <td className="px-1.5 py-1.5">
                        <Input
                          className="h-8 w-36 font-mono text-[11.5px]"
                          value={c.gate ?? ""}
                          placeholder="0x…"
                          spellCheck={false}
                          onChange={(e) =>
                            editChain(i, { gate: e.target.value || undefined })
                          }
                        />
                      </td>
                      <td className="px-1.5 py-1.5">
                        <Input
                          className="h-8 w-36 font-mono text-[11.5px]"
                          value={c.token ?? ""}
                          placeholder="0x…"
                          spellCheck={false}
                          onChange={(e) =>
                            editChain(i, { token: e.target.value || undefined })
                          }
                        />
                      </td>
                      <td className="px-1.5 py-1.5">
                        <Button
                          variant="ghost"
                          size="icon"
                          onClick={() => removeChainRow(i)}
                          title="Remove this chain"
                          aria-label={`Remove ${c.name || c.chainId}`}
                        >
                          <Trash2 />
                        </Button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>

            {dupChainIds.size > 0 && (
              <p className="mt-2 text-[12px] text-warning">
                Duplicate chain id{dupChainIds.size > 1 ? "s" : ""}{" "}
                {[...dupChainIds].join(", ")} — each network must have a unique
                chain id, or selecting it won't work correctly.
              </p>
            )}

            <Button
              variant="subtle"
              size="sm"
              className="mt-3"
              onClick={addChainRow}
            >
              <Plus />
              Add chain
            </Button>
          </div>
        )}
      </div>
    </Card>
  );
}
