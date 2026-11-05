import {
  Connection,
  Keypair,
  PublicKey,
  Transaction,
  sendAndConfirmTransaction,
} from "@solana/web3.js";
import { createHash } from "crypto";
import {
  TENSOR_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  NORTHTAIL_EXCHANGE_PROGRAM_ID,
  findMarginConfigPDA,
  findMarginMarketPDAByIndex,
  findVaultAuthorityPDA,
  findAssociatedTokenAddress,
  findNorthtailMarketPDA,
  findNorthtailPoolPDA,
} from "./pda.js";
import { decodeMarginAccount, decodeMarginMarket } from "./decoder.js";
import { AccountHealth, type OnChainMarginAccount } from "./accounts.js";
import { liquidateIx, liquidateSpotIx, computeMarginIx } from "./ix.js";

// ── Types ────────────────────────────────────────────────────────

export interface LiquidatorConfig {
  /** RPC endpoint */
  rpcUrl: string;
  /** Keypair for signing transactions */
  signerSecretKey: Uint8Array;
  /** Program ID (default: TENSOR_PROGRAM_ID) */
  programId?: string;
  /** Poll interval in ms (default: 5_000) */
  pollIntervalMs?: number;
  /** Max liquidations per cycle (default: 5) */
  maxLiquidationsPerCycle?: number;
  /** Whether to refresh margin before liquidating stale accounts (default: true) */
  refreshMarginFirst?: boolean;
  /** Margin staleness threshold in seconds (default: 30) */
  marginStalenessSeconds?: number;
  /** Spot step: max pool price impact per liquidate_spot chunk, in bps (default: 100) */
  spotMaxImpactBps?: number;
  /** Spot step: tolerance below the expected output for min_amount_out, in bps (default: 50) */
  spotSlippageBps?: number;
  /** Log function (default: console.log) */
  log?: (...args: unknown[]) => void;
}

export interface LiquidationEvent {
  marginAccount: PublicKey;
  health: AccountHealth;
  equity: bigint;
  maintenanceMargin: bigint;
  txSignature: string;
}

export interface LiquidatorStats {
  cyclesRun: number;
  accountsScanned: number;
  liquidationsAttempted: number;
  liquidationsSucceeded: number;
  liquidationsFailed: number;
  marginRefreshes: number;
}

// ── Discriminators ───────────────────────────────────────────────

function accountDiscriminator(name: string): Buffer {
  return createHash("sha256").update(`account:${name}`).digest().subarray(0, 8);
}

const MARGIN_ACCOUNT_DISC = accountDiscriminator("MarginAccount");

// ── Spot step of the waterfall ───────────────────────────────────

/** MarginConfig.collateral_mint offset: discriminator + authority + fee_collector. */
const CONFIG_COLLATERAL_MINT_OFFSET = 8 + 32 + 32;
/** Northtail Pool offsets (after the 8-byte discriminator). */
const POOL_AUTHORITY_OFFSET = 96;
const POOL_SECURITY_VAULT_OFFSET = 128;
const POOL_QUOTE_VAULT_OFFSET = 160;

/**
 * The on-chain waterfall reaches spot only when no option or perp is open.
 * Mirrors tensor_math::liquidation_priority for that one decision.
 */
export function needsSpotLiquidation(decoded: OnChainMarginAccount): boolean {
  const hasPerp = decoded.perpPositions.some((p) => p.isActive);
  const hasOption = decoded.optionPositions.some((o) => o.isActive);
  const hasSpot = decoded.spotBalances.some((s) => s.isActive);
  return !hasPerp && !hasOption && hasSpot;
}

// ── Northtail account readers (layouts as in tensor-cpi northtail.rs) ──

export interface NorthtailMarketInfo {
  tradingFeeBps: number;
  minTradeSize: bigint;
  /** 0n = no maximum */
  maxTradeSize: bigint;
  /** is_active && status == Active */
  isTrading: boolean;
}

export function parseNorthtailMarket(data: Buffer): NorthtailMarketInfo {
  const status = data.readUInt8(105);
  const tradingFeeBps = data.readUInt16LE(138);
  const minTradeSize = data.readBigUInt64LE(142);
  const maxTradeSize = data.readBigUInt64LE(150);
  let at = 190;
  for (let i = 0; i < 2; i++) at += 4 + data.readUInt32LE(at); // symbol, name
  at += 1 + (data.readUInt8(at) === 1 ? 12 : 0); // isin
  const isActive = data.readUInt8(at) !== 0;
  return { tradingFeeBps, minTradeSize, maxTradeSize, isTrading: isActive && status === 0 };
}

