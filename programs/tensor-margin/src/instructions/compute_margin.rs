use anchor_lang::prelude::*;
use crate::state::*;

/// Keeper instruction: recompute margin for an account using current market data.
///
/// This is the heart of the unified margin engine. It:
/// 1. Reads all positions across all product types
/// 2. Computes aggregate portfolio Greeks (delta-netting)
/// 3. Calculates initial and maintenance margin requirements
/// 4. Determines account health
///
/// Can be called by anyone (permissionless crank) to keep accounts up-to-date.
#[derive(Accounts)]
pub struct ComputeMargin<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, margin_account.owner.as_ref()],
        bump = margin_account.bump,
    )]
    pub margin_account: Box<Account<'info, MarginAccount>>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, MarginConfig>>,

    // Remaining accounts: MarginMarket accounts for each active position's market_index
    // Passed as remaining_accounts to support variable number of markets
}

pub fn handler<'info>(ctx: Context<'_, '_, 'info, 'info, ComputeMargin<'info>>) -> Result<()> {
    let clock = Clock::get()?;
    let config = &ctx.accounts.config;
    let account = &mut ctx.accounts.margin_account;

    // Collect mark prices and vol surface from remaining accounts (MarginMarket PDAs)
    let mut mark_prices = vec![0u64; 256]; // indexed by market_index
    let mut primary_mark_price = 0u64;
    let mut primary_implied_vol = 0u64;
    let primary_realized_var: u64 = 0; // for dynamic gamma margin

    // Vol surface from primary market (used for per-position IV)
    let mut vol_surface = [[0u64; 9]; 4];
    let mut vol_moneyness_nodes = [0u64; 9];
    let mut vol_expiry_days = [0u16; 4];
    let mut vol_node_count: usize = 0;
    let mut vol_expiry_count: usize = 0;

    for market_ai in ctx.remaining_accounts.iter() {
        if let Ok(market_data) = Account::<MarginMarket>::try_from(market_ai) {
            let idx = market_data.index as usize;
            if idx < mark_prices.len() {
                mark_prices[idx] = market_data.mark_price;
                if primary_mark_price == 0 {
                    primary_mark_price = market_data.mark_price;
                    primary_implied_vol = market_data.implied_vol_bps;
                    // Capture vol surface from primary market
                    vol_surface = market_data.vol_surface;
                    vol_moneyness_nodes = market_data.vol_moneyness_nodes;
                    vol_expiry_days = market_data.vol_expiry_days;
                    vol_node_count = market_data.vol_node_count as usize;
                    vol_expiry_count = market_data.vol_expiry_count as usize;
                }
            }
        }
    }

    // Apply funding to perp positions
    for perp in account.perp_positions.iter_mut().filter(|p| p.is_active) {
        let idx = perp.market_index as usize;
        if idx < mark_prices.len() {
            let price = mark_prices[idx];
            if price > 0 {
                perp.unrealized_pnl = perp.mark_pnl(price);
            }
        }
    }

    // Accrue lending interest
    for lending in account.lending_positions.iter_mut().filter(|l| l.is_active) {
        let elapsed = clock.unix_timestamp - lending.last_accrual;
        if elapsed > 0 {
            let interest = tensor_math::accrue_interest(
                lending.principal,
                lending.rate_bps,
                elapsed,
            );
            match lending.side {
                tensor_types::LendingSide::Supply => {
                    lending.accrued_interest += interest as i64;
                }
                tensor_types::LendingSide::Borrow => {
                    lending.accrued_interest -= interest as i64;
                }
            }
            lending.last_accrual = clock.unix_timestamp;
        }
    }

    // Compute portfolio Greeks
    let greeks = tensor_math::compute_portfolio_greeks(
        &account.perp_positions[..],
        &account.spot_balances[..],
        &account.option_positions[..],
        &mark_prices,
        clock.unix_timestamp,
    );

    // Compute equity
    let equity = tensor_math::compute_equity(
        account.collateral,
        &account.perp_positions[..],
        &account.spot_balances[..],
        &account.option_positions[..],
        &account.lending_positions[..],
        &mark_prices,
    );

    // Dynamic gamma margin: scale gamma_margin_bps by realized vol / baseline vol
    // Baseline: implied_vol_bps as the "normal" level. If realized > implied, tighten margin.
    let effective_gamma_bps = tensor_math::dynamic_gamma_margin_bps(
        config.gamma_margin_bps,
        primary_realized_var,
        primary_implied_vol,
    );

    // Compute base margin (delta + gamma charges)
    let base_margin = tensor_math::compute_initial_margin(
        &greeks,
        primary_mark_price,
        primary_implied_vol,
        config.initial_margin_bps,
        effective_gamma_bps,
        0, // vega computed separately via surface
    );

    // Compute vega charge using vol surface (or flat fallback)
    let vega_charge = if vol_node_count > 0 {
        let per_pos_vols = tensor_math::compute_per_position_vols(
            &account.option_positions[..],
            &vol_moneyness_nodes,
            &vol_expiry_days,
            &vol_surface,
            vol_node_count,
            vol_expiry_count,
            primary_mark_price,
            clock.unix_timestamp,
            primary_implied_vol,
        );
        tensor_math::compute_vega_charge_surface(
            &account.option_positions[..],
            &per_pos_vols,
            config.vega_margin_bps,
            clock.unix_timestamp,
        )
    } else {
        // Flat IV fallback: compute via standard formula
        let abs_vega = if greeks.vega < 0 { -greeks.vega } else { greeks.vega } as u128;
        (abs_vega * primary_implied_vol as u128 * config.vega_margin_bps as u128
            / (10_000u128 * 10_000u128)) as u64
    };

    let initial_margin = base_margin.saturating_add(vega_charge);

    let maint_margin = tensor_math::compute_maintenance_margin(
        initial_margin,
        config.maintenance_ratio_bps,
    );

    // Apply ZK credit discount to initial margin
    let discount_bps = account.zk_credit_tier.margin_discount_bps();
    let initial_margin = tensor_math::apply_credit_discount(
        initial_margin, discount_bps, maint_margin
    );

    // Update account
    account.greeks = greeks;
    account.initial_margin_required = initial_margin;
    account.maintenance_margin_required = maint_margin;
    account.equity = equity;
    account.margin_ratio_bps = tensor_math::margin_ratio_bps(equity, maint_margin);
    account.health = tensor_math::compute_health(equity, maint_margin);
    account.last_margin_update = clock.unix_timestamp;

    emit!(MarginComputed {
        owner: account.owner,
        equity,
        initial_margin,
        maintenance_margin: maint_margin,
        health: account.health,
        net_delta: greeks.delta,
        net_gamma: greeks.gamma,
    });

    Ok(())
}

#[event]
pub struct MarginComputed {
    pub owner: Pubkey,
    pub equity: i64,
    pub initial_margin: u64,
    pub maintenance_margin: u64,
    pub health: tensor_types::AccountHealth,
    pub net_delta: i64,
    pub net_gamma: i64,
}
