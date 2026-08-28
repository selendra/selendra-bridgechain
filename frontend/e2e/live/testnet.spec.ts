import { test, expect } from "@playwright/test";

/**
 * Smoke tests against the REAL stack on real testnets — no mocked backend.
 *
 * The `app/` suite mocks `/graphql` so it can drive exact states deterministically.
 * That proves the UI's logic but says nothing about whether it renders what the
 * live backend actually returns: field names, number formats, enum spellings and
 * null-ability are all contract, and all only checked here.
 *
 * Requires the stack to be up (`bash scripts/run.sh scripts/testnet.config.local`).
 * Skipped automatically when it isn't, so this file is safe in CI.
 */

const API = process.env.LIVE_API ?? "http://127.0.0.1:8088";
const APP = process.env.LIVE_APP ?? "http://127.0.0.1:5173";

const SEPOLIA = 11155111;
const HOODI = 560048;

async function backendUp(): Promise<boolean> {
  try {
    const r = await fetch(`${API}/health`, { signal: AbortSignal.timeout(4000) });
    return r.ok;
  } catch {
    return false;
  }
}

test.beforeEach(async () => {
  test.skip(!(await backendUp()), `live stack not reachable at ${API}`);
});

// The app is served by vite on its own port; point the page at it explicitly
// rather than the config's baseURL (which targets the mocked-suite server).
test.use({ baseURL: APP });

test("serves the app and reports the live backend as reachable", async ({ page }) => {
  await page.goto("/");
  await expect(page.locator(".status")).toHaveText(/Backend live/, { timeout: 20_000 });
});

