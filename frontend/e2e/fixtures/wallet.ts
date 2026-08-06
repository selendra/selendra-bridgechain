import type { Page } from "@playwright/test";

/**
 * An injected EIP-1193 wallet, so the app's real code paths run without MetaMask.
 *
 * It announces itself over EIP-6963 *and* sets `window.ethereum`, matching what
 * the app's discovery in `useWallet` looks for. Every request is recorded on
 * `window.__walletCalls`, which is how the tests assert what was actually sent —
 * the chain-id binding on writes, the exact calldata, the approve→send order.
 *
 * `eth_call` returns are keyed by the 4-byte selector so token reads (decimals,
 * balanceOf, allowance) can be programmed per test.
 */

export interface WalletCall {
  method: string;
  params: unknown[];
}

export interface WalletSetup {
  /** Current chain, as the wallet reports it from `eth_chainId`. */
  chainId?: number;
  accounts?: string[];
  /**
   * selector (8 hex chars, no 0x) -> the `eth_call` return.
   *
   * A bare hex string is left-padded into one 32-byte word (the common case:
   * decimals, balanceOf, allowance). Prefix with `raw:` to return the bytes
   * verbatim — needed for dynamic returns like `remoteRouter(uint256)`.
   */
  calls?: Record<string, string>;
  /** Receipt status for any sent tx. */
  receiptStatus?: "0x1" | "0x0";
  /** Logs attached to the receipt (for `extractSent`). */
  receiptLogs?: { address: string; topics: string[]; data: string }[];
  /** Make eth_sendTransaction reject as the user would. */
  rejectSend?: boolean;
  /**
   * Report a DIFFERENT chain from `eth_chainId` than the one the app already
   * believes it is on — but only once a test calls `driftChain(page, id)`. This
   * is the race the chain-id guard exists for: the wallet has moved and the UI's
   * cached chainId has not caught up, so it is still offering a chain-A form.
   * Applying it at load instead would just look like connecting to chain 999.
   */
  driftChainIdTo?: number;
  /**
   * Start already authorized, as a returning user's wallet does — `eth_accounts`
   * answers before any click. Default false: the app must show "Connect Wallet"
   * until the user approves.
   */
  preAuthorized?: boolean;
}

export const ACCOUNT = "0x70997970c51812dc3a010c7d01b50e0d17dc79c8";

export async function installWallet(page: Page, setup: WalletSetup = {}): Promise<void> {
  await page.addInitScript((cfg: WalletSetup & { account: string }) => {
    const calls: WalletCall[] = [];
    (window as unknown as { __walletCalls: WalletCall[] }).__walletCalls = calls;

    let chainId = cfg.chainId ?? 1337;
    const accounts = cfg.accounts ?? [cfg.account];
    // A real wallet reports no accounts until the site is authorized — and then
    // REMEMBERS that across reloads, which the recovery tests depend on.
    const AUTH_KEY = "__test_wallet_authorized";
    let authorized = cfg.preAuthorized ?? sessionStorage.getItem(AUTH_KEY) === "1";
    let sendCount = 0;
    // Set by `driftChain()` once the app has settled on the original chain.
    let drifted: number | null = null;
    (window as unknown as { __driftChain: (id: number) => void }).__driftChain = (id) => {
      drifted = id;
    };

    const hex = (n: number) => "0x" + n.toString(16);
    const word = (h: string) => "0x" + h.replace(/^0x/, "").padStart(64, "0");

    const provider = {
      isMetaMask: true,
      _listeners: {} as Record<string, ((...a: unknown[]) => void)[]>,
      on(event: string, handler: (...a: unknown[]) => void) {
        (this._listeners[event] ??= []).push(handler);
      },
      removeListener(event: string, handler: (...a: unknown[]) => void) {
        this._listeners[event] = (this._listeners[event] ?? []).filter((h) => h !== handler);
      },
      emit(event: string, payload: unknown) {
        (this._listeners[event] ?? []).forEach((h) => h(payload));
      },
      async request({ method, params = [] }: { method: string; params?: unknown[] }) {
        calls.push({ method, params });
        switch (method) {
          case "eth_accounts":
            return authorized ? accounts : [];
          case "eth_requestAccounts":
            authorized = true;
            try {
              sessionStorage.setItem(AUTH_KEY, "1");
            } catch {
              /* storage disabled */
            }
            return accounts;
          case "eth_chainId":
            // The drift case: the wallet has moved and deliberately did NOT emit
            // `chainChanged`, so the app's cached value is stale.
            return hex(drifted ?? chainId);
          case "wallet_switchEthereumChain": {
            const target = parseInt((params[0] as { chainId: string }).chainId, 16);
            chainId = target;
            provider.emit("chainChanged", hex(target));
            return null;
          }
          case "eth_call": {
            const data = (params[0] as { data: string }).data ?? "";
            const sel = data.slice(2, 10);
            const hit = cfg.calls?.[sel];
            if (hit === undefined) return word("0");
            return hit.startsWith("raw:") ? "0x" + hit.slice(4) : word(hit);
          }
          case "eth_sendTransaction": {
            if (cfg.rejectSend) {
              const err = new Error("User rejected the request.") as Error & { code: number };
              err.code = 4001;
              throw err;
            }
            sendCount += 1;
            return "0x" + sendCount.toString(16).padStart(64, "0");
          }
          case "eth_getTransactionReceipt":
            return {
              blockNumber: "0x1",
              status: cfg.receiptStatus ?? "0x1",
              logs: cfg.receiptLogs ?? [],
            };
          default:
            return null;
        }
      },
    };

    (window as unknown as { ethereum: unknown }).ethereum = provider;

    const announce = () =>
      window.dispatchEvent(
        new CustomEvent("eip6963:announceProvider", {
          detail: {
            info: {
              uuid: "11111111-2222-3333-4444-555555555555",
              name: "MetaMask",
              icon: "data:image/svg+xml,",
              rdns: "io.metamask",
            },
            provider,
          },
        })
      );
    window.addEventListener("eip6963:requestProvider", announce);
    announce();
  }, { ...setup, account: ACCOUNT });
}

/**
 * Move the wallet to another chain WITHOUT emitting `chainChanged`, so the app's
 * cached chainId goes stale. This is the exact race the pre-write chain check
 * defends against.
 */
export async function driftChain(page: Page, chainId: number): Promise<void> {
  await page.evaluate((id) => (window as unknown as { __driftChain: (n: number) => void }).__driftChain(id), chainId);
}

/** Everything the page asked the wallet to do, in order. */
export async function walletCalls(page: Page): Promise<WalletCall[]> {
  return page.evaluate(() => (window as unknown as { __walletCalls: WalletCall[] }).__walletCalls ?? []);
}

/** Just the write attempts. */
export async function sentTransactions(
  page: Page
): Promise<{ from: string; to: string; data: string; chainId?: string }[]> {
  const calls = await walletCalls(page);
  return calls
    .filter((c) => c.method === "eth_sendTransaction")
    .map((c) => c.params[0] as { from: string; to: string; data: string; chainId?: string });
}

/** 32-byte word for a uint — for programming `eth_call` returns. */
export function uintWord(v: bigint | number): string {
  return BigInt(v).toString(16);
}