export interface NorthtailPoolInfo {
  securityLiquidity: bigint;
  quoteLiquidity: bigint;
  authority: PublicKey;
  securityVault: PublicKey;
  quoteVault: PublicKey;
  isActive: boolean;
}

export function parseNorthtailPool(data: Buffer): NorthtailPoolInfo {
  const key = (off: number) => new PublicKey(data.subarray(off, off + 32));
  return {
    securityLiquidity: data.readBigUInt64LE(40),
    quoteLiquidity: data.readBigUInt64LE(48),
    authority: key(POOL_AUTHORITY_OFFSET),
    securityVault: key(POOL_SECURITY_VAULT_OFFSET),
    quoteVault: key(POOL_QUOTE_VAULT_OFFSET),
    isActive: data.readUInt8(256) !== 0,
  };
}

// ── Spot sale planning (mirrors the on-chain checks) ─────────────

/** On-chain MAX_SPOT_LIQUIDATION_SLIPPAGE_BPS. */
export const MAX_SPOT_LIQUIDATION_SLIPPAGE_BPS = 500n;

/** Constant-product output with fee, as northtail computes it. */
export function northtailSwapOutput(amountIn: bigint, inReserve: bigint, outReserve: bigint, feeBps: number): bigint {
  if (inReserve === 0n || outReserve === 0n) return 0n;
  const withFee = amountIn - (amountIn * BigInt(feeBps)) / 10_000n;
  return (withFee * outReserve) / (inReserve + withFee);
}

/** On-chain floor: amount_in * mark / 1e6, less 5%. */
export function spotLiquidationMinOut(amountIn: bigint, markPrice: bigint): bigint {
  return ((amountIn * markPrice) / 1_000_000n) * (10_000n - MAX_SPOT_LIQUIDATION_SLIPPAGE_BPS) / 10_000n;
}

/** Mirrors liquidate_spot::spot_is_sellable. */
export function isSpotSellable(
  marketActive: boolean,
  markPrice: bigint,
  nt: NorthtailMarketInfo,
  pool: NorthtailPoolInfo,
  balance: bigint,
): boolean {
  if (!marketActive || markPrice === 0n || !nt.isTrading || !pool.isActive) return false;
  const chunk = nt.minTradeSize > 0n ? nt.minTradeSize : 1n;
  if (balance < chunk) return false;
  if (nt.maxTradeSize > 0n && nt.maxTradeSize < chunk) return false;
  const out = northtailSwapOutput(chunk, pool.securityLiquidity, pool.quoteLiquidity, nt.tradingFeeBps);
  return out >= spotLiquidationMinOut(chunk, markPrice);
}

export interface SpotChunk {
  amountIn: bigint;
  minAmountOut: bigint;
}

/**
 * Pick the next chunk to sell: at most max_trade_size, and small enough that
 * the pool price impact stays under `maxImpactBps`; then halve until the
 * expected output clears the on-chain floor. min_amount_out is the expected
 * output less `slippageBps` (never below the on-chain floor).
 * Returns null when no legal chunk clears the floor.
 */
export function planSpotChunk(
  balance: bigint,
  markPrice: bigint,
  nt: NorthtailMarketInfo,
  pool: NorthtailPoolInfo,
  maxImpactBps: bigint,
  slippageBps: bigint,
): SpotChunk | null {
  const minTrade = nt.minTradeSize > 0n ? nt.minTradeSize : 1n;
  if (balance < minTrade || markPrice === 0n) return null;
  if (maxImpactBps <= 0n || maxImpactBps >= 10_000n) throw new Error("maxImpactBps must be in (0, 10000)");
  // Selling x into reserve S moves the price by about x / (S + x).
  const impactCap = (pool.securityLiquidity * maxImpactBps) / (10_000n - maxImpactBps);
  let chunk = balance;
  if (nt.maxTradeSize > 0n && chunk > nt.maxTradeSize) chunk = nt.maxTradeSize;
  if (chunk > impactCap) chunk = impactCap;
  if (chunk < minTrade) chunk = minTrade;
  for (;;) {
    const out = northtailSwapOutput(chunk, pool.securityLiquidity, pool.quoteLiquidity, nt.tradingFeeBps);
    const floor = spotLiquidationMinOut(chunk, markPrice);
    if (out >= floor) {
      const tolerated = (out * (10_000n - slippageBps)) / 10_000n;
      return { amountIn: chunk, minAmountOut: tolerated > floor ? tolerated : floor };
    }
    if (chunk === minTrade) return null;
    chunk = chunk / 2n < minTrade ? minTrade : chunk / 2n;
  }
}

