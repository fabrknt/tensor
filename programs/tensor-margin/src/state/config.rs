use anchor_lang::prelude::*;
use tensor_types::MarginMode;

/// Global protocol configuration. One per deployment.
#[account]
#[derive(InitSpace)]
pub struct MarginConfig {
    /// Protocol admin (can update config, pause, add markets)
    pub authority: Pubkey,

    /// Fee collector address
    pub fee_collector: Pubkey,

    /// Collateral mint (primary quote currency, typically USDC)
    pub collateral_mint: Pubkey,

    /// Default initial margin in bps (e.g., 1000 = 10%)
    pub initial_margin_bps: u64,

    /// Default maintenance margin ratio (% of initial, e.g., 5000 = 50%)
    pub maintenance_ratio_bps: u64,

    /// Gamma margin charge in bps (e.g., 100 = 1%)
    pub gamma_margin_bps: u64,

    /// Vega margin charge in bps (e.g., 50 = 0.5%)
    pub vega_margin_bps: u64,

    /// Liquidation fee in bps (e.g., 50 = 0.5%)
    pub liquidation_fee_bps: u64,

    /// Trading fee in bps
    pub trading_fee_bps: u64,

    /// Maximum allowed margin mode
    pub max_margin_mode: MarginMode,

    /// KYC registry program (accredit)
    pub kyc_registry: Pubkey,

    /// Sovereign identity program
    pub identity_program: Pubkey,

    /// Insurance fund balance
    pub insurance_fund: u64,

    /// Total number of margin accounts
    pub total_accounts: u64,

    /// Total number of registered markets
    pub total_markets: u16,

    /// Protocol paused
    pub is_paused: bool,

    /// Max absolute gamma notional per account (1e6 scaled, 0 = unlimited)
    pub max_account_gamma_notional: u64,

    /// Max absolute gamma notional per market (1e6 scaled, 0 = unlimited)
    pub max_market_gamma_notional: u64,

    /// Solver auction duration in seconds (0 = no auction required)
    pub auction_duration_seconds: i64,

    /// Minimum stake required to register as a solver (in collateral units)
    pub min_solver_stake: u64,

    /// Slash rate in bps for failed solver execution
    pub solver_slash_rate_bps: u16,

    /// PDA bump
    pub bump: u8,
}

impl MarginConfig {
    pub const SEED: &'static [u8] = b"margin_config";
    pub const VAULT_AUTHORITY_SEED: &'static [u8] = b"vault_authority";

    /// The collateral vault: the associated token account of the
    /// vault_authority PDA for the collateral mint.
    pub fn collateral_vault(&self) -> Pubkey {
        let (authority, _) = Pubkey::find_program_address(&[Self::VAULT_AUTHORITY_SEED], &crate::ID);
        anchor_spl::associated_token::get_associated_token_address(&authority, &self.collateral_mint)
    }
}
