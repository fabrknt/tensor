/**
 * Compatibility layer for API consumers.
 *
 * Provides:
 * - d1d2WithSigmaFirst: alternative arg order matching the API convention
 *   (S, K, T, sigma, r) instead of (S, K, T, r, sigma)
 * - "perp" as an alias for "perpetual" in PositionType
 * - marginWeightForApiType: accepts "perp" and maps to "perpetual"
 */

import { d1d2 } from './greeks';
import { marginWeightFor } from './margin';

// ---------------------------------------------------------------------------
// d1d2 with sigma-first argument order (API convention)
// ---------------------------------------------------------------------------

/**
 * Compute d1 and d2 with the API's argument order where implied
 * volatility (sigma) comes before the risk-free rate (r).
 *
 * @param S      Underlying price
 * @param K      Strike price
 * @param T      Time to expiry in years (must be > 0)
 * @param sigma  Implied volatility (annualized)
 * @param r      Risk-free rate (annualized)
 */
export function d1d2SigmaFirst(
  S: number,
  K: number,
  T: number,
  sigma: number,
  r: number,
): { d1: number; d2: number } {
  return d1d2(S, K, T, r, sigma);
}

// ---------------------------------------------------------------------------
// Position type alias
// ---------------------------------------------------------------------------

/**
 * Extended position type that includes the "perp" shorthand used by
 * the API, in addition to the SDK's canonical "perpetual".
 */
export type ApiPositionType = 'perpetual' | 'perp' | 'option' | 'spot' | 'lending';

/**
 * Normalize a position type string: maps "perp" to "perpetual" so that
 * downstream SDK functions receive a known instrument_type.
 */
export function normalizePositionType(
  type: ApiPositionType | string,
): string {
  return type === 'perp' ? 'perpetual' : type;
}

// ---------------------------------------------------------------------------
// Margin weight with "perp" support
// ---------------------------------------------------------------------------

/**
 * Return the initial-margin weight for a given instrument type.
 * Accepts the API's "perp" alias in addition to "perpetual".
 */
export function marginWeightForApiType(instrumentType: string): number {
  return marginWeightFor(normalizePositionType(instrumentType));
}
