import type { Page, Route } from "@playwright/test";

/**
 * A stand-in for `graphql-api`, wired through `page.route`.
 *
 * The app issues one POST per query to `/graphql`, so the mock dispatches on the
 * operation text. Everything is overridable per test, and every request is
 * recorded — which is how the injection guards are asserted: we check what
 * document actually went over the wire, not just what the UI rendered.
 */

export interface Chain {
  chainId: number;
  name: string;
  rpcUrl: string;
  gate: string;
  token: string;
  tokens: { symbol: string; address: string }[];
  router: string;
}

export const GATE_A = "0x1111111111111111111111111111111111111111";
export const GATE_B = "0x2222222222222222222222222222222222222222";
export const ROUTER_A = "0x3333333333333333333333333333333333333333";
export const ROUTER_B = "0x4444444444444444444444444444444444444444";
export const TOKEN_18 = "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
export const TOKEN_6 = "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
export const STABLE_A = "0xcccccccccccccccccccccccccccccccccccccccc";
export const STABLE_B = "0xdddddddddddddddddddddddddddddddddddddddd";
export const POOL_A = "0xeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

export const CHAINS: Chain[] = [
  {
    chainId: 1337,
    name: "Chain A",
    rpcUrl: "http://127.0.0.1:8545",
    gate: GATE_A,
    token: TOKEN_18,
    tokens: [
      { symbol: "TST", address: TOKEN_18 },
      { symbol: "USDC", address: TOKEN_6 },
    ],
    router: ROUTER_A,
  },
  {
    chainId: 1338,
    name: "Chain B",
    rpcUrl: "http://127.0.0.1:8546",
    gate: GATE_B,
    token: STABLE_B,
    tokens: [{ symbol: "TST", address: STABLE_B }],
    router: ROUTER_B,
  },
];

export interface BackendOptions {
  chains?: Chain[];
  /** Return null to make `/health` fail (offline banner). */
  healthy?: boolean;
  /** submissionId -> status, driving the awaiting → ready-to-finalize poll. */
  submissionStatus?: Record<string, string>;
  swapPool?: Record<number, unknown>;
  swapQuote?: string | null;
  history?: unknown[];
  submissions?: unknown[];
  swapHistory?: unknown[];
  stats?: unknown;
}

export interface BackendMock {
  /** Every GraphQL document the app sent, in order. */
  queries: string[];
  /** Mutable — a test can flip a submission to EXECUTED mid-flight. */
  options: BackendOptions;
}

const defaultPool = (chainId: number, stable: string, other: string) => ({
  chainId,
  address: POOL_A,
  stable,
  tokens: [
    {
      token: stable,
      symbol: "USDS",
      decimals: 18,
      price: "1000000000000000000",
      reserve: "1000000000000000000000",
      maxSwapUsd: "1000000",
      isStable: true,
    },
    {
      token: other,
      symbol: "TST",
      decimals: 18,
      price: "1000000000000000000",
      reserve: "500000000000000000000",
      maxSwapUsd: "1000000",
      isStable: false,
    },
  ],
});

export async function mockBackend(page: Page, options: BackendOptions = {}): Promise<BackendMock> {
  const mock: BackendMock = { queries: [], options };

  await page.route("**/health", (route: Route) =>
    route.fulfill(
      options.healthy === false
        ? { status: 503, body: "down" }
        : { status: 200, body: "ok" }
    )
  );

  await page.route("**/graphql", async (route: Route) => {
    const body = JSON.parse(route.request().postData() ?? "{}") as {
      query: string;
      variables?: Record<string, unknown>;
    };
    mock.queries.push(body.query);
    const q = body.query;
    const o = mock.options;

    const data = (payload: unknown) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ data: payload }),
      });

    if (q.includes("chains {")) return data({ chains: o.chains ?? CHAINS });
    if (q.includes("stats {")) {
      return data({
        stats: o.stats ?? { total: 0, signed: 0, ready: 0, threshold: 2, routes: [] },
      });
    }
    if (q.includes("swapPool(")) {
      const id = Number(q.match(/swapPool\(chainId:\s*(\d+)\)/)?.[1] ?? 0);
      const fromOpts = o.swapPool?.[id];
      if (fromOpts !== undefined) return data({ swapPool: fromOpts });
      return data({
        swapPool:
          id === 1337 ? defaultPool(1337, STABLE_A, TOKEN_18) : defaultPool(1338, STABLE_B, TOKEN_6),
      });
    }
    if (q.includes("swapQuote(")) {
      return data({ swapQuote: o.swapQuote === undefined ? (body.variables?.amt as string) : o.swapQuote });
    }
    if (q.includes("swapHistory(")) return data({ swapHistory: o.swapHistory ?? [] });
    if (q.includes("submission(submissionId")) {
      const id = String(body.variables?.id ?? "");
      const status = o.submissionStatus?.[id.toLowerCase()];
      return data({
        submission: status
          ? {
              submissionId: id,
              debridgeId: "0x" + "22".repeat(32),
              amount: "1000",
              chainIdFrom: 1337,
              chainIdTo: 1338,
              nonce: 1,
              receiver: TOKEN_18,
              nativeSender: ROUTER_A,
              autoParams: "0x",
              signatureCount: 2,
              meetsThreshold: true,
              status,
              signatures: [],
            }
          : null,
      });
    }
    if (q.includes("submissions(filter")) return data({ submissions: o.submissions ?? [] });
    if (q.includes("history(filter")) return data({ history: o.history ?? [] });

    return route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ errors: [{ message: `unmocked query: ${q.slice(0, 80)}` }] }),
    });
  });

  return mock;
}
