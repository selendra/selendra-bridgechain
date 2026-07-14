// Real EVM wallet connection — EIP-1193 + EIP-6963 provider discovery, with
// race-proof detection.
//
// The subtlety: MetaMask injects `window.ethereum` and/or announces via EIP-6963
// asynchronously, often AFTER our React tree first renders. A one-shot read of
// `window.ethereum` therefore misses it and the UI wrongly shows "Install
// MetaMask". So we (a) keep an EIP-6963 announce listener mounted, (b) also
// listen for MetaMask's legacy `ethereum#initialized` event, and (c) poll
// `window.ethereum` briefly — any of which flips detection on reactively.

import { useCallback, useEffect, useMemo, useState } from "react";

type Handler = (...args: unknown[]) => void;

interface Eip1193Provider {
  request(args: { method: string; params?: unknown[] }): Promise<unknown>;
  on(event: string, handler: Handler): void;
  removeListener(event: string, handler: Handler): void;
  isMetaMask?: boolean;
}

interface Eip6963ProviderInfo {
  uuid: string;
  name: string;
  icon: string;
  rdns: string;
}
interface Eip6963ProviderDetail {
  info: Eip6963ProviderInfo;
  provider: Eip1193Provider;
}

declare global {
  interface Window {
    ethereum?: Eip1193Provider;
  }
}

export interface WalletState {
  available: boolean;
  walletName: string | null;
  address: string | null;
  chainId: number | null;
  connecting: boolean;
  error: string | null;
  connect: () => Promise<void>;
  disconnect: () => void;
  switchChain: (chainId: number) => Promise<void>;
  /** Raw EIP-1193 request against the active provider (for reads + txs). */
  request: (args: { method: string; params?: unknown[] }) => Promise<unknown>;
}

export function useWallet(): WalletState {
  const [details, setDetails] = useState<Eip6963ProviderDetail[]>([]);
  const [legacy, setLegacy] = useState<Eip1193Provider | null>(null);
  const [address, setAddress] = useState<string | null>(null);
  const [chainId, setChainId] = useState<number | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // --- EIP-6963 discovery: catch every wallet that announces itself.
  useEffect(() => {
    const found = new Map<string, Eip6963ProviderDetail>();
    const onAnnounce = (e: Event) => {
      const d = (e as CustomEvent<Eip6963ProviderDetail>).detail;
      if (d?.info?.uuid && d.provider) {
        found.set(d.info.uuid, d);
        setDetails([...found.values()]);
      }
    };
    window.addEventListener("eip6963:announceProvider", onAnnounce as EventListener);
    window.dispatchEvent(new Event("eip6963:requestProvider"));
    return () => window.removeEventListener("eip6963:announceProvider", onAnnounce as EventListener);
  }, []);

  // --- Legacy window.ethereum: it may appear late, so poll + listen for the
  //     `ethereum#initialized` event MetaMask fires when it's ready.
  useEffect(() => {
    let done = false;
    let tries = 0;
    const capture = () => {
      if (done) return true;
      if (window.ethereum) {
        setLegacy(window.ethereum);
        done = true;
        return true;
      }
      return false;
    };
    if (capture()) return;
    const onInit = () => capture();
    window.addEventListener("ethereum#initialized", onInit, { once: true });
    const id = window.setInterval(() => {
      if (capture() || ++tries > 25) window.clearInterval(id); // ~5s max
    }, 200);
    return () => {
      done = true;
      window.clearInterval(id);
      window.removeEventListener("ethereum#initialized", onInit);
    };
  }, []);

  // Prefer MetaMask (via 6963), else any announced wallet, else legacy injection.
  const active = useMemo<{ provider: Eip1193Provider; name: string } | null>(() => {
    const mm = details.find((d) => d.info.rdns === "io.metamask");
    if (mm) return { provider: mm.provider, name: mm.info.name };
    if (details[0]) return { provider: details[0].provider, name: details[0].info.name };
    if (legacy) return { provider: legacy, name: legacy.isMetaMask ? "MetaMask" : "Injected Wallet" };
    return null;
  }, [details, legacy]);

  const provider = active?.provider ?? null;

  // --- Re-hydrate session + subscribe to account/chain changes.
  useEffect(() => {
    if (!provider) return;
    let alive = true;

    (async () => {
      try {
        const accts = (await provider.request({ method: "eth_accounts" })) as string[];
        if (alive && accts.length) setAddress(accts[0]);
        const cid = (await provider.request({ method: "eth_chainId" })) as string;
        if (alive) setChainId(parseInt(cid, 16));
      } catch {
        /* not connected yet */
      }
    })();

    const onAccounts = (...args: unknown[]) => {
      const accts = (args[0] as string[]) ?? [];
      setAddress(accts.length ? accts[0] : null);
    };
    const onChain = (...args: unknown[]) => setChainId(parseInt(args[0] as string, 16));
    provider.on("accountsChanged", onAccounts);
    provider.on("chainChanged", onChain);
    return () => {
      alive = false;
      provider.removeListener("accountsChanged", onAccounts);
      provider.removeListener("chainChanged", onChain);
    };
  }, [provider]);

  const connect = useCallback(async () => {
    if (!provider) {
      setError("No EVM wallet detected. Install MetaMask to connect.");
      return;
    }
    setConnecting(true);
    setError(null);
    try {
      const accts = (await provider.request({ method: "eth_requestAccounts" })) as string[];
      setAddress(accts[0] ?? null);
      const cid = (await provider.request({ method: "eth_chainId" })) as string;
      setChainId(parseInt(cid, 16));
    } catch (e) {
      const code = (e as { code?: number })?.code;
      const msg = e instanceof Error ? e.message : String(e);
      if (code === 4001 || /reject|denied/i.test(msg)) {
        setError("Connection rejected");
      } else if (code === -32002 || /already processing|already pending/i.test(msg)) {
        setError("Open the MetaMask extension to approve the pending request.");
      } else {
        setError(msg);
      }
    } finally {
      setConnecting(false);
    }
  }, [provider]);

  const disconnect = useCallback(() => {
    setAddress(null);
    setError(null);
  }, []);

  const switchChain = useCallback(
    async (target: number) => {
      if (!provider) return;
      try {
        await provider.request({
          method: "wallet_switchEthereumChain",
          params: [{ chainId: "0x" + target.toString(16) }],
        });
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
      }
    },
    [provider]
  );

  const request = useCallback(
    (args: { method: string; params?: unknown[] }) => {
      if (!provider) return Promise.reject(new Error("No wallet connected"));
      return provider.request(args);
    },
    [provider]
  );

  return {
    available: !!provider,
    walletName: active?.name ?? null,
    address,
    chainId,
    connecting,
    error,
    connect,
    disconnect,
    switchChain,
    request,
  };
}
