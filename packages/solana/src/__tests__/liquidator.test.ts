import { describe, it, expect } from "vitest";
import { PublicKey } from "@solana/web3.js";
import { anchorDiscriminator, liquidateIx, liquidateSpotIx } from "../ix.js";
import { AccountHealth, type OnChainMarginAccount } from "../accounts.js";
import {
  needsSpotLiquidation,
  planSpotChunk,
  isSpotSellable,
  parseNorthtailMarket,
  spotLiquidationMinOut,
} from "../liquidator.js";

describe("liquidateIx", () => {
  const programId = PublicKey.unique();
  const marginAccount = PublicKey.unique();
  const market = PublicKey.unique();
  const config = PublicKey.unique();
  const liquidator = PublicKey.unique();

  it("creates instruction with correct program ID", () => {
    const ix = liquidateIx(marginAccount, market, config, liquidator, programId);
    expect(ix.programId.equals(programId)).toBe(true);
  });

  it("has 4 account keys in correct order", () => {
    const ix = liquidateIx(marginAccount, market, config, liquidator, programId);
    expect(ix.keys).toHaveLength(4);
    expect(ix.keys[0].pubkey.equals(marginAccount)).toBe(true);
    expect(ix.keys[1].pubkey.equals(market)).toBe(true);
    expect(ix.keys[2].pubkey.equals(config)).toBe(true);
    expect(ix.keys[3].pubkey.equals(liquidator)).toBe(true);
  });

  it("margin_account is writable, not signer", () => {
    const ix = liquidateIx(marginAccount, market, config, liquidator, programId);
    expect(ix.keys[0].isWritable).toBe(true);
    expect(ix.keys[0].isSigner).toBe(false);
  });

  it("market is writable, not signer", () => {
    const ix = liquidateIx(marginAccount, market, config, liquidator, programId);
    expect(ix.keys[1].isWritable).toBe(true);
    expect(ix.keys[1].isSigner).toBe(false);
  });

  it("config is writable, not signer", () => {
    const ix = liquidateIx(marginAccount, market, config, liquidator, programId);
    expect(ix.keys[2].isWritable).toBe(true);
    expect(ix.keys[2].isSigner).toBe(false);
  });

  it("liquidator is signer and writable", () => {
    const ix = liquidateIx(marginAccount, market, config, liquidator, programId);
    expect(ix.keys[3].isSigner).toBe(true);
    expect(ix.keys[3].isWritable).toBe(true);
  });

  it("instruction data is the liquidate discriminator", () => {
    const ix = liquidateIx(marginAccount, market, config, liquidator, programId);
    const expectedDisc = anchorDiscriminator("liquidate");
    expect(Buffer.from(ix.data).equals(expectedDisc)).toBe(true);
  });

  it("discriminator differs from other instructions", () => {
    const liqDisc = anchorDiscriminator("liquidate");
    const settleDisc = anchorDiscriminator("settle_auction");
    const marginDisc = anchorDiscriminator("compute_margin");
    expect(liqDisc.equals(settleDisc)).toBe(false);
    expect(liqDisc.equals(marginDisc)).toBe(false);
  });
});

describe("AccountHealth enum values", () => {
  it("Liquidatable = 2, Bankrupt = 3", () => {
    expect(AccountHealth.Liquidatable).toBe(2);
    expect(AccountHealth.Bankrupt).toBe(3);
  });

  it("has correct ordering for triage", () => {
    expect(AccountHealth.Healthy).toBeLessThan(AccountHealth.Warning);
    expect(AccountHealth.Warning).toBeLessThan(AccountHealth.Liquidatable);
    expect(AccountHealth.Liquidatable).toBeLessThan(AccountHealth.Bankrupt);
  });
});

describe("liquidateSpotIx", () => {
  const programId = PublicKey.unique();
  const accounts = {
    marginAccount: PublicKey.unique(),
    market: PublicKey.unique(),
    config: PublicKey.unique(),
    vaultAuthority: PublicKey.unique(),
    collateralVault: PublicKey.unique(),
    baseVault: PublicKey.unique(),
    exchangeMarket: PublicKey.unique(),
    pool: PublicKey.unique(),
    poolAuthority: PublicKey.unique(),
    poolSecurityVault: PublicKey.unique(),
    poolQuoteVault: PublicKey.unique(),
    exchangeProgram: PublicKey.unique(),
    tokenProgram: PublicKey.unique(),
    liquidator: PublicKey.unique(),
  };

  it("orders the 14 accounts like the LiquidateSpot context", () => {
    const ix = liquidateSpotIx(accounts, 5n, 7n, programId);
    const order = [
      accounts.marginAccount, accounts.market, accounts.config, accounts.vaultAuthority,
      accounts.collateralVault, accounts.baseVault, accounts.exchangeMarket, accounts.pool,
      accounts.poolAuthority, accounts.poolSecurityVault, accounts.poolQuoteVault,
      accounts.exchangeProgram, accounts.tokenProgram, accounts.liquidator,
    ];
    expect(ix.keys).toHaveLength(14);
    order.forEach((k, i) => expect(ix.keys[i].pubkey.equals(k)).toBe(true));
    expect(ix.keys[13].isSigner).toBe(true);
    expect(ix.keys.filter((k) => k.isSigner)).toHaveLength(1);
  });

  it("encodes discriminator + amount_in + min_amount_out", () => {
    const ix = liquidateSpotIx(accounts, 5n, 7n, programId);
    const data = Buffer.from(ix.data);
    expect(data.subarray(0, 8).equals(anchorDiscriminator("liquidate_spot"))).toBe(true);
    expect(data.readBigUInt64LE(8)).toBe(5n);
    expect(data.readBigUInt64LE(16)).toBe(7n);
    expect(data.length).toBe(24);
  });
});

