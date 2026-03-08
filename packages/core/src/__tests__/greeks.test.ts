import { describe, it, expect } from "vitest";
import { computeGreeks, aggregatePortfolioGreeks } from "../greeks";
import type { OptionPosition } from "../types";

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

/** Return a far-future expiry so tests are time-independent */
function futureExpiry(yearsFromNow: number = 1): string {
  const d = new Date();
  d.setFullYear(d.getFullYear() + yearsFromNow);
  return d.toISOString();
}

function makeOption(overrides: Partial<OptionPosition> = {}): OptionPosition {
  return {
    asset: "SOL",
    option_type: "call",
    side: "long",
    size: 1,
    strike: 100,
    expiry: futureExpiry(1),
    underlying_price: 100,
    implied_volatility: 0.3,
    risk_free_rate: 0.05,
    ...overrides,
  };
}

/* ------------------------------------------------------------------ */
/*  computeGreeks — single position                                    */
/* ------------------------------------------------------------------ */

describe("computeGreeks", () => {
  it("ATM call delta is approximately 0.5 (long)", () => {
    const g = computeGreeks(makeOption({ option_type: "call", strike: 100, underlying_price: 100 }));
    // For ATM with r>0 and vol>0, delta is slightly above 0.5
    expect(g.delta).toBeGreaterThan(0.4);
    expect(g.delta).toBeLessThan(0.7);
  });

  it("ATM put delta is approximately -0.5 (long)", () => {
    const g = computeGreeks(makeOption({ option_type: "put", strike: 100, underlying_price: 100 }));
    expect(g.delta).toBeGreaterThan(-0.7);
    expect(g.delta).toBeLessThan(-0.3);
  });

  it("put-call parity: call delta - put delta ≈ 1 (ATM, same params)", () => {
    const callG = computeGreeks(makeOption({ option_type: "call" }));
    const putG = computeGreeks(makeOption({ option_type: "put" }));

    // N(d1) - (N(d1) - 1) = 1
    expect(callG.delta - putG.delta).toBeCloseTo(1, 1);
  });

  it("gamma is positive for a long option", () => {
    const g = computeGreeks(makeOption());
    expect(g.gamma).toBeGreaterThan(0);
  });

  it("vega is positive for a long option", () => {
    const g = computeGreeks(makeOption());
    expect(g.vega).toBeGreaterThan(0);
  });

  it("theta is negative for a long option (time decay)", () => {
    const g = computeGreeks(makeOption());
    expect(g.theta).toBeLessThan(0);
  });

  it("short side flips delta sign", () => {
    const longG = computeGreeks(makeOption({ side: "long" }));
    const shortG = computeGreeks(makeOption({ side: "short" }));

    expect(shortG.delta).toBeCloseTo(-longG.delta, 6);
  });

  it("gamma stays positive regardless of side", () => {
    const shortG = computeGreeks(makeOption({ side: "short" }));
    expect(shortG.gamma).toBeGreaterThan(0);
  });

  it("scales with position size", () => {
    const g1 = computeGreeks(makeOption({ size: 1 }));
    const g10 = computeGreeks(makeOption({ size: 10 }));

    expect(g10.delta).toBeCloseTo(g1.delta * 10, 6);
    expect(g10.gamma).toBeCloseTo(g1.gamma * 10, 6);
  });

  it("deep ITM call has delta close to 1", () => {
    const g = computeGreeks(
      makeOption({ option_type: "call", strike: 50, underlying_price: 100 }),
    );
    expect(g.delta).toBeGreaterThan(0.9);
  });

  it("deep OTM call has delta close to 0", () => {
    const g = computeGreeks(
      makeOption({ option_type: "call", strike: 200, underlying_price: 100 }),
    );
    expect(g.delta).toBeLessThan(0.1);
  });
});

/* ------------------------------------------------------------------ */
/*  aggregatePortfolioGreeks                                           */
/* ------------------------------------------------------------------ */

describe("aggregatePortfolioGreeks", () => {
  it("sums greeks across positions", () => {
    const positions = [
      makeOption({ option_type: "call", size: 1 }),
      makeOption({ option_type: "call", size: 2 }),
    ];
    const portfolio = aggregatePortfolioGreeks(positions);

    const g1 = computeGreeks(positions[0]);
    const g2 = computeGreeks(positions[1]);

    expect(portfolio.delta).toBeCloseTo(g1.delta + g2.delta, 6);
    expect(portfolio.gamma).toBeCloseTo(g1.gamma + g2.gamma, 6);
    expect(portfolio.vega).toBeCloseTo(g1.vega + g2.vega, 6);
    expect(portfolio.theta).toBeCloseTo(g1.theta + g2.theta, 6);
  });

  it("net_exposure equals total delta", () => {
    const positions = [
      makeOption({ option_type: "call", size: 5 }),
      makeOption({ option_type: "put", size: 3 }),
    ];
    const portfolio = aggregatePortfolioGreeks(positions);
    expect(portfolio.net_exposure).toBe(portfolio.delta);
  });

  it("delta-neutral portfolio has near-zero net exposure", () => {
    // Long call + long put at same strike/size: deltas should roughly cancel
    const positions = [
      makeOption({ option_type: "call", size: 1 }),
      makeOption({ option_type: "put", size: 1 }),
    ];
    const portfolio = aggregatePortfolioGreeks(positions);

    // Not exactly zero due to r>0 skewing call delta above 0.5, but much
    // smaller than either individual delta (~0.55 and ~-0.45)
    expect(Math.abs(portfolio.net_exposure)).toBeLessThan(0.5);
  });

  it("returns all individual position greeks", () => {
    const positions = [
      makeOption({ asset: "SOL", option_type: "call" }),
      makeOption({ asset: "ETH", option_type: "put" }),
    ];
    const portfolio = aggregatePortfolioGreeks(positions);
    expect(portfolio.positions).toHaveLength(2);
    expect(portfolio.positions[0].asset).toBe("SOL");
    expect(portfolio.positions[1].asset).toBe("ETH");
  });
});
