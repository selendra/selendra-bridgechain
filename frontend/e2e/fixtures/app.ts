import { test as base, expect, type Page } from "@playwright/test";
import { mockBackend, type BackendMock, type BackendOptions } from "./backend";
import { installWallet, type WalletSetup } from "./wallet";

/**
 * One place to stand the app up: mocked backend, mocked wallet, mocked chain
 * RPCs. Individual tests only say what is DIFFERENT about their world.
 */

export { expect };

/** The registry's `rpcUrl`s are read directly (not through the wallet) by
 *  `useChainDecimals`. Serve them so tests don't depend on a live anvil. */
export async function mockChainRpcs(page: Page, decimals = 18): Promise<void> {
  for (const url of ["**/127.0.0.1:8545/**", "**/127.0.0.1:8546/**"]) {
    await page.route(url, (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          jsonrpc: "2.0",
          id: 1,
          result: "0x" + decimals.toString(16).padStart(64, "0"),
        }),
      })
    );
  }
}

export interface AppWorld {
  backend: BackendMock;
}

export async function startApp(
  page: Page,
  opts: { backend?: BackendOptions; wallet?: WalletSetup | null } = {}
): Promise<AppWorld> {
  const backend = await mockBackend(page, opts.backend ?? {});
  await mockChainRpcs(page);
  if (opts.wallet !== null) await installWallet(page, opts.wallet ?? {});
  await page.goto("/");
  await expect(page.getByRole("button", { name: "Bridge", exact: true })).toBeVisible();
  return { backend };
}

/** Connect the injected wallet through the navbar and wait for the chip. */
export async function connectWallet(page: Page): Promise<void> {
  await page.getByRole("button", { name: "Connect Wallet" }).first().click();
  await expect(page.locator(".wallet-chip")).toBeVisible();
}

export async function gotoView(page: Page, view: "Bridge" | "Swap" | "Explorer"): Promise<void> {
  await page.locator(".nav__links").getByRole("button", { name: view, exact: true }).click();
}

/** Pick an option out of a `Dropdown` by its visible label. */
export async function chooseFromDropdown(
  page: Page,
  trigger: ReturnType<Page["locator"]>,
  label: string
): Promise<void> {
  await trigger.click();
  await page.getByRole("option", { name: label, exact: false }).first().click();
}

export const test = base;