/**
 * Spot step: sell one chunk of the most valuable sellable spot balance with
 * liquidate_spot, or, when no spot balance is sellable, call liquidate with
 * the per-slot [MarginMarket, northtail Market, Pool] accounts so the
 * waterfall moves on to lending.
 */
async function buildSpotStepIx(
  connection: Connection,
  programId: PublicKey,
  configPda: PublicKey,
  liquidator: PublicKey,
  marginAccount: PublicKey,
  decoded: OnChainMarginAccount,
  maxImpactBps: bigint,
  slippageBps: bigint,
) {
  const configInfo = await connection.getAccountInfo(configPda);
  if (!configInfo) throw new Error("config account missing");
  const collateralMint = new PublicKey(
    (configInfo.data as Buffer).subarray(CONFIG_COLLATERAL_MINT_OFFSET, CONFIG_COLLATERAL_MINT_OFFSET + 32),
  );

  const slots = decoded.spotBalances.filter((s) => s.isActive);
  const proof: PublicKey[] = [];
  let best: { slot: (typeof slots)[number]; chunk: SpotChunk; marketPda: PublicKey; exchangeMarket: PublicKey; pool: PublicKey; poolInfo: NorthtailPoolInfo } | null = null;

  for (const slot of slots) {
    const [marketPda] = findMarginMarketPDAByIndex(slot.marketIndex, programId);
    const [exchangeMarket] = findNorthtailMarketPDA(slot.mint, collateralMint);
    const [pool] = findNorthtailPoolPDA(exchangeMarket);
    proof.push(marketPda, exchangeMarket, pool);

    const [mInfo, ntInfo, pInfo] = await connection.getMultipleAccountsInfo([marketPda, exchangeMarket, pool]);
    if (!mInfo || !ntInfo || !pInfo) continue;
    const market = decodeMarginMarket(mInfo.data as Buffer);
    const nt = parseNorthtailMarket(ntInfo.data as Buffer);
    const poolInfo = parseNorthtailPool(pInfo.data as Buffer);
    if (!isSpotSellable(market.isActive, market.markPrice, nt, poolInfo, slot.balance)) continue;
    const chunk = planSpotChunk(slot.balance, market.markPrice, nt, poolInfo, maxImpactBps, slippageBps);
    if (chunk && (!best || slot.value > best.slot.value)) {
      best = { slot, chunk, marketPda, exchangeMarket, pool, poolInfo };
    }
  }

  if (!best) {
    const [marketPda] = findMarginMarketPDAByIndex(slots[0]?.marketIndex ?? 0, programId);
    return liquidateIx(marginAccount, marketPda, configPda, liquidator, programId, proof);
  }

  const [vaultAuthority] = findVaultAuthorityPDA(programId);
  return liquidateSpotIx(
    {
      marginAccount,
      market: best.marketPda,
      config: configPda,
      vaultAuthority,
      collateralVault: findAssociatedTokenAddress(vaultAuthority, collateralMint),
      baseVault: findAssociatedTokenAddress(vaultAuthority, best.slot.mint),
      exchangeMarket: best.exchangeMarket,
      pool: best.pool,
      poolAuthority: best.poolInfo.authority,
      poolSecurityVault: best.poolInfo.securityVault,
      poolQuoteVault: best.poolInfo.quoteVault,
      exchangeProgram: NORTHTAIL_EXCHANGE_PROGRAM_ID,
      tokenProgram: TOKEN_PROGRAM_ID,
      liquidator,
    },
    best.chunk.amountIn,
    best.chunk.minAmountOut,
    programId,
  );
}

// ── Core logic ───────────────────────────────────────────────────

