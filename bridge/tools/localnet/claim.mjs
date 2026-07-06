// Real EVM -> Solana claim against the deployed solana-gate program.
//
// Creates an SPL mint + a program-owned vault + a receiver token account, inits
// the gate with the 3 EVM validators (threshold 2), then submits a Claim carrying
// 2 real validator signatures (from the Rust helper). Asserts the SPL is released
// to the receiver and that a replay is rejected on-chain.
//
// argv: <programId> <genBinaryPath> <payerKeypairJson>
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import {
  Connection, Keypair, PublicKey, SystemProgram,
  Transaction, TransactionInstruction, sendAndConfirmTransaction,
} from "@solana/web3.js";
import {
  TOKEN_PROGRAM_ID, createMint, getOrCreateAssociatedTokenAccount, mintTo,
} from "@solana/spl-token";

const [programIdStr, genBin, keypairPath] = process.argv.slice(2);
const programId = new PublicKey(programIdStr);
const conn = new Connection("http://127.0.0.1:8899", "confirmed");
const payer = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(readFileSync(keypairPath))));

const hexToBuf = (h) => Buffer.from(h.replace(/^0x/, ""), "hex");
const AMOUNT = 1000n;

function pda(seeds) {
  return PublicKey.findProgramAddressSync(seeds, programId)[0];
}

async function main() {
  console.log("payer:", payer.publicKey.toBase58());

  // 1. Asset: an SPL mint, a program-owned vault (pre-funded), a receiver account.
  const mint = await createMint(conn, payer, payer.publicKey, null, 0);
  const vaultAuthority = pda([Buffer.from("vault_authority")]);
  const vault = await getOrCreateAssociatedTokenAccount(conn, payer, mint, vaultAuthority, true);
  await mintTo(conn, payer, mint, vault.address, payer, 10_000n); // liquidity
  const receiverOwner = Keypair.generate();
  const receiver = await getOrCreateAssociatedTokenAccount(conn, payer, mint, receiverOwner.publicKey);
  console.log("mint:", mint.toBase58(), "\nvault:", vault.address.toBase58(),
    `(${(await conn.getTokenAccountBalance(vault.address)).value.amount})`,
    "\nreceiver:", receiver.address.toBase58());

  // 2. Instruction bytes + real validator signatures from the Rust helper,
  //    bound to this run's actual receiver token account. (2 sigs, nonce 0.)
  const recvHex = receiver.address.toBuffer().toString("hex");
  const gen = JSON.parse(execFileSync(genBin, [recvHex, "2", "0"]).toString());
  const submissionId = hexToBuf(gen.submission_id);
  console.log("submissionId:", gen.submission_id, "\nvalidators:", gen.validators.join(", "));

  const configPda = pda([Buffer.from("config")]);
  const executedPda = pda([Buffer.from("executed"), submissionId]);

  // 3. Init the gate (validators + threshold) once; reuse it on re-runs since the
  //    validator persists state (the validator set is deterministic either way).
  if (await conn.getAccountInfo(configPda)) {
    console.log("gate already initialized (reusing config)");
  } else {
    const initIx = new TransactionInstruction({
      programId,
      keys: [
        { pubkey: configPda, isSigner: false, isWritable: true },
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data: hexToBuf(gen.init_ix),
    });
    await sendAndConfirmTransaction(conn, new Transaction().add(initIx), [payer]);
    console.log("gate initialized (threshold 2 of 3)");
  }

  // 4. Claim: verify 2-of-3 sigs on-chain and release SPL to the receiver.
  const claimIx = () => new TransactionInstruction({
    programId,
    keys: [
      { pubkey: configPda, isSigner: false, isWritable: false },
      { pubkey: executedPda, isSigner: false, isWritable: true },
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: vault.address, isSigner: false, isWritable: true },
      { pubkey: receiver.address, isSigner: false, isWritable: true },
      { pubkey: vaultAuthority, isSigner: false, isWritable: false },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: hexToBuf(gen.claim_ix),
  });
  const sig = await sendAndConfirmTransaction(conn, new Transaction().add(claimIx()), [payer]);
  console.log("claim tx:", sig);

  const bal = BigInt((await conn.getTokenAccountBalance(receiver.address)).value.amount);
  if (bal !== AMOUNT) throw new Error(`receiver balance ${bal} != expected ${AMOUNT}`);
  console.log(`OK: receiver credited ${bal} (2-of-3 validator signatures verified on-chain)`);

  // 5. Replay must be rejected (the executed PDA already exists).
  let replayRejected = false;
  try {
    await sendAndConfirmTransaction(conn, new Transaction().add(claimIx()), [payer]);
  } catch {
    replayRejected = true;
  }
  if (!replayRejected) throw new Error("replay was NOT rejected");
  const bal2 = BigInt((await conn.getTokenAccountBalance(receiver.address)).value.amount);
  if (bal2 !== AMOUNT) throw new Error(`replay changed balance to ${bal2}`);
  console.log("OK: replay rejected on-chain; balance unchanged");

  // 6. Below threshold: a NEW transfer (nonce 1) carrying only 1 of 3 signatures
  //    must be refused by the on-chain verifier — no funds move without quorum.
  const receiver2 = await getOrCreateAssociatedTokenAccount(
    conn, payer, mint, Keypair.generate().publicKey);
  const gen1 = JSON.parse(
    execFileSync(genBin, [receiver2.address.toBuffer().toString("hex"), "1", "1"]).toString());
  const executed2 = pda([Buffer.from("executed"), hexToBuf(gen1.submission_id)]);
  const claim1Ix = new TransactionInstruction({
    programId,
    keys: [
      { pubkey: configPda, isSigner: false, isWritable: false },
      { pubkey: executed2, isSigner: false, isWritable: true },
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: vault.address, isSigner: false, isWritable: true },
      { pubkey: receiver2.address, isSigner: false, isWritable: true },
      { pubkey: vaultAuthority, isSigner: false, isWritable: false },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data: hexToBuf(gen1.claim_ix),
  });
  let belowRejected = false;
  try {
    await sendAndConfirmTransaction(conn, new Transaction().add(claim1Ix), [payer]);
  } catch {
    belowRejected = true;
  }
  if (!belowRejected) throw new Error("below-threshold (1-of-3) claim was NOT rejected");
  const bal3 = BigInt((await conn.getTokenAccountBalance(receiver2.address)).value.amount);
  if (bal3 !== 0n) throw new Error(`below-threshold claim moved funds: ${bal3}`);
  console.log("OK: 1-of-3 claim rejected on-chain; no funds released");

  console.log("\nPASS: EVM->Solana claim verified on a real Solana validator.");
}

main().catch((e) => { console.error("FAIL:", e.message || e); process.exit(1); });
