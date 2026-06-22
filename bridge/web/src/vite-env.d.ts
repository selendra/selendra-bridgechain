/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_GRAPHQL_URL?: string;
  /** JSON array of BridgeChain to seed the network registry. */
  readonly VITE_BRIDGE_CHAINS?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}

/** Minimal EIP-1193 provider shape injected by MetaMask et al. */
interface Eip1193Provider {
  request(args: { method: string; params?: unknown[] | object }): Promise<unknown>;
  on(event: string, handler: (...args: any[]) => void): void;
  removeListener(event: string, handler: (...args: any[]) => void): void;
  isMetaMask?: boolean;
}

interface Window {
  ethereum?: Eip1193Provider;
}
