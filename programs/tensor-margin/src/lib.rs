use anchor_lang::prelude::*;

pub mod errors;
pub mod instructions;
pub mod state;

use instructions::*;

declare_id!("3uztvRNHpQcS9KgbdY6NFoL9HamSZYujkH9FQWtFoP1h");

#[program]
pub mod tensor_margin {
    use super::*;

    // -----------------------------------------------------------------------
    // Admin
    // -----------------------------------------------------------------------

    /// Initialize the protocol configuration. Called once.
    pub fn initialize_config(
        ctx: Context<InitializeConfig>,
        params: InitializeConfigParams,
    ) -> Result<()> {
        instructions::initialize_config::handler(ctx, params)
    }

    /// Register a new tradeable market (asset).
    pub fn register_market(
        ctx: Context<RegisterMarket>,
        params: RegisterMarketParams,
    ) -> Result<()> {
        instructions::register_market::handler(ctx, params)
    }

    /// Keeper: update mark price, implied vol, and funding rate for a market.
    pub fn update_mark_price(
        ctx: Context<UpdateMarkPrice>,
        mark_price: u64,
        implied_vol_bps: u64,
        funding_rate_bps: i64,
    ) -> Result<()> {
        instructions::update_mark_price::handler(ctx, mark_price, implied_vol_bps, funding_rate_bps)
    }

    // -----------------------------------------------------------------------
    // Account Management
    // -----------------------------------------------------------------------

    /// Create a new unified margin account for the signer.
    pub fn create_margin_account(
        ctx: Context<CreateMarginAccount>,
        margin_mode: tensor_types::MarginMode,
        investor_category: tensor_types::InvestorCategory,
    ) -> Result<()> {
        instructions::create_margin_account::handler(ctx, margin_mode, investor_category)
    }

    /// Deposit USDC collateral into a margin account.
    pub fn deposit_collateral(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
        instructions::deposit_collateral::handler(ctx, amount)
    }

    /// Withdraw available USDC collateral (checks margin sufficiency).
    pub fn withdraw_collateral(ctx: Context<WithdrawCollateral>, amount: u64) -> Result<()> {
        instructions::withdraw_collateral::handler(ctx, amount)
    }

    // -----------------------------------------------------------------------
    // Trading — Perpetuals
    // -----------------------------------------------------------------------

    /// Open or modify a perpetual futures position.
    /// Size is signed: positive = long, negative = short.
    pub fn open_perp(ctx: Context<OpenPerp>, params: OpenPerpParams) -> Result<()> {
        instructions::open_perp::handler(ctx, params)
    }

    /// Close an entire perpetual futures position, realizing PnL.
    pub fn close_perp(ctx: Context<ClosePerp>, market_index: u16) -> Result<()> {
        instructions::close_perp::handler(ctx, market_index)
    }

    // -----------------------------------------------------------------------
    // Trading — Options
    // -----------------------------------------------------------------------

    /// Open an option position (vanilla, Asian, or barrier).
    /// Contracts signed: positive = buy, negative = write/sell.
    pub fn open_option(
        ctx: Context<OpenOption>,
        market_index: u16,
        params: OpenOptionParams,
    ) -> Result<()> {
        instructions::open_option::handler(ctx, market_index, params)
    }

    // -----------------------------------------------------------------------
    // Risk Engine
    // -----------------------------------------------------------------------

    /// Permissionless crank: recompute margin for any account.
    /// Pass MarginMarket accounts as remaining_accounts.
    pub fn compute_margin<'info>(
        ctx: Context<'_, '_, 'info, 'info, ComputeMargin<'info>>,
    ) -> Result<()> {
        instructions::compute_margin::handler(ctx)
    }

    /// Liquidate an unhealthy account (waterfall: options → perps → spot → lending).
    pub fn liquidate(ctx: Context<Liquidate>) -> Result<()> {
        instructions::liquidate::handler(ctx)
    }

    // -----------------------------------------------------------------------
    // Phase 2 — Oracle-powered instructions (CPI reads)
    // -----------------------------------------------------------------------

    /// Permissionless crank: recompute margin reading prices from Sigma oracle.
    /// remaining_accounts: groups of (MarginMarket, PriceFeed, [VarianceTracker])
    pub fn compute_margin_oracle<'info>(
        ctx: Context<'_, '_, 'info, 'info, ComputeMarginOracle<'info>>,
    ) -> Result<()> {
        instructions::compute_margin_oracle::handler(ctx)
    }

    /// Permissionless: update mark price from Sigma shared-oracle.
    pub fn update_mark_price_oracle(ctx: Context<UpdateMarkPriceOracle>) -> Result<()> {
        instructions::update_mark_price_oracle::handler(ctx)
    }

    // -----------------------------------------------------------------------
    // Phase 2 — Spot Trading (Northtail AMM)
    // -----------------------------------------------------------------------

    /// Execute a spot swap through Northtail's constant-product AMM.
    pub fn execute_spot_swap(
        ctx: Context<ExecuteSpotSwap>,
        params: SpotSwapParams,
    ) -> Result<()> {
        instructions::execute_spot_swap::handler(ctx, params)
    }

    // -----------------------------------------------------------------------
    // Phase 2 — Identity-gated Leverage
    // -----------------------------------------------------------------------

    /// Refresh investor category from Sovereign reputation tier.
    pub fn refresh_identity(ctx: Context<RefreshIdentity>) -> Result<()> {
        instructions::refresh_identity::handler(ctx)
    }

    // -----------------------------------------------------------------------
    // Phase 3 — Intent Language
    // -----------------------------------------------------------------------

    /// Submit a multi-leg trading intent.
    pub fn submit_intent(ctx: Context<SubmitIntent>, args: SubmitIntentArgs) -> Result<()> {
        instructions::submit_intent::handler(ctx, args)
    }

    /// Execute a single leg of an intent (called by keeper/solver).
    pub fn execute_intent(ctx: Context<ExecuteIntent>, leg_index: u8, exec_price: u64) -> Result<()> {
        instructions::execute_intent::handler(ctx, leg_index, exec_price)
    }

    /// Cancel a pending or partially filled intent.
    pub fn cancel_intent(ctx: Context<CancelIntent>) -> Result<()> {
        instructions::cancel_intent::handler(ctx)
    }

    // -----------------------------------------------------------------------
    // Phase 3 — ZK Credit Scores
    // -----------------------------------------------------------------------

    /// Refresh ZK credit score from oracle, updating margin discount and leverage bonus.
    pub fn refresh_zk_credit(ctx: Context<RefreshZkCredit>) -> Result<()> {
        instructions::refresh_zk_credit::handler(ctx)
    }
}
