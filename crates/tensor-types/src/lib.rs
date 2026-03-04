use anchor_lang::prelude::*;

pub const PRECISION: u128 = 1_000_000; // 1e6 fixed-point
pub const BPS_PRECISION: u128 = 10_000;
pub const MAX_PERP_POSITIONS: usize = 8;
pub const MAX_SPOT_BALANCES: usize = 16;
pub const MAX_OPTION_POSITIONS: usize = 8;
pub const MAX_LENDING_POSITIONS: usize = 8;
pub const MAX_INTENT_LEGS: usize = 4;

// ---------------------------------------------------------------------------
// Product Types
// ---------------------------------------------------------------------------

/// All product types that contribute to unified margin
#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum ProductType {
    Spot,
    Perpetual,
    Option,
    Lending,
    VarianceSwap,
}

// ---------------------------------------------------------------------------
// Account Health
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum AccountHealth {
    /// equity / maintenance_margin > 1.5
    Healthy,
    /// 1.0 < equity / maintenance_margin <= 1.5
    Warning,
    /// equity / maintenance_margin <= 1.0, eligible for liquidation
    Liquidatable,
    /// equity <= 0, socialized loss
    Bankrupt,
}

impl Default for AccountHealth {
    fn default() -> Self {
        AccountHealth::Healthy
    }
}

// ---------------------------------------------------------------------------
// Perp Position
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, InitSpace, Debug)]
pub struct PerpPosition {
    /// Market index (maps to a MarginMarket)
    pub market_index: u16,
    /// Position size in base units (signed: positive=long, negative=short)
    pub size: i64,
    /// Entry price in quote units (1e6 precision)
    pub entry_price: u64,
    /// Cumulative realized PnL
    pub realized_pnl: i64,
    /// Unrealized PnL at last mark
    pub unrealized_pnl: i64,
    /// Cumulative funding payments (positive = received, negative = paid)
    pub cumulative_funding: i64,
    /// Last funding rate snapshot
    pub last_funding_index: i64,
    /// Timestamp of position open
    pub opened_at: i64,
    /// Whether this slot is occupied
    pub is_active: bool,
}

impl PerpPosition {
    /// Notional value = |size| * mark_price / PRECISION
    pub fn notional(&self, mark_price: u64) -> u64 {
        let abs_size = if self.size < 0 { -self.size } else { self.size } as u128;
        (abs_size * mark_price as u128 / PRECISION) as u64
    }

    /// Delta contribution: size (linear, 1:1 with underlying)
    pub fn delta(&self) -> i64 {
        self.size
    }

    /// Mark-to-market PnL = size * (mark_price - entry_price) / PRECISION
    pub fn mark_pnl(&self, mark_price: u64) -> i64 {
        let diff = mark_price as i128 - self.entry_price as i128;
        (self.size as i128 * diff / PRECISION as i128) as i64
    }
}

// ---------------------------------------------------------------------------
// Spot Balance
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, InitSpace, Debug)]
pub struct SpotBalance {
    /// Token mint
    pub mint: Pubkey,
    /// Balance in token units (always positive)
    pub balance: u64,
    /// Value in quote currency at last mark (1e6 precision)
    pub value: u64,
    /// Market index for price lookups
    pub market_index: u16,
    /// Whether this slot is occupied
    pub is_active: bool,
}

impl SpotBalance {
    /// Delta contribution from spot: balance (positive, always long)
    pub fn delta(&self) -> i64 {
        self.balance as i64
    }
}

// ---------------------------------------------------------------------------
// Option Position
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum OptionSide {
    Call,
    Put,
}

