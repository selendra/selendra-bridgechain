import { test, expect, startApp, connectWallet, gotoView } from "../fixtures/app";
import { walletCalls, ACCOUNT } from "../fixtures/wallet";

/**
 * The app shell: navigation, backend reachability, and the whole wallet
 * lifecycle (detect → connect → chain display → switch → disconnect).
 */

test("opens on Swap and navigates between all three views", async ({ page }) => {
  await startApp(page);

  await expect(page.getByRole("heading", { name: "Swap" })).toBeVisible();

  await gotoView(page, "Bridge");
  await expect(page.getByRole("heading", { name: "Bridge" })).toBeVisible();

  await gotoView(page, "Explorer");
  await expect(page.getByRole("heading", { name: "Explorer" })).toBeVisible();

  await gotoView(page, "Swap");
  await expect(page.getByRole("heading", { name: "Swap" })).toBeVisible();
});

test("marks the active view", async ({ page }) => {
  await startApp(page);
  await gotoView(page, "Explorer");
  await expect(page.locator(".nav__link--active")).toHaveText("Explorer");
});

test("reports the backend as live when /health and the registry both answer", async ({ page }) => {
  await startApp(page);
  await expect(page.locator(".status")).toHaveText(/Backend live/);
});

test("reports the backend as offline when /health fails", async ({ page }) => {
  await startApp(page, { backend: { healthy: false } });
  await expect(page.locator(".status")).toHaveText(/Backend offline/);
});

test.describe("wallet", () => {
  test("detects an injected wallet and offers to connect", async ({ page }) => {
    await startApp(page);
    await expect(page.getByRole("button", { name: "Connect Wallet" }).first()).toBeVisible();
  });

  test("offers the MetaMask download when no wallet is present", async ({ page }) => {
    await startApp(page, { wallet: null });
    await expect(page.getByRole("button", { name: "Install MetaMask" })).toBeVisible();
  });

  test("connects and shows the truncated account", async ({ page }) => {
    await startApp(page);
    await connectWallet(page);

    // 0x7099…79c8 — shortHex(address, 6, 4)
    await expect(page.locator(".wallet-chip")).toContainText(ACCOUNT.slice(0, 6));
    await expect(page.locator(".wallet-chip")).toContainText(ACCOUNT.slice(-4));

    const methods = (await walletCalls(page)).map((c) => c.method);
    expect(methods).toContain("eth_requestAccounts");
    expect(methods).toContain("eth_chainId");
  });

  test("shows the connected network and full address in the menu", async ({ page }) => {
    await startApp(page, { wallet: { chainId: 1337 } });
    await connectWallet(page);
    await page.locator(".wallet-chip").click();

    await expect(page.locator(".wallet__net")).toContainText("Anvil A");
    await expect(page.locator(".wallet__net-id")).toHaveText("#1337");
    await expect(page.locator(".wallet__addr")).toHaveText(ACCOUNT);
    await expect(page.locator(".wallet__connected-with")).toContainText("MetaMask");
  });

  test("labels an unrecognised network by id rather than showing nothing", async ({ page }) => {
    await startApp(page, { wallet: { chainId: 4242 } });
    await connectWallet(page);
    await page.locator(".wallet-chip").click();
    await expect(page.locator(".wallet__net")).toContainText("Chain 4242");
  });

  test("disconnects back to the connect button", async ({ page }) => {
    await startApp(page);
    await connectWallet(page);
    await page.locator(".wallet-chip").click();
    await page.getByRole("button", { name: "Disconnect" }).click();
    await expect(page.getByRole("button", { name: "Connect Wallet" }).first()).toBeVisible();
  });

  test("closes the wallet menu on an outside click", async ({ page }) => {
    await startApp(page);
    await connectWallet(page);
    await page.locator(".wallet-chip").click();
    await expect(page.locator(".wallet__menu")).toBeVisible();
    await page.locator(".app__panel").click({ position: { x: 5, y: 200 } });
    await expect(page.locator(".wallet__menu")).toBeHidden();
  });

  test("surfaces a rejected connection instead of failing silently", async ({ page }) => {
    await startApp(page);
    await page.evaluate(() => {
      const p = (window as unknown as { ethereum: { request: unknown } }).ethereum;
      p.request = async ({ method }: { method: string }) => {
        if (method === "eth_requestAccounts") {
          const e = new Error("User rejected the request.") as Error & { code: number };
          e.code = 4001;
          throw e;
        }
        return method === "eth_accounts" ? [] : "0x539";
      };
    });
    await page.getByRole("button", { name: "Connect Wallet" }).first().click();
    await expect(page.locator(".wallet__error")).toHaveText("Connection rejected");
  });

  test("reacts to the wallet switching chains underneath it", async ({ page }) => {
    await startApp(page, { wallet: { chainId: 1337 } });
    await connectWallet(page);

    // The provider emits `chainChanged`, exactly as MetaMask does.
    await page.evaluate(() => {
      const p = window as unknown as {
        ethereum: { emit: (e: string, v: unknown) => void };
      };
      p.ethereum.emit("chainChanged", "0x53a"); // 1338
    });

    await page.locator(".wallet-chip").click();
    await expect(page.locator(".wallet__net-id")).toHaveText("#1338");
  });
});
