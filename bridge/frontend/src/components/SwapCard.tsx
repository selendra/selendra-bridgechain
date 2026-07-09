import { useEffect, useMemo, useState } from "react";
import { AmountRow } from "./AmountRow";
import { RouteBox, type RouteHop } from "./RouteBox";
import { ArrowDown, Refresh, Sliders, Help, ArrowRight } from "./icons";
import { tokenBySymbol } from "../data/assets";
import type { Chain, Stats, SubmissionFilter } from "../api/types";

type Tab = "simple" | "advanced";

// Illustrative constants — the backend is a signature store, not a quote engine,
// so rate/fee/time are indicative. Chains + route activity are live.
const BALANCE = 13.089;
const FEE_RATE = 0.0515;
const SLIPPAGE = 0.927381;

interface SwapCardProps {
  chains: Chain[];
  stats: Stats | null;
  /** Jump to the Explorer, filtered to the current corridor. */
  onReview: (filter: SubmissionFilter) => void;
}

export function SwapCard({ chains, stats, onReview }: SwapCardProps) {
  const [tab, setTab] = useState<Tab>("simple");

  const [fromChain, setFromChain] = useState<number | null>(null);
  const [toChain, setToChain] = useState<number | null>(null);
  const [fromToken, setFromToken] = useState("TT");
  const [toToken, setToToken] = useState("TT");
  const [amount, setAmount] = useState("3");

  // Adopt real chains once discovered (first as source, second as destination).
  useEffect(() => {
    if (chains.length === 0) return;
    setFromChain((c) => c ?? chains[0].chainId);
    setToChain((c) => c ?? (chains[1]?.chainId ?? chains[0].chainId));
  }, [chains]);

  const rate = useMemo(() => {
    const f = tokenBySymbol(fromToken).usd;
    const t = tokenBySymbol(toToken).usd;
    return (f / t) * SLIPPAGE;
  }, [fromToken, toToken]);

  const parsedAmount = parseFloat(amount) || 0;
  const receiveStr = parsedAmount ? (parsedAmount * rate).toFixed(3) : "0";
  const feeAmount = parsedAmount * FEE_RATE;

  const hops: RouteHop[] = [
    { via: "Wormhole", token: "RSOL" },
    { via: "celer.network", token: toToken },
  ];

  // Live: how many transfers the backend has seen on the selected corridor.
  const routeCount = useMemo(() => {
    if (!stats || fromChain == null || toChain == null) return null;
    return stats.routes
      .filter((r) => r.chainIdFrom === fromChain && r.chainIdTo === toChain)
      .reduce((n, r) => n + r.count, 0);
  }, [stats, fromChain, toChain]);

  const swapDirection = () => {
    setFromChain(toChain);
    setToChain(fromChain);
    setFromToken(toToken);
    setToToken(fromToken);
  };

  const ready = fromChain != null && toChain != null && parsedAmount > 0;

  return (
    <section className="card">
      <div className="card__head">
        <div className="tabs">
          <button className={`tab${tab === "simple" ? " tab--active" : ""}`} onClick={() => setTab("simple")}>
            Simple
          </button>
          <button className={`tab${tab === "advanced" ? " tab--active" : ""}`} onClick={() => setTab("advanced")}>
            Advanced
          </button>
        </div>
        <div className="card__tools">
          <button type="button" className="icon-btn" aria-label="Refresh quote">
            <Refresh size={18} />
          </button>
          <button type="button" className="icon-btn" aria-label="Swap settings">
            <Sliders size={18} />
          </button>
        </div>
      </div>

      <div className="swap-box">
        <AmountRow
          label="from"
          chains={chains}
          chainId={fromChain ?? 0}
          token={fromToken}
          amount={amount}
          balance={BALANCE}
          editable
          onChainChange={setFromChain}
          onTokenChange={setFromToken}
          onAmountChange={setAmount}
          onMax={() => setAmount(String(BALANCE))}
        />

        <button type="button" className="swap-arrow" aria-label="Swap direction" onClick={swapDirection}>
          <ArrowDown size={18} />
        </button>

        <AmountRow
          label="to"
          chains={chains}
          chainId={toChain ?? 0}
          token={toToken}
          amount={receiveStr}
          balance={BALANCE}
          editable={false}
          onChainChange={setToChain}
          onTokenChange={setToToken}
        />
      </div>

      <RouteBox fromToken={fromToken} hops={hops} />

      <dl className="summary">
        <div className="summary__row">
          <dt>
            Rate <Help size={14} />
          </dt>
          <dd>
            1 {fromToken} ≈ {rate.toFixed(6)} {toToken}
          </dd>
        </div>
        <div className="summary__row">
          <dt>
            Transaction fee <Help size={14} />
          </dt>
          <dd>
            {feeAmount.toFixed(4)} {fromToken}
          </dd>
        </div>
        <div className="summary__row">
          <dt>
            Estimated transaction time <Help size={14} />
          </dt>
          <dd>~ 15mins</dd>
        </div>
        {routeCount !== null && (
          <div className="summary__row">
            <dt>
              Transfers on this route <Help size={14} />
            </dt>
            <dd>
              <button
                type="button"
                className="summary__link"
                onClick={() => onReview({ chainIdFrom: fromChain!, chainIdTo: toChain! })}
              >
                {routeCount} live <ArrowRight size={13} />
              </button>
            </dd>
          </div>
        )}
      </dl>

      <button
        type="button"
        className="review-btn"
        disabled={!ready}
        onClick={() => onReview({ chainIdFrom: fromChain!, chainIdTo: toChain! })}
      >
        Review Transaction
      </button>
    </section>
  );
}
