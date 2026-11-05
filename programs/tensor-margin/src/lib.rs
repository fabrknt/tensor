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
    /// With `spot_enabled`, remaining_accounts = [base_mint, collateral_mint],
    /// which must have equal decimals.
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
    /// The spot step is `liquidate_spot`; this instruction fails with
    /// SpotLiquidationRequired while some spot balance can still be sold.
    /// remaining_accounts (spot step only): [MarginMarket, northtail Market,
    /// northtail Pool] per active spot slot.
    pub fn liquidate<'info>(ctx: Context<'_, '_, 'info, 'info, Liquidate<'info>>) -> Result<()> {
        instructions::liquidate::handler(ctx)
    }

    /// Spot step of the liquidation waterfall: sell an unhealthy account's
    /// spot balance through Northtail with an on-chain minimum output.
    /// Permissionless; runs once options and perps are gone.
    pub fn liquidate_spot(
        ctx: Context<LiquidateSpot>,
        params: LiquidateSpotParams,
    ) -> Result<()> {
        instructions::liquidate_spot::handler(ctx, params)
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

    // -----------------------------------------------------------------------
    // Phase 4 — Solver Decentralization
    // -----------------------------------------------------------------------

    /// Register as a solver with stake deposit.
    pub fn register_solver(ctx: Context<RegisterSolver>, stake: u64) -> Result<()> {
        instructions::register_solver::handler(ctx, stake)
    }

    /// Deregister a solver and return their staked tokens.
    pub fn deregister_solver(ctx: Context<DeregisterSolver>) -> Result<()> {
        instructions::deregister_solver::handler(ctx)
    }

    /// Permissionless: slash a solver that won an auction but failed to fill before deadline.
    pub fn slash_solver(ctx: Context<SlashSolver>) -> Result<()> {
        instructions::slash_solver::handler(ctx)
    }

    /// Submit a bid on an intent during the auction window.
    pub fn submit_bid(ctx: Context<SubmitBid>, bid_price: u64) -> Result<()> {
        instructions::submit_bid::handler(ctx, bid_price)
    }

    /// Permissionless crank: settle the solver auction, selecting the best bid.
    pub fn settle_auction(ctx: Context<SettleAuction>) -> Result<()> {
        instructions::settle_auction::handler(ctx)
    }

    // -----------------------------------------------------------------------
    // Phase 4 — Volatility Surface
    // -----------------------------------------------------------------------

    /// Update the volatility surface for a market (admin/keeper only).
    pub fn update_vol_surface(
        ctx: Context<UpdateVolSurface>,
        params: UpdateVolSurfaceParams,
    ) -> Result<()> {
        instructions::update_vol_surface::handler(ctx, params)
    }
}