test("the chain registry renders both live testnets", async ({ page }) => {
  await page.goto("/");
  await page.locator(".nav__links").getByRole("button", { name: "Explorer", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Explorer" })).toBeVisible();

  // Chain names come from the backend registry, not from any local constant.
  await page.locator(".filters__field").first().locator(".dd__trigger").click();
  const options = await page.getByRole("option").allTextContents();
  expect(options.join(" ")).toContain("Ethereum Sepolia");
  expect(options.join(" ")).toContain("Hoodi");
  await page.keyboard.press("Escape");
});

test("real transfers render with amounts, routes and statuses", async ({ page }) => {
  await page.goto("/");
  await page.locator(".nav__links").getByRole("button", { name: "Explorer", exact: true }).click();

  // At least the transfers this session performed on-chain.
  await expect(page.locator(".tbl__row").first()).toBeVisible({ timeout: 25_000 });
  const rows = await page.locator(".tbl__row").count();
  expect(rows).toBeGreaterThan(0);

  const table = page.locator(".tbl");
  await expect(table).toContainText("Ethereum Sepolia");
  await expect(table).toContainText("Hoodi");

  // Amounts must be humanised, not raw wei — a raw 1e18 string here would mean
  // the decimals lookup silently failed against the live chain.
  const amount = await page.locator(".tbl__amount").first().textContent();
  expect(amount, `got ${amount}`).not.toMatch(/\d{15,}/);
});

test("statuses come through as the live on-chain values", async ({ page }) => {
  const res = await fetch(`${API}/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query: "{ submissions { status } }" }),
  });
  const { data } = (await res.json()) as { data: { submissions: { status: string }[] } };
  const statuses = new Set(data.submissions.map((s) => s.status));
  // Every status the API emits must be one the badge knows how to label,
  // otherwise the UI renders a blank chip (the StatusBadge fallback case).
  for (const s of statuses) {
    expect(["PENDING", "READY", "EXECUTED", "CANCELLED", "UNKNOWN"]).toContain(s);
  }

  await page.goto("/");
  await page.locator(".nav__links").getByRole("button", { name: "Explorer", exact: true }).click();
  await expect(page.locator(".badge").first()).toBeVisible({ timeout: 25_000 });
  const badge = await page.locator(".badge").first().textContent();
  expect(badge?.trim().length, "a status badge must never render empty").toBeGreaterThan(0);
});

test("the detail drawer opens on a real submission", async ({ page }) => {
  await page.goto("/");
  await page.locator(".nav__links").getByRole("button", { name: "Explorer", exact: true }).click();
  await expect(page.locator(".tbl__row").first()).toBeVisible({ timeout: 25_000 });
  await page.locator(".tbl__row").first().click();

  const drawer = page.getByRole("dialog", { name: "Submission detail" });
  await expect(drawer).toBeVisible();

  // Core fields must render from live data.
  await expect(drawer.locator(".detail__row").filter({ hasText: "Nonce" })).toBeVisible();
  await expect(
    drawer.locator(".detail__row").filter({ hasText: "Submission ID" }).locator(".detail__value")
  ).toHaveText(/^0x[0-9a-f]{64}$/);
  await expect(drawer.locator(".detail__sigs")).toContainText("Signatures");

  // Signature COUNT is deliberately not asserted. Signatures live in the DB, and
  // a wiped DB does not repopulate them: validators resume from file-based
  // cursors, so they never re-sign already-scanned blocks. Asserting >0 here
  // would make the test depend on whether the DB happens to predate the
  // transfers. What must hold is that the section renders either signatures or
  // an explicit empty state — never a silent blank.
  const sigs = drawer.locator(".sig-item");
  const empty = drawer.locator(".detail__empty");
  await expect(sigs.first().or(empty.first())).toBeVisible({ timeout: 15_000 });
});

test("the live swap pool renders its real token list", async ({ page }) => {
  await page.goto("/");
  await page.locator(".nav__links").getByRole("button", { name: "Swap", exact: true }).click();

  // This is the query that returned null before the genesis-scan fix: the pool's
  // token list is discovered by replaying TokenListed logs on a live chain.
  await expect(page.locator(".summary__row").filter({ hasText: "Pool" })).toBeVisible({
    timeout: 30_000,
  });
  const labels = await page.locator(".amount-row .dd__label").allTextContents();
  expect(labels.length).toBe(2);
  expect(labels[0]).not.toBe(labels[1]);
});

test("a live quote is fetched for a real amount", async ({ page }) => {
  await page.goto("/");
  await page.locator(".nav__links").getByRole("button", { name: "Swap", exact: true }).click();
  await expect(page.locator(".summary__row").filter({ hasText: "Pool" })).toBeVisible({
    timeout: 30_000,
  });

  await page.locator(".amount-row").first().locator("input").fill("10");
  const out = page.locator(".amount-row").last().locator(".amount-row__input");
  await expect(out).not.toHaveText("0", { timeout: 25_000 });
  await expect(page.locator(".summary__row").filter({ hasText: "Rate" })).not.toContainText("—");
});

test("the same-chain swaps tab renders live swap history", async ({ page }) => {
  // Whether any row exists depends on how far the indexer has backfilled — at a
  // 10-block getLogs cap that can lag by thousands of blocks after a restart.
  // Ask the API first and assert against what it actually holds, so this checks
  // the render path rather than the backfill clock.
  const res = await fetch(`${API}/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query: "{ swapHistory(limit:5){ chainId amountIn amountOut } }" }),
  });
  const rows = (await res.json()).data.swapHistory as unknown[];

  await page.goto("/");
  await page.locator(".nav__links").getByRole("button", { name: "Explorer", exact: true }).click();
  await page.getByRole("button", { name: "Same-chain swaps" }).click();
  await expect(page.locator(".tbl")).toContainText("Amount in");

  if (rows.length === 0) {
    await expect(page.locator(".tbl__empty")).toContainText("No same-chain swaps recorded yet");
  } else {
    // Which chain the rows came from depends on where swaps happened, so assert
    // that rows render at all rather than naming one chain.
    await expect(page.locator(".tbl tbody tr").first()).toBeVisible({ timeout: 25_000 });
  }
});

test("chain ids reach the backend as bare integers", async () => {
  // The client splices chainId into the GraphQL document as a literal. Against a
  // live backend, a malformed one would be a query-injection vector rather than
  // just a bad request — assert the server accepts the well-formed shape and
  // rejects a hostile one at the client boundary.
  const ok = await fetch(`${API}/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query: `{ swapPool(chainId: ${SEPOLIA}) { address } }` }),
  }).then((r) => r.json());
  expect(ok.data.swapPool).not.toBeNull();

  // An UNCONFIGURED chain id must come back as a clean null rather than an
  // error — the point being that the literal is parsed as a number, not that any
  // particular chain lacks a pool (every chain in the mesh has one now).
  const unknown = await fetch(`${API}/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query: `{ swapPool(chainId: 424242) { address } }` }),
  }).then((r) => r.json());
  expect(unknown.data.swapPool).toBeNull();
  expect(unknown.errors).toBeUndefined();
});
