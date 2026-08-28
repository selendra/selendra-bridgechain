// Solana wallet connection (Phantom and anything that speaks its provider API).
//
// Mirrors `useWallet`'s shape so `SwapView` can hold one wallet or the other
// without branching on more than "which chain is this pool on".
//
// Deliberately narrow: connect, disconnect, and sign+send ONE prepared message.
// The app never asks a wallet to build a transaction, because building it is
// exactly where the destination account gets decided — see `wallet/solana.ts`.

import { useCallback, useEffect, useMemo, useState } from "react";
import { b58encode } from "./solana";

interface SolanaProvider {
  isPhantom?: boolean;
  publicKey?: { toString(): string } | null;
  connect(opts?: { onlyIfTrusted?: boolean }): Promise<{ publicKey: { toString(): string } }>;
  disconnect(): Promise<void>;
  request(args: { method: string; params?: unknown }): Promise<unknown>;
  on?(event: string, handler: (...args: unknown[]) => void): void;
  removeListener?(event: string, handler: (...args: unknown[]) => void): void;
}

declare global {
  interface Window {
    solana?: SolanaProvider;
    phantom?: { solana?: SolanaProvider };
  }
}

export interface SolanaWalletState {
  available: boolean;
  walletName: string | null;
  address: string | null;
  connecting: boolean;
  error: string | null;
  connect: () => Promise<void>;
  disconnect: () => void;
  /** Sign and send a serialized legacy message; resolves to the signature. */
  signAndSend: (message: Uint8Array) => Promise<string>;
}

function findProvider(): SolanaProvider | null {
  return window.phantom?.solana ?? window.solana ?? null;
}

export function useSolanaWallet(): SolanaWalletState {
  const [provider, setProvider] = useState<SolanaProvider | null>(null);
  const [address, setAddress] = useState<string | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // The extension may inject after our first render, so poll briefly rather
  // than reading `window` once and declaring the wallet missing.
  useEffect(() => {
    let tries = 0;
    const found = findProvider();
    if (found) {
      setProvider(found);
      return;
    }
    const id = setInterval(() => {
      const p = findProvider();
      if (p || ++tries > 20) {
        if (p) setProvider(p);
        clearInterval(id);
      }
    }, 150);
    return () => clearInterval(id);
  }, []);

  // Reconnect silently if the site is already trusted, and follow account
  // changes — a wallet switched under the app must not keep the old address.
  useEffect(() => {
    if (!provider) return;
    provider.connect({ onlyIfTrusted: true })
      .then((r) => setAddress(r.publicKey.toString()))
      .catch(() => {});
    const onAccountChanged = (...args: unknown[]) => {
      const key = args[0] as { toString(): string } | null;
      setAddress(key ? key.toString() : null);
    };
    provider.on?.("accountChanged", onAccountChanged);
    return () => provider.removeListener?.("accountChanged", onAccountChanged);
  }, [provider]);

  const connect = useCallback(async () => {
    if (!provider) {
      setError("No Solana wallet found — install Phantom");
      return;
    }
    setConnecting(true);
    setError(null);
    try {
      const r = await provider.connect();
      setAddress(r.publicKey.toString());
    } catch (e) {
      setError(e instanceof Error ? e.message : "Connection rejected");
    } finally {
      setConnecting(false);
    }
  }, [provider]);

  const disconnect = useCallback(() => {
    provider?.disconnect().catch(() => {});
    setAddress(null);
  }, [provider]);

  const signAndSend = useCallback(
    async (message: Uint8Array): Promise<string> => {
      if (!provider) throw new Error("No Solana wallet connected");
      // The wallet is handed a base58 MESSAGE and returns a signature; it adds
      // the signature and broadcasts through its own RPC, so the app needs no
      // endpoint of its own to send.
      const res = (await provider.request({
        method: "signAndSendTransaction",
        params: { message: b58encode(message) },
      })) as { signature?: string } | string;
      const sig = typeof res === "string" ? res : res?.signature;
      if (!sig) throw new Error("Wallet returned no signature");
      return sig;
    },
    [provider]
  );

  return useMemo(
    () => ({
      available: provider != null,
      walletName: provider ? (provider.isPhantom ? "Phantom" : "Solana wallet") : null,
      address,
      connecting,
      error,
      connect,
      disconnect,
      signAndSend,
    }),
    [provider, address, connecting, error, connect, disconnect, signAndSend]
  );
}
