import type { Page } from "@playwright/test";

/**
 * A stand-in Phantom provider, injected before the app boots.
 *
 * It records every message handed to `signAndSendTransaction`, which is what
 * the Solana swap test actually asserts on: not that a button changed colour,
 * but that the transaction the UI produced is the one we pinned at the byte
 * level in `e2e/unit/solana.spec.ts`.
 */

export const SOLANA_ACCOUNT = "EgZc1wGaqZXYn6jy7oSRe95qWmp2NM9SRxXJWjhxuGkC";

export interface SolanaWalletSetup {
  account?: string;
  /** Reject the connection, to exercise the error path. */
  rejectConnect?: boolean;
  /** Reject the signature request. */
  rejectSign?: boolean;
}

export async function installSolanaWallet(page: Page, setup: SolanaWalletSetup = {}): Promise<void> {
  await page.addInitScript((s: SolanaWalletSetup) => {
    const account = s.account ?? "EgZc1wGaqZXYn6jy7oSRe95qWmp2NM9SRxXJWjhxuGkC";
    const sent: string[] = [];
    (window as unknown as { __solSent: string[] }).__solSent = sent;
    const provider = {
      isPhantom: true,
      publicKey: null as unknown,
      async connect(opts?: { onlyIfTrusted?: boolean }) {
        // Silent reconnect must NOT auto-approve, or the test could never see
        // the disconnected state the real wallet starts in.
        if (opts?.onlyIfTrusted) throw new Error("not trusted");
        if (s.rejectConnect) throw new Error("User rejected the request");
        this.publicKey = { toString: () => account };
        return { publicKey: this.publicKey as { toString(): string } };
      },
      async disconnect() {
        this.publicKey = null;
      },
      async request(args: { method: string; params?: { message?: string } }) {
        if (args.method !== "signAndSendTransaction") throw new Error(`unexpected ${args.method}`);
        if (s.rejectSign) throw new Error("User rejected the request");
        sent.push(args.params?.message ?? "");
        return { signature: "5xDAzZkGtMBa34GhDASwJtL3nmSNe3G8K6Sq3G99rvno" };
      },
      on() {},
      removeListener() {},
    };
    (window as unknown as { phantom: unknown }).phantom = { solana: provider };
  }, setup);
}

/** The base58 messages the wallet was asked to sign, in order. */
export async function signedMessages(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as unknown as { __solSent: string[] }).__solSent ?? []);
}
