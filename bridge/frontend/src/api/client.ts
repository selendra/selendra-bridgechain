// Tiny GraphQL client. Talks to the backend same-origin via the Vite dev proxy
// (/graphql and /health -> 127.0.0.1:8088, see vite.config.ts). In a production
// build, serve the SPA behind the same origin as graphql-api, or set VITE_API.

import type {
  Chain,
  Stats,
  Submission,
  SubmissionFilter,
  SwapPoolInfo,
} from "./types";

const API_BASE = import.meta.env.VITE_API ?? "";

export async function gql<T>(query: string, variables?: Record<string, unknown>): Promise<T> {
  const res = await fetch(`${API_BASE}/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query, variables }),
  });
  if (!res.ok) throw new Error(`backend HTTP ${res.status}`);
  const json = (await res.json()) as { data?: T; errors?: { message: string }[] };
  if (json.errors && json.errors.length) throw new Error(json.errors[0].message);
  if (!json.data) throw new Error("empty GraphQL response");
  return json.data;
}

export async function health(): Promise<boolean> {
  try {
    const res = await fetch(`${API_BASE}/health`);
    return res.ok;
  } catch {
    return false;
  }
}

// --- bridge / signature store -------------------------------------------

export function fetchChains(): Promise<Chain[]> {
  return gql<{ chains: Chain[] }>(`{ chains { chainId name rpcUrl gate token router } }`).then((d) => d.chains);
}

export function fetchStats(): Promise<Stats> {
  return gql<{ stats: Stats }>(
    `{ stats { total signed ready threshold routes { chainIdFrom chainIdTo count } } }`
  ).then((d) => d.stats);
}

export function fetchSubmissions(filter?: SubmissionFilter): Promise<Submission[]> {
  return gql<{ submissions: Submission[] }>(
    `query Subs($filter: SubmissionFilter) {
       submissions(filter: $filter) {
         submissionId debridgeId amount chainIdFrom chainIdTo nonce receiver
         nativeSender signatureCount meetsThreshold status
         signatures { signer }
       }
     }`,
    { filter }
  ).then((d) => d.submissions);
}

export function fetchSubmission(submissionId: string): Promise<Submission | null> {
  return gql<{ submission: Submission | null }>(
    `query Sub($id: String!) {
       submission(submissionId: $id) {
         submissionId debridgeId amount chainIdFrom chainIdTo nonce receiver
         nativeSender autoParams signatureCount meetsThreshold status
         signatures { signer signature }
       }
     }`,
    { id: submissionId }
  ).then((d) => d.submission);
}

// --- swap (same-chain SwapPool) -----------------------------------------
// chainId is inlined as an integer literal (it's a validated number, not free
// text) to sidestep the u64 GraphQL scalar name; string args use variables.

export function fetchSwapPool(chainId: number): Promise<SwapPoolInfo | null> {
  return gql<{ swapPool: SwapPoolInfo | null }>(
    `{ swapPool(chainId: ${chainId}) {
         chainId address stable
         tokens { token symbol decimals price reserve maxSwapUsd isStable }
       } }`
  ).then((d) => d.swapPool);
}

export function fetchSwapQuote(
  chainId: number,
  tokenIn: string,
  tokenOut: string,
  amountIn: string
): Promise<string | null> {
  return gql<{ swapQuote: string | null }>(
    `query Q($in: String!, $out: String!, $amt: String!) {
       swapQuote(chainId: ${chainId}, tokenIn: $in, tokenOut: $out, amountIn: $amt)
     }`,
    { in: tokenIn, out: tokenOut, amt: amountIn }
  ).then((d) => d.swapQuote);
}
