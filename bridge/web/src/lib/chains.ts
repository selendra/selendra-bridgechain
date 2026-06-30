// Network registry for the bridge UI. The Gate/token addresses are deployed
// fresh by the local scripts (anvil redeploys each run), so they aren't baked
// in — they're held in localStorage and editable from the UI. Defaults can be
// seeded at build time via VITE_BRIDGE_CHAINS (a JSON array of BridgeChain).

export interface BridgeChain {
  chainId: number;
  name: string;
  /** Read-only RPC (used to read token decimals when this chain isn't the
   *  wallet's active chain). Optional — falls back to the wallet provider. */
  rpcUrl?: string;
  /** Deployed Gate contract on this chain. */
  gate?: string;
  /** Default ERC-20 to bridge from this chain (prefilled, still editable). */
  token?: string;
}

const STORAGE_KEY = "bridge.chains.v1";

// The local three-anvil topology the scripts spin up (see scripts/anytoany.sh).
const DEFAULTS: BridgeChain[] = [
  { chainId: 1337, name: "Anvil A", rpcUrl: "http://127.0.0.1:8545" },
  { chainId: 1338, name: "Anvil B", rpcUrl: "http://127.0.0.1:8546" },
  { chainId: 1339, name: "Anvil C", rpcUrl: "http://127.0.0.1:8547" },
];

function seedDefaults(): BridgeChain[] {
  const env = import.meta.env.VITE_BRIDGE_CHAINS;
  if (env) {
    try {
      const parsed = JSON.parse(env) as BridgeChain[];
      if (Array.isArray(parsed) && parsed.length > 0) return parsed;
    } catch {
      // fall through to built-in defaults
    }
  }
  return DEFAULTS;
}

export function loadChains(): BridgeChain[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed = JSON.parse(raw) as BridgeChain[];
      if (Array.isArray(parsed) && parsed.length > 0) return parsed;
    }
  } catch {
    // ignore malformed storage
  }
  return seedDefaults();
}

export function saveChains(chains: BridgeChain[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(chains));
  } catch {
    // storage may be unavailable (private mode); non-fatal
  }
}

export function findChain(chains: BridgeChain[], chainId: number | null): BridgeChain | undefined {
  if (chainId == null) return undefined;
  return chains.find((c) => c.chainId === chainId);
}

/** Backend-advertised chain (nullable fields) -> local BridgeChain (optionals). */
export function fromRemote(c: {
  chainId: number;
  name: string;
  rpcUrl: string | null;
  gate: string | null;
  token: string | null;
}): BridgeChain {
  return {
    chainId: c.chainId,
    name: c.name,
    rpcUrl: c.rpcUrl ?? undefined,
    gate: c.gate ?? undefined,
    token: c.token ?? undefined,
  };
}

/**
 * Non-destructively fold a backend-served registry into the local one: append
 * any chain the user hasn't seen (by chainId), but never clobber local edits to
 * gate/token/rpc the user has made. Returns the SAME `local` reference when
 * there's nothing to add, so callers can skip a state update cheaply.
 */
export function mergeChains(local: BridgeChain[], remote: BridgeChain[]): BridgeChain[] {
  const have = new Set(local.map((c) => c.chainId));
  const additions = remote.filter((c) => !have.has(c.chainId));
  return additions.length === 0 ? local : [...local, ...additions];
}

/** A blank chain row for the "add chain" action (chainId 0 = "fill me in"). */
export function blankChain(): BridgeChain {
  return { chainId: 0, name: "" };
}
