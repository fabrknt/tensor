import { describe, it, expect } from "vitest";
import { validateIntent, solveIntent } from "../intents";
import type { TradingIntent, Leg } from "../types";

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function makeLeg(overrides: Partial<Leg> = {}): Leg {
  return {
    asset: "SOL-PERP",
    side: "buy",
    size: 10,
    instrument_type: "perpetual",
    ...overrides,
  };
}

function makeIntent(overrides: Partial<TradingIntent> = {}): TradingIntent {
  return {
    strategy: "delta-neutral",
    legs: [makeLeg()],
    ...overrides,
  };
}

/* ------------------------------------------------------------------ */
/*  validateIntent                                                     */
/* ------------------------------------------------------------------ */

describe("validateIntent", () => {
  it("accepts a valid intent with known strategy", () => {
    const result = validateIntent(makeIntent());
    expect(result.valid).toBe(true);
    expect(result.errors).toHaveLength(0);
    expect(result.strategy_recognized).toBe(true);
    expect(result.strategy_type).toBe("delta-neutral");
  });

  it("recognizes all known strategies", () => {
    const strategies = [
      "bull-call-spread",
      "bear-put-spread",
      "straddle",
      "strangle",
      "iron-condor",
      "butterfly",
      "collar",
      "covered-call",
      "delta-neutral",
      "basis-trade",
      "calendar-spread",
    ];
    for (const strategy of strategies) {
      const result = validateIntent(makeIntent({ strategy }));
      expect(result.strategy_recognized).toBe(true);
      expect(result.strategy_type).toBe(strategy);
    }
  });

  it("flags unrecognized strategy with warning", () => {
    const result = validateIntent(makeIntent({ strategy: "moon-shot" }));
    expect(result.strategy_recognized).toBe(false);
    expect(result.strategy_type).toBeNull();
    expect(result.warnings.some((w) => w.includes("not a recognized pattern"))).toBe(true);
  });

  it("rejects leg with missing asset", () => {
    const result = validateIntent(
      makeIntent({ legs: [makeLeg({ asset: "" })] }),
    );
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.includes("asset is required"))).toBe(true);
  });

  it("rejects leg with zero size", () => {
    const result = validateIntent(
      makeIntent({ legs: [makeLeg({ size: 0 })] }),
    );
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.includes("size must be positive"))).toBe(true);
  });

  it("rejects leg with negative size", () => {
    const result = validateIntent(
      makeIntent({ legs: [makeLeg({ size: -5 })] }),
    );
    expect(result.valid).toBe(false);
  });

  it("rejects option leg missing option_type", () => {
    const result = validateIntent(
      makeIntent({
        legs: [
          makeLeg({
            instrument_type: "option",
            strike: 100,
            expiry: "2027-01-01",
          }),
        ],
      }),
    );
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.includes("option_type is required"))).toBe(true);
  });

  it("rejects option leg missing strike", () => {
    const result = validateIntent(
      makeIntent({
        legs: [
          makeLeg({
            instrument_type: "option",
            option_type: "call",
            expiry: "2027-01-01",
          }),
        ],
      }),
    );
    expect(result.valid).toBe(false);
    expect(result.errors.some((e) => e.includes("strike is required"))).toBe(true);
  });

  it("warns on high slippage tolerance", () => {
    const result = validateIntent(makeIntent({ max_slippage_bps: 600 }));
    expect(result.warnings.some((w) => w.includes("high slippage"))).toBe(true);
  });

  it("no slippage warning when within threshold", () => {
    const result = validateIntent(makeIntent({ max_slippage_bps: 100 }));
    expect(result.warnings.some((w) => w.includes("slippage"))).toBe(false);
  });

  it("estimates margin impact based on legs", () => {
    const result = validateIntent(
      makeIntent({
        legs: [makeLeg({ size: 10, limit_price: 100 })],
      }),
    );
    // 10 * 100 * 0.10 = 100
    expect(result.estimated_margin_impact).toBe(100);
  });

  it("collects errors from multiple legs", () => {
    const result = validateIntent(
      makeIntent({
        legs: [makeLeg({ asset: "" }), makeLeg({ size: 0 })],
      }),
    );
    expect(result.valid).toBe(false);
    expect(result.errors.length).toBeGreaterThanOrEqual(2);
  });
});

