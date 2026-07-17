// Wire types mirroring the graphql-api schema (crates/graphql-api/src/schema.rs).

export type SubmissionStatus = "PENDING" | "READY" | "EXECUTED" | "UNKNOWN";

export interface Chain {
  chainId: number;
  name: string;
  rpcUrl: string | null;
  gate: string | null;
  token: string | null;
  /** Deployed SwapRouter on this chain, for cross-chain swap. Null if unset. */
  router: string | null;
}

export interface SignatureRef {
  signer: string;
  signature?: string;
}

export interface Submission {
  submissionId: string;
  debridgeId?: string;
  amount: string; // uint256 as decimal string (wei)
  chainIdFrom: number;
  chainIdTo: number;
  nonce: number;
  receiver: string;
  nativeSender?: string;
  autoParams?: string;
  signatureCount: number;
  meetsThreshold: boolean | null;
  status: SubmissionStatus;
  signatures: SignatureRef[];
}

export interface RouteCount {
  chainIdFrom: number;
  chainIdTo: number;
  count: number;
}

export interface Stats {
  total: number;
  signed: number;
  ready: number;
  threshold: number | null;
  routes: RouteCount[];
}

export interface SubmissionFilter {
  chainIdFrom?: number;
  chainIdTo?: number;
  minSignatures?: number;
  ready?: boolean;
}

// --- swap (same-chain SwapPool read view) --------------------------------

export interface PoolToken {
  token: string; // 0x address (lowercase)
  symbol: string;
  decimals: number;
  price: string; // 1e18-scaled USD, decimal string
  reserve: string; // base units, decimal string — this is the swap lock
  maxSwapUsd: string; // reserve*price/10^decimals, 1e18-scaled, decimal string
  isStable: boolean;
}

export interface SwapPoolInfo {
  chainId: number;
  address: string; // SwapPool contract — approve/swap target
  stable: string;
  tokens: PoolToken[];
}
