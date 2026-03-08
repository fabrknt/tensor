/* ------------------------------------------------------------------ */
/*  Shared types for Tensor core math library                         */
/* ------------------------------------------------------------------ */

// ---- Position types ------------------------------------------------

export interface Position {
  asset: string;
  side: "long" | "short";
  size: number;
  entry_price: number;
  mark_price: number;
  instrument_type: "perpetual" | "option" | "spot" | "lending";
  option_type?: "call" | "put";
  strike?: number;
  expiry?: string;
}

export interface OptionPosition {
  asset: string;
  option_type: "call" | "put";
  side: "long" | "short";
  size: number;
  strike: number;
  expiry: string;
  underlying_price: number;
  implied_volatility: number;
  risk_free_rate?: number;
}

// ---- Margin types --------------------------------------------------

export interface PositionMarginDetail {
  asset: string;
  position_margin: number;
  weight: number;
}

export interface MarginResult {
  initial_margin: number;
  maintenance_margin: number;
  margin_used: number;
  margin_available: number;
  positions: PositionMarginDetail[];
}

export type HealthStatus = "healthy" | "warning" | "critical" | "liquidatable";

export interface HealthResult {
  equity: number;
  total_maintenance_margin: number;
  margin_ratio: number;
  liquidation_distance: number;
  health: HealthStatus;
}

export interface NettingGroup {
  asset: string;
  long_delta: number;
  short_delta: number;
  net_delta: number;
  margin_reduction: number;
}

export interface DeltaNetResult {
  gross_margin: number;
  netted_margin: number;
  savings: number;
  savings_pct: number;
  netting_groups: NettingGroup[];
}

// ---- Greeks types --------------------------------------------------

export interface Greeks {
  delta: number;
  gamma: number;
  vega: number;
  theta: number;
}

export interface PositionGreeks extends Greeks {
  asset: string;
  option_type: "call" | "put";
  side: "long" | "short";
  size: number;
}

export interface PortfolioGreeks extends Greeks {
  positions: PositionGreeks[];
  net_exposure: number;
}

// ---- Intent types --------------------------------------------------

export interface Leg {
  asset: string;
  side: "buy" | "sell";
  size: number;
  instrument_type: "perpetual" | "option" | "spot";
  option_type?: "call" | "put";
  strike?: number;
  expiry?: string;
  limit_price?: number;
}

export interface TradingIntent {
  strategy: string;
  legs: Leg[];
  max_slippage_bps?: number;
  deadline?: string;
}

export interface ValidationResult {
  valid: boolean;
  errors: string[];
  warnings: string[];
  estimated_margin_impact: number;
  strategy_recognized: boolean;
  strategy_type: string | null;
}

export interface ExecutionStep {
  sequence: number;
  action: "open" | "close" | "modify";
  asset: string;
  side: "buy" | "sell";
  size: number;
  instrument_type: string;
  reason: string;
  estimated_fill_price?: number;
}

export interface SolverConstraints {
  max_steps?: number;
  prefer_atomic?: boolean;
  gas_budget?: number;
}

export interface SolverResult {
  feasible: boolean;
  steps: ExecutionStep[];
  total_steps: number;
  estimated_gas: number;
  estimated_margin_required: number;
  optimization_notes: string[];
}
