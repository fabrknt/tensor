use anchor_lang::prelude::*;

/// Registered market within the unified margin system.
/// Each market represents a tradeable asset (SOL, BTC, ETH, etc.)
/// and links to oracle feeds + product-specific programs.
#[account]
#[derive(InitSpace)]
pub struct MarginMarket {
    /// Market index (unique, sequential)
    pub index: u16,

    /// Asset symbol
    #[max_len(10)]
    pub symbol: String,

    /// Base token mint (the asset being priced)
    pub base_mint: Pubkey,

    /// Price oracle feed (reads from sigma shared-oracle or northtail-oracle)
    pub oracle: Pubkey,

    /// Variance tracker (from sigma shared-oracle, for vol data)
    pub variance_tracker: Pubkey,

    /// Supported product types for this market
    pub spot_enabled: bool,
    pub perp_enabled: bool,
    pub options_enabled: bool,
    pub lending_enabled: bool,

    /// Market-specific margin overrides (0 = use global default)
    pub initial_margin_bps: u64,
    pub maintenance_ratio_bps: u64,

    /// Maximum position size (in base units, 0 = unlimited)
    pub max_position_size: u64,

    /// Current mark price (updated by keepers)
    pub mark_price: u64,

    /// Implied volatility in bps (updated by keepers, from sigma oracle)
    pub implied_vol_bps: u64,

    /// Funding rate (for perps, signed, in bps per 8h)
    pub funding_rate_bps: i64,

    /// Cumulative funding index (1e9 precision)
    pub cumulative_funding_index: i128,

    /// Last funding update timestamp
    pub last_funding_update: i64,

    /// Open interest (total notional across all positions)
    pub open_interest_long: u64,
    pub open_interest_short: u64,

    /// Total volume (lifetime)
    pub total_volume: u128,

    /// Whether this market is active
    pub is_active: bool,

    /// PDA bump
    pub bump: u8,
}

impl MarginMarket {
    pub const SEED: &'static [u8] = b"margin_market";

    /// Get effective initial margin (market override or global)
    pub fn effective_initial_margin(&self, global_bps: u64) -> u64 {
        if self.initial_margin_bps > 0 {
            self.initial_margin_bps
        } else {
            global_bps
        }
    }

    /// Get effective maintenance ratio (market override or global)
    pub fn effective_maintenance_ratio(&self, global_bps: u64) -> u64 {
        if self.maintenance_ratio_bps > 0 {
            self.maintenance_ratio_bps
        } else {
            global_bps
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_market() -> MarginMarket {
        MarginMarket {
            index: 0,
            symbol: "SOL".to_string(),
            base_mint: Pubkey::new_unique(),
            oracle: Pubkey::new_unique(),
            variance_tracker: Pubkey::new_unique(),
            spot_enabled: true,
            perp_enabled: true,
            options_enabled: true,
            lending_enabled: false,
            initial_margin_bps: 0,      // use global
            maintenance_ratio_bps: 0,   // use global
            max_position_size: 0,
            mark_price: 150_000_000,
            implied_vol_bps: 3000,
            funding_rate_bps: 50,
            cumulative_funding_index: 0,
            last_funding_update: 0,
            open_interest_long: 0,
            open_interest_short: 0,
            total_volume: 0,
            is_active: true,
            bump: 255,
        }
    }

    #[test]
    fn test_effective_initial_margin_uses_global() {
        let market = make_market();
        assert_eq!(market.effective_initial_margin(1000), 1000);
    }

    #[test]
    fn test_effective_initial_margin_uses_override() {
        let mut market = make_market();
        market.initial_margin_bps = 2000;
        assert_eq!(market.effective_initial_margin(1000), 2000);
    }

    #[test]
    fn test_effective_maintenance_ratio_uses_global() {
        let market = make_market();
        assert_eq!(market.effective_maintenance_ratio(5000), 5000);
    }

    #[test]
    fn test_effective_maintenance_ratio_uses_override() {
        let mut market = make_market();
        market.maintenance_ratio_bps = 8000;
        assert_eq!(market.effective_maintenance_ratio(5000), 8000);
    }
}
