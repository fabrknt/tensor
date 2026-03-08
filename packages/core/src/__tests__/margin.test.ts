import { describe, it, expect } from "vitest";
import { calculateMargin, calculateHealth, deltaNet } from "../margin";
import type { Position } from "../types";

/* ------------------------------------------------------------------ */
/*  Helpers                                                            */
/* ------------------------------------------------------------------ */

function makePosition(overrides: Partial<Position> = {}): Position {
  return {
    asset: "SOL-PERP",
    side: "long",
    size: 10,
    entry_price: 100,
    mark_price: 100,
    instrument_type: "perpetual",
    ...overrides,
  };
}

/* ------------------------------------------------------------------ */
/*  calculateMargin                                                    */
/* ------------------------------------------------------------------ */

describe("calculateMargin", () => {
  it("calculates margin for a single perpetual position", () => {
    const pos = makePosition({ size: 10, mark_price: 100 });
    const result = calculateMargin([pos], 1000);

    // notional = 10 * 100 = 1000, weight = 0.10, initial = 100
    expect(result.initial_margin).toBe(100);
    expect(result.maintenance_margin).toBe(50); // 100 * 0.5
    expect(result.margin_used).toBe(100);
    expect(result.margin_available).toBe(900); // 1000 - 100
    expect(result.positions).toHaveLength(1);
    expect(result.positions[0].weight).toBe(0.1);
  });

  it("calculates margin for multiple positions", () => {
    const positions = [
      makePosition({ asset: "SOL-PERP", size: 10, mark_price: 100 }),
      makePosition({ asset: "ETH-PERP", size: 5, mark_price: 200 }),
    ];
    const result = calculateMargin(positions, 5000);

    // SOL: 10*100*0.10 = 100, ETH: 5*200*0.10 = 100 => total = 200
    expect(result.initial_margin).toBe(200);
    expect(result.maintenance_margin).toBe(100);
    expect(result.margin_available).toBe(4800);
    expect(result.positions).toHaveLength(2);
  });

  it("applies higher weight for option positions", () => {
    const option = makePosition({
      instrument_type: "option",
      size: 10,
      mark_price: 100,
    });
    const result = calculateMargin([option], 1000);

    // weight = 0.15 for options => 10*100*0.15 = 150
    expect(result.initial_margin).toBe(150);
    expect(result.positions[0].weight).toBe(0.15);
  });

  it("applies lower weight for lending positions", () => {
    const lending = makePosition({
      instrument_type: "lending",
      size: 100,
      mark_price: 100,
    });
    const result = calculateMargin([lending], 10000);

    // weight = 0.05 => 100*100*0.05 = 500
    expect(result.initial_margin).toBe(500);
    expect(result.positions[0].weight).toBe(0.05);
  });

  it("clamps margin_available to zero when collateral is insufficient", () => {
    const pos = makePosition({ size: 100, mark_price: 100 });
    const result = calculateMargin([pos], 500);

    // initial = 100*100*0.10 = 1000, collateral = 500
    expect(result.margin_available).toBe(0);
  });

  it("handles short positions (uses abs size)", () => {
    const pos = makePosition({ side: "short", size: -10, mark_price: 100 });
    const result = calculateMargin([pos], 1000);

    // abs(-10) * 100 * 0.10 = 100
    expect(result.initial_margin).toBe(100);
  });
});

/* ------------------------------------------------------------------ */
/*  calculateHealth                                                    */
/* ------------------------------------------------------------------ */

