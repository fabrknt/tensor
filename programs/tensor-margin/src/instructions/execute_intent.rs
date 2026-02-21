use anchor_lang::prelude::*;
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct ExecuteIntent<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, margin_account.owner.as_ref()],
        bump = margin_account.bump,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        mut,
        seeds = [
            IntentAccount::SEED,
            margin_account.key().as_ref(),
            &intent_account.intent_id.to_le_bytes(),
        ],
        bump = intent_account.bump,
        constraint = intent_account.margin_account == margin_account.key() @ TensorError::Unauthorized,
    )]
    pub intent_account: Account<'info, IntentAccount>,

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

    /// Keeper/solver authority that executes the intent leg
    pub authority: Signer<'info>,
}

pub fn handler(ctx: Context<ExecuteIntent>, leg_index: u8, exec_price: u64) -> Result<()> {
    let clock = Clock::get()?;
    let config = &ctx.accounts.config;
    let intent = &mut ctx.accounts.intent_account;

    // Validate intent state
    require!(
        intent.status == IntentStatus::Pending || intent.status == IntentStatus::PartiallyFilled,
        TensorError::IntentAlreadyResolved
    );
    require!(!intent.is_expired(clock.unix_timestamp), TensorError::IntentExpired);
    require!((leg_index as usize) < intent.leg_count as usize, TensorError::InvalidIntentState);

    let leg = intent.legs[leg_index as usize];
    require!(leg.is_active, TensorError::InvalidIntentState);

    // Slippage check
    if leg.limit_price > 0 && intent.max_slippage_bps > 0 {
        let slippage_bps = if leg.size > 0 {
            // Buying: exec_price should be <= limit_price + slippage
            if exec_price > leg.limit_price {
                ((exec_price - leg.limit_price) as u128 * 10_000 / leg.limit_price as u128) as u16
            } else {
                0
            }
        } else {
            // Selling: exec_price should be >= limit_price - slippage
            if exec_price < leg.limit_price {
                ((leg.limit_price - exec_price) as u128 * 10_000 / leg.limit_price as u128) as u16
            } else {
                0
            }
        };
        require!(slippage_bps <= intent.max_slippage_bps, TensorError::SlippageExceeded);
    }

    let account = &mut ctx.accounts.margin_account;
    let market = &ctx.accounts.market;

    // Execute the leg based on product type
    match leg.product_type {
        ProductType::Perpetual => {
            require!(market.is_active, TensorError::MarketNotActive);
            require!(market.perp_enabled, TensorError::ProductNotEnabled);
            require!(market.index == leg.market_index, TensorError::InvalidIntentState);

            let slot_idx = if let Some(idx) = account.find_perp_by_market(leg.market_index) {
                idx
            } else {
                account
                    .find_empty_perp_slot()
                    .ok_or(TensorError::PositionSlotFull)?
            };

            let is_existing = account.perp_positions[slot_idx].is_active;

            if is_existing {
                let old_size = account.perp_positions[slot_idx].size;
                let old_entry = account.perp_positions[slot_idx].entry_price;
                let new_size = old_size + leg.size;

                // Realize PnL if reducing
                if (old_size > 0 && leg.size < 0) || (old_size < 0 && leg.size > 0) {
                    let close_size = leg.size.unsigned_abs().min(old_size.unsigned_abs()) as i128;
                    let entry = old_entry as i128;
                    let mark = exec_price as i128;
                    let direction = if old_size > 0 { 1i128 } else { -1i128 };
                    let realized = direction * close_size * (mark - entry) / 1_000_000;
                    account.perp_positions[slot_idx].realized_pnl += realized as i64;
                    account.total_realized_pnl += realized as i64;
                }

                if new_size == 0 {
                    account.perp_positions[slot_idx] = PerpPosition::default();
                    account.perp_count = account.perp_count.saturating_sub(1);
                } else {
                    if (old_size > 0 && leg.size > 0) || (old_size < 0 && leg.size < 0) {
                        let old_notional = old_size.unsigned_abs() as u128 * old_entry as u128;
                        let new_notional = leg.size.unsigned_abs() as u128 * exec_price as u128;
                        let total_size = new_size.unsigned_abs() as u128;
                        account.perp_positions[slot_idx].entry_price =
                            ((old_notional + new_notional) / total_size) as u64;
                    }
                    account.perp_positions[slot_idx].size = new_size;
                }
            } else {
                account.perp_positions[slot_idx].market_index = leg.market_index;
                account.perp_positions[slot_idx].size = leg.size;
                account.perp_positions[slot_idx].entry_price = exec_price;
                account.perp_positions[slot_idx].realized_pnl = 0;
                account.perp_positions[slot_idx].unrealized_pnl = 0;
                account.perp_positions[slot_idx].cumulative_funding = 0;
                account.perp_positions[slot_idx].last_funding_index =
                    market.cumulative_funding_index as i64;
                account.perp_positions[slot_idx].opened_at = clock.unix_timestamp;
                account.perp_positions[slot_idx].is_active = true;
                account.perp_count += 1;
            }
        }
        // Spot, Option, Lending legs handled similarly but simplified for Phase 3 MVP
        _ => {
            // For non-perp legs, mark as filled without opening positions
            // (full execution logic for spot/options would mirror execute_spot_swap / open_option)
        }
    }

    // Mark leg as filled
    intent.legs[leg_index as usize].is_active = false;
    intent.filled_legs += 1;
    intent.updated_at = clock.unix_timestamp;

    // Update intent status
    if intent.filled_legs >= intent.leg_count {
        intent.status = IntentStatus::Filled;
        // Decrement active count when fully filled
        account.active_intent_count = account.active_intent_count.saturating_sub(1);
    } else {
        intent.status = IntentStatus::PartiallyFilled;
    }

    // Re-compute margin inline
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

    // Apply credit discount
    let maint_ratio = market.effective_maintenance_ratio(config.maintenance_ratio_bps);
    let maint_margin = tensor_math::compute_maintenance_margin(initial_margin, maint_ratio);
    let discount_bps = account.zk_credit_tier.margin_discount_bps();
    let adjusted_initial = tensor_math::apply_credit_discount(
        initial_margin,
        discount_bps,
        maint_margin,
    );

    // Check margin health
    require!(
        account.equity >= adjusted_initial as i64,
        TensorError::InsufficientMargin
    );

    // Cache values
    account.greeks = greeks;
    account.initial_margin_required = adjusted_initial;
    account.maintenance_margin_required = maint_margin;
    account.margin_ratio_bps = tensor_math::margin_ratio_bps(account.equity, maint_margin);
    account.health = tensor_math::compute_health(account.equity, maint_margin);
    account.last_margin_update = clock.unix_timestamp;
    account.total_trades += 1;

    // Track margin used
    intent.total_margin_used = adjusted_initial;

    // Check max cost constraint
    if intent.max_total_cost > 0 {
        require!(
            intent.total_margin_used <= intent.max_total_cost,
            TensorError::MaxCostExceeded
        );
    }

    emit!(IntentLegExecuted {
        owner: account.owner,
        intent_id: intent.intent_id,
        leg_index,
        exec_price,
        filled_legs: intent.filled_legs,
        total_legs: intent.leg_count,
    });

    Ok(())
}

#[event]
pub struct IntentLegExecuted {
    pub owner: Pubkey,
    pub intent_id: u64,
    pub leg_index: u8,
    pub exec_price: u64,
    pub filled_legs: u8,
    pub total_legs: u8,
}
