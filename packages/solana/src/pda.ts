import { PublicKey } from "@solana/web3.js";

export const TENSOR_PROGRAM_ID = new PublicKey(
  "3uztvRNHpQcS9KgbdY6NFoL9HamSZYujkH9FQWtFoP1h"
);

export function findMarginAccountPDA(
  owner: PublicKey,
  programId: PublicKey = TENSOR_PROGRAM_ID,
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("margin_account"), owner.toBuffer()],
    programId,
  );
}

export function findMarginMarketPDA(
  programId: PublicKey = TENSOR_PROGRAM_ID,
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("margin_market")],
    programId,
  );
}

export function findMarginConfigPDA(
  programId: PublicKey = TENSOR_PROGRAM_ID,
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("margin_config")],
    programId,
  );
}

export function findIntentAccountPDA(
  marginAccount: PublicKey,
  intentId: bigint,
  programId: PublicKey = TENSOR_PROGRAM_ID,
): [PublicKey, number] {
  const intentIdBuf = Buffer.alloc(8);
  intentIdBuf.writeBigUInt64LE(intentId);
  return PublicKey.findProgramAddressSync(
    [Buffer.from("intent"), marginAccount.toBuffer(), intentIdBuf],
    programId,
  );
}

export function findMarginMarketPDAByIndex(
  index: number,
  programId: PublicKey = TENSOR_PROGRAM_ID,
): [PublicKey, number] {
  const indexBuf = Buffer.alloc(2);
  indexBuf.writeUInt16LE(index);
  return PublicKey.findProgramAddressSync(
    [Buffer.from("margin_market"), indexBuf],
    programId,
  );
}

export function findSolverRegistryPDA(
  programId: PublicKey = TENSOR_PROGRAM_ID,
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("solver_registry")],
    programId,
  );
}

// ── Vaults and Northtail accounts (used by liquidate_spot) ──────

export const TOKEN_PROGRAM_ID = new PublicKey(
  "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA"
);
export const ASSOCIATED_TOKEN_PROGRAM_ID = new PublicKey(
  "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
);
export const NORTHTAIL_EXCHANGE_PROGRAM_ID = new PublicKey(
  "2NueLoUpXxihu4AT6JkEBTHkqpZE8xDgS14ZHX1qPJ1H"
);

/** vault_authority PDA: owns the protocol collateral and base vaults. */
export function findVaultAuthorityPDA(
  programId: PublicKey = TENSOR_PROGRAM_ID,
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("vault_authority")],
    programId,
  );
}

/** Associated token account of `owner` for `mint` (SPL Token). */
export function findAssociatedTokenAddress(owner: PublicKey, mint: PublicKey): PublicKey {
  return PublicKey.findProgramAddressSync(
    [owner.toBuffer(), TOKEN_PROGRAM_ID.toBuffer(), mint.toBuffer()],
    ASSOCIATED_TOKEN_PROGRAM_ID,
  )[0];
}

/** Northtail market PDA for (security_mint, quote_mint). */
export function findNorthtailMarketPDA(
  securityMint: PublicKey,
  quoteMint: PublicKey,
): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("market"), securityMint.toBuffer(), quoteMint.toBuffer()],
    NORTHTAIL_EXCHANGE_PROGRAM_ID,
  );
}

/** Northtail pool PDA for a northtail market. */
export function findNorthtailPoolPDA(northtailMarket: PublicKey): [PublicKey, number] {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("pool"), northtailMarket.toBuffer()],
    NORTHTAIL_EXCHANGE_PROGRAM_ID,
  );
}
