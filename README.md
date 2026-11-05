# Tensor

[![npm version](https://img.shields.io/npm/v/@fabrknt/tensor-core.svg)](https://www.npmjs.com/package/@fabrknt/tensor-core)
[![npm downloads](https://img.shields.io/npm/dm/@fabrknt/tensor-core.svg)](https://www.npmjs.com/package/@fabrknt/tensor-core)

Unified margin engine with Greeks-aware portfolio margining across perpetuals, options, spot, and lending.

Part of [Fabrknt](https://fabrknt.com) — plug-in compliance for existing DeFi protocols. `npm install @fabrknt/tensor-core`

## Architecture

```
tensor/
  programs/
    tensor-margin          Anchor program — margin engine, trading, risk, solver auctions
  crates/
    tensor-types           Shared types (positions, Greeks, enums, investor categories)
    tensor-math            Margin math, equity, health, liquidation, vol surface interpolation
    tensor-cpi             Zero-copy CPI readers (Sigma, Sovereign, Northtail, ZK Credit)
    tensor-intents         Intent language — multi-leg bundles, builder pattern
    tensor-solver          Off-chain solver — decomposition, ordering, margin simulation
  packages/
    core                   Chain-agnostic TypeScript types, math, Greeks, vol surface, solver client
    solana                 Solana adapter, borsh decoders, instruction builders, keeper bots
    sdk                    TypeScript SDK (WIP)
    qn-addon               QuickNode Marketplace add-on (margin, greeks, intents)
```

### Chain-Agnostic Core

The core algorithm crates (`tensor-types`, `tensor-math`, `tensor-intents`, `tensor-solver`) are chain-agnostic. Each has an `anchor` feature flag (enabled by default) that controls Anchor/Solana dependencies:

| Crate | `anchor` feature ON (default) | `anchor` feature OFF |
|-------|-------------------------------|----------------------|
| tensor-types | `AnchorSerialize`/`AnchorDeserialize`/`InitSpace` derives, `Pubkey` fields | `borsh::BorshSerialize`/`BorshDeserialize` derives, `[u8; 32]` fields |
| tensor-math | Depends on tensor-types with anchor | Pure math, no Anchor dependency |
| tensor-intents | Depends on tensor-types with anchor | Pure intent DSL, no Anchor dependency |
| tensor-solver | Depends on all above with anchor | Pure solver, no Anchor dependency |

The Solana-specific crates (`tensor-cpi`, `tensor-margin`) always require Anchor.

## Key Features

- **Portfolio Margining** — Delta-netting across spot, perps, and options reduces margin for hedged positions to near zero.
- **Greeks-Aware Risk** — Gamma and vega charges capture non-linear option risk. Theta decay is tracked.
- **Multi-Product** — Perpetual futures, vanilla/exotic options (Asian, barrier), spot trading (via Northtail AMM), and lending/borrowing in a single margin account.
- **Volatility Surface** — Bilinear interpolation over a moneyness x expiry grid (9 strikes, 4 tenors). Replaces flat IV with strike/expiry-dependent implied volatility for accurate vega charges.
- **Dynamic Gamma Margin** — Gamma margin scales up when realized volatility exceeds implied volatility, capped at 5x base rate.
- **Gamma Concentration Limits** — Per-account and per-market gamma limits, tiered by investor category (Retail 10B, Qualified 100B, Institutional 500B).
- **Intent Language** — Declarative multi-leg trading intents (e.g., delta-neutral spread, covered call) with constraint validation.
- **Solver Auctions** — Decentralized intent execution via competitive bidding. Solvers stake collateral, submit bids during a timed auction, and the best price wins. Failed fills trigger slashing.
- **Off-Chain Solver** — Decomposes intents into optimal execution sequences, orders hedging legs first to minimize peak margin.
- **ZK Credit Scores** — Privacy-preserving credit tiers (Bronze through Platinum) that reduce initial margin requirements by up to 20% and increase max leverage.
- **Identity-Gated Leverage** — Sovereign reputation tiers map to investor categories (Retail 5x, Qualified 20x, Institutional 50x).
- **Permissionless Cranks** — Anyone can call `compute_margin`, `liquidate`, `liquidate_spot`, or `settle_auction`.

## On-Chain Instructions

The Anchor program exposes 27 instructions:

| Category | Instructions |
|----------|-------------|
| Admin | `initialize_config`, `register_market` |
| Account | `create_margin_account`, `deposit_collateral`, `withdraw_collateral` |
| Trading | `open_perp`, `close_perp`, `open_option`, `execute_spot_swap` |
| Risk (permissionless) | `compute_margin`, `compute_margin_oracle`, `liquidate`, `liquidate_spot` (sells spot through Northtail with a mark-price floor) |
| Oracle | `update_mark_price`, `update_mark_price_oracle`, `update_vol_surface` |
| Identity/Credit | `refresh_identity`, `refresh_zk_credit` |
| Intents | `submit_intent`, `execute_intent`, `cancel_intent` |
| Solver Auctions | `register_solver`, `deregister_solver`, `submit_bid`, `settle_auction`, `slash_solver` |

### Registering Markets

`register_market` is called by the config authority. A market with `spot_enabled = true` must pass `[base_mint, collateral_mint]` as remaining accounts, and both mints must have the same decimals (`MintDecimalsMismatch` otherwise). Spot prices are quote atoms per base atom (1e6 scaled), which equals the per-token price only when the decimals match. `base_mint` may not equal the collateral mint.

### Spot Swaps

`execute_spot_swap` (owner only) moves tokens with a CPI into the Northtail exchange `swap`, signed by the `vault_authority` PDA. The input leaves one protocol vault and the output lands in the other: the collateral vault (`config.collateral_vault()`) and the base vault (the `vault_authority` ATA for `market.base_mint`). The account is booked by the amounts that actually moved. `params.market_index` must equal the market account's index, and the Northtail market and pool must be the ones derived from `(market.base_mint, config.collateral_mint)`.

## Liquidation Waterfall

When an account's health is `Liquidatable` or `Bankrupt`, anyone can liquidate it, one step at a time, in this order:

1. Options expiring within 24 hours (`liquidate`)
2. Perps, largest position in the given market first (`liquidate`)
3. Remaining options, nearest expiry first (`liquidate`)
4. Spot balances, sold through Northtail (`liquidate_spot`)
5. Lending supply, largest first (`liquidate`)

### `liquidate_spot`

Permissionless. Sells `amount_in` of the account's spot balance for `market.base_mint` through the same Northtail swap CPI as `execute_spot_swap`, and credits the quote that actually arrived.

- Arguments: `amount_in: u64` (at most the balance; a partial amount lets a large sale be split) and `min_amount_out: u64`.
- Price floor: the program computes `amount_in × mark_price / 1e6 × 95%` on chain (`MAX_SPOT_LIQUIDATION_SLIPPAGE_BPS = 500`). `min_amount_out` can only raise this floor. If the pool would pay less, the swap fails.
- Checks: the account is liquidatable, spot is the current waterfall step, the market is active with a non-zero mark price, the spot slot's `market_index` equals the market's index, and the Northtail pool is active.
- Accounts: `margin_account`, `market`, `config`, `vault_authority`, `collateral_vault`, `base_vault`, `exchange_market`, `pool`, `pool_authority`, `pool_security_vault`, `pool_quote_vault`, `exchange_program`, `token_program`, `liquidator` (signer).
- The liquidation fee is taken on the quote received, with the same split as `liquidate`.

### Passing the spot step

While spot is the current step, `liquidate` returns `SpotLiquidationRequired`. It moves on to lending only when every active spot balance is structurally unsellable:

- the balance is below Northtail's `min_trade_size` (dust);
- the tensor market is inactive or has a zero mark price;
- the Northtail market is not trading, or the pool is inactive;
- no legal trade size exists (`max_trade_size` below `min_trade_size`).

To show this, `liquidate` takes `[MarginMarket, Northtail Market, Northtail Pool]` per active spot slot, in slot order, as remaining accounts. Unsellable balances stay on the books as they are; nothing is written off.

The pool price is never a reason to pass spot. When the pool trades below the floor (for example in a crash), the spot step waits until the mark price catches up and a sale clears the floor.

## Keeper Bots

The `@fabrknt/tensor-solana` package includes three runnable keeper services:

### Crank Bot

Settles expired solver auctions and refreshes stale margin accounts.

```sh
cd packages/solana
RPC_URL=http://localhost:8899 pnpm crank
```

| Env Var | Default | Description |
|---------|---------|-------------|
| `RPC_URL` | `http://localhost:8899` | Solana RPC endpoint |
| `KEYPAIR_PATH` | `~/.config/solana/id.json` | Signer keypair |
| `POLL_INTERVAL_MS` | `10000` | Poll interval in ms |
| `CRANK_MARGIN` | `true` | Also refresh stale margins |

### Vol Surface Keeper

Reads oracle variance, builds vol surfaces with skew/term structure, and updates on-chain.

```sh
cd packages/solana
MARKET_INDICES=0,1 RPC_URL=http://localhost:8899 pnpm vol-keeper
```

| Env Var | Default | Description |
|---------|---------|-------------|
| `RPC_URL` | `http://localhost:8899` | Solana RPC endpoint |
| `KEYPAIR_PATH` | `~/.config/solana/id.json` | Authority keypair |
| `MARKET_INDICES` | `0` | Comma-separated market indices |
| `POLL_INTERVAL_MS` | `60000` | Update interval in ms |

### Liquidation Bot

Scans for undercollateralized accounts and liquidates them, collecting fees.

```sh
cd packages/solana
RPC_URL=http://localhost:8899 pnpm liquidator
```

| Env Var | Default | Description |
|---------|---------|-------------|
| `RPC_URL` | `http://localhost:8899` | Solana RPC endpoint |
| `KEYPAIR_PATH` | `~/.config/solana/id.json` | Signer keypair |
| `POLL_INTERVAL_MS` | `5000` | Poll interval in ms |
| `MAX_LIQUIDATIONS` | `5` | Max liquidations per cycle |
| `REFRESH_MARGIN` | `true` | Refresh stale margins before scanning |
| `MARGIN_STALENESS_SECONDS` | `30` | Staleness threshold for margin refresh |

The liquidator prioritizes bankrupt accounts first, then by lowest equity. It optionally refreshes stale margins to discover newly-liquidatable accounts before attempting liquidation.

At the spot step it sells the most valuable sellable balance in chunks with `liquidate_spot`. Each chunk is at most Northtail's `max_trade_size`, is sized to keep the pool price impact near `spotMaxImpactBps` (default 100), and must clear the on-chain floor; `min_amount_out` is the expected output less `spotSlippageBps` (default 50), never below the floor. If spot is sellable but no chunk clears the floor, the bot logs it and waits. If all spot is unsellable, it sends `liquidate` with the spot proof accounts so the waterfall moves to lending. `spotMaxImpactBps` and `spotSlippageBps` are `startLiquidator` options (no env vars).

## Building

The project uses **pnpm** (`packageManager: pnpm@10.31.0`) and **turbo** for build orchestration.

Build everything (TypeScript packages via turbo + Anchor build for programs):

```sh
pnpm build
```

Build only the Solana program:

```sh
anchor build
```

## Testing

### Rust

```sh
cargo test
```

272 Rust tests across all crates, including 32 integration scenarios and 9 LiteSVM tests, covering:

- Margin math, Greeks computation, delta-netting
- Liquidation waterfall priority
- Spot liquidation through Northtail: chunked sales, the mark-price floor, the structural unsellable checks, and a pool moved in the same transaction (LiteSVM)
- Spot market registration with mismatched decimals (LiteSVM)
- Vol surface interpolation (moneyness + expiry dimensions)
- Dynamic gamma margin scaling (calm, volatile, extreme markets)
- Gamma concentration limits by investor category
- Solver registration, auction lifecycle, settlement, slashing
- Intent validation, solver optimization, credit discounts
- End-to-end trading scenarios (long/short perps, straddles, multi-product portfolios)

#### LiteSVM tests

`programs/tensor-margin/tests/liquidate_spot_svm.rs` loads two SBF binaries:

- this program at `target/deploy/tensor_margin.so` (`cd programs/tensor-margin && cargo-build-sbf --tools-version v1.51`);
- northtail-exchange, at the path in `NORTHTAIL_EXCHANGE_SO` (an SBF build of the northtail-exchange program).

```sh
NORTHTAIL_EXCHANGE_SO=/path/to/northtail_exchange.so cargo test -p tensor-margin --test liquidate_spot_svm
```

A missing binary fails the test with a message naming it. On a machine without the SBF toolchain, set `TENSOR_SKIP_SVM=1` to skip these tests on purpose; they print a note and pass.

### TypeScript

```sh
pnpm test
```

134 TypeScript tests across `@fabrknt/tensor-core` (87) and `@fabrknt/tensor-solana` (47) covering:

- Black-Scholes Greeks (delta, gamma, vega, theta)
- Portfolio aggregation and put-call parity
- Vol surface generation, on-chain serialization, oracle fitting
- Solver bid evaluation, auction processing, settlement scanning
- Borsh decoder correctness (MarginAccount, MarginMarket, SolverRegistry, IntentAccount)
- Instruction builder structure (liquidate, liquidate_spot, settle_auction, compute_margin, update_vol_surface)
- Liquidator spot planning (sellable checks, chunk sizing, floor)
- Margin calculations, health status, delta netting
- Intent bundle creation, validation, constraint checking

**Total: 406 tests.**

### Chain-Agnostic Build

To verify the core crates compile without any Solana/Anchor dependency:

```sh
cargo check -p tensor-types -p tensor-math -p tensor-intents -p tensor-solver --no-default-features
```

### Using Core Crates Without Anchor

Add the crate with default features disabled:

```toml
[dependencies]
tensor-types = { git = "...", default-features = false }
tensor-math = { git = "...", default-features = false }
```

Types use `borsh` serialization and `[u8; 32]` for address fields instead of `Pubkey`. All math, intent, and solver logic works identically.

## TypeScript Packages

### @fabrknt/tensor-core

Chain-agnostic math and types. No Solana dependency.

```typescript
import {
  computeGreeks,
  aggregatePortfolioGreeks,
  interpolateVol,
  buildVolSurface,
  volSurfaceToOnChain,
  fitVolSurfaceFromOracle,
  evaluateBidOpportunity,
  processAuction,
  findSettleableAuctions,
  solveIntent,
  evmCostEstimator,
} from "@fabrknt/tensor-core";
```

Key modules:
- **greeks** — Black-Scholes Greeks with optional vol surface interpolation
- **margin** — Margin calculations, health status, delta netting
- **intents** — Intent bundle creation, validation, auction evaluation
- **vol-surface** — Vol surface generation from ATM IV + skew/term multipliers
- **solver-client** — Solver bid evaluation, auction processing, crank bot logic

### @fabrknt/tensor-solana

Solana adapter, borsh decoders, instruction builders, and keeper bots.

```typescript
import {
  // Adapter
  SolanaAdapter,
  // PDAs
  findMarginAccountPDA,
  findMarginMarketPDAByIndex,
  findMarginConfigPDA,
  findSolverRegistryPDA,
  findIntentAccountPDA,
  findVaultAuthorityPDA,
  findNorthtailMarketPDA,
  findNorthtailPoolPDA,
  // Decoders
  decodeMarginAccount,
  decodeMarginMarket,
  decodeSolverRegistry,
  decodeIntentAccount,
  // Instruction builders
  liquidateIx,
  liquidateSpotIx,
  settleAuctionIx,
  computeMarginIx,
  updateVolSurfaceIx,
  // Keeper bots
  startCrankBot,
  startLiquidator,
  startVolKeeper,
  // Spot liquidation planning
  planSpotChunk,
  spotLiquidationMinOut,
  isSpotSellable,
  // Vol surface
  buildVolSurfaceParams,
  buildVolSurfaceFromAtmVol,
} from "@fabrknt/tensor-solana";
```

## QuickNode Add-on

The `packages/qn-addon` package provides a QuickNode Marketplace add-on (slug: `fabrknt-margin-engine`) with the following API endpoints:

| Route | Description |
|-------|-------------|
| `/margin` | Margin calculations and portfolio health queries |
| `/greeks` | Greeks computation (delta, gamma, vega, theta) for positions |
| `/intents` | Submit and query multi-leg trading intents |

## Upgrading an Existing Deployment

- No account layouts changed. `MarginConfig`, `MarginMarket` and `MarginAccount` keep their size and fields, so existing accounts stay valid.
- Instruction account lists changed: `execute_spot_swap` and the new `liquidate_spot` take the vault, Northtail market/pool and token program accounts listed above, and `liquidate` takes spot proof accounts when it passes the spot step. Clients must use the current `@fabrknt/tensor-solana` builders.
- Spot markets registered before the decimals check are not re-checked. Confirm that each existing spot market's base mint has the same decimals as the collateral mint.
- Spot trades now move tokens, so each spot market needs its base vault (the `vault_authority` ATA for `base_mint`). The program does not create it; create it before trading. Spot balances booked by an earlier version were not moved into that vault; reconcile them against the vault balance before relying on `liquidate_spot`.

## Known Limitations

- The liquidator's half of the liquidation fee is not paid out yet. The fee is deducted from the liquidated account, half is added to `insurance_fund`, and the liquidator half is only reported in the `AccountLiquidated` event.
- The market mark price has no staleness timestamp. `liquidate_spot` uses `mark_price` as it was last written by `update_mark_price` or `update_mark_price_oracle`.

## Program ID

```
3uztvRNHpQcS9KgbdY6NFoL9HamSZYujkH9FQWtFoP1h
```

## License

Apache-2.0
