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
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, MarginConfig>,

    // Remaining accounts: MarginMarket accounts for each active position's market_index
    // Passed as remaining_accounts to support variable number of markets
}

pub fn handler<'info>(ctx: Context<'_, '_, 'info, 'info, ComputeMargin<'info>>) -> Result<()> {
    let clock = Clock::get()?;
    let config = &ctx.accounts.config;
    let account = &mut ctx.accounts.margin_account;

    // Collect mark prices from remaining accounts (MarginMarket PDAs)
    let mut mark_prices = vec![0u64; 256]; // indexed by market_index
    let mut primary_mark_price = 0u64;
    let mut primary_implied_vol = 0u64;

    for market_ai in ctx.remaining_accounts.iter() {
        if let Ok(market_data) = Account::<MarginMarket>::try_from(market_ai) {
            let idx = market_data.index as usize;
            if idx < mark_prices.len() {
                mark_prices[idx] = market_data.mark_price;
                if primary_mark_price == 0 {
                    primary_mark_price = market_data.mark_price;
                    primary_implied_vol = market_data.implied_vol_bps;
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
        &account.perp_positions,
        &account.spot_balances,
        &account.option_positions,
        &mark_prices,
        clock.unix_timestamp,
    );

    // Compute equity
    let equity = tensor_math::compute_equity(
        account.collateral,
        &account.perp_positions,
        &account.spot_balances,
        &account.option_positions,
        &account.lending_positions,
        &mark_prices,
    );

    // Compute margin requirements
    let initial_margin = tensor_math::compute_initial_margin(
        &greeks,
        primary_mark_price,
        primary_implied_vol,
        config.initial_margin_bps,
        config.gamma_margin_bps,
        config.vega_margin_bps,
    );

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
