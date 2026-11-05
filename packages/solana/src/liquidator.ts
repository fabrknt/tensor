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

/**
 * Build a `liquidate_spot` instruction that sells the account's largest spot
 * balance in full. min_amount_out is 0: the program applies its own floor
 * from the market mark price.
 */
async function buildSpotLiquidationIx(
  connection: Connection,
  programId: PublicKey,
  configPda: PublicKey,
  liquidator: PublicKey,
  marginAccount: PublicKey,
  decoded: OnChainMarginAccount,
) {
  const spot = decoded.spotBalances
    .filter((s) => s.isActive && s.balance > 0n)
    .sort((a, b) => (a.value > b.value ? -1 : a.value < b.value ? 1 : 0))[0];
  if (!spot) throw new Error("no spot balance to sell");

  const [marketPda] = findMarginMarketPDAByIndex(spot.marketIndex, programId);
  const [marketInfo, configInfo] = await connection.getMultipleAccountsInfo([marketPda, configPda]);
  if (!marketInfo || !configInfo) throw new Error("market or config account missing");
  const baseMint = decodeMarginMarket(marketInfo.data as Buffer).baseMint;
  const collateralMint = new PublicKey(
    (configInfo.data as Buffer).subarray(CONFIG_COLLATERAL_MINT_OFFSET, CONFIG_COLLATERAL_MINT_OFFSET + 32),
  );

  const [exchangeMarket] = findNorthtailMarketPDA(baseMint, collateralMint);
  const [pool] = findNorthtailPoolPDA(exchangeMarket);
  const poolInfo = await connection.getAccountInfo(pool);
  if (!poolInfo) throw new Error("northtail pool missing");
  const poolData = poolInfo.data as Buffer;
  const key = (off: number) => new PublicKey(poolData.subarray(off, off + 32));

  const [vaultAuthority] = findVaultAuthorityPDA(programId);
  return liquidateSpotIx(
    {
      marginAccount,
      market: marketPda,
      config: configPda,
      vaultAuthority,
      collateralVault: findAssociatedTokenAddress(vaultAuthority, collateralMint),
      baseVault: findAssociatedTokenAddress(vaultAuthority, baseMint),
      exchangeMarket,
      pool,
      poolAuthority: key(POOL_AUTHORITY_OFFSET),
      poolSecurityVault: key(POOL_SECURITY_VAULT_OFFSET),
      poolQuoteVault: key(POOL_QUOTE_VAULT_OFFSET),
      exchangeProgram: NORTHTAIL_EXCHANGE_PROGRAM_ID,
      tokenProgram: TOKEN_PROGRAM_ID,
      liquidator,
    },
    spot.balance,
    0n,
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
        ? await buildSpotLiquidationIx(connection, programId, configPda, signer.publicKey, pubkey, decoded)
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
