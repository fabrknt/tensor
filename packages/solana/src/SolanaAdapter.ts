import { Connection, PublicKey } from "@solana/web3.js";
import { Program, AnchorProvider } from "@coral-xyz/anchor";
import type { ChainAdapter, Chain, Position, TradingIntent } from "@tensor/core";
import { TENSOR_PROGRAM_ID, findMarginAccountPDA, findMarginMarketPDA } from "./pda.js";
import { PRECISION, type OnChainMarginAccount, type OnChainMarginMarket } from "./accounts.js";

/**
 * Solana implementation of the Tensor ChainAdapter.
 * Reads MarginAccount and MarginMarket state from the on-chain Anchor program.
 */
export class SolanaAdapter implements ChainAdapter {
  readonly chain: Chain = "solana";
  private connection: Connection;
  private programId: PublicKey;

  constructor(
    connection: Connection,
    programId: PublicKey = TENSOR_PROGRAM_ID,
  ) {
    this.connection = connection;
    this.programId = programId;
  }

  async getPositions(account: string): Promise<Position[]> {
    const owner = new PublicKey(account);
    const [pda] = findMarginAccountPDA(owner, this.programId);
    const accountInfo = await this.connection.getAccountInfo(pda);
    if (!accountInfo) return [];

    // Decode via Anchor IDL (requires IDL to be loaded)
    // For now, delegate to the program's account parser
    const marginAccount = await this.fetchMarginAccount(owner);
    if (!marginAccount) return [];

    const positions: Position[] = [];

    // Convert perp positions
    for (let i = 0; i < marginAccount.perpCount; i++) {
      const perp = marginAccount.perpPositions[i];
      if (!perp.isActive) continue;
      positions.push({
        asset: `MARKET-${perp.marketIndex}`,
        side: Number(perp.size) >= 0 ? "long" : "short",
        size: Math.abs(Number(perp.size)) / PRECISION,
        entry_price: Number(perp.entryPrice) / PRECISION,
        mark_price: 0, // Filled by getMarkPrices
        instrument_type: "perpetual",
      });
    }

    // Convert spot balances
    for (let i = 0; i < marginAccount.spotCount; i++) {
      const spot = marginAccount.spotBalances[i];
      if (!spot.isActive) continue;
      positions.push({
        asset: `MARKET-${spot.marketIndex}`,
        side: "long",
        size: Number(spot.balance) / PRECISION,
        entry_price: 0,
        mark_price: Number(spot.value) / PRECISION,
        instrument_type: "spot",
      });
    }

    // Convert option positions
    for (let i = 0; i < marginAccount.optionCount; i++) {
      const opt = marginAccount.optionPositions[i];
      if (!opt.isActive) continue;
      positions.push({
        asset: `MARKET-${opt.marketIndex}`,
        side: Number(opt.contracts) >= 0 ? "long" : "short",
        size: Math.abs(Number(opt.contracts)) / PRECISION,
        entry_price: Number(opt.strike) / PRECISION,
        mark_price: 0,
        instrument_type: "option",
        option_type: opt.side === 0 ? "call" : "put",
        strike: Number(opt.strike) / PRECISION,
        expiry: new Date(Number(opt.expiry) * 1000).toISOString(),
      });
    }

    // Convert lending positions
    for (let i = 0; i < marginAccount.lendingCount; i++) {
      const lend = marginAccount.lendingPositions[i];
      if (!lend.isActive) continue;
      positions.push({
        asset: `MARKET-${lend.marketIndex}`,
        side: lend.side === 0 ? "long" : "short",
        size: Number(lend.principal) / PRECISION,
        entry_price: 0,
        mark_price: Number(lend.effectiveValue) / PRECISION,
        instrument_type: "lending",
      });
    }

    return positions;
  }

  async getCollateral(account: string): Promise<number> {
    const marginAccount = await this.fetchMarginAccount(new PublicKey(account));
    if (!marginAccount) return 0;
    return Number(marginAccount.collateral) / PRECISION;
  }

  async getMarkPrices(assets: string[]): Promise<Record<string, number>> {
    const prices: Record<string, number> = {};
    // MarginMarket accounts store mark prices, updated by keepers
    // In production, iterate registered markets and match by symbol/index
    // For now, return empty — prices are populated by keepers on-chain
    return prices;
  }

  async submitIntent(
    intent: TradingIntent,
  ): Promise<{ txId: string }> {
    // Intent submission requires a wallet signer, which the adapter
    // doesn't hold. Callers should use the Anchor program directly
    // for write operations. This read-focused adapter throws to
    // make the limitation explicit.
    throw new Error(
      "SolanaAdapter is read-only. Use the Anchor program directly to submit intents.",
    );
  }

  // ─── Internal helpers ──────────────────────────────────────────────

  private async fetchMarginAccount(
    owner: PublicKey,
  ): Promise<OnChainMarginAccount | null> {
    const [pda] = findMarginAccountPDA(owner, this.programId);
    const accountInfo = await this.connection.getAccountInfo(pda);
    if (!accountInfo) return null;

    // Account deserialization depends on the Anchor IDL being available.
    // When the IDL is loaded via `Program`, Anchor handles this automatically.
    // For raw deserialization without IDL, use borsh decoding against
    // the OnChainMarginAccount layout.
    //
    // This is a placeholder — real deserialization should be wired to
    // the generated IDL types from `anchor build`.
    throw new Error(
      "Raw account deserialization not yet implemented. " +
      "Load the IDL via @coral-xyz/anchor Program for automatic decoding.",
    );
  }
}
