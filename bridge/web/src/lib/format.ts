// Display helpers shared across components.

/** Shorten a 0x hash/address to `0x1234…cdef` for tables; full value on hover. */
export function shortHex(hex: string, lead = 6, tail = 4): string {
  if (!hex.startsWith("0x")) return hex;
  if (hex.length <= 2 + lead + tail) return hex;
  return `${hex.slice(0, 2 + lead)}…${hex.slice(-tail)}`;
}

/**
 * Render a wei-scale decimal string as a human ether amount. The bridge stores
 * amounts as base-unit decimal strings (uint256) that can exceed Number range,
 * so divide with BigInt and format the fractional part by hand.
 */
export function formatAmount(raw: string, decimals = 18): string {
  let big: bigint;
  try {
    big = BigInt(raw);
  } catch {
    return raw; // not a clean integer — show as-is
  }
  const base = 10n ** BigInt(decimals);
  const whole = big / base;
  const frac = big % base;
  if (frac === 0n) return whole.toString();
  // Trim trailing zeros, keep at most 6 fractional digits.
  let fracStr = frac.toString().padStart(decimals, "0").slice(0, 6);
  fracStr = fracStr.replace(/0+$/, "");
  return fracStr ? `${whole.toString()}.${fracStr}` : whole.toString();
}

/** A 0x-prefixed receiver may be a full 32-byte word; surface the trailing
 * 20 bytes as an address when it looks like a left-padded EVM address. */
export function receiverAddress(receiver: string): string {
  const h = receiver.startsWith("0x") ? receiver.slice(2) : receiver;
  if (h.length === 64 && h.slice(0, 24) === "0".repeat(24)) {
    return "0x" + h.slice(24);
  }
  return receiver;
}
