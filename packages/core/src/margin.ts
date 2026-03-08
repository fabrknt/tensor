import type {
  Position,
  MarginResult,
  PositionMarginDetail,
  HealthResult,
  HealthStatus,
  DeltaNetResult,
  NettingGroup,
} from "./types";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

/** Initial margin weight by instrument type */
const MARGIN_WEIGHTS: Record<string, number> = {
  perpetual: 0.10,
  spot: 0.10,
  lending: 0.05,
  option: 0.15,
};

/** Maintenance margin is this fraction of the initial margin */
const MAINTENANCE_RATIO = 0.5;

/** Health thresholds (margin ratio = equity / maintenance margin) */
const HEALTH_THRESHOLDS: Array<{ min: number; status: HealthStatus }> = [
  { min: 3.0, status: "healthy" },
  { min: 1.5, status: "warning" },
  { min: 1.0, status: "critical" },
];

/* ------------------------------------------------------------------ */
/*  marginWeightFor                                                    */
/* ------------------------------------------------------------------ */

/**
 * Return the initial-margin weight for a given instrument type.
 * Falls back to 0.10 for unknown types.
 */
export function marginWeightFor(instrumentType: string): number {
  return MARGIN_WEIGHTS[instrumentType] ?? 0.10;
}

/* ------------------------------------------------------------------ */
/*  calculateMargin                                                    */
/* ------------------------------------------------------------------ */

/**
 * Calculate initial and maintenance margin requirements for a set of
 * positions against a given collateral amount.
 */
export function calculateMargin(
  positions: Position[],
  collateral: number,
): MarginResult {
  let totalInitial = 0;
  let totalMaintenance = 0;

  const details: PositionMarginDetail[] = positions.map((p) => {
    const notional = Math.abs(p.size * p.mark_price);
    const weight = marginWeightFor(p.instrument_type);
    const positionMargin = notional * weight;

    totalInitial += positionMargin;
    totalMaintenance += positionMargin * MAINTENANCE_RATIO;

    return {
      asset: p.asset,
      position_margin: positionMargin,
      weight,
    };
  });

  return {
    initial_margin: totalInitial,
    maintenance_margin: totalMaintenance,
    margin_used: totalInitial,
    margin_available: Math.max(0, collateral - totalInitial),
    positions: details,
  };
}

/* ------------------------------------------------------------------ */
/*  calculateHealth                                                    */
/* ------------------------------------------------------------------ */

/**
 * Evaluate the health of an account given its positions, collateral,
 * and unrealized PnL. Returns equity, maintenance margin, margin ratio,
 * liquidation distance, and a health classification.
 */
export function calculateHealth(
  positions: Position[],
  collateral: number,
  unrealizedPnl: number = 0,
): HealthResult {
  const equity = collateral + unrealizedPnl;

  let totalMaintenance = 0;
  for (const p of positions) {
    const notional = Math.abs(p.size * p.mark_price);
    const weight = marginWeightFor(p.instrument_type);
    totalMaintenance += notional * weight * MAINTENANCE_RATIO;
  }

  const marginRatio =
    totalMaintenance > 0 ? equity / totalMaintenance : Infinity;
  const liquidationDistance = Math.max(0, equity - totalMaintenance);

  let health: HealthStatus = "liquidatable";
  for (const threshold of HEALTH_THRESHOLDS) {
    if (marginRatio >= threshold.min) {
      health = threshold.status;
      break;
    }
  }

  return {
    equity,
    total_maintenance_margin: totalMaintenance,
    margin_ratio: marginRatio,
    liquidation_distance: liquidationDistance,
    health,
  };
}

/* ------------------------------------------------------------------ */
/*  deltaNet                                                           */
/* ------------------------------------------------------------------ */

/**
 * Compute delta-netted margin. Groups positions by underlying asset,
 * calculates the net delta for each group, and determines the margin
 * reduction from hedging offsets.
 */
export function deltaNet(positions: Position[]): DeltaNetResult {
  const groups: Record<string, { longDelta: number; shortDelta: number }> = {};
  let grossMargin = 0;

  for (const p of positions) {
    const notional = Math.abs(p.size * p.mark_price);
    const weight = marginWeightFor(p.instrument_type);
    grossMargin += notional * weight;

    // Derive the underlying from the asset name (e.g. "SOL-PERP" -> "SOL")
    const underlying = p.asset.split("-")[0] || p.asset;
    if (!groups[underlying]) {
      groups[underlying] = { longDelta: 0, shortDelta: 0 };
    }

    const delta = p.side === "long" ? p.size : -p.size;
    if (delta > 0) {
      groups[underlying].longDelta += delta;
    } else {
      groups[underlying].shortDelta += Math.abs(delta);
    }
  }

  const nettingGroups: NettingGroup[] = Object.entries(groups).map(
    ([asset, g]) => {
      const netDelta = Math.abs(g.longDelta - g.shortDelta);
      const grossDelta = g.longDelta + g.shortDelta;
      const reduction = grossDelta > 0 ? 1 - netDelta / grossDelta : 0;

      return {
        asset,
        long_delta: g.longDelta,
        short_delta: g.shortDelta,
        net_delta: netDelta,
        margin_reduction: reduction,
      };
    },
  );

  const avgReduction =
    nettingGroups.length > 0
      ? nettingGroups.reduce((sum, g) => sum + g.margin_reduction, 0) /
        nettingGroups.length
      : 0;

  const nettedMargin = grossMargin * (1 - avgReduction);
  const savings = grossMargin - nettedMargin;

  return {
    gross_margin: grossMargin,
    netted_margin: nettedMargin,
    savings,
    savings_pct: grossMargin > 0 ? (savings / grossMargin) * 100 : 0,
    netting_groups: nettingGroups,
  };
}
