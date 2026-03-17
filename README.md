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
- **Permissionless Cranks** — Anyone can call `compute_margin`, `liquidate`, or `settle_auction`.

## On-Chain Instructions

The Anchor program exposes 27 instructions:

| Category | Instructions |
|----------|-------------|
| Admin | `initialize_config`, `register_market` |
| Account | `create_margin_account`, `deposit_collateral`, `withdraw_collateral` |
| Trading | `open_perp`, `close_perp`, `open_option`, `execute_spot_swap` |
| Risk (permissionless) | `compute_margin`, `compute_margin_oracle`, `liquidate` |
| Oracle | `update_mark_price`, `update_mark_price_oracle`, `update_vol_surface` |
| Identity/Credit | `refresh_identity`, `refresh_zk_credit` |
| Intents | `submit_intent`, `execute_intent`, `cancel_intent` |
| Solver Auctions | `register_solver`, `deregister_solver`, `submit_bid`, `settle_auction`, `slash_solver` |

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

249 Rust tests across all crates and 32 integration scenarios covering:

- Margin math, Greeks computation, delta-netting
- Liquidation waterfall priority
- Vol surface interpolation (moneyness + expiry dimensions)
- Dynamic gamma margin scaling (calm, volatile, extreme markets)
- Gamma concentration limits by investor category
- Solver registration, auction lifecycle, settlement, slashing
- Intent validation, solver optimization, credit discounts
- End-to-end trading scenarios (long/short perps, straddles, multi-product portfolios)

### TypeScript

```sh
pnpm test
```

125 TypeScript tests across `@fabrknt/tensor-core` (87) and `@fabrknt/tensor-solana` (38) covering:

- Black-Scholes Greeks (delta, gamma, vega, theta)
- Portfolio aggregation and put-call parity
- Vol surface generation, on-chain serialization, oracle fitting
- Solver bid evaluation, auction processing, settlement scanning
- Borsh decoder correctness (MarginAccount, MarginMarket, SolverRegistry, IntentAccount)
- Instruction builder structure (liquidate, settle_auction, compute_margin, update_vol_surface)
- Margin calculations, health status, delta netting
- Intent bundle creation, validation, constraint checking

**Total: 374 tests, all passing.**

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
  // Decoders
  decodeMarginAccount,
  decodeMarginMarket,
  decodeSolverRegistry,
  decodeIntentAccount,
  // Instruction builders
  liquidateIx,
  settleAuctionIx,
  computeMarginIx,
  updateVolSurfaceIx,
  // Keeper bots
  startCrankBot,
  startLiquidator,
  startVolKeeper,
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

## Program ID

```
3uztvRNHpQcS9KgbdY6NFoL9HamSZYujkH9FQWtFoP1h
```

## License

MIT
