// Tiny typed GraphQL client over `fetch` — no Apollo/urql, the schema is small
// and we only ever issue three queries and one mutation. Types here mirror
// graphql-api/src/schema.rs exactly.

const ENDPOINT = import.meta.env.VITE_GRAPHQL_URL ?? "/graphql";

export class GqlError extends Error {}

export async function gql<T>(
  query: string,
  variables?: Record<string, unknown>,
  signal?: AbortSignal,
): Promise<T> {
  let res: Response;
  try {
    res = await fetch(ENDPOINT, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ query, variables }),
      signal,
    });
  } catch (e) {
    if (e instanceof DOMException && e.name === "AbortError") throw e;
    throw new GqlError(
      `cannot reach the GraphQL API at ${ENDPOINT} — is graphql-api running?`,
    );
  }
  if (!res.ok) throw new GqlError(`GraphQL API returned HTTP ${res.status}`);
  const body = (await res.json()) as {
    data?: T;
    errors?: Array<{ message: string }>;
  };
  if (body.errors?.length) {
    throw new GqlError(body.errors.map((e) => e.message).join("; "));
  }
  if (body.data == null) throw new GqlError("GraphQL response had no data");
  return body.data;
}

// ---- Schema types -------------------------------------------------------

export type SubmissionStatus = "PENDING" | "READY" | "EXECUTED" | "UNKNOWN";

export interface Signature {
  signer: string;
  signature: string;
}

export interface Submission {
  submissionId: string;
  debridgeId: string;
  amount: string;
  chainIdFrom: number;
  chainIdTo: number;
  nonce: number;
  receiver: string;
  autoParams: string;
  nativeSender: string;
  signatures: Signature[];
  signatureCount: number;
  meetsThreshold: boolean | null;
  status: SubmissionStatus;
  executed: boolean | null;
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
  chainIdFrom?: number | null;
  chainIdTo?: number | null;
  minSignatures?: number | null;
  ready?: boolean | null;
}

// ---- Queries ------------------------------------------------------------

// Shared field set so the list and the by-id lookup stay in sync.
const SUBMISSION_FIELDS = `
  submissionId
  debridgeId
  amount
  chainIdFrom
  chainIdTo
  nonce
  receiver
  autoParams
  nativeSender
  signatureCount
  meetsThreshold
  status
  executed
  signatures { signer signature }
`;

const STATS_QUERY = `
  query Stats {
    stats {
      total
      signed
      ready
      threshold
      routes { chainIdFrom chainIdTo count }
    }
  }
`;

const SUBMISSIONS_QUERY = `
  query Submissions($filter: SubmissionFilter) {
    submissions(filter: $filter) { ${SUBMISSION_FIELDS} }
  }
`;

const SUBMISSION_QUERY = `
  query Submission($id: String!) {
    submission(submissionId: $id) { ${SUBMISSION_FIELDS} }
  }
`;

export function fetchStats(signal?: AbortSignal): Promise<Stats> {
  return gql<{ stats: Stats }>(STATS_QUERY, undefined, signal).then((d) => d.stats);
}

export function fetchSubmissions(
  filter: SubmissionFilter | undefined,
  signal?: AbortSignal,
): Promise<Submission[]> {
  // Drop empty fields so the server sees a clean filter (or none at all).
  const clean = filter ? pruneFilter(filter) : undefined;
  return gql<{ submissions: Submission[] }>(
    SUBMISSIONS_QUERY,
    { filter: clean },
    signal,
  ).then((d) => d.submissions);
}

export function fetchSubmission(
  id: string,
  signal?: AbortSignal,
): Promise<Submission | null> {
  return gql<{ submission: Submission | null }>(
    SUBMISSION_QUERY,
    { id },
    signal,
  ).then((d) => d.submission);
}

function pruneFilter(f: SubmissionFilter): SubmissionFilter | undefined {
  const out: SubmissionFilter = {};
  if (f.chainIdFrom != null) out.chainIdFrom = f.chainIdFrom;
  if (f.chainIdTo != null) out.chainIdTo = f.chainIdTo;
  if (f.minSignatures != null) out.minSignatures = f.minSignatures;
  if (f.ready != null) out.ready = f.ready;
  return Object.keys(out).length ? out : undefined;
}
