import { test, expect } from "@playwright/test";
import {
  fetchChains,
  fetchStats,
  fetchSubmission,
  fetchSubmissions,
  fetchHistory,
  fetchSwapHistory,
  fetchSwapPool,
  fetchSwapQuote,
  gql,
  health,
} from "../../src/api/client";

/**
 * `src/api/client.ts` — the GraphQL layer.
 *
 * Most arguments are bound as variables. `chainId` and `limit` are `u64` args
 * with no convenient variable scalar name, so they are spliced into the document
 * as literals — and a literal built from an unvalidated value is a query
 * injection. These tests capture what actually goes over the wire.
 */

interface Captured {
  url: string;
  query: string;
  variables?: Record<string, unknown>;
}

/** Swap in a fetch that records the request and answers with `data`. */
function captureFetch(data: unknown, opts: { ok?: boolean; errors?: { message: string }[] } = {}) {
  const seen: Captured[] = [];
  const original = globalThis.fetch;
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    const body = init?.body ? JSON.parse(String(init.body)) : {};
    seen.push({ url: String(url), query: body.query, variables: body.variables });
    if (opts.ok === false) return new Response("nope", { status: 502 });
    return new Response(JSON.stringify(opts.errors ? { errors: opts.errors } : { data }), {
      headers: { "content-type": "application/json" },
    });
  }) as unknown as typeof fetch;
  return { seen, restore: () => (globalThis.fetch = original) };
}

test.describe("query construction", () => {
  test("fetchChains asks for the registry fields the app renders", async () => {
    const cap = captureFetch({ chains: [] });
    try {
      await fetchChains();
      expect(cap.seen[0].url).toContain("/graphql");
      expect(cap.seen[0].query).toContain("chains {");
      for (const field of ["chainId", "name", "rpcUrl", "gate", "tokens", "router"]) {
        expect(cap.seen[0].query).toContain(field);
      }
    } finally {
      cap.restore();
    }
  });

  test("fetchStats requests the counters and the threshold", async () => {
    const cap = captureFetch({ stats: {} });
    try {
      await fetchStats();
      expect(cap.seen[0].query).toContain("threshold");
    } finally {
      cap.restore();
    }
  });

  test("filters travel as bound variables, never spliced into the document", async () => {
    const cap = captureFetch({ submissions: [] });
    try {
      await fetchSubmissions({ chainIdFrom: 1337, chainIdTo: 1338 });
      expect(cap.seen[0].variables).toEqual({ filter: { chainIdFrom: 1337, chainIdTo: 1338 } });
      expect(cap.seen[0].query).toContain("$filter: SubmissionFilter");
      expect(cap.seen[0].query).not.toContain("1337");
    } finally {
      cap.restore();
    }
  });

  test("a submission id travels as a variable, so a hostile id cannot reshape the query", async () => {
    const cap = captureFetch({ submission: null });
    const hostile = '0xdead") { __typename } evil: submissions(filter: null) { submissionId } #';
    try {
      await fetchSubmission(hostile);
      expect(cap.seen[0].variables).toEqual({ id: hostile });
      expect(cap.seen[0].query).not.toContain("evil");
    } finally {
      cap.restore();
    }
  });

  test("history filters are bound too", async () => {
    const cap = captureFetch({ history: [] });
    try {
      await fetchHistory({ submissionId: "0xabc" });
      expect(cap.seen[0].variables).toEqual({ filter: { submissionId: "0xabc" } });
    } finally {
      cap.restore();
    }
  });

  test("swapQuote binds its token addresses and amount", async () => {
    const cap = captureFetch({ swapQuote: "1" });
    try {
      await fetchSwapQuote(1337, "0xin", "0xout", "1000");
      expect(cap.seen[0].variables).toEqual({ in: "0xin", out: "0xout", amt: "1000" });
      expect(cap.seen[0].query).toContain("swapQuote(chainId: 1337");
    } finally {
      cap.restore();
    }
  });
});

