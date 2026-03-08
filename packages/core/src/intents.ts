import type {
  TradingIntent,
  Leg,
  ValidationResult,
  ExecutionStep,
  SolverConstraints,
  SolverResult,
} from "./types";

/* ------------------------------------------------------------------ */
/*  Constants                                                          */
/* ------------------------------------------------------------------ */

const KNOWN_STRATEGIES = new Set([
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
]);

/** Maximum slippage (bps) before a warning is emitted */
const HIGH_SLIPPAGE_BPS = 500;

/** Default margin weight for estimating margin impact */
const DEFAULT_MARGIN_WEIGHT = 0.10;

/** Gas estimate per execution step (lamports / compute units) */
const GAS_PER_STEP = 200_000;

/** Maximum legs that can be submitted atomically */
const MAX_ATOMIC_LEGS = 4;

/** Delta-netting discount when a strategy has both buy and sell legs */
const HEDGE_DISCOUNT = 0.30;

/* ------------------------------------------------------------------ */
/*  validateLeg                                                        */
/* ------------------------------------------------------------------ */

function validateLeg(leg: Leg, index: number): string[] {
  const errors: string[] = [];

  if (!leg.asset) errors.push(`Leg ${index}: asset is required`);
  if (!leg.side) errors.push(`Leg ${index}: side is required`);
  if (!leg.size || leg.size <= 0)
    errors.push(`Leg ${index}: size must be positive`);

  if (leg.instrument_type === "option") {
    if (!leg.option_type)
      errors.push(`Leg ${index}: option_type is required for options`);
    if (!leg.strike)
      errors.push(`Leg ${index}: strike is required for options`);
    if (!leg.expiry)
      errors.push(`Leg ${index}: expiry is required for options`);
  }

  return errors;
}

/* ------------------------------------------------------------------ */
/*  estimateMarginImpact                                               */
/* ------------------------------------------------------------------ */

function estimateMarginImpact(legs: Leg[]): number {
  let impact = 0;
  for (const leg of legs) {
    impact += leg.size * (leg.limit_price || 100) * DEFAULT_MARGIN_WEIGHT;
  }
  return impact;
}

/* ------------------------------------------------------------------ */
/*  validateIntent                                                     */
/* ------------------------------------------------------------------ */

/**
 * Validate a multi-leg trading intent. Checks every leg for required
 * fields, recognizes known strategy patterns, and estimates the
 * margin impact.
 */
export function validateIntent(intent: TradingIntent): ValidationResult {
  const errors: string[] = [];
  const warnings: string[] = [];

  // Validate each leg
  for (let i = 0; i < intent.legs.length; i++) {
    errors.push(...validateLeg(intent.legs[i], i));
  }

  // Slippage warning
  if (
    intent.max_slippage_bps !== undefined &&
    intent.max_slippage_bps > HIGH_SLIPPAGE_BPS
  ) {
    warnings.push(
      "max_slippage_bps exceeds 5% — high slippage tolerance",
    );
  }

  // Strategy recognition
  const recognized = KNOWN_STRATEGIES.has(intent.strategy || "");
  if (!recognized && intent.strategy) {
    warnings.push(
      `Strategy "${intent.strategy}" is not a recognized pattern — custom validation only`,
    );
  }

  return {
    valid: errors.length === 0,
    errors,
    warnings,
    estimated_margin_impact: estimateMarginImpact(intent.legs),
    strategy_recognized: recognized,
    strategy_type: recognized ? intent.strategy : null,
  };
}

/* ------------------------------------------------------------------ */
/*  solveIntent                                                        */
/* ------------------------------------------------------------------ */

/**
 * Decompose a trading intent into an ordered sequence of execution
 * steps. Sells are placed before buys to free up margin. Applies
 * delta-netting discount when the strategy has both buy and sell legs.
 */
export function solveIntent(
  intent: TradingIntent,
  constraints?: SolverConstraints,
): SolverResult {
  const preferAtomic = constraints?.prefer_atomic ?? true;
  const notes: string[] = [];

  // Sort legs: sells before buys to maximise margin availability
  const sortedLegs = [...intent.legs].sort((a, b) => {
    if (a.side === "sell" && b.side === "buy") return -1;
    if (a.side === "buy" && b.side === "sell") return 1;
    return 0;
  });

  // Detect if reordering occurred
  const wasReordered = sortedLegs.some(
    (leg, i) => leg !== intent.legs[i],
  );
  if (wasReordered) {
    notes.push("Legs reordered: sells first to maximize margin availability");
  }

  // Build execution steps
  const steps: ExecutionStep[] = sortedLegs.map((leg, i) => ({
    sequence: i + 1,
    action: "open" as const,
    asset: leg.asset,
    side: leg.side,
    size: leg.size,
    instrument_type: leg.instrument_type,
    reason:
      i === 0
        ? "Primary leg — establishes directional exposure"
        : `Leg ${i + 1} — ${leg.side === "sell" ? "hedges" : "extends"} position`,
    estimated_fill_price: leg.limit_price,
  }));

  // Atomicity notes
  if (preferAtomic && steps.length <= MAX_ATOMIC_LEGS) {
    notes.push(
      "All legs can be submitted atomically in a single transaction",
    );
  } else if (steps.length > MAX_ATOMIC_LEGS) {
    notes.push(
      "Too many legs for atomic execution — split into sequential batches",
    );
  }

  // Estimate margin
  let estimatedMargin = estimateMarginImpact(intent.legs);

  // Delta-netting discount for hedged positions
  const hasBothSides =
    intent.legs.some((l) => l.side === "buy") &&
    intent.legs.some((l) => l.side === "sell");

  if (hasBothSides) {
    estimatedMargin *= 1 - HEDGE_DISCOUNT;
    notes.push(
      `Delta-netting applied: ${HEDGE_DISCOUNT * 100}% margin reduction for hedged legs`,
    );
  }

  // Feasibility check
  const maxSteps = constraints?.max_steps;
  const feasible = maxSteps === undefined || steps.length <= maxSteps;

  if (!feasible) {
    notes.push(
      `Infeasible: ${steps.length} steps required but max_steps=${maxSteps}`,
    );
  }

  return {
    feasible,
    steps,
    total_steps: steps.length,
    estimated_gas: steps.length * GAS_PER_STEP,
    estimated_margin_required: estimatedMargin,
    optimization_notes: notes,
  };
}
