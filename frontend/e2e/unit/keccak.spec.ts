import { test, expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { bytesToHex, hexToBytes, keccak256, submissionId } from "../../src/wallet/keccak";

/**
 * The submissionId is the one value the whole bridge agrees on: Solidity, Rust
 * and the Solana program hash the same bytes, and a browser that computes it
 * differently would build a transfer nobody can claim.
 *
 * These are the SAME fixtures the Solidity and Rust implementations are pinned
 * to (`contracts/fixtures/submission_ids.json`, written by GenFixtures.t.sol).
 */
const fx = JSON.parse(
  readFileSync(fileURLToPath(new URL("../../../contracts/fixtures/submission_ids.json", import.meta.url)), "utf8")
);

test("keccak256 matches the known empty-input digest", () => {
  expect(bytesToHex(keccak256(new Uint8Array()))).toBe(
    "0xc5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
  );
  expect(bytesToHex(keccak256(new TextEncoder().encode("abc")))).toBe(
    "0x4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"
  );
});

test("submissionId matches every Solidity fixture without an auto payload", () => {
  const cases = fx.fixtures.filter((f: { hasAuto: boolean }) => !f.hasAuto);
  expect(cases.length).toBeGreaterThan(0);
  for (const f of cases) {
    const got = submissionId({
      bridgeDomain: f.bridgeDomain,
      debridgeId: f.debridgeId,
      amount: BigInt(f.amount),
      chainIdFrom: BigInt(f.chainIdFrom),
      chainIdTo: BigInt(f.chainIdTo),
      nonce: BigInt(f.nonce),
      receiver: hexToBytes(f.receiver),
    });
    expect(bytesToHex(got), `fixture ${f.name}`).toBe(f.submissionId);
  }
});

test("a 32-byte Solana receiver hashes at its own width, not padded", () => {
  // `long-receiver` is the fixture with a 32-byte (Solana) receiver — the case
  // that would silently break if the receiver were word-padded like the numbers.
  const f = fx.fixtures.find((x: { name: string }) => x.name === "long-receiver");
  expect(hexToBytes(f.receiver)).toHaveLength(32);
  expect(
    bytesToHex(
      submissionId({
        bridgeDomain: f.bridgeDomain,
        debridgeId: f.debridgeId,
        amount: BigInt(f.amount),
        chainIdFrom: BigInt(f.chainIdFrom),
        chainIdTo: BigInt(f.chainIdTo),
        nonce: BigInt(f.nonce),
        receiver: hexToBytes(f.receiver),
      })
    )
  ).toBe(f.submissionId);
});