/* ------------------------------------------------------------------ */
/*  solveIntent                                                        */
/* ------------------------------------------------------------------ */

describe("solveIntent", () => {
  it("produces a step for each leg", () => {
    const intent = makeIntent({
      legs: [makeLeg(), makeLeg({ asset: "ETH-PERP" })],
    });
    const result = solveIntent(intent);

    expect(result.steps).toHaveLength(2);
    expect(result.total_steps).toBe(2);
    expect(result.feasible).toBe(true);
  });

  it("orders sells before buys", () => {
    const intent = makeIntent({
      legs: [
        makeLeg({ side: "buy", asset: "SOL-PERP" }),
        makeLeg({ side: "sell", asset: "ETH-PERP" }),
      ],
    });
    const result = solveIntent(intent);

    expect(result.steps[0].side).toBe("sell");
    expect(result.steps[1].side).toBe("buy");
    expect(result.optimization_notes.some((n) => n.includes("sells first"))).toBe(true);
  });

  it("returns infeasible when max_steps is exceeded", () => {
    const intent = makeIntent({
      legs: [makeLeg(), makeLeg(), makeLeg()],
    });
    const result = solveIntent(intent, { max_steps: 2 });

    expect(result.feasible).toBe(false);
    expect(result.optimization_notes.some((n) => n.includes("Infeasible"))).toBe(true);
  });

  it("marks atomic when legs <= 4", () => {
    const intent = makeIntent({
      legs: [makeLeg(), makeLeg()],
    });
    const result = solveIntent(intent, { prefer_atomic: true });

    expect(result.optimization_notes.some((n) => n.includes("atomically"))).toBe(true);
  });

  it("warns about non-atomic when legs > 4", () => {
    const intent = makeIntent({
      legs: Array.from({ length: 5 }, () => makeLeg()),
    });
    const result = solveIntent(intent);

    expect(result.optimization_notes.some((n) => n.includes("sequential batches"))).toBe(true);
  });

  it("applies delta-netting discount for hedged legs", () => {
    const intent = makeIntent({
      legs: [
        makeLeg({ side: "buy", size: 10, limit_price: 100 }),
        makeLeg({ side: "sell", size: 10, limit_price: 100 }),
      ],
    });
    const result = solveIntent(intent);

    // Without discount: 2 * 10 * 100 * 0.10 = 200
    // With 30% discount: 200 * 0.70 = 140
    expect(result.estimated_margin_required).toBe(140);
    expect(result.optimization_notes.some((n) => n.includes("Delta-netting"))).toBe(true);
  });

  it("does not apply discount for one-sided intent", () => {
    const intent = makeIntent({
      legs: [
        makeLeg({ side: "buy", size: 10, limit_price: 100 }),
        makeLeg({ side: "buy", size: 5, limit_price: 100 }),
      ],
    });
    const result = solveIntent(intent);

    // 15 * 100 * 0.10 = 150, no discount
    expect(result.estimated_margin_required).toBe(150);
  });

  it("estimates gas proportional to step count", () => {
    const intent = makeIntent({
      legs: [makeLeg(), makeLeg(), makeLeg()],
    });
    const result = solveIntent(intent);

    // 3 steps * 200_000 = 600_000
    expect(result.estimated_gas).toBe(600_000);
  });

  it("sets sequence numbers starting at 1", () => {
    const intent = makeIntent({
      legs: [makeLeg(), makeLeg()],
    });
    const result = solveIntent(intent);

    expect(result.steps[0].sequence).toBe(1);
    expect(result.steps[1].sequence).toBe(2);
  });
});