test.describe("inlined integer literals", () => {
  test("a valid chainId is spliced in as plain digits", async () => {
    const cap = captureFetch({ swapPool: null });
    try {
      await fetchSwapPool(1338);
      expect(cap.seen[0].query).toMatch(/swapPool\(chainId: 1338\)/);
    } finally {
      cap.restore();
    }
  });

  test("swapHistory inlines both of its optional arguments as digits", async () => {
    const cap = captureFetch({ swapHistory: [] });
    try {
      await fetchSwapHistory(1337, 100);
      expect(cap.seen[0].query).toMatch(/swapHistory\(chainId: 1337, limit: 100\)/);
    } finally {
      cap.restore();
    }
  });

  test("swapHistory omits arguments that were not supplied", async () => {
    const cap = captureFetch({ swapHistory: [] });
    try {
      await fetchSwapHistory();
      expect(cap.seen[0].query).toMatch(/swapHistory\(\)/);
    } finally {
      cap.restore();
    }
  });

  /**
   * `number` is a compile-time claim. These values come from backend JSON and
   * from `parseInt` on a wallet's `eth_chainId`, so the integer-ness has to be
   * enforced at the boundary — otherwise the value is concatenated verbatim into
   * the query text, where it can close the argument list and append selections.
   */
  test("a non-integer chainId is refused instead of being concatenated in", async () => {
    const cap = captureFetch({ swapPool: null });
    try {
      for (const bad of [NaN, Infinity, 1.5, -1, 2 ** 53]) {
        await expect(fetchSwapPool(bad), String(bad)).rejects.toThrow(/non-negative integer/);
      }
      // Nothing malformed ever reached the network.
      expect(cap.seen).toHaveLength(0);
    } finally {
      cap.restore();
    }
  });

  test("an injected string masquerading as a chainId is refused", async () => {
    const cap = captureFetch({ swapPool: null });
    const hostile = '1337) { chainId } evil: swapPool(chainId: 1' as unknown as number;
    try {
      await expect(fetchSwapPool(hostile)).rejects.toThrow(/non-negative integer/);
      await expect(fetchSwapHistory(hostile)).rejects.toThrow(/non-negative integer/);
      await expect(fetchSwapQuote(hostile, "0x", "0x", "1")).rejects.toThrow(
        /non-negative integer/
      );
      expect(cap.seen).toHaveLength(0);
    } finally {
      cap.restore();
    }
  });

  test("a hostile limit is refused as well", async () => {
    const cap = captureFetch({ swapHistory: [] });
    try {
      await expect(fetchSwapHistory(1337, 1.5)).rejects.toThrow(/limit must be/);
      expect(cap.seen).toHaveLength(0);
    } finally {
      cap.restore();
    }
  });
});

test.describe("error handling", () => {
  test("a non-2xx response becomes a readable error", async () => {
    const cap = captureFetch(null, { ok: false });
    try {
      await expect(gql("{ x }")).rejects.toThrow("backend HTTP 502");
    } finally {
      cap.restore();
    }
  });

  test("a GraphQL error body surfaces the first message", async () => {
    const cap = captureFetch(null, { errors: [{ message: "field not found" }] });
    try {
      await expect(gql("{ x }")).rejects.toThrow("field not found");
    } finally {
      cap.restore();
    }
  });

  test("a response with neither data nor errors is an error, not silent success", async () => {
    const original = globalThis.fetch;
    globalThis.fetch = (async () =>
      new Response("{}", { headers: { "content-type": "application/json" } })) as typeof fetch;
    try {
      await expect(gql("{ x }")).rejects.toThrow("empty GraphQL response");
    } finally {
      globalThis.fetch = original;
    }
  });

  test("health reports false rather than throwing when the backend is unreachable", async () => {
    const original = globalThis.fetch;
    globalThis.fetch = (async () => {
      throw new Error("ECONNREFUSED");
    }) as typeof fetch;
    try {
      expect(await health()).toBe(false);
    } finally {
      globalThis.fetch = original;
    }
  });

  test("health reports true on a 2xx", async () => {
    const original = globalThis.fetch;
    globalThis.fetch = (async () => new Response("ok")) as typeof fetch;
    try {
      expect(await health()).toBe(true);
    } finally {
      globalThis.fetch = original;
    }
  });
});
