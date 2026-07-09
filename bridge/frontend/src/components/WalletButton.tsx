import { useEffect, useRef, useState } from "react";
import { ChevronDown, Glyph } from "./icons";
import { chainViz, shortHex } from "../data/assets";
import { useWallet } from "../wallet/useWallet";

const KNOWN_CHAIN_NAMES: Record<number, string> = {
  1: "Ethereum",
  8453: "Base",
  42161: "Arbitrum",
  10: "Optimism",
  137: "Polygon",
  1337: "Anvil A",
  1338: "Anvil B",
  1339: "Anvil C",
  31337: "Localhost",
};

export function WalletButton() {
  const wallet = useWallet();
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  // Not connected → a Connect button (works for MetaMask / any injected wallet).
  if (!wallet.address) {
    return (
      <div className="wallet" ref={ref}>
        <button
          type="button"
          className="connect-btn"
          onClick={() =>
            wallet.available
              ? wallet.connect()
              : window.open("https://metamask.io/download/", "_blank", "noopener")
          }
          disabled={wallet.connecting}
          title={wallet.available ? "Connect MetaMask" : "No EVM wallet detected — get MetaMask"}
        >
          {wallet.connecting
            ? "Connecting… check MetaMask"
            : wallet.available
              ? "Connect Wallet"
              : "Install MetaMask"}
        </button>
        {wallet.error && <span className="wallet__error">{wallet.error}</span>}
      </div>
    );
  }

  const chainId = wallet.chainId ?? 0;
  const viz = chainViz(chainId, KNOWN_CHAIN_NAMES[chainId]);
  const netName = KNOWN_CHAIN_NAMES[chainId] ?? (wallet.chainId ? `Chain ${chainId}` : "Unknown");

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(wallet.address!);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      /* clipboard unavailable */
    }
  };

  return (
    <div className="wallet" ref={ref}>
      <button type="button" className="wallet-chip wallet-chip--live" onClick={() => setOpen((v) => !v)}>
        <Glyph gradient={viz.gradient} size={20} />
        <span>{shortHex(wallet.address, 6, 4)}</span>
        <ChevronDown size={15} />
      </button>
      {open && (
        <div className="wallet__menu">
          {wallet.walletName && <div className="wallet__connected-with">Connected with {wallet.walletName}</div>}
          <div className="wallet__net">
            <Glyph gradient={viz.gradient} size={18} />
            <span>{netName}</span>
            <span className="wallet__net-id">#{chainId}</span>
          </div>
          <div className="wallet__addr">{wallet.address}</div>
          <button type="button" className="wallet__action" onClick={copy}>
            {copied ? "Copied ✓" : "Copy address"}
          </button>
          <button
            type="button"
            className="wallet__action wallet__action--danger"
            onClick={() => {
              wallet.disconnect();
              setOpen(false);
            }}
          >
            Disconnect
          </button>
        </div>
      )}
    </div>
  );
}
