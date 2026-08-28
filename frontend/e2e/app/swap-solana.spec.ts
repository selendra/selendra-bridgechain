import { test, expect, startApp, gotoView } from "../fixtures/app";
import { installSolanaWallet, signedMessages, SOLANA_ACCOUNT } from "../fixtures/solana-wallet";
import { b58decode } from "../../src/wallet/solana";

/**
 * The Swap view against a SOLANA pool.
 *
 * The unit suite pins the transaction bytes against `solana-sdk`; this pins the
 * flow that produces them — that the UI asks for the Solana wallet rather than
 * MetaMask, skips the approve step an SPL transfer does not have, and hands the
 * wallet a message containing the swap instruction with the user's own derived
 * accounts.
 */

const SOL_CHAIN = 7565164;
const PROGRAM = "E28r29Hyky3UqVBcdSvFk6qNedbRN8X2z4R8hYGDUk88";
const TST = "8T2cxAqp8mDNkdTTb5giew9eYgZ7NmHdEWz6kMeE7WFV";
const WRAP = "Bqt4xDpu6oEPgTgVLjZVQ56hFUGo2F4M8zFuK98NHe32";
const TST_VAULT = "FtnmRC62mfLeW5PE2vRU2mFi5skpa36K7EnHqfaAXryk";
const WRAP_VAULT = "66UA92nBYZHQQ9Rb8rCMEKdeeNJq9CLwRcnWizqPFfDY";

const solanaPool = {
  chainId: SOL_CHAIN,
  address: PROGRAM,
  stable: TST,
  tokens: [
    {
      token: TST,
      symbol: "TST",
      decimals: 6,
      price: "1000000000000000000",
      reserve: "1100000000",
      maxSwapUsd: "1100000000000000000000",
      isStable: true,
      vault: TST_VAULT,
    },
    {
      token: WRAP,
      symbol: "WRAP",
      decimals: 9,
      price: "3180000000000000000000",
      reserve: "299968553460",
      maxSwapUsd: "953900000002800000000000",
      isStable: false,
      vault: WRAP_VAULT,
    },
  ],
};

const CHAINS = [
  {
    chainId: SOL_CHAIN,
    name: "Solana Devnet",
    rpcUrl: null as unknown as string,
    gate: null as unknown as string,
    token: TST,
    tokens: [
      { symbol: "TST", address: TST },
      { symbol: "WRAP", address: WRAP },
    ],
    router: null as unknown as string,
  },
];

const primaryButton = (page: import("@playwright/test").Page) => page.locator(".review-btn");
const payAmount = (page: import("@playwright/test").Page) =>
  page.locator(".amount-row").first().locator("input");

async function openSolanaSwap(page: import("@playwright/test").Page, wallet = {}) {
  await installSolanaWallet(page, wallet);
  await startApp(page, {
    backend: { chains: CHAINS, swapPool: { [SOL_CHAIN]: solanaPool }, swapQuote: "31446540" },
    wallet: null, // no EVM wallet: a Solana pool must not need one
  });
  await gotoView(page, "Swap");
  await expect(page.locator(".card__subtitle")).toContainText("Solana Devnet", { timeout: 10_000 });
}

test("asks for the Solana wallet, not MetaMask", async ({ page }) => {
  await openSolanaSwap(page);
  await expect(primaryButton(page)).toHaveText("Connect Phantom");
});

test("offers no approve step — an SPL transfer is authorised by the signer", async ({ page }) => {
  await openSolanaSwap(page);
  await primaryButton(page).click();
  await payAmount(page).fill("100");
  // The EVM path would show "Approve TST" here before the swap becomes available.
  await expect(primaryButton(page)).toHaveText("Swap", { timeout: 10_000 });
});

test("hands the wallet a transaction carrying the swap instruction", async ({ page }) => {
  await openSolanaSwap(page);
  await primaryButton(page).click();
  await payAmount(page).fill("100");
  await expect(primaryButton(page)).toHaveText("Swap", { timeout: 10_000 });
  await primaryButton(page).click();

  await expect.poll(async () => (await signedMessages(page)).length, { timeout: 15_000 }).toBe(1);
  const msg = b58decode((await signedMessages(page))[0]);

  // The swap instruction's data: variant 5, then amount_in as a LE u64.
  // 100 TST at 6 decimals = 100_000_000.
  const hex = Buffer.from(msg).toString("hex");
  expect(hex).toContain("0500e1f505000000");
  // Both the pool program and the associated-token program are in the message:
  // the destination account is created idempotently before the swap, because a
  // user who has never held the output mint has no account for it.
  for (const key of [PROGRAM, TST_VAULT, WRAP_VAULT, SOLANA_ACCOUNT]) {
    expect(hex).toContain(Buffer.from(b58decode(key)).toString("hex"));
  }
});

test("a rejected signature surfaces as an error, not a stuck spinner", async ({ page }) => {
  await openSolanaSwap(page, { rejectSign: true });
  await primaryButton(page).click();
  await payAmount(page).fill("100");
  await expect(primaryButton(page)).toHaveText("Swap", { timeout: 10_000 });
  await primaryButton(page).click();
  await expect(page.locator(".txbar--error")).toContainText(/reject/i, { timeout: 15_000 });
});

test("shows the SPL balance the wallet actually holds", async ({ page }) => {
  await openSolanaSwap(page);
  await primaryButton(page).click();
  // 1_000_000_000_000 base units at 6 decimals = 1000000 TST — read from the
  // owner's derived associated token account, not from anything the pool says.
  await expect(page.locator(".amount-row").first()).toContainText("Bal: 1000000", { timeout: 10_000 });
});
