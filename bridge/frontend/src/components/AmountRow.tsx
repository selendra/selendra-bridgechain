import { Dropdown, type DropdownOption } from "./Dropdown";
import { Glyph } from "./icons";
import { chainViz, TOKENS, tokenBySymbol } from "../data/assets";
import type { Chain } from "../api/types";

interface AmountRowProps {
  label: "from" | "to";
  /** Chains discovered from the backend. */
  chains: Chain[];
  chainId: number;
  token: string;
  amount: string;
  balance: number;
  /** Editable input (from) vs read-only computed output (to). */
  editable: boolean;
  onChainChange: (id: number) => void;
  onTokenChange: (symbol: string) => void;
  onAmountChange?: (value: string) => void;
  onMax?: () => void;
}

export function AmountRow(props: AmountRowProps) {
  const { chains, chainId, token, amount, balance, editable } = props;
  const tk = tokenBySymbol(token);

  const chainOptions: DropdownOption[] = chains.map((c) => {
    const v = chainViz(c.chainId, c.name);
    return { value: String(c.chainId), label: c.name, glyph: <Glyph gradient={v.gradient} size={22} /> };
  });

  // No backend token registry — offer the generic set on every chain.
  const tokenOptions: DropdownOption[] = TOKENS.map((t) => ({
    value: t.symbol,
    label: t.symbol,
    sub: t.name,
    glyph: <Glyph gradient={t.gradient} size={22} />,
  }));

  const usdValue = (parseFloat(amount) || 0) * tk.usd;

  return (
    <div className="amount-row">
      <div className="amount-row__top">
        <Dropdown
          variant="chain"
          value={String(chainId)}
          options={chainOptions}
          onChange={(v) => props.onChainChange(Number(v))}
        />
        <Dropdown variant="token" value={token} options={tokenOptions} onChange={props.onTokenChange} />
      </div>

      <div className="amount-row__bottom">
        <div className="amount-row__value">
          {editable ? (
            <input
              className="amount-row__input"
              inputMode="decimal"
              value={amount}
              placeholder="0"
              // size to content so the token symbol sits right after the number
              style={{ width: `${Math.max(1, amount.length)}ch` }}
              onChange={(e) => {
                const v = e.target.value;
                if (v === "" || /^\d*\.?\d*$/.test(v)) props.onAmountChange?.(v);
              }}
            />
          ) : (
            <span className="amount-row__input amount-row__input--readonly">{amount || "0"}</span>
          )}
          <span className="amount-row__symbol">{token}</span>
          {!editable && usdValue > 0 && (
            <span className="amount-row__usd">~${usdValue.toLocaleString(undefined, { maximumFractionDigits: 2 })}</span>
          )}
        </div>

        <div className="amount-row__meta">
          {editable && (
            <button type="button" className="max-btn" onClick={props.onMax}>
              Max
            </button>
          )}
          <span className="amount-row__bal">Bal: {balance.toLocaleString(undefined, { maximumFractionDigits: 3 })}</span>
        </div>
      </div>
    </div>
  );
}
