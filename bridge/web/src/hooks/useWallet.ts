import { useCallback, useEffect, useState } from "react";
import { BrowserProvider } from "ethers";

export interface WalletState {
  /** True once an injected EIP-1193 provider (MetaMask) is detected. */
  available: boolean;
  account: string | null;
  chainId: number | null;
  connecting: boolean;
  error: string | null;
  provider: BrowserProvider | null;
  connect: () => Promise<void>;
  switchChain: (chainId: number) => Promise<void>;
}

function parseChainId(hex: unknown): number | null {
  if (typeof hex === "string") return Number.parseInt(hex, 16);
  if (typeof hex === "number") return hex;
  return null;
}

export function useWallet(): WalletState {
  const eth = typeof window !== "undefined" ? window.ethereum : undefined;
  const available = !!eth;

  const [account, setAccount] = useState<string | null>(null);
  const [chainId, setChainId] = useState<number | null>(null);
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const provider = available ? new BrowserProvider(eth!) : null;

  const refreshChain = useCallback(async () => {
    if (!eth) return;
    try {
      const id = (await eth.request({ method: "eth_chainId" })) as string;
      setChainId(parseChainId(id));
    } catch {
      /* ignore */
    }
  }, [eth]);

  const connect = useCallback(async () => {
    if (!eth) {
      setError("No Ethereum wallet found. Install MetaMask to bridge.");
      return;
    }
    setConnecting(true);
    setError(null);
    try {
      const accounts = (await eth.request({ method: "eth_requestAccounts" })) as string[];
      setAccount(accounts[0] ?? null);
      await refreshChain();
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to connect wallet.");
    } finally {
      setConnecting(false);
    }
  }, [eth, refreshChain]);

  const switchChain = useCallback(
    async (target: number) => {
      if (!eth) return;
      const hexId = "0x" + target.toString(16);
      try {
        await eth.request({
          method: "wallet_switchEthereumChain",
          params: [{ chainId: hexId }],
        });
      } catch (e) {
        // 4902 = chain not added to the wallet.
        const code = (e as { code?: number })?.code;
        if (code === 4902) {
          setError(`Chain ${target} isn't in your wallet — add it first (RPC + chain id).`);
        } else {
          setError(e instanceof Error ? e.message : "Failed to switch chain.");
        }
      }
    },
    [eth],
  );

  // Wire up account/chain change listeners and pick up an already-authorized
  // account on mount (without prompting).
  useEffect(() => {
    if (!eth) return;
    eth
      .request({ method: "eth_accounts" })
      .then((accs) => setAccount((accs as string[])[0] ?? null))
      .catch(() => {});
    refreshChain();

    const onAccounts = (accs: string[]) => setAccount(accs[0] ?? null);
    const onChain = (id: string) => setChainId(parseChainId(id));
    eth.on("accountsChanged", onAccounts);
    eth.on("chainChanged", onChain);
    return () => {
      eth.removeListener("accountsChanged", onAccounts);
      eth.removeListener("chainChanged", onChain);
    };
  }, [eth, refreshChain]);

  return {
    available,
    account,
    chainId,
    connecting,
    error,
    provider,
    connect,
    switchChain,
  };
}
