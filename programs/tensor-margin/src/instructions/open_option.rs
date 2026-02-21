use anchor_lang::prelude::*;
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct OpenOptionParams {
    /// Call or Put
    pub side: OptionSide,
    /// Vanilla, Asian, BarrierKnockOut, BarrierKnockIn
    pub kind: OptionKind,
    /// Strike price (1e6 precision)
    pub strike: u64,
    /// Barrier price if applicable (0 for vanilla/asian)
    pub barrier: u64,
    /// Number of contracts (positive=buy, negative=sell/write)
    pub contracts: i64,
    /// Notional per contract
    pub notional_per_contract: u64,
    /// Expiry timestamp
    pub expiry: i64,
    /// Premium per contract (1e6 precision)
    pub premium: u64,
    /// Greeks per contract (from off-chain pricer or sigma oracle)
    pub delta_per_contract: i64,
    pub gamma_per_contract: i64,
    pub vega_per_contract: i64,
    pub theta_per_contract: i64,
}

#[derive(Accounts)]
pub struct OpenOption<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, authority.key().as_ref()],
        bump = margin_account.bump,
        constraint = (margin_account.owner == authority.key()
            || margin_account.delegate == authority.key()) @ TensorError::Unauthorized,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        seeds = [MarginMarket::SEED, &market.index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Account<'info, MarginMarket>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, MarginConfig>,

    pub authority: Signer<'info>,
}

pub fn handler(ctx: Context<OpenOption>, market_index: u16, params: OpenOptionParams) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(!config.is_paused, TensorError::ProtocolPaused);

    let market = &ctx.accounts.market;
    require!(market.is_active, TensorError::MarketNotActive);
    require!(market.options_enabled, TensorError::ProductNotEnabled);
    require!(params.contracts != 0, TensorError::InvalidAmount);
    require!(params.strike > 0, TensorError::InvalidPrice);

    let clock = Clock::get()?;
    require!(params.expiry > clock.unix_timestamp, TensorError::OptionExpired);

    let account = &mut ctx.accounts.margin_account;

    let slot_idx = account.find_empty_option_slot()
        .ok_or(TensorError::PositionSlotFull)?;

    // Calculate premium cost/credit
    let abs_contracts = params.contracts.unsigned_abs() as u128;
    let total_premium = (abs_contracts * params.premium as u128 / 1_000_000) as u64;

    if params.contracts > 0 {
        // Buying options: deduct premium from collateral
        require!(account.collateral >= total_premium, TensorError::InsufficientCollateral);
        account.collateral -= total_premium;
    } else {
        // Writing options: premium received, but margin required
        account.collateral += total_premium;
    }

    // Store option position
    let opt = &mut account.option_positions[slot_idx];
    opt.market_index = market_index;
    opt.side = params.side;
    opt.kind = params.kind;
    opt.strike = params.strike;
    opt.barrier = params.barrier;
    opt.contracts = params.contracts;
    opt.notional_per_contract = params.notional_per_contract;
    opt.expiry = params.expiry;
    opt.premium = params.premium;
    opt.delta_per_contract = params.delta_per_contract;
    opt.gamma_per_contract = params.gamma_per_contract;
    opt.vega_per_contract = params.vega_per_contract;
    opt.theta_per_contract = params.theta_per_contract;
    opt.opened_at = clock.unix_timestamp;
    opt.is_active = true;

    account.option_count += 1;

    // Recompute margin with new option position
    let mark_prices: Vec<u64> = vec![market.mark_price];
    let greeks = tensor_math::compute_portfolio_greeks(
        &account.perp_positions,
        &account.spot_balances,
        &account.option_positions,
        &mark_prices,
        clock.unix_timestamp,
    );

    let im_bps = market.effective_initial_margin(config.initial_margin_bps);
    let initial_margin = tensor_math::compute_initial_margin(
        &greeks,
        market.mark_price,
        market.implied_vol_bps,
        im_bps,
        config.gamma_margin_bps,
        config.vega_margin_bps,
    );

    let maint_ratio = market.effective_maintenance_ratio(config.maintenance_ratio_bps);
    let maint_margin = tensor_math::compute_maintenance_margin(initial_margin, maint_ratio);

    // Verify sufficient margin
    let equity = tensor_math::compute_equity(
        account.collateral,
        &account.perp_positions,
        &account.spot_balances,
        &account.option_positions,
        &account.lending_positions,
        &mark_prices,
    );
    require!(equity >= initial_margin as i64, TensorError::InsufficientMargin);

    // Cache
    account.greeks = greeks;
    account.initial_margin_required = initial_margin;
    account.maintenance_margin_required = maint_margin;
    account.equity = equity;
    account.margin_ratio_bps = tensor_math::margin_ratio_bps(equity, maint_margin);
    account.health = tensor_math::compute_health(equity, maint_margin);
    account.last_margin_update = clock.unix_timestamp;
    account.total_trades += 1;

    emit!(OptionOpened {
        owner: account.owner,
        market_index,
        side: params.side,
        kind: params.kind,
        contracts: params.contracts,
        strike: params.strike,
        premium: total_premium,
        net_delta: greeks.delta,
        net_gamma: greeks.gamma,
    });

    Ok(())
}

#[event]
pub struct OptionOpened {
    pub owner: Pubkey,
    pub market_index: u16,
    pub side: OptionSide,
    pub kind: OptionKind,
    pub contracts: i64,
    pub strike: u64,
    pub premium: u64,
    pub net_delta: i64,
    pub net_gamma: i64,
}
