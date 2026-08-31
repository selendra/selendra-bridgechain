import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { webcrypto } from "node:crypto";
import { b58decode, b58encode } from "../../src/wallet/solana";

/**
 * The Solana swap driven through the REAL app, in a REAL browser, against the
 * REAL devnet program.
 *
 * What a wallet extension does is: take a message, sign it with a key the page
 * never sees, broadcast it, hand back a signature. That is exactly what the
 * shim below does — the signing happens in the test process (via `exposeFunction`),
 * outside the page, with a real devnet keypair. So every line of app code on the
 * path runs for real: the React flow, the PDA derivation, the instruction
 * encoding, the message serialization, the confirmation poll. Only the
 * extension's approval dialog is absent, because Playwright cannot click one.
 *
 * Skips unless SOLANA_LIVE=1 and a keypair is provided, since it spends devnet
 * tokens:
 *
 *   SOLANA_LIVE=1 SOLANA_KEYPAIR=.solana/payer.json \
 *   LIVE_API=http://127.0.0.1:5173 LIVE_APP=http://127.0.0.1:5173 \
 *   npx playwright test --project=live e2e/live/solana-swap.spec.ts
 */

const API = process.env.LIVE_API ?? "http://127.0.0.1:8088";
const APP = process.env.LIVE_APP ?? "http://127.0.0.1:5173";
const RPC = process.env.SOLANA_RPC ?? "https://api.devnet.solana.com";
const KEYPAIR = process.env.SOLANA_KEYPAIR;
const SOL_CHAIN = 7565164;

test.use({ baseURL: APP });

test.skip(process.env.SOLANA_LIVE !== "1" || !KEYPAIR, "set SOLANA_LIVE=1 and SOLANA_KEYPAIR");

/** Sign as a wallet does: outside the page, with a key it never sees. */
async function signer() {
  const raw = new Uint8Array(JSON.parse(readFileSync(KEYPAIR!, "utf8")));
  const seed = raw.slice(0, 32);
  const publicKey = b58encode(raw.slice(32, 64));
  // PKCS8 wrapper for a raw Ed25519 seed — the only form WebCrypto imports.
  const pkcs8 = new Uint8Array([
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
    ...seed,
  ]);
  const key = await webcrypto.subtle.importKey(
    "pkcs8",
    pkcs8.buffer.slice(0) as ArrayBuffer,
    { name: "Ed25519" },
    false,
    ["sign"]
  );
  return {
    publicKey,
    async signAndSend(messageB58: string): Promise<string> {
      const message = b58decode(messageB58);
      const sig = new Uint8Array(
        await webcrypto.subtle.sign({ name: "Ed25519" }, key, message.buffer.slice(0) as ArrayBuffer)
      );
      const wire = new Uint8Array([1, ...sig, ...message]);
      const res = await fetch(RPC, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          method: "sendTransaction",
          params: [Buffer.from(wire).toString("base64"), { encoding: "base64" }],
        }),
      }).then((r) => r.json());
      if (res.error) throw new Error(JSON.stringify(res.error));
      return res.result as string;
    },
  };
}

async function poolReserves(): Promise<Record<string, string>> {
  const res = await fetch(`${API}/graphql`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ query: `{ pools(chainId: ${SOL_CHAIN}) { symbol reserve } }` }),
  }).then((r) => r.json());
  const out: Record<string, string> = {};
  for (const t of res.data.pools ?? []) out[t.symbol] = t.reserve;
  return out;
}

test("swaps on Solana from the browser, end to end", async ({ page }) => {
  // A real devnet round trip — quote, wallet, submit, confirm — outlives the
  // default per-test budget several times over.
  test.setTimeout(240_000);
  const wallet = await signer();
  await page.exposeFunction("__walletSignAndSend", (msg: string) => wallet.signAndSend(msg));
  await page.addInitScript((account: string) => {
    const provider = {
      isPhantom: true,
      publicKey: null as unknown,
      async connect(opts?: { onlyIfTrusted?: boolean }) {
        if (opts?.onlyIfTrusted) throw new Error("not trusted");
        this.publicKey = { toString: () => account };
        return { publicKey: this.publicKey as { toString(): string } };
      },
      async disconnect() {
        this.publicKey = null;
      },
      async request(args: { method: string; params?: { message?: string } }) {
        const w = window as unknown as { __walletSignAndSend(m: string): Promise<string> };
        return { signature: await w.__walletSignAndSend(args.params!.message!) };
      },
      on() {},
      removeListener() {},
    };
    (window as unknown as { phantom: unknown }).phantom = { solana: provider };
  }, wallet.publicKey);

  const before = await poolReserves();
  expect(Object.keys(before).length, "the live Solana pool must be configured").toBeGreaterThan(1);

  await page.goto("/");
  await page.locator(".nav__links").getByRole("button", { name: "Swap", exact: true }).click();
  // Pick the Solana pool.
  await page.locator(".card__head").locator("..").getByRole("button").first().click();
  await page.getByRole("option", { name: /Solana/ }).click();
  await expect(page.locator(".card__subtitle")).toContainText("Solana", { timeout: 20_000 });

  // Wait for the view to settle on the Solana pool before connecting: until the
  // pool loads it is still offering the EVM wallet, and a click landing then
  // connects nothing.
  await expect(page.locator(".review-btn")).toHaveText("Connect Phantom", { timeout: 30_000 });
  await page.locator(".review-btn").click();
  await expect(page.locator(".review-btn")).not.toHaveText("Connect Phantom", { timeout: 20_000 });

  await page.locator(".amount-row").first().locator("input").fill("1");
  await expect(page.locator(".review-btn")).toHaveText("Swap", { timeout: 30_000 });
  await page.locator(".review-btn").click();

  await expect(page.locator(".txbar--done")).toContainText(/Swapped for/i, { timeout: 90_000 });

  // And the chain agrees: the pool's reserves moved.
  await expect
    .poll(async () => (await poolReserves()).TST, { timeout: 60_000 })
    .not.toBe(before.TST);
});
