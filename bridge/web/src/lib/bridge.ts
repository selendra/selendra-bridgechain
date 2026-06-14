// On-chain bridging: lock funds on the source Gate via send(), which emits the
// Sent event the validators watch. This mirrors scripts/anytoany.sh:
//   approve(gate, amount)  ->  gate.send(token, amount, chainIdTo, receiver, "0x")
import {
  BrowserProvider,
  Contract,
  type ContractRunner,
  type ContractTransactionResponse,
  JsonRpcProvider,
  getAddress,
  hexlify,
  parseUnits,
} from "ethers";

// ethers v6 types methods reached through the dynamic proxy as possibly
// undefined; describe the ABI surface we use and cast the Contract to it.
// (We don't `extends Contract` — that brings a string index signature that
//  conflicts with these concrete method types.)
interface Erc20 {
  decimals(): Promise<bigint>;
  symbol(): Promise<string>;
  balanceOf(owner: string): Promise<bigint>;
  allowance(owner: string, spender: string): Promise<bigint>;
  approve(spender: string, amount: bigint): Promise<ContractTransactionResponse>;
}

interface GateContract {
  send(
    token: string,
    amount: bigint,
    chainIdTo: bigint,
    receiver: string,
    autoParams: string,
  ): Promise<ContractTransactionResponse>;
  /** Used to decode the Sent event from the receipt logs. */
  interface: Contract["interface"];
}

function erc20At(address: string, runner: ContractRunner): Erc20 {
  return new Contract(getAddress(address), ERC20_ABI, runner) as unknown as Erc20;
}

function gateAt(address: string, runner: ContractRunner): GateContract {
  return new Contract(getAddress(address), GATE_ABI, runner) as unknown as GateContract;
}

export const GATE_ABI = [
  "function send(address token, uint256 amount, uint256 chainIdTo, bytes receiver, bytes autoParams) returns (bytes32 submissionId)",
  "event Sent(bytes32 indexed submissionId, bytes32 indexed debridgeId, uint256 amount, uint256 chainIdFrom, uint256 chainIdTo, bytes receiver, uint256 nonce, bytes autoParams, bytes nativeSender)",
];

export const ERC20_ABI = [
  "function decimals() view returns (uint8)",
  "function symbol() view returns (string)",
  "function balanceOf(address) view returns (uint256)",
  "function allowance(address owner, address spender) view returns (uint256)",
  "function approve(address spender, uint256 amount) returns (bool)",
];

export interface SendParams {
  gate: string;
  token: string;
  /** Human-readable amount, e.g. "1.5". */
  amount: string;
  chainIdTo: number;
  /** Destination recipient as a 20-byte EVM address (0x + 40 hex). */
  receiver: string;
}

export interface SendResult {
  submissionId: string;
  txHash: string;
  approvalTxHash?: string;
  rawAmount: bigint;
  symbol: string;
}

/** Validate + normalize the receiver to a checksummed 20-byte address. */
export function normalizeReceiver(receiver: string): string {
  // getAddress throws on anything that isn't a valid 20-byte address.
  return getAddress(receiver.trim());
}

export interface TokenMeta {
  decimals: number;
  symbol: string;
  balance: bigint;
}

/** Read decimals/symbol/balance for the connected account, off the wallet
 *  provider (the wallet is on the source chain when bridging). */
export async function readToken(
  provider: BrowserProvider,
  token: string,
  account: string,
): Promise<TokenMeta> {
  const erc20 = erc20At(token, provider);
  const [decimals, symbol, balance] = await Promise.all([
    erc20.decimals(),
    erc20.symbol().catch(() => "TOKEN"),
    erc20.balanceOf(account),
  ]);
  return { decimals: Number(decimals), symbol, balance };
}

export type SendProgress = (stage: "approving" | "sending" | "confirming") => void;

/**
 * Approve (only if needed) then call Gate.send, wait for the receipt, and pull
 * the submissionId out of the Sent event.
 */
export async function bridgeSend(
  provider: BrowserProvider,
  params: SendParams,
  onProgress?: SendProgress,
): Promise<SendResult> {
  const signer = await provider.getSigner();
  const account = await signer.getAddress();

  const token = getAddress(params.token);
  const gate = getAddress(params.gate);
  const receiver = normalizeReceiver(params.receiver);

  const erc20 = erc20At(token, signer);
  const decimals = Number(await erc20.decimals());
  const symbol = await erc20.symbol().catch(() => "TOKEN");
  const rawAmount = parseUnits(params.amount.trim(), decimals);
  if (rawAmount <= 0n) throw new Error("Amount must be greater than zero.");

  const balance = await erc20.balanceOf(account);
  if (balance < rawAmount) {
    throw new Error(
      `Insufficient ${symbol}: balance is ${balance} but ${rawAmount} required (raw units).`,
    );
  }

  // Approve the gate to pull the tokens, only if the current allowance is short.
  let approvalTxHash: string | undefined;
  const allowance = await erc20.allowance(account, gate);
  if (allowance < rawAmount) {
    onProgress?.("approving");
    const approveTx = await erc20.approve(gate, rawAmount);
    approvalTxHash = approveTx.hash;
    await approveTx.wait();
  }

  // receiver must be exactly 20 packed bytes (the Gate rejects other lengths).
  const receiverBytes = hexlify(receiver);

  onProgress?.("sending");
  const gateContract = gateAt(gate, signer);
  const tx = await gateContract.send(
    token,
    rawAmount,
    BigInt(params.chainIdTo),
    receiverBytes,
    "0x",
  );

  onProgress?.("confirming");
  const receipt = await tx.wait();

  // Pull submissionId from the Sent event (indexed topic[1]).
  let submissionId = "";
  for (const log of receipt?.logs ?? []) {
    try {
      const parsed = gateContract.interface.parseLog(log);
      if (parsed?.name === "Sent") {
        submissionId = parsed.args.submissionId as string;
        break;
      }
    } catch {
      // not a Gate log
    }
  }

  return { submissionId, txHash: tx.hash, approvalTxHash, rawAmount, symbol };
}

/** A read-only provider for a given RPC (used for decimals when the wallet is
 *  on a different chain). Currently informational; reserved for future use. */
export function rpcProvider(rpcUrl: string): JsonRpcProvider {
  return new JsonRpcProvider(rpcUrl);
}