impl Default for OptionSide {
    fn default() -> Self {
        OptionSide::Call
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum OptionKind {
    Vanilla,
    Asian,
    BarrierKnockOut,
    BarrierKnockIn,
}

impl Default for OptionKind {
    fn default() -> Self {
        OptionKind::Vanilla
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, InitSpace, Debug)]
pub struct OptionPosition {
    /// Market index for the underlying
    pub market_index: u16,
    /// Call or Put
    pub side: OptionSide,
    /// Option variant
    pub kind: OptionKind,
    /// Strike price (1e6 precision)
    pub strike: u64,
    /// Barrier price if applicable (1e6 precision)
    pub barrier: u64,
    /// Number of contracts (signed: positive=long, negative=short/written)
    pub contracts: i64,
    /// Notional per contract
    pub notional_per_contract: u64,
    /// Expiry timestamp
    pub expiry: i64,
    /// Premium paid/received per contract (1e6 precision)
    pub premium: u64,
    /// Pre-computed delta per contract (1e6 scaled, signed)
    pub delta_per_contract: i64,
    /// Pre-computed gamma per contract (1e6 scaled)
    pub gamma_per_contract: i64,
    /// Pre-computed vega per contract (1e6 scaled)
    pub vega_per_contract: i64,
    /// Pre-computed theta per contract (1e6 scaled, typically negative)
    pub theta_per_contract: i64,
    /// Timestamp of position open
    pub opened_at: i64,
    /// Whether this slot is occupied
    pub is_active: bool,
}

impl OptionPosition {
    /// Total delta = contracts * delta_per_contract
    pub fn delta(&self) -> i64 {
        (self.contracts as i128 * self.delta_per_contract as i128 / PRECISION as i128) as i64
    }

    /// Total gamma = |contracts| * gamma_per_contract
    pub fn gamma(&self) -> i64 {
        (self.contracts as i128 * self.gamma_per_contract as i128 / PRECISION as i128) as i64
    }

    /// Total vega = contracts * vega_per_contract
    pub fn vega(&self) -> i64 {
        (self.contracts as i128 * self.vega_per_contract as i128 / PRECISION as i128) as i64
    }

    /// Total theta = contracts * theta_per_contract
    pub fn theta(&self) -> i64 {
        (self.contracts as i128 * self.theta_per_contract as i128 / PRECISION as i128) as i64
    }

    /// Notional value of position
    pub fn notional(&self) -> u64 {
        let abs_contracts = if self.contracts < 0 { -self.contracts } else { self.contracts } as u128;
        (abs_contracts * self.notional_per_contract as u128 / PRECISION) as u64
    }
}

// ---------------------------------------------------------------------------
// Lending Position
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum LendingSide {
    /// Supplying collateral (earns yield)
    Supply,
    /// Borrowing (pays interest)
    Borrow,
}

impl Default for LendingSide {
    fn default() -> Self {
        LendingSide::Supply
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, InitSpace, Debug)]
pub struct LendingPosition {
    /// Token mint
    pub mint: Pubkey,
    /// Market index
    pub market_index: u16,
    /// Supply or borrow
    pub side: LendingSide,
    /// Principal amount
    pub principal: u64,
    /// Accrued interest
    pub accrued_interest: i64,
    /// Current annual rate in bps
    pub rate_bps: u16,
    /// Haircut applied to collateral value (bps, for supply-side)
    pub haircut_bps: u16,
    /// Effective value after haircut (supply) or full value (borrow)
    pub effective_value: u64,
    /// Last interest accrual timestamp
    pub last_accrual: i64,
    /// Whether this slot is occupied
    pub is_active: bool,
}

impl LendingPosition {
    /// Effective collateral contribution (supply adds, borrow subtracts)
    pub fn margin_contribution(&self) -> i64 {
        match self.side {
            LendingSide::Supply => self.effective_value as i64,
            LendingSide::Borrow => -(self.principal as i64 + self.accrued_interest),
        }
    }
}

// ---------------------------------------------------------------------------
// Portfolio Greeks (aggregated)
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Default, InitSpace, Debug)]
pub struct PortfolioGreeks {
    /// Net delta across all products (base units, signed)
    pub delta: i64,
    /// Net gamma (from options only, 1e6 scaled)
    pub gamma: i64,
    /// Net vega (from options + vol swaps, 1e6 scaled)
    pub vega: i64,
    /// Net theta (from options, 1e6 scaled, typically negative)
    pub theta: i64,
    /// Total notional exposure
    pub total_notional: u64,
    /// Timestamp of last computation
    pub computed_at: i64,
}

// ---------------------------------------------------------------------------
// Collateral Type (compatible with northtail-collateral)
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum CollateralType {
    Usdc,
    Usdt,
    Sol,
    Jgb,
    Stablecoin,
    Equity,
    /// Wrapped BTC (cbBTC, WBTC) — custodial, deep liquidity
    Btc,
    /// Yield-bearing BTC (LBTC, SolvBTC) — earns staking yield, extra smart contract risk
    BtcYield,
    /// Trust-minimized BTC (zBTC) — permissionless bridge, lower liquidity
    BtcTrustMinimized,
}

