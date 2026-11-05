import { PublicKey, TransactionInstruction } from "@solana/web3.js";
import { createHash } from "crypto";

// ── Discriminator helper ─────────────────────────────────────────

/** Compute Anchor instruction discriminator: sha256("global:<name>")[0..8] */
export function anchorDiscriminator(name: string): Buffer {
  const hash = createHash("sha256").update(`global:${name}`).digest();
  return hash.subarray(0, 8);
}

// ── Instruction builders ─────────────────────────────────────────

/**
 * Build a `settle_auction` instruction.
 * Accounts: [intent_account (writable)]
 */
export function settleAuctionIx(
  intentAccount: PublicKey,
  programId: PublicKey,
): TransactionInstruction {
  const data = anchorDiscriminator("settle_auction");
  return new TransactionInstruction({
    programId,
    keys: [
      { pubkey: intentAccount, isSigner: false, isWritable: true },
    ],
    data,
  });
}

/**
 * Build a `compute_margin` instruction.
 * Accounts: [margin_account (writable), config (readonly), ...market_accounts (readonly)]
 */
export function computeMarginIx(
  marginAccount: PublicKey,
  config: PublicKey,
  marketAccounts: PublicKey[],
  programId: PublicKey,
): TransactionInstruction {
  const data = anchorDiscriminator("compute_margin");
  return new TransactionInstruction({
    programId,
    keys: [
      { pubkey: marginAccount, isSigner: false, isWritable: true },
      { pubkey: config, isSigner: false, isWritable: false },
      ...marketAccounts.map((pubkey) => ({
        pubkey,
        isSigner: false,
        isWritable: false,
      })),
    ],
    data,
  });
}

// ── Vol surface update ───────────────────────────────────────────

export interface UpdateVolSurfaceParams {
  /** IV in bps — [4][9] grid */
  vol_surface: number[][];
  /** Moneyness nodes — 1e6 fixed-point, length 9 */
  moneyness_nodes: number[];
  /** Expiry days — length 4 */
  expiry_days: number[];
  /** Number of moneyness nodes (max 9) */
  node_count: number;
  /** Number of expiry rows (max 4) */
  expiry_count: number;
}

/**
 * Borsh-serialize UpdateVolSurfaceParams into a Buffer.
 * Layout: 4×9 u32 grid + 9 u32 moneyness + 4 u32 expiry + u8 node_count + u8 expiry_count
 */
function serializeUpdateVolSurfaceParams(params: UpdateVolSurfaceParams): Buffer {
  const VOL_ROWS = 4;
  const VOL_COLS = 9;
  // vol_surface: 4*9*4 = 144 bytes
  // moneyness_nodes: 9*4 = 36 bytes
  // expiry_days: 4*4 = 16 bytes
  // node_count: 1 byte
  // expiry_count: 1 byte
  const size = VOL_ROWS * VOL_COLS * 4 + VOL_COLS * 4 + VOL_ROWS * 4 + 1 + 1;
  const buf = Buffer.alloc(size);
  let offset = 0;

  // vol_surface [4][9] as u32 LE
  for (let r = 0; r < VOL_ROWS; r++) {
    for (let c = 0; c < VOL_COLS; c++) {
      const val = params.vol_surface[r]?.[c] ?? 0;
      buf.writeUInt32LE(val, offset);
      offset += 4;
    }
  }

  // moneyness_nodes [9] as u32 LE
  for (let i = 0; i < VOL_COLS; i++) {
    buf.writeUInt32LE(params.moneyness_nodes[i] ?? 0, offset);
    offset += 4;
  }

  // expiry_days [4] as u32 LE
  for (let i = 0; i < VOL_ROWS; i++) {
    buf.writeUInt32LE(params.expiry_days[i] ?? 0, offset);
    offset += 4;
  }

  buf.writeUInt8(params.node_count, offset);
  offset += 1;
  buf.writeUInt8(params.expiry_count, offset);

  return buf;
}

/**
 * Build a `liquidate` instruction. At the spot step it fails with
 * SpotLiquidationRequired unless every spot balance is unsellable (pass
 * `spotProof` so the program can check); sell with `liquidateSpotIx`.
 * Accounts: [margin_account (writable), market (writable), config (writable), liquidator (signer)]
 */
