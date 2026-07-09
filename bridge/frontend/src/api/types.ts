// Wire types mirroring the graphql-api schema (crates/graphql-api/src/schema.rs).

export type SubmissionStatus = "PENDING" | "READY" | "EXECUTED" | "UNKNOWN";

export interface Chain {
  chainId: number;
  name: string;
  rpcUrl: string | null;
  gate: string | null;
  token: string | null;
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