async function liquidatorCycle(
  connection: Connection,
  signer: Keypair,
  programId: PublicKey,
  maxLiquidations: number,
  refreshMarginFirst: boolean,
  marginStalenessSeconds: number,
  spotMaxImpactBps: bigint,
  spotSlippageBps: bigint,
  stats: LiquidatorStats,
  log: (...args: unknown[]) => void,
): Promise<LiquidationEvent[]> {
  const events: LiquidationEvent[] = [];
  const nowSec = BigInt(Math.floor(Date.now() / 1000));
  const [configPda] = findMarginConfigPDA(programId);

  // Fetch all margin accounts
  let marginAccounts;
  try {
    marginAccounts = await connection.getProgramAccounts(programId, {
      filters: [
        { memcmp: { offset: 0, bytes: MARGIN_ACCOUNT_DISC.toString("base64"), encoding: "base64" } },
      ],
    });
  } catch (err) {
    log("[liquidator] error fetching margin accounts:", err);
    return events;
  }

  stats.accountsScanned += marginAccounts.length;
  stats.cyclesRun++;

  // Separate accounts into: needs margin refresh vs liquidatable
  const staleAccounts: { pubkey: PublicKey; decoded: ReturnType<typeof decodeMarginAccount>; marketIndices: Set<number> }[] = [];
  const liquidatable: { pubkey: PublicKey; decoded: ReturnType<typeof decodeMarginAccount>; marketIndices: Set<number> }[] = [];

  for (const { pubkey, account } of marginAccounts) {
    let decoded;
    try {
      decoded = decodeMarginAccount(account.data as Buffer);
    } catch {
      continue;
    }

    // Collect active market indices
    const marketIndices = new Set<number>();
    for (let i = 0; i < decoded.perpCount; i++) {
      if (decoded.perpPositions[i].isActive) {
        marketIndices.add(decoded.perpPositions[i].marketIndex);
      }
    }
    for (let i = 0; i < decoded.optionCount; i++) {
      if (decoded.optionPositions[i].isActive) {
        marketIndices.add(decoded.optionPositions[i].marketIndex);
      }
    }
    for (let i = 0; i < decoded.spotCount; i++) {
      if (decoded.spotBalances[i].isActive) {
        marketIndices.add(decoded.spotBalances[i].marketIndex);
      }
    }

    // Skip accounts with no positions
    if (marketIndices.size === 0) continue;

    // Check if margin is stale and needs refresh first
    const staleThreshold = nowSec - BigInt(marginStalenessSeconds);
    if (refreshMarginFirst && decoded.lastMarginUpdate < staleThreshold) {
      staleAccounts.push({ pubkey, decoded, marketIndices });
    }

    // Check if liquidatable
    if (decoded.health === AccountHealth.Liquidatable || decoded.health === AccountHealth.Bankrupt) {
      liquidatable.push({ pubkey, decoded, marketIndices });
    }
  }

  // Step 1: Refresh stale margins to get accurate health readings
  if (refreshMarginFirst && staleAccounts.length > 0) {
    // Only refresh accounts that aren't already known-liquidatable
    // to discover newly-liquidatable accounts
    const liquidatablePubkeys = new Set(liquidatable.map(l => l.pubkey.toBase58()));
    const toRefresh = staleAccounts.filter(s => !liquidatablePubkeys.has(s.pubkey.toBase58()));

    for (const { pubkey, marketIndices } of toRefresh.slice(0, 10)) {
      try {
        const marketPdas = [...marketIndices].map((idx) => findMarginMarketPDAByIndex(idx, programId)[0]);
        const ix = computeMarginIx(pubkey, configPda, marketPdas, programId);
        const tx = new Transaction().add(ix);
        await sendAndConfirmTransaction(connection, tx, [signer]);
        stats.marginRefreshes++;
      } catch {
        // Continue — margin refresh is best-effort
      }
    }

    // Re-fetch refreshed accounts to check for newly-liquidatable
    for (const { pubkey, marketIndices } of toRefresh.slice(0, 10)) {
      try {
        const accountInfo = await connection.getAccountInfo(pubkey);
        if (!accountInfo) continue;
        const refreshed = decodeMarginAccount(accountInfo.data as Buffer);
        if (refreshed.health === AccountHealth.Liquidatable || refreshed.health === AccountHealth.Bankrupt) {
          // Add if not already in liquidatable list
          if (!liquidatablePubkeys.has(pubkey.toBase58())) {
            liquidatable.push({ pubkey, decoded: refreshed, marketIndices });
          }
        }
      } catch {
        continue;
      }
    }
  }

  if (liquidatable.length === 0) return events;

  log(`[liquidator] found ${liquidatable.length} liquidatable account(s)`);

  // Step 2: Sort by urgency — bankrupt first, then by lowest equity
  liquidatable.sort((a, b) => {
    // Bankrupt accounts first
    if (a.decoded.health === AccountHealth.Bankrupt && b.decoded.health !== AccountHealth.Bankrupt) return -1;
    if (b.decoded.health === AccountHealth.Bankrupt && a.decoded.health !== AccountHealth.Bankrupt) return 1;
    // Then by lowest equity (most urgent)
    if (a.decoded.equity < b.decoded.equity) return -1;
    if (a.decoded.equity > b.decoded.equity) return 1;
    return 0;
  });

  // Step 3: Liquidate
  let liquidated = 0;
  for (const { pubkey, decoded, marketIndices } of liquidatable) {
    if (liquidated >= maxLiquidations) break;

    // Pick the primary market for the liquidation instruction
    // The on-chain handler uses liquidation_priority() to decide what to close
    const primaryMarketIndex = [...marketIndices][0];
    const [marketPda] = findMarginMarketPDAByIndex(primaryMarketIndex, programId);

    stats.liquidationsAttempted++;

    try {
      // Spot is sold through the northtail swap by liquidate_spot;
      // liquidate handles the other steps of the waterfall.
      const ix = needsSpotLiquidation(decoded)
        ? await buildSpotStepIx(
            connection, programId, configPda, signer.publicKey, pubkey, decoded,
            spotMaxImpactBps, spotSlippageBps,
          )
        : liquidateIx(pubkey, marketPda, configPda, signer.publicKey, programId);
      const tx = new Transaction().add(ix);
      const sig = await sendAndConfirmTransaction(connection, tx, [signer]);

      log(`[liquidator] liquidated ${pubkey.toBase58()} (health=${AccountHealth[decoded.health]}, equity=${decoded.equity}) — tx ${sig}`);

      events.push({
        marginAccount: pubkey,
        health: decoded.health,
        equity: decoded.equity,
        maintenanceMargin: decoded.maintenanceMarginRequired,
        txSignature: sig,
      });

      stats.liquidationsSucceeded++;
      liquidated++;
    } catch (err) {
      log(`[liquidator] FAILED to liquidate ${pubkey.toBase58()}:`, err);
      stats.liquidationsFailed++;
    }
  }

  if (liquidated > 0) {
    log(`[liquidator] liquidated ${liquidated}/${liquidatable.length} account(s)`);
  }

  return events;
}

