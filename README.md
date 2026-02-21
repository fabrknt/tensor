# Tensor

Unified margin engine on Solana with Greeks-aware portfolio margining across perpetuals, options, spot, and lending.

## Architecture

```
tensor/
  programs/
    tensor-margin          Anchor program — margin engine, trading, risk
  crates/
    tensor-types           Shared types (positions, Greeks, enums)
    tensor-math            Margin math, equity, health, liquidation
    tensor-cpi             Zero-copy CPI readers (Sigma, Sovereign, Northtail, ZK Credit)
    tensor-intents         Intent language — multi-leg bundles, builder pattern
    tensor-solver          Off-chain solver — decomposition, ordering, margin simulation
  packages/
    types                  TypeScript type definitions
    sdk                    TypeScript SDK
```

## Key Features

- **Portfolio Margining** — Delta-netting across spot, perps, and options reduces margin for hedged positions to near zero.
- **Greeks-Aware Risk** — Gamma and vega charges capture non-linear option risk. Theta decay is tracked.
- **Multi-Product** — Perpetual futures, vanilla/exotic options (Asian, barrier), spot trading (via Northtail AMM), and lending/borrowing in a single margin account.
- **Intent Language** — Declarative multi-leg trading intents (e.g., delta-neutral spread, covered call) with constraint validation.
- **Off-Chain Solver** — Decomposes intents into optimal execution sequences, orders hedging legs first to minimize peak margin.
- **ZK Credit Scores** — Privacy-preserving credit tiers (Bronze through Platinum) that reduce initial margin requirements by up to 20% and increase max leverage.
- **Identity-Gated Leverage** — Sovereign reputation tiers map to investor categories (Retail 5x, Qualified 20x, Institutional 50x).
- **Permissionless Cranks** — Anyone can call `compute_margin` to keep accounts up to date.

## Building

```sh
anchor build
```

## Testing

```sh
cargo test
```

217 tests cover margin math, Greeks computation, delta-netting, liquidation waterfall, intent validation, solver optimization, credit discounts, and end-to-end trading scenarios.

## Program ID

```
3uztvRNHpQcS9KgbdY6NFoL9HamSZYujkH9FQWtFoP1h
```

## License

BUSL-1.1