export function liquidateIx(
  marginAccount: PublicKey,
  market: PublicKey,
  config: PublicKey,
  liquidator: PublicKey,
  programId: PublicKey,
  /** Spot step only: [MarginMarket, northtail Market, northtail Pool] per active spot slot, in slot order */
  spotProof: PublicKey[] = [],
): TransactionInstruction {
  const data = anchorDiscriminator("liquidate");
  return new TransactionInstruction({
    programId,
    keys: [
      { pubkey: marginAccount, isSigner: false, isWritable: true },
      { pubkey: market, isSigner: false, isWritable: true },
      { pubkey: config, isSigner: false, isWritable: true },
      { pubkey: liquidator, isSigner: true, isWritable: true },
      ...spotProof.map((pubkey) => ({ pubkey, isSigner: false, isWritable: false })),
    ],
    data,
  });
}

export interface LiquidateSpotAccounts {
  marginAccount: PublicKey;
  market: PublicKey;
  config: PublicKey;
  vaultAuthority: PublicKey;
  collateralVault: PublicKey;
  baseVault: PublicKey;
  exchangeMarket: PublicKey;
  pool: PublicKey;
  poolAuthority: PublicKey;
  poolSecurityVault: PublicKey;
  poolQuoteVault: PublicKey;
  exchangeProgram: PublicKey;
  tokenProgram: PublicKey;
  liquidator: PublicKey;
}

/**
 * Build a `liquidate_spot` instruction: sells `amountIn` of the account's
 * spot balance for market.base_mint through Northtail. The program enforces
 * a floor from the market mark price; `minAmountOut` can only raise it.
 * Data: discriminator + amount_in (u64) + min_amount_out (u64).
 */
export function liquidateSpotIx(
  accounts: LiquidateSpotAccounts,
  amountIn: bigint,
  minAmountOut: bigint,
  programId: PublicKey,
): TransactionInstruction {
  const args = Buffer.alloc(16);
  args.writeBigUInt64LE(amountIn, 0);
  args.writeBigUInt64LE(minAmountOut, 8);
  const data = Buffer.concat([anchorDiscriminator("liquidate_spot"), args]);
  const a = accounts;
  return new TransactionInstruction({
    programId,
    keys: [
      { pubkey: a.marginAccount, isSigner: false, isWritable: true },
      { pubkey: a.market, isSigner: false, isWritable: true },
      { pubkey: a.config, isSigner: false, isWritable: true },
      { pubkey: a.vaultAuthority, isSigner: false, isWritable: true },
      { pubkey: a.collateralVault, isSigner: false, isWritable: true },
      { pubkey: a.baseVault, isSigner: false, isWritable: true },
      { pubkey: a.exchangeMarket, isSigner: false, isWritable: true },
      { pubkey: a.pool, isSigner: false, isWritable: true },
      { pubkey: a.poolAuthority, isSigner: false, isWritable: false },
      { pubkey: a.poolSecurityVault, isSigner: false, isWritable: true },
      { pubkey: a.poolQuoteVault, isSigner: false, isWritable: true },
      { pubkey: a.exchangeProgram, isSigner: false, isWritable: false },
      { pubkey: a.tokenProgram, isSigner: false, isWritable: false },
      { pubkey: a.liquidator, isSigner: true, isWritable: true },
    ],
    data,
  });
}

/**
 * Build an `update_vol_surface` instruction.
 * Accounts: [market (writable), config (readonly), authority (signer)]
 */
export function updateVolSurfaceIx(
  market: PublicKey,
  config: PublicKey,
  authority: PublicKey,
  params: UpdateVolSurfaceParams,
  programId: PublicKey,
): TransactionInstruction {
  const disc = anchorDiscriminator("update_vol_surface");
  const args = serializeUpdateVolSurfaceParams(params);
  const data = Buffer.concat([disc, args]);

  return new TransactionInstruction({
    programId,
    keys: [
      { pubkey: market, isSigner: false, isWritable: true },
      { pubkey: config, isSigner: false, isWritable: false },
      { pubkey: authority, isSigner: true, isWritable: false },
    ],
    data,
  });
}