describe("calculateHealth", () => {
  const pos = makePosition({ size: 10, mark_price: 100 });
  // maintenance = 10*100*0.10*0.5 = 50

  it("returns healthy when margin ratio >= 3.0", () => {
    // equity = 200, ratio = 200/50 = 4.0
    const result = calculateHealth([pos], 200);
    expect(result.health).toBe("healthy");
    expect(result.margin_ratio).toBeGreaterThanOrEqual(3.0);
  });

  it("returns warning when margin ratio >= 1.5 but < 3.0", () => {
    // equity = 100, ratio = 100/50 = 2.0
    const result = calculateHealth([pos], 100);
    expect(result.health).toBe("warning");
    expect(result.margin_ratio).toBeGreaterThanOrEqual(1.5);
    expect(result.margin_ratio).toBeLessThan(3.0);
  });

  it("returns critical when margin ratio >= 1.0 but < 1.5", () => {
    // equity = 60, ratio = 60/50 = 1.2
    const result = calculateHealth([pos], 60);
    expect(result.health).toBe("critical");
    expect(result.margin_ratio).toBeGreaterThanOrEqual(1.0);
    expect(result.margin_ratio).toBeLessThan(1.5);
  });

  it("returns liquidatable when margin ratio < 1.0", () => {
    // equity = 40, ratio = 40/50 = 0.8
    const result = calculateHealth([pos], 40);
    expect(result.health).toBe("liquidatable");
    expect(result.margin_ratio).toBeLessThan(1.0);
  });

  it("incorporates unrealized PnL in equity", () => {
    // equity = 40 + 200 = 240, ratio = 240/50 = 4.8 => healthy
    const result = calculateHealth([pos], 40, 200);
    expect(result.equity).toBe(240);
    expect(result.health).toBe("healthy");
  });

  it("calculates liquidation_distance correctly", () => {
    const result = calculateHealth([pos], 200);
    // liquidation_distance = max(0, equity - maintenance) = 200 - 50 = 150
    expect(result.liquidation_distance).toBe(
      result.equity - result.total_maintenance_margin,
    );
  });

  it("returns Infinity margin_ratio with no positions", () => {
    const result = calculateHealth([], 1000);
    expect(result.margin_ratio).toBe(Infinity);
    expect(result.health).toBe("healthy");
  });
});

/* ------------------------------------------------------------------ */
/*  deltaNet                                                           */
/* ------------------------------------------------------------------ */

describe("deltaNet", () => {
  it("nets positions in the same underlying asset", () => {
    const positions: Position[] = [
      makePosition({ asset: "SOL-PERP", side: "long", size: 10, mark_price: 100 }),
      makePosition({ asset: "SOL-SPOT", side: "short", size: 8, mark_price: 100 }),
    ];
    const result = deltaNet(positions);

    // Both map to underlying "SOL"
    expect(result.netting_groups).toHaveLength(1);
    expect(result.netting_groups[0].asset).toBe("SOL");
    expect(result.netting_groups[0].long_delta).toBe(10);
    expect(result.netting_groups[0].short_delta).toBe(8);
    expect(result.netting_groups[0].net_delta).toBe(2); // |10 - 8|
  });

  it("groups different assets separately", () => {
    const positions: Position[] = [
      makePosition({ asset: "SOL-PERP", side: "long", size: 10, mark_price: 100 }),
      makePosition({ asset: "ETH-PERP", side: "long", size: 5, mark_price: 200 }),
    ];
    const result = deltaNet(positions);

    expect(result.netting_groups).toHaveLength(2);
    const assets = result.netting_groups.map((g) => g.asset).sort();
    expect(assets).toEqual(["ETH", "SOL"]);
  });

  it("calculates savings from netting", () => {
    const positions: Position[] = [
      makePosition({ asset: "SOL-PERP", side: "long", size: 10, mark_price: 100 }),
      makePosition({ asset: "SOL-PERP", side: "short", size: 10, mark_price: 100 }),
    ];
    const result = deltaNet(positions);

    // Perfectly hedged: net_delta = 0, reduction = 1.0
    expect(result.netting_groups[0].net_delta).toBe(0);
    expect(result.netting_groups[0].margin_reduction).toBe(1);
    expect(result.savings).toBeGreaterThan(0);
    expect(result.savings_pct).toBe(100);
    expect(result.netted_margin).toBe(0);
  });

  it("returns zero savings when no netting is possible", () => {
    const positions: Position[] = [
      makePosition({ asset: "SOL-PERP", side: "long", size: 10, mark_price: 100 }),
    ];
    const result = deltaNet(positions);

    // Only longs, no shorts to net against
    expect(result.savings).toBe(0);
    expect(result.savings_pct).toBe(0);
    expect(result.netted_margin).toBe(result.gross_margin);
  });

  it("handles empty positions", () => {
    const result = deltaNet([]);
    expect(result.gross_margin).toBe(0);
    expect(result.netted_margin).toBe(0);
    expect(result.savings).toBe(0);
    expect(result.netting_groups).toHaveLength(0);
  });
});
