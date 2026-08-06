import { test, expect, startApp, connectWallet, gotoView } from "../fixtures/app";
import { TOKEN_6 } from "../fixtures/backend";

/**
 * The custom `Dropdown` and the polling hook, exercised through the app.
 *
 * The dropdown is a select substitute, so the property that matters is that it
 * never SHOWS a selection its owner does not hold — a control that displays
 * "USDC" while the form carries a different address is worse than one that
 * shows nothing.
 */

const CALLS = {
  "313ce567": "12",
  "70a08231": (1000n * 10n ** 18n).toString(16),
  dd62ed3e: (2n ** 255n).toString(16),
};

async function openBridge(page: import("@playwright/test").Page) {
  await startApp(page, { wallet: { chainId: 1337, calls: CALLS } });
  await connectWallet(page);
  await gotoView(page, "Bridge");
  await expect(page.getByRole("heading", { name: "Bridge" })).toBeVisible();
}

test.describe("Dropdown", () => {
  test("opens a listbox and reports its expanded state", async ({ page }) => {
    await openBridge(page);
    const trigger = page.locator(".token-picker .dd__trigger");
    await expect(trigger).toHaveAttribute("aria-expanded", "false");
    await trigger.click();
    await expect(page.getByRole("listbox")).toBeVisible();
    await expect(trigger).toHaveAttribute("aria-expanded", "true");
  });

  test("marks the current option as selected", async ({ page }) => {
    await openBridge(page);
    await page.locator(".token-picker .dd__trigger").click();
    await expect(page.getByRole("option", { selected: true })).toHaveCount(1);
  });

  test("closes on Escape without changing the selection", async ({ page }) => {
    await openBridge(page);
    const label = page.locator(".token-picker .dd__label");
    const before = await label.textContent();
    await page.locator(".token-picker .dd__trigger").click();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("listbox")).toBeHidden();
    await expect(label).toHaveText(before!);
  });

  test("closes on an outside click", async ({ page }) => {
    await openBridge(page);
    await page.locator(".token-picker .dd__trigger").click();
    await expect(page.getByRole("listbox")).toBeVisible();
    await page.locator(".card__head").click();
    await expect(page.getByRole("listbox")).toBeHidden();
  });

  test("selecting an option closes the menu and reports the value up", async ({ page }) => {
    await openBridge(page);
    await page.locator(".token-picker .dd__trigger").click();
    await page.getByRole("option", { name: /USDC/ }).click();
    await expect(page.getByRole("listbox")).toBeHidden();
    await expect(page.locator(".token-picker .dd__label")).toHaveText("USDC");
    await expect(
      page.locator(".field").filter({ hasText: /Token \(ERC-20/ }).locator("input")
    ).toHaveValue(TOKEN_6);
  });

  /**
   * The regression: the trigger used to fall back to `options[0]` when the
   * parent's value matched nothing, so a hand-typed custom token address left
   * the picker confidently displaying the FIRST registry token. The transfer
   * being built and the token named on screen were different assets.
   */
  test("shows a placeholder — not the first option — for a value it does not have", async ({ page }) => {
    await openBridge(page);
    const label = page.locator(".token-picker .dd__label");
    await expect(label).toHaveText("TST");

    await page.locator(".field").filter({ hasText: /Token \(ERC-20/ }).locator("input").fill(
      "0x" + "9".repeat(40)
    );

    await expect(label).toHaveText("Custom address");
    await expect(label).not.toHaveText("TST");
  });

  test("renders a disabled placeholder while the option list is still empty", async ({ page }) => {
    // No chains from the backend => the destination picker has nothing to offer.
    await startApp(page, { backend: { chains: [] }, wallet: { chainId: 1337, calls: CALLS } });
    await connectWallet(page);
    await gotoView(page, "Bridge");
    await expect(page.locator(".bridge-route .dd__trigger")).toBeDisabled();
    await expect(page.locator(".bridge-route .dd__label")).toHaveText("—");
  });

  test("the destination picker excludes the source chain", async ({ page }) => {
    await openBridge(page);
    await page.locator(".bridge-route .dd__trigger").click();
    const labels = await page.getByRole("option").allTextContents();
    expect(labels.join(" ")).not.toContain("Chain A");
    expect(labels.join(" ")).toContain("Chain B");
  });
});

test.describe("polling", () => {
  /**
   * `usePoll` carries a generation guard so a slow request from a previous
   * arming cannot resolve late and overwrite fresher data. Rapidly re-arming the
   * Explorer's filters is the way to provoke that.
   */
  test("a superseded request does not overwrite the current view", async ({ page }) => {
    let n = 0;
    await startApp(page, { backend: {} });

    // First `submissions` query is slow and returns a stale row; later ones are
    // fast and return the truth.
    await page.route("**/graphql", async (route) => {
      const body = JSON.parse(route.request().postData() ?? "{}");
      if (String(body.query).includes("submissions(filter")) {
        const stale = n++ === 0;
        if (stale) await new Promise((r) => setTimeout(r, 2500));
        return route.fulfill({
          status: 200,
          contentType: "application/json",
          body: JSON.stringify({
            data: {
              submissions: [
                {
                  submissionId: "0x" + (stale ? "de" : "fe").repeat(32),
                  debridgeId: "0x" + "11".repeat(32),
                  amount: "1",
                  chainIdFrom: 1337,
                  chainIdTo: 1338,
                  nonce: 1,
                  receiver: "0x" + "ee".repeat(20),
                  nativeSender: "0x" + "ff".repeat(20),
                  signatureCount: 1,
                  meetsThreshold: false,
                  status: "PENDING",
                  signatures: [],
                },
              ],
            },
          }),
        });
      }
      return route.fallback();
    });

    await gotoView(page, "Explorer");
    // Re-arm immediately, superseding the slow first request.
    await page.locator(".filters__field").first().locator(".dd__trigger").click();
    await page.getByRole("option", { name: "Chain A" }).click();

    await expect(page.locator(".tbl__row")).toContainText("0xfefefefe", { timeout: 15_000 });
    // Give the stale response time to land — it must be ignored.
    await page.waitForTimeout(3000);
    await expect(page.locator(".tbl__row")).toContainText("0xfefefefe");
    await expect(page.locator(".tbl__row")).not.toContainText("0xdededede");
  });

  test("keeps the last good data when a poll tick fails", async ({ page }) => {
    let failing = false;
    await startApp(page, { backend: {} });
    await page.route("**/graphql", async (route) => {
      const body = JSON.parse(route.request().postData() ?? "{}");
      if (String(body.query).includes("stats {") && failing) {
        return route.fulfill({ status: 500, body: "boom" });
      }
      return route.fallback();
    });

    await gotoView(page, "Explorer");
    await expect(page.locator(".stat-grid")).toContainText("Total transfers");

    failing = true;
    // The counters must not blank out just because one tick failed.
    await page.waitForTimeout(6000);
    await expect(page.locator(".stat-grid .stat").first()).not.toHaveText("—");
  });
});
