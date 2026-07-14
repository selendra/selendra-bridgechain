// Minimal, dependency-free EVM calldata encoding + read/write helpers for the
// exact calls this app makes. We deliberately avoid ethers/viem: every function
// we touch uses only static `address`/`uint256` args, plus dynamic `bytes` for
// Gate.send — small enough to hand-encode reliably. Selectors were taken from
// `cast sig` and are asserted by the contract ABIs.

/** An EIP-1193 `request` function (from the connected wallet). */
export type Eip1193Request = (args: { method: string; params?: unknown[] }) => Promise<unknown>;

const SEL = {
  approve: "095ea7b3", // approve(address,uint256)
  allowance: "dd62ed3e", // allowance(address,address)
  balanceOf: "70a08231", // balanceOf(address)
  decimals: "313ce567", // decimals()
  swap: "d5bcb9b5", // swap(address,address,uint256,uint256,address)
  send: "565443e9", // send(address,uint256,uint256,bytes,bytes)
} as const;

function strip0x(h: string): string {
  return h.startsWith("0x") || h.startsWith("0X") ? h.slice(2) : h;
}

function word(hex: string): string {
  return hex.padStart(64, "0");
}

function encAddress(addr: string): string {
  const a = strip0x(addr).toLowerCase();
  if (a.length !== 40 || /[^0-9a-f]/.test(a)) throw new Error(`bad address: ${addr}`);
  return word(a);
}

function encUint(v: bigint): string {
  if (v < 0n) throw new Error("negative uint");
  return word(v.toString(16));
}

/** A dynamic `bytes` tail: length word + right-padded data. */
function encBytesTail(hexData: string): string {
  const data = strip0x(hexData).toLowerCase();
  if (data.length % 2 !== 0 || /[^0-9a-f]/.test(data)) throw new Error(`bad bytes: ${hexData}`);
  const padded = data.length % 64 === 0 ? data : data + "0".repeat(64 - (data.length % 64));
  return encUint(BigInt(data.length / 2)) + padded;
}

function hexToBigInt(h: string): bigint {
  if (!h || h === "0x") return 0n;
  return BigInt(h);
}

// --- calldata builders ---------------------------------------------------

export function encodeApprove(spender: string, amount: bigint): string {
  return "0x" + SEL.approve + encAddress(spender) + encUint(amount);
}

export function encodeSwap(
  tokenIn: string,
  tokenOut: string,
  amountIn: bigint,
  minAmountOut: bigint,
  to: string
): string {
  return (
    "0x" +
    SEL.swap +
    encAddress(tokenIn) +
    encAddress(tokenOut) +
    encUint(amountIn) +
    encUint(minAmountOut) +
    encAddress(to)
  );
}

export function encodeSend(
  token: string,
  amount: bigint,
  chainIdTo: bigint,
  receiverHex: string,
  autoParamsHex: string
): string {
  // head: token, amount, chainIdTo, off(receiver), off(autoParams) => 5 words.
  const recvTail = encBytesTail(receiverHex);
  const autoTail = encBytesTail(autoParamsHex || "0x");
  const offReceiver = BigInt(5 * 32);
  const offAuto = offReceiver + BigInt(recvTail.length / 2);
  return (
    "0x" +
    SEL.send +
    encAddress(token) +
    encUint(amount) +
    encUint(chainIdTo) +
    encUint(offReceiver) +
    encUint(offAuto) +
    recvTail +
    autoTail
  );
}

// --- reads (eth_call) ----------------------------------------------------

async function ethCall(req: Eip1193Request, to: string, data: string): Promise<string> {
  return (await req({ method: "eth_call", params: [{ to, data }, "latest"] })) as string;
}

export async function readBalance(req: Eip1193Request, token: string, owner: string): Promise<bigint> {
  return hexToBigInt(await ethCall(req, token, "0x" + SEL.balanceOf + encAddress(owner)));
}

export async function readAllowance(
  req: Eip1193Request,
  token: string,
  owner: string,
  spender: string
): Promise<bigint> {
  return hexToBigInt(await ethCall(req, token, "0x" + SEL.allowance + encAddress(owner) + encAddress(spender)));
}

export async function readDecimals(req: Eip1193Request, token: string): Promise<number> {
  return Number(hexToBigInt(await ethCall(req, token, "0x" + SEL.decimals)));
}

// --- writes (eth_sendTransaction) ---------------------------------------

async function sendTx(req: Eip1193Request, from: string, to: string, data: string): Promise<string> {
  return (await req({ method: "eth_sendTransaction", params: [{ from, to, data }] })) as string;
}

export function sendApprove(
  req: Eip1193Request,
  from: string,
  token: string,
  spender: string,
  amount: bigint
): Promise<string> {
  return sendTx(req, from, token, encodeApprove(spender, amount));
}

export function sendSwap(
  req: Eip1193Request,
  from: string,
  pool: string,
  tokenIn: string,
  tokenOut: string,
  amountIn: bigint,
  minAmountOut: bigint,
  to: string
): Promise<string> {
  return sendTx(req, from, pool, encodeSwap(tokenIn, tokenOut, amountIn, minAmountOut, to));
}

export function sendBridge(
  req: Eip1193Request,
  from: string,
  gate: string,
  token: string,
  amount: bigint,
  chainIdTo: number,
  receiverHex: string,
  autoParamsHex = "0x"
): Promise<string> {
  return sendTx(req, from, gate, encodeSend(token, amount, BigInt(chainIdTo), receiverHex, autoParamsHex));
}

// --- confirmation --------------------------------------------------------

/** Poll for a receipt; resolves { success } once mined, throws on timeout. */
export async function waitReceipt(
  req: Eip1193Request,
  hash: string,
  timeoutMs = 90_000
): Promise<{ success: boolean }> {
  const start = Date.now();
  while (Date.now() - start < timeoutMs) {
    const r = (await req({ method: "eth_getTransactionReceipt", params: [hash] })) as {
      blockNumber?: string;
      status?: string;
    } | null;
    if (r && r.blockNumber) return { success: r.status === "0x1" };
    await new Promise((res) => setTimeout(res, 1200));
  }
  throw new Error("Timed out waiting for confirmation");
}

/** Normalize a wallet/RPC error into a short human message. */
export function errMsg(e: unknown): string {
  const code = (e as { code?: number })?.code;
  const msg = e instanceof Error ? e.message : String(e);
  if (code === 4001 || /reject|denied/i.test(msg)) return "Rejected in wallet";
  // trim revert noise
  const m = msg.match(/reverted[^:]*:?\s*(.*)/i);
  return (m?.[1] || msg).slice(0, 160);
}
