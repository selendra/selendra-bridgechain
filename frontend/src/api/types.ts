// Wire types mirroring the graphql-api schema (crates/graphql-api/src/schema.rs).

export type SubmissionStatus = "PENDING" | "READY" | "EXECUTED" | "CANCELLED" | "UNKNOWN";

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

// --- transaction history (database-backed, requires graphql-api --db-url) -

/** The swap intent (and destination outcome) of a swap-then-bridge transfer. */
export interface SwapIntent {
  tokenIn: string;
  amountIn: string;
  stableOut: string;
  finalToken: string;
  finalReceiver: string;
  finalizeTx: string | null;
  finalizeAmountOut: string | null;
  finalizeFallback: boolean | null;
  finalizedAt: string | null;
}

/**
 * A row from the DB-backed `history` query: every bridge transfer the indexer
 * has observed, including ones stuck at zero signatures (which `submissions`
 * can never show — that view only exists once a validator has signed).
 */
export interface HistoryEntry {
  submissionId: string;
  debridgeId: string;
  amount: string;
  chainIdFrom: number;
  chainIdTo: number;
  nonce: number;
  receiver: string;
  status: string; // 'signed' | 'claimed' (DB lifecycle, not the live-checked SubmissionStatus)
  claimTx: string | null;
  signatureCount: number;
  createdAt: string;
  updatedAt: string;
  /** True once this transfer has entered the refund lifecycle at all. */
  stuck: boolean;
  /**
   * 'none' | 'eligible' | 'cancelled' | 'refunded'.
   *
   * 'cancelled' means the transfer was burned on the destination chain so the
   * source could repay it — the funds did NOT arrive; they went back.
   */
  refundStatus: string;
  /** Source-chain Gate.refund tx hash. */
  refundTx: string | null;
  /** Destination-chain Gate.cancel tx hash. */
  cancelTx: string | null;
  /** The source-chain ERC-20 that was locked. */
  token: string | null;
  /** Validators attesting the destination burn. */
  cancelSignatureCount: number;
  /** Validators attesting the source payout. */
  refundSignatureCount: number;
  swapIntent: SwapIntent | null;
}

export interface HistoryFilter {
  chainIdFrom?: number;
  chainIdTo?: number;
  stuckOnly?: boolean;
  submissionId?: string;
}

/** A completed same-chain swap (SwapPool.Swapped), mirrored by the indexer. */
export interface SwapHistoryEntry {
  chainId: number;
  txHash: string;
  sender: string;
  receiver: string;
  tokenIn: string;
  tokenOut: string;
  amountIn: string;
  amountOut: string;
  blockNumber: number;
  createdAt: string;
}
