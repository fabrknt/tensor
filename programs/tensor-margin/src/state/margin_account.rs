use anchor_lang::prelude::*;
use tensor_types::*;

/// Per-user unified margin account.
///
/// This is the centerpiece of the Tensor protocol. A single account holds
/// all of a user's positions across spot, perps, options, and lending,
/// enabling portfolio-level margin calculation with Greeks-based netting.
///
/// Design choices:
/// - Fixed-size arrays instead of Vec to avoid realloc and enable zero-copy reads
/// - Inline positions instead of separate accounts to minimize CPI overhead
/// - Pre-computed Greeks cached on account to avoid recomputation on reads
#[account]
#[derive(InitSpace)]
pub struct MarginAccount {
    // -- Identity --
    /// Account owner
    pub owner: Pubkey,
    /// Delegate authority (can trade on behalf of owner)
    pub delegate: Pubkey,

    // -- Collateral --
    /// Total deposited collateral (USDC, primary quote)
    pub collateral: u64,
    /// Collateral locked for positions (cannot withdraw)
    pub locked_collateral: u64,

    // -- Perp Positions --
    pub perp_positions: [PerpPosition; MAX_PERP_POSITIONS],
    pub perp_count: u8,

    // -- Spot Balances --
    pub spot_balances: [SpotBalance; MAX_SPOT_BALANCES],
    pub spot_count: u8,

    // -- Option Positions --
    pub option_positions: [OptionPosition; MAX_OPTION_POSITIONS],
    pub option_count: u8,

    // -- Lending Positions --
    pub lending_positions: [LendingPosition; MAX_LENDING_POSITIONS],
    pub lending_count: u8,

    // -- Aggregate Risk (cached, updated by compute_margin) --
    pub greeks: PortfolioGreeks,
    pub initial_margin_required: u64,
    pub maintenance_margin_required: u64,
    pub equity: i64,
    pub margin_ratio_bps: u16,
    pub health: AccountHealth,

    // -- Margin Mode --
    pub margin_mode: MarginMode,

    // -- Compliance --
    pub investor_category: InvestorCategory,
    /// Optional link to sovereign identity PDA
    pub identity: Pubkey,

    // -- ZK Credit (Phase 3) --
    pub zk_credit_score: u16,
    pub zk_credit_tier: ZkCreditTier,
    pub zk_score_updated_at: i64,
    pub zk_credit_oracle: Pubkey,
    pub active_intent_count: u8,

    // -- Metadata --
    pub created_at: i64,
    pub last_margin_update: i64,
    pub total_trades: u64,
    pub total_realized_pnl: i64,
    pub bump: u8,
}

impl MarginAccount {
    pub const SEED: &'static [u8] = b"margin_account";

    /// Available collateral = total - locked
    pub fn available_collateral(&self) -> u64 {
        self.collateral.saturating_sub(self.locked_collateral)
    }

    /// Find first empty perp position slot
    pub fn find_empty_perp_slot(&self) -> Option<usize> {
        self.perp_positions.iter().position(|p| !p.is_active)
    }

    /// Find perp position by market index
    pub fn find_perp_by_market(&self, market_index: u16) -> Option<usize> {
        self.perp_positions
            .iter()
            .position(|p| p.is_active && p.market_index == market_index)
    }

    /// Find first empty spot balance slot
    pub fn find_empty_spot_slot(&self) -> Option<usize> {
        self.spot_balances.iter().position(|s| !s.is_active)
    }

    /// Find spot balance by mint
    pub fn find_spot_by_mint(&self, mint: &Pubkey) -> Option<usize> {
        self.spot_balances
            .iter()
            .position(|s| s.is_active && s.mint == *mint)
    }

    /// Find first empty option position slot
    pub fn find_empty_option_slot(&self) -> Option<usize> {
        self.option_positions.iter().position(|o| !o.is_active)
    }