impl Default for CollateralType {
    fn default() -> Self {
        CollateralType::Usdc
    }
}

impl CollateralType {
    /// Default haircut in basis points
    pub fn default_haircut_bps(&self) -> u16 {
        match self {
            CollateralType::Usdc => 0,      // No haircut for primary quote
            CollateralType::Usdt => 50,     // 0.5% for USDT
            CollateralType::Sol => 1500,    // 15% for SOL
            CollateralType::Jgb => 500,     // 5% for JGBs
            CollateralType::Stablecoin => 200, // 2% for other stables
            CollateralType::Equity => 2500, // 25% for equities
            CollateralType::Btc => 1000,    // 10% for wrapped BTC (cbBTC, WBTC)
            CollateralType::BtcYield => 1200, // 12% for yield-bearing BTC (LBTC)
            CollateralType::BtcTrustMinimized => 1500, // 15% for trust-minimized BTC (zBTC)
        }
    }
}

// ---------------------------------------------------------------------------
// Margin Mode
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, InitSpace, Debug)]
pub enum MarginMode {
    /// Each position margined independently
    Isolated,
    /// Positions cross-margined within a product type
    Cross,
    /// Full portfolio margining with Greeks-based netting
    Portfolio,
}

impl Default for MarginMode {
    fn default() -> Self {
        MarginMode::Cross
    }
}

// ---------------------------------------------------------------------------
// Investor Category (compatible with northtail-types)
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, InitSpace, Debug)]
pub enum InvestorCategory {
    /// Retail — most restricted
    Retail,
    /// Qualified — fewer restrictions
    Qualified,
    /// Institutional — fewest restrictions
    Institutional,
}

impl Default for InvestorCategory {
    fn default() -> Self {
        InvestorCategory::Retail
    }
}

impl InvestorCategory {
    /// Maximum leverage multiplier (in bps, 10000 = 1x)
    pub fn max_leverage_bps(&self) -> u64 {
        match self {
            InvestorCategory::Retail => 50_000,        // 5x
            InvestorCategory::Qualified => 200_000,    // 20x
            InvestorCategory::Institutional => 500_000, // 50x
        }
    }
}

// ---------------------------------------------------------------------------
// Intent Types (Phase 3)
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, Default, InitSpace)]
pub enum IntentStatus {
    #[default]
    Pending,
    PartiallyFilled,
    Filled,
    Cancelled,
    Expired,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, Default, InitSpace)]
pub enum IntentType {
    #[default]
    Market,
    Limit,
    Conditional,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, Debug, Default, InitSpace)]
pub struct IntentLeg {
    pub product_type: ProductType,
    pub market_index: u16,
    /// Signed size: positive = buy/long, negative = sell/short
    pub size: i64,
    /// Limit price (1e6 precision), 0 = market order
    pub limit_price: u64,
    pub is_active: bool,
}

impl Default for ProductType {
    fn default() -> Self {
        ProductType::Spot
    }
}

// ---------------------------------------------------------------------------
// ZK Credit Types (Phase 3)
// ---------------------------------------------------------------------------

#[derive(AnchorSerialize, AnchorDeserialize, Clone, Copy, PartialEq, Eq, Debug, Default, InitSpace)]
pub enum ZkCreditTier {
    #[default]
    None,
    Bronze,
    Silver,
    Gold,
    Platinum,
}

impl ZkCreditTier {
    pub fn from_score(score: u16) -> Self {
        match score {
            800.. => ZkCreditTier::Platinum,
            650..=799 => ZkCreditTier::Gold,
            500..=649 => ZkCreditTier::Silver,
            300..=499 => ZkCreditTier::Bronze,
            _ => ZkCreditTier::None,
        }
    }

    /// Margin discount in BPS (applied to initial margin)
    pub fn margin_discount_bps(&self) -> u64 {
        match self {
            ZkCreditTier::Platinum => 2000, // 20%
            ZkCreditTier::Gold => 1500,     // 15%
            ZkCreditTier::Silver => 1000,   // 10%
            ZkCreditTier::Bronze => 500,    // 5%
            ZkCreditTier::None => 0,
        }
    }

