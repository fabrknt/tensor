# @fabrknt/tensor-core

Chain-agnostic math, types, and solver client for Tensor -- unified margin engine with Greeks-aware portfolio margining across perpetuals, options, spot, and lending.

Not every DeFi protocol needs TradFi compliance -- but if yours does, you shouldn't have to rebuild from scratch. Fabrknt plugs into your existing protocol with composable SDKs and APIs. No permissioned forks, no separate deployments.

## Install

```bash
npm install @fabrknt/tensor-core
```

## Quick Start

```typescript
import {
  computeGreeks,
  aggregatePortfolioGreeks,
  computeMargin,
  computeHealth,
  deltaNet,
  buildVolSurface,
  solveIntent,
} from "@fabrknt/tensor-core";

// Compute Black-Scholes Greeks for an option position
const greeks = computeGreeks({
  asset: "ETH",
  option_type: "call",
  side: "long",
  size: 1,
  strike: 3000,
  expiry: "2026-06-30",
  underlying_price: 2800,
  implied_volatility: 0.6,
});

// Portfolio margin with delta netting
const margin = computeMargin(positions, equity);
const health = computeHealth(margin, equity);
```

## Features

- Zero runtime dependencies -- pure TypeScript
- Black-Scholes Greeks (delta, gamma, vega, theta) with vol surface interpolation
- Portfolio margining with delta netting across spot, perps, and options
- Health status classification (healthy, warning, critical, liquidatable)
- Volatility surface generation (moneyness x expiry grid with skew/term structure)
- Intent language for multi-leg strategies (spreads, straddles, iron condors)
- Solver client for bid evaluation, auction processing, and settlement scanning
- Plug-in architecture: chain adapters for Solana, EVM, or custom integrations

## API Summary

### Greeks

| Export | Description |
|--------|-------------|
| `computeGreeks(position)` | Black-Scholes Greeks for a single option position |
| `aggregatePortfolioGreeks(positions)` | Aggregate Greeks across a portfolio |

### Margin

| Export | Description |
|--------|-------------|
| `computeMargin(positions, equity)` | Initial/maintenance margin with per-position detail |
| `computeHealth(margin, equity)` | Health status and margin ratio |
| `deltaNet(positions)` | Delta netting across correlated positions |

### Volatility Surface

| Export | Description |
|--------|-------------|
| `buildVolSurface(atmVol, ...)` | Generate a vol surface from ATM IV + skew/term multipliers |
| `interpolateVol(surface, moneyness, expiry)` | Bilinear interpolation over the vol grid |
| `volSurfaceToOnChain(surface)` | Serialize for on-chain storage |
| `fitVolSurfaceFromOracle(data)` | Fit a surface from oracle variance data |

### Intents and Solver

| Export | Description |
|--------|-------------|
| `solveIntent(intent, constraints)` | Decompose a multi-leg intent into execution steps |
| `evaluateBidOpportunity(intent)` | Evaluate profitability of filling an intent |
| `processAuction(bids)` | Rank and select winning solver bids |
| `findSettleableAuctions(auctions)` | Identify auctions ready for on-chain settlement |

### Types

Key type exports: `Position`, `VolSurface`, `OptionPosition`, `Greeks`, `PortfolioGreeks`, `MarginResult`, `HealthResult`, `HealthStatus`, `TradingIntent`, `Leg`, `SolverBid`, `SolverResult`, `ExecutionStep`.

### Adapters

| Export | Description |
|--------|-------------|
| `ChainAdapter` / `Chain` | Interface for plugging in chain-specific adapters |
| `CostEstimator` | Gas/fee estimation interface |
| `solanaCostEstimator` | Default cost estimator for Solana |
| `evmCostEstimator` | Default cost estimator for EVM chains |

## Chain Adapters

This package defines the `ChainAdapter` interface. Chain-specific implementations are provided separately:

- `@fabrknt/tensor-solana` -- Solana adapter with borsh decoders, instruction builders, and keeper bots

## Documentation

See the [main repository README](https://github.com/fabrknt/tensor) for full architecture docs, on-chain program details, keeper bot configuration, and testing instructions.

## License

BUSL-1.1
