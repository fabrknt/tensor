use anchor_lang::prelude::*;
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct OpenPerpParams {
    /// Signed size: positive = long, negative = short
    pub size: i64,
    /// Maximum acceptable entry price (for longs) or minimum (for shorts)
    pub limit_price: u64,
}

#[derive(Accounts)]
pub struct OpenPerp<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, authority.key().as_ref()],
        bump = margin_account.bump,
        constraint = (margin_account.owner == authority.key()
            || margin_account.delegate == authority.key()) @ TensorError::Unauthorized,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        mut,
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

pub fn handler(ctx: Context<OpenPerp>, params: OpenPerpParams) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(!config.is_paused, TensorError::ProtocolPaused);

    let market = &ctx.accounts.market;
    require!(market.is_active, TensorError::MarketNotActive);
    require!(market.perp_enabled, TensorError::ProductNotEnabled);
    require!(market.mark_price > 0, TensorError::InvalidPrice);
    require!(params.size != 0, TensorError::InvalidAmount);

    // Check position size limit
    let abs_size = if params.size < 0 { -params.size } else { params.size } as u64;
    if market.max_position_size > 0 {
        require!(abs_size <= market.max_position_size, TensorError::ExceedsPositionLimit);
    }

    let clock = Clock::get()?;
    let account = &mut ctx.accounts.margin_account;

    // Check if we already have a position in this market
    let slot_idx = if let Some(idx) = account.find_perp_by_market(market.index) {
        // Modify existing position
        idx
    } else {
        // New position — find empty slot
        account.find_empty_perp_slot()
            .ok_or(TensorError::PositionSlotFull)?
    };

    let is_existing = account.perp_positions[slot_idx].is_active;

    if is_existing {
        // Modifying existing position: calculate realized PnL on closed portion
        let old_size = account.perp_positions[slot_idx].size;
        let old_entry = account.perp_positions[slot_idx].entry_price;
        let new_size = old_size + params.size;

        // If flipping sides or reducing, realize PnL
        if (old_size > 0 && params.size < 0) || (old_size < 0 && params.size > 0) {
            let close_size = params.size.unsigned_abs().min(old_size.unsigned_abs()) as i128;
            let entry = old_entry as i128;
            let mark = market.mark_price as i128;
            let direction = if old_size > 0 { 1i128 } else { -1i128 };
            let realized = direction * close_size * (mark - entry) / 1_000_000;
            account.perp_positions[slot_idx].realized_pnl += realized as i64;
            account.total_realized_pnl += realized as i64;
        }

        if new_size == 0 {
            // Fully closed
            account.perp_positions[slot_idx] = PerpPosition::default();
            account.perp_count = account.perp_count.saturating_sub(1);
        } else {
            // Update entry price (weighted average for same-direction increase)
            if (old_size > 0 && params.size > 0) || (old_size < 0 && params.size < 0) {
                let old_notional = old_size.unsigned_abs() as u128 * old_entry as u128;
                let new_notional = params.size.unsigned_abs() as u128 * market.mark_price as u128;
                let total_size = new_size.unsigned_abs() as u128;
                account.perp_positions[slot_idx].entry_price =
                    ((old_notional + new_notional) / total_size) as u64;
            }
            account.perp_positions[slot_idx].size = new_size;
        }
    } else {
        // Brand new position
        account.perp_positions[slot_idx].market_index = market.index;
        account.perp_positions[slot_idx].size = params.size;
        account.perp_positions[slot_idx].entry_price = market.mark_price;
        account.perp_positions[slot_idx].realized_pnl = 0;
        account.perp_positions[slot_idx].unrealized_pnl = 0;
        account.perp_positions[slot_idx].cumulative_funding = 0;
        account.perp_positions[slot_idx].last_funding_index = market.cumulative_funding_index as i64;
        account.perp_positions[slot_idx].opened_at = clock.unix_timestamp;
        account.perp_positions[slot_idx].is_active = true;
        account.perp_count += 1;
    }

    // Update market open interest
    let market = &mut ctx.accounts.market;
    if params.size > 0 {
        market.open_interest_long = market.open_interest_long
            .saturating_add(params.size.unsigned_abs());
    } else {
        market.open_interest_short = market.open_interest_short
            .saturating_add(params.size.unsigned_abs());
    }

    // Increment trade count
    account.total_trades += 1;

    // Re-compute margin requirements inline (fast path)
    let mark_prices: Vec<u64> = vec![market.mark_price]; // simplified: single market
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

    let maint_margin = tensor_math::compute_maintenance_margin(
        initial_margin,
        market.effective_maintenance_ratio(config.maintenance_ratio_bps),
    );

    // Check leverage limit (credit-adjusted)
    let base_leverage = account.investor_category.max_leverage_bps();
    let bonus = account.zk_credit_tier.leverage_bonus_bps();
    let max_leverage = tensor_math::effective_max_leverage_bps(base_leverage, bonus);
    if greeks.total_notional > 0 && account.collateral > 0 {
        let leverage_bps = (greeks.total_notional as u128 * 10_000) / account.collateral as u128;
        require!(leverage_bps <= max_leverage as u128, TensorError::ExceedsLeverageLimit);
    }

    // Verify sufficient margin
    require!(
        account.equity >= initial_margin as i64,
        TensorError::InsufficientMargin
    );

    // Cache computed values
    account.greeks = greeks;
    account.initial_margin_required = initial_margin;
    account.maintenance_margin_required = maint_margin;
    account.margin_ratio_bps = tensor_math::margin_ratio_bps(account.equity, maint_margin);
    account.health = tensor_math::compute_health(account.equity, maint_margin);
    account.last_margin_update = clock.unix_timestamp;

    emit!(PerpOpened {
        owner: account.owner,
        market_index: market.index,
        size: params.size,
        entry_price: market.mark_price,
        net_delta: greeks.delta,
        initial_margin,
    });

    Ok(())
}

#[event]
pub struct PerpOpened {
    pub owner: Pubkey,
    pub market_index: u16,
    pub size: i64,
    pub entry_price: u64,
    pub net_delta: i64,
    pub initial_margin: u64,
}