    /// Leverage bonus in BPS (added to max leverage)
    pub fn leverage_bonus_bps(&self) -> u64 {
        match self {
            ZkCreditTier::Platinum => 10000, // +1x
            ZkCreditTier::Gold => 7500,      // +0.75x
            ZkCreditTier::Silver => 5000,    // +0.5x
            ZkCreditTier::Bronze => 2500,    // +0.25x
            ZkCreditTier::None => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // PerpPosition tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_perp_notional_long() {
        let perp = PerpPosition {
            size: 100_000_000, // 100 units
            entry_price: 150_000_000, // $150
            is_active: true,
            ..Default::default()
        };
        // notional = |100_000_000| * 150_000_000 / 1_000_000 = 15_000_000_000
        assert_eq!(perp.notional(150_000_000), 15_000_000_000);
    }

    #[test]
    fn test_perp_notional_short() {
        let perp = PerpPosition {
            size: -50_000_000, // short 50
            entry_price: 200_000_000,
            is_active: true,
            ..Default::default()
        };
        // notional = |50_000_000| * 200_000_000 / 1_000_000 = 10_000_000_000
        assert_eq!(perp.notional(200_000_000), 10_000_000_000);
    }

    #[test]
    fn test_perp_notional_zero_price() {
        let perp = PerpPosition {
            size: 100_000_000,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(perp.notional(0), 0);
    }

    #[test]
    fn test_perp_delta() {
        let perp = PerpPosition {
            size: 42_000_000,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(perp.delta(), 42_000_000);

        let short = PerpPosition {
            size: -10_000_000,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(short.delta(), -10_000_000);
    }

    #[test]
    fn test_perp_mark_pnl_profit() {
        let perp = PerpPosition {
            size: 10_000_000, // 10 units
            entry_price: 100_000_000, // $100
            is_active: true,
            ..Default::default()
        };
        // PnL = 10_000_000 * (120_000_000 - 100_000_000) / 1_000_000 = 200_000_000
        assert_eq!(perp.mark_pnl(120_000_000), 200_000_000);
    }

    #[test]
    fn test_perp_mark_pnl_loss() {
        let perp = PerpPosition {
            size: 10_000_000,
            entry_price: 100_000_000,
            is_active: true,
            ..Default::default()
        };
        // PnL = 10_000_000 * (80_000_000 - 100_000_000) / 1_000_000 = -200_000_000
        assert_eq!(perp.mark_pnl(80_000_000), -200_000_000);
    }

    #[test]
    fn test_perp_mark_pnl_short_profit() {
        let perp = PerpPosition {
            size: -10_000_000,
            entry_price: 100_000_000,
            is_active: true,
            ..Default::default()
        };
        // Short profit when price drops: -10 * (80 - 100) / 1e6 = +200_000_000
        assert_eq!(perp.mark_pnl(80_000_000), 200_000_000);
    }

    #[test]
    fn test_perp_mark_pnl_short_loss() {
        let perp = PerpPosition {
            size: -10_000_000,
            entry_price: 100_000_000,
            is_active: true,
            ..Default::default()
        };
        // Short loss when price rises: -10 * (120 - 100) / 1e6 = -200_000_000
        assert_eq!(perp.mark_pnl(120_000_000), -200_000_000);
    }

    #[test]
    fn test_perp_mark_pnl_no_change() {
        let perp = PerpPosition {
            size: 50_000_000,
            entry_price: 150_000_000,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(perp.mark_pnl(150_000_000), 0);
    }

    // -----------------------------------------------------------------------
    // SpotBalance tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_spot_delta() {
        let spot = SpotBalance {
            balance: 500_000_000,
            value: 75_000_000_000,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(spot.delta(), 500_000_000);
    }

    #[test]
    fn test_spot_delta_zero() {
        let spot = SpotBalance::default();
        assert_eq!(spot.delta(), 0);
    }

    // -----------------------------------------------------------------------
    // OptionPosition tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_option_delta_long_call() {
        let opt = OptionPosition {
            contracts: 10_000_000, // 10 contracts
            delta_per_contract: 500_000, // 0.5 delta
            is_active: true,
            expiry: 1_000_000,
            ..Default::default()
        };
        // delta = 10_000_000 * 500_000 / 1_000_000 = 5_000_000
        assert_eq!(opt.delta(), 5_000_000);
    }

    #[test]
    fn test_option_delta_short_call() {
        let opt = OptionPosition {
            contracts: -10_000_000,
            delta_per_contract: 500_000,
            is_active: true,
            expiry: 1_000_000,
            ..Default::default()
        };
        // delta = -10_000_000 * 500_000 / 1_000_000 = -5_000_000
        assert_eq!(opt.delta(), -5_000_000);
    }

    #[test]
    fn test_option_gamma() {
        let opt = OptionPosition {
            contracts: -10_000_000,
            gamma_per_contract: 50_000, // 0.05
            is_active: true,
            expiry: 1_000_000,
            ..Default::default()
        };
        // gamma = -10_000_000 * 50_000 / 1_000_000 = -500_000
        assert_eq!(opt.gamma(), -500_000);
    }

    #[test]
    fn test_option_vega() {
        let opt = OptionPosition {
            contracts: 5_000_000,
            vega_per_contract: 200_000,
            is_active: true,
            expiry: 1_000_000,
            ..Default::default()
        };
        // vega = 5_000_000 * 200_000 / 1_000_000 = 1_000_000
        assert_eq!(opt.vega(), 1_000_000);
    }

    #[test]
    fn test_option_theta() {
        let opt = OptionPosition {
            contracts: 10_000_000,
            theta_per_contract: -20_000,
            is_active: true,
            expiry: 1_000_000,
            ..Default::default()
        };
        // theta = 10_000_000 * (-20_000) / 1_000_000 = -200_000
        assert_eq!(opt.theta(), -200_000);
    }

    #[test]
    fn test_option_notional() {
        let opt = OptionPosition {
            contracts: -20_000_000,
            notional_per_contract: 1_000_000,
            is_active: true,
            expiry: 1_000_000,
            ..Default::default()
        };
        // notional = |20_000_000| * 1_000_000 / 1_000_000 = 20_000_000
        assert_eq!(opt.notional(), 20_000_000);
    }

    // -----------------------------------------------------------------------
    // LendingPosition tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_lending_supply_contribution() {
        let lending = LendingPosition {
            side: LendingSide::Supply,
            principal: 10_000_000_000,
            effective_value: 9_500_000_000, // after 5% haircut
            is_active: true,
            ..Default::default()
        };
        assert_eq!(lending.margin_contribution(), 9_500_000_000);
    }

    #[test]
    fn test_lending_borrow_contribution() {
        let lending = LendingPosition {
            side: LendingSide::Borrow,
            principal: 5_000_000_000,
            accrued_interest: 100_000_000,
            is_active: true,
            ..Default::default()
        };
        // margin_contribution = -(principal + interest) = -(5_000_000_000 + 100_000_000)
        assert_eq!(lending.margin_contribution(), -5_100_000_000);
    }

    // -----------------------------------------------------------------------
    // CollateralType haircuts
    // -----------------------------------------------------------------------

    #[test]
    fn test_collateral_haircuts() {
        assert_eq!(CollateralType::Usdc.default_haircut_bps(), 0);
        assert_eq!(CollateralType::Usdt.default_haircut_bps(), 50);
        assert_eq!(CollateralType::Sol.default_haircut_bps(), 1500);
        assert_eq!(CollateralType::Jgb.default_haircut_bps(), 500);
        assert_eq!(CollateralType::Stablecoin.default_haircut_bps(), 200);
        assert_eq!(CollateralType::Equity.default_haircut_bps(), 2500);
        assert_eq!(CollateralType::Btc.default_haircut_bps(), 1000);
        assert_eq!(CollateralType::BtcYield.default_haircut_bps(), 1200);
        assert_eq!(CollateralType::BtcTrustMinimized.default_haircut_bps(), 1500);
    }

    // -----------------------------------------------------------------------
    // InvestorCategory leverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_investor_leverage_limits() {
        assert_eq!(InvestorCategory::Retail.max_leverage_bps(), 50_000);
        assert_eq!(InvestorCategory::Qualified.max_leverage_bps(), 200_000);
        assert_eq!(InvestorCategory::Institutional.max_leverage_bps(), 500_000);
    }

    #[test]
    fn test_investor_category_ordering() {
        assert!(InvestorCategory::Retail < InvestorCategory::Qualified);
        assert!(InvestorCategory::Qualified < InvestorCategory::Institutional);
    }

    // -----------------------------------------------------------------------
    // Default values
    // -----------------------------------------------------------------------

    #[test]
    fn test_defaults() {
        assert_eq!(AccountHealth::default(), AccountHealth::Healthy);
        assert_eq!(OptionSide::default(), OptionSide::Call);
        assert_eq!(OptionKind::default(), OptionKind::Vanilla);
        assert_eq!(LendingSide::default(), LendingSide::Supply);
        assert_eq!(MarginMode::default(), MarginMode::Cross);
        assert_eq!(InvestorCategory::default(), InvestorCategory::Retail);
        assert_eq!(CollateralType::default(), CollateralType::Usdc);
    }

    #[test]
    fn test_perp_position_default_inactive() {
        let p = PerpPosition::default();
        assert!(!p.is_active);
        assert_eq!(p.size, 0);
        assert_eq!(p.entry_price, 0);
        assert_eq!(p.notional(100_000_000), 0);
        assert_eq!(p.mark_pnl(100_000_000), 0);
    }

    #[test]
    fn test_option_position_default_inactive() {
        let o = OptionPosition::default();
        assert!(!o.is_active);
        assert_eq!(o.contracts, 0);
        assert_eq!(o.delta(), 0);
        assert_eq!(o.gamma(), 0);
        assert_eq!(o.vega(), 0);
        assert_eq!(o.theta(), 0);
        assert_eq!(o.notional(), 0);
    }

    // -----------------------------------------------------------------------
    // ZkCreditTier tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_zk_credit_tier_from_score_boundaries() {
        assert_eq!(ZkCreditTier::from_score(0), ZkCreditTier::None);
        assert_eq!(ZkCreditTier::from_score(299), ZkCreditTier::None);
        assert_eq!(ZkCreditTier::from_score(300), ZkCreditTier::Bronze);
        assert_eq!(ZkCreditTier::from_score(499), ZkCreditTier::Bronze);
        assert_eq!(ZkCreditTier::from_score(500), ZkCreditTier::Silver);
        assert_eq!(ZkCreditTier::from_score(649), ZkCreditTier::Silver);
        assert_eq!(ZkCreditTier::from_score(650), ZkCreditTier::Gold);
        assert_eq!(ZkCreditTier::from_score(799), ZkCreditTier::Gold);
        assert_eq!(ZkCreditTier::from_score(800), ZkCreditTier::Platinum);
        assert_eq!(ZkCreditTier::from_score(1000), ZkCreditTier::Platinum);
    }

    #[test]
    fn test_zk_credit_margin_discount_bps() {
        assert_eq!(ZkCreditTier::None.margin_discount_bps(), 0);
        assert_eq!(ZkCreditTier::Bronze.margin_discount_bps(), 500);
        assert_eq!(ZkCreditTier::Silver.margin_discount_bps(), 1000);
        assert_eq!(ZkCreditTier::Gold.margin_discount_bps(), 1500);
        assert_eq!(ZkCreditTier::Platinum.margin_discount_bps(), 2000);
    }

    #[test]
    fn test_zk_credit_leverage_bonus_bps() {
        assert_eq!(ZkCreditTier::None.leverage_bonus_bps(), 0);
        assert_eq!(ZkCreditTier::Bronze.leverage_bonus_bps(), 2500);
        assert_eq!(ZkCreditTier::Silver.leverage_bonus_bps(), 5000);
        assert_eq!(ZkCreditTier::Gold.leverage_bonus_bps(), 7500);
        assert_eq!(ZkCreditTier::Platinum.leverage_bonus_bps(), 10000);
    }

    #[test]
    fn test_intent_leg_default() {
        let leg = IntentLeg::default();
        assert_eq!(leg.size, 0);
        assert_eq!(leg.limit_price, 0);
        assert!(!leg.is_active);
    }

    #[test]
    fn test_intent_status_default() {
        assert_eq!(IntentStatus::default(), IntentStatus::Pending);
    }

    #[test]
    fn test_intent_type_default() {
        assert_eq!(IntentType::default(), IntentType::Market);
    }

    #[test]
    fn test_zk_credit_tier_default() {
        assert_eq!(ZkCreditTier::default(), ZkCreditTier::None);
    }
}