// ── Public API ───────────────────────────────────────────────────

export function startLiquidator(config: LiquidatorConfig): {
  stop: () => void;
  stats: () => LiquidatorStats;
  onLiquidation: (handler: (event: LiquidationEvent) => void) => void;
} {
  const {
    rpcUrl,
    signerSecretKey,
    programId: programIdStr,
    pollIntervalMs = 5_000,
    maxLiquidationsPerCycle = 5,
    refreshMarginFirst = true,
    marginStalenessSeconds = 30,
    spotMaxImpactBps = 100,
    spotSlippageBps = 50,
    log = console.log,
  } = config;

  const connection = new Connection(rpcUrl, "confirmed");
  const signer = Keypair.fromSecretKey(signerSecretKey);
  const programId = programIdStr
    ? new PublicKey(programIdStr)
    : TENSOR_PROGRAM_ID;

  const stats: LiquidatorStats = {
    cyclesRun: 0,
    accountsScanned: 0,
    liquidationsAttempted: 0,
    liquidationsSucceeded: 0,
    liquidationsFailed: 0,
    marginRefreshes: 0,
  };

  const handlers: ((event: LiquidationEvent) => void)[] = [];

  log(`[liquidator] bot started — program ${programId.toBase58()}`);
  log(`[liquidator] signer: ${signer.publicKey.toBase58()}`);
  log(`[liquidator] poll interval: ${pollIntervalMs}ms, max per cycle: ${maxLiquidationsPerCycle}`);
  log(`[liquidator] refresh margin: ${refreshMarginFirst}, staleness: ${marginStalenessSeconds}s`);

  let running = true;

  const tick = async () => {
    if (!running) return;
    try {
      const events = await liquidatorCycle(
        connection,
        signer,
        programId,
        maxLiquidationsPerCycle,
        refreshMarginFirst,
        marginStalenessSeconds,
        BigInt(spotMaxImpactBps),
        BigInt(spotSlippageBps),
        stats,
        log,
      );
      for (const event of events) {
        for (const handler of handlers) {
          try {
            handler(event);
          } catch {
            // Don't let handler errors crash the bot
          }
        }
      }
    } catch (err) {
      log("[liquidator] unexpected error in cycle:", err);
    }
  };

  void tick();
  const interval = setInterval(() => void tick(), pollIntervalMs);

  return {
    stop: () => {
      running = false;
      clearInterval(interval);
      log("[liquidator] bot stopped");
      log(`[liquidator] final stats: ${JSON.stringify(stats)}`);
    },
    stats: () => ({ ...stats }),
    onLiquidation: (handler) => {
      handlers.push(handler);
    },
  };
}