    /// Find first empty lending position slot
    pub fn find_empty_lending_slot(&self) -> Option<usize> {
        self.lending_positions.iter().position(|l| !l.is_active)
    }

    /// Find lending position by mint and side
    pub fn find_lending_by_mint(&self, mint: &Pubkey, side: LendingSide) -> Option<usize> {
        self.lending_positions
            .iter()
            .position(|l| l.is_active && l.mint == *mint && l.side == side)
    }

    /// Check if account has any active positions
    pub fn has_positions(&self) -> bool {
        self.perp_positions.iter().any(|p| p.is_active)
            || self.spot_balances.iter().any(|s| s.is_active)
            || self.option_positions.iter().any(|o| o.is_active)
            || self.lending_positions.iter().any(|l| l.is_active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_account() -> MarginAccount {
        MarginAccount {
            owner: Pubkey::new_unique(),
            delegate: Pubkey::default(),
            collateral: 10_000_000_000,
            locked_collateral: 2_000_000_000,
            perp_positions: [PerpPosition::default(); MAX_PERP_POSITIONS],
            perp_count: 0,
            spot_balances: [SpotBalance::default(); MAX_SPOT_BALANCES],
            spot_count: 0,
            option_positions: [OptionPosition::default(); MAX_OPTION_POSITIONS],
            option_count: 0,
            lending_positions: [LendingPosition::default(); MAX_LENDING_POSITIONS],
            lending_count: 0,
            greeks: PortfolioGreeks::default(),
            initial_margin_required: 0,
            maintenance_margin_required: 0,
            equity: 10_000_000_000,
            margin_ratio_bps: u16::MAX,
            health: AccountHealth::Healthy,
            margin_mode: MarginMode::Portfolio,
            investor_category: InvestorCategory::Retail,
            identity: Pubkey::default(),
            zk_credit_score: 0,
            zk_credit_tier: ZkCreditTier::None,
            zk_score_updated_at: 0,
            zk_credit_oracle: Pubkey::default(),
            active_intent_count: 0,
            created_at: 0,
            last_margin_update: 0,
            total_trades: 0,
            total_realized_pnl: 0,
            bump: 255,
        }
    }

    // -----------------------------------------------------------------------
    // Available collateral
    // -----------------------------------------------------------------------

    #[test]
    fn test_available_collateral() {
        let acc = make_account();
        assert_eq!(acc.available_collateral(), 8_000_000_000);
    }

    #[test]
    fn test_available_collateral_none_locked() {
        let mut acc = make_account();
        acc.locked_collateral = 0;
        assert_eq!(acc.available_collateral(), 10_000_000_000);
    }

    #[test]
    fn test_available_collateral_all_locked() {
        let mut acc = make_account();
        acc.locked_collateral = 10_000_000_000;
        assert_eq!(acc.available_collateral(), 0);
    }

    #[test]
    fn test_available_collateral_saturates() {
        let mut acc = make_account();
        acc.locked_collateral = 999_000_000_000; // more than collateral
        assert_eq!(acc.available_collateral(), 0);
    }

    // -----------------------------------------------------------------------
    // Perp slot management
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_empty_perp_slot_all_empty() {
        let acc = make_account();
        assert_eq!(acc.find_empty_perp_slot(), Some(0));
    }

    #[test]
    fn test_find_empty_perp_slot_some_filled() {
        let mut acc = make_account();
        acc.perp_positions[0].is_active = true;
        acc.perp_positions[1].is_active = true;
        assert_eq!(acc.find_empty_perp_slot(), Some(2));
    }

    #[test]
    fn test_find_empty_perp_slot_all_full() {
        let mut acc = make_account();
        for p in acc.perp_positions.iter_mut() {
            p.is_active = true;
        }
        assert_eq!(acc.find_empty_perp_slot(), None);
    }

    #[test]
    fn test_find_perp_by_market() {
        let mut acc = make_account();
        acc.perp_positions[3] = PerpPosition {
            market_index: 5,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(acc.find_perp_by_market(5), Some(3));
        assert_eq!(acc.find_perp_by_market(0), None);
        assert_eq!(acc.find_perp_by_market(6), None);
    }

    #[test]
    fn test_find_perp_by_market_ignores_inactive() {
        let mut acc = make_account();
        acc.perp_positions[0] = PerpPosition {
            market_index: 5,
            is_active: false, // inactive!
            ..Default::default()
        };
        assert_eq!(acc.find_perp_by_market(5), None);
    }

    // -----------------------------------------------------------------------
    // Spot slot management
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_empty_spot_slot() {
        let acc = make_account();
        assert_eq!(acc.find_empty_spot_slot(), Some(0));
    }

    #[test]
    fn test_find_spot_by_mint() {
        let mut acc = make_account();
        let mint = Pubkey::new_unique();
        acc.spot_balances[5] = SpotBalance {
            mint,
            balance: 100,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(acc.find_spot_by_mint(&mint), Some(5));
        assert_eq!(acc.find_spot_by_mint(&Pubkey::new_unique()), None);
    }

    #[test]
    fn test_find_spot_by_mint_ignores_inactive() {
        let mut acc = make_account();
        let mint = Pubkey::new_unique();
        acc.spot_balances[0] = SpotBalance {
            mint,
            is_active: false,
            ..Default::default()
        };
        assert_eq!(acc.find_spot_by_mint(&mint), None);
    }

    // -----------------------------------------------------------------------
    // Option slot management
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_empty_option_slot() {
        let acc = make_account();
        assert_eq!(acc.find_empty_option_slot(), Some(0));
    }

    #[test]
    fn test_find_empty_option_slot_full() {
        let mut acc = make_account();
        for o in acc.option_positions.iter_mut() {
            o.is_active = true;
        }
        assert_eq!(acc.find_empty_option_slot(), None);
    }

    // -----------------------------------------------------------------------
    // Lending slot management
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_empty_lending_slot() {
        let acc = make_account();
        assert_eq!(acc.find_empty_lending_slot(), Some(0));
    }

    #[test]
    fn test_find_lending_by_mint() {
        let mut acc = make_account();
        let mint = Pubkey::new_unique();
        acc.lending_positions[2] = LendingPosition {
            mint,
            side: LendingSide::Supply,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(acc.find_lending_by_mint(&mint, LendingSide::Supply), Some(2));
        assert_eq!(acc.find_lending_by_mint(&mint, LendingSide::Borrow), None);
        assert_eq!(acc.find_lending_by_mint(&Pubkey::new_unique(), LendingSide::Supply), None);
    }

    // -----------------------------------------------------------------------
    // has_positions
    // -----------------------------------------------------------------------

    #[test]
    fn test_has_positions_empty() {
        let acc = make_account();
        assert!(!acc.has_positions());
    }

    #[test]
    fn test_has_positions_perp() {
        let mut acc = make_account();
        acc.perp_positions[0].is_active = true;
        assert!(acc.has_positions());
    }

    #[test]
    fn test_has_positions_spot() {
        let mut acc = make_account();
        acc.spot_balances[0].is_active = true;
        assert!(acc.has_positions());
    }

    #[test]
    fn test_has_positions_option() {
        let mut acc = make_account();
        acc.option_positions[0].is_active = true;
        assert!(acc.has_positions());
    }

    #[test]
    fn test_has_positions_lending() {
        let mut acc = make_account();
        acc.lending_positions[0].is_active = true;
        assert!(acc.has_positions());
    }

    // -----------------------------------------------------------------------
    // ZK credit field defaults
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_zk_fields_default() {
        let acc = make_account();
        assert_eq!(acc.zk_credit_score, 0);
        assert_eq!(acc.zk_credit_tier, ZkCreditTier::None);
        assert_eq!(acc.zk_score_updated_at, 0);
        assert_eq!(acc.zk_credit_oracle, Pubkey::default());
        assert_eq!(acc.active_intent_count, 0);
    }
}