describe("needsSpotLiquidation", () => {
  const inactive = { isActive: false };
  const make = (perp: boolean, option: boolean, spot: boolean) =>
    ({
      perpPositions: [perp ? { isActive: true } : inactive],
      optionPositions: [option ? { isActive: true } : inactive],
      spotBalances: [spot ? { isActive: true } : inactive],
    }) as unknown as OnChainMarginAccount;

  it("only when spot is the next waterfall step", () => {
    expect(needsSpotLiquidation(make(false, false, true))).toBe(true);
    expect(needsSpotLiquidation(make(true, false, true))).toBe(false);
    expect(needsSpotLiquidation(make(false, true, true))).toBe(false);
    expect(needsSpotLiquidation(make(false, false, false))).toBe(false);
  });
});

describe("liquidateIx spot proof", () => {
  it("appends the proof accounts read-only after the liquidator", () => {
    const keys = [PublicKey.unique(), PublicKey.unique(), PublicKey.unique()];
    const ix = liquidateIx(
      PublicKey.unique(), PublicKey.unique(), PublicKey.unique(), PublicKey.unique(),
      PublicKey.unique(), keys,
    );
    expect(ix.keys).toHaveLength(7);
    keys.forEach((k, i) => {
      expect(ix.keys[4 + i].pubkey.equals(k)).toBe(true);
      expect(ix.keys[4 + i].isWritable).toBe(false);
      expect(ix.keys[4 + i].isSigner).toBe(false);
    });
  });
});

describe("spot sale planning", () => {
  const MARK = 150_000_000n;
  const S = 1_000_000_000n;
  const nt = { tradingFeeBps: 30, minTradeSize: 1000n, maxTradeSize: 0n, isTrading: true };
  const pool = (price: bigint) => ({
    securityLiquidity: S,
    quoteLiquidity: (S * price) / 1_000_000n,
    authority: PublicKey.unique(),
    securityVault: PublicKey.unique(),
    quoteVault: PublicKey.unique(),
    isActive: true,
  });

  it("bounds a large sale by the price-impact cap", () => {
    const c = planSpotChunk(S, MARK, nt, pool(MARK), 100n, 50n)!;
    expect(c.amountIn).toBe((S * 100n) / 9_900n);
    expect(c.minAmountOut >= spotLiquidationMinOut(c.amountIn, MARK)).toBe(true);
  });

  it("bounds a sale by max_trade_size", () => {
    const c = planSpotChunk(S, MARK, { ...nt, maxTradeSize: 5000n }, pool(MARK), 100n, 50n)!;
    expect(c.amountIn).toBe(5000n);
  });

  it("returns null for dust and for a pool below the floor", () => {
    expect(planSpotChunk(999n, MARK, nt, pool(MARK), 100n, 50n)).toBeNull();
    expect(planSpotChunk(4000n, MARK, nt, pool((MARK * 90n) / 100n), 100n, 50n)).toBeNull();
  });

  it("isSpotSellable mirrors the on-chain check", () => {
    expect(isSpotSellable(true, MARK, nt, pool(MARK), 5000n)).toBe(true);
    expect(isSpotSellable(true, MARK, nt, pool(MARK), 999n)).toBe(false);
    expect(isSpotSellable(false, MARK, nt, pool(MARK), 5000n)).toBe(false);
    expect(isSpotSellable(true, MARK, { ...nt, isTrading: false }, pool(MARK), 5000n)).toBe(false);
    expect(isSpotSellable(true, MARK, { ...nt, maxTradeSize: 999n }, pool(MARK), 5000n)).toBe(false);
    expect(isSpotSellable(true, 0n, nt, pool(MARK), 5000n)).toBe(false);
    // A pool below the floor is still sellable: the bot waits instead of skipping
    expect(isSpotSellable(true, MARK, nt, pool((MARK * 90n) / 100n), 5000n)).toBe(true);
    expect(planSpotChunk(5000n, MARK, nt, pool((MARK * 90n) / 100n), 100n, 50n)).toBeNull();
  });

  it("parses a northtail Market", () => {
    const d = Buffer.alloc(300);
    d.writeUInt16LE(30, 138);
    d.writeBigUInt64LE(1000n, 142);
    d.writeBigUInt64LE(7n, 150);
    d.writeUInt32LE(3, 190); // symbol len
    d.writeUInt32LE(6, 197); // name len
    d.writeUInt8(1, 207); // isin Some
    d.writeUInt8(1, 220); // is_active
    expect(parseNorthtailMarket(d)).toEqual({
      tradingFeeBps: 30, minTradeSize: 1000n, maxTradeSize: 7n, isTrading: true,
    });
  });
});
