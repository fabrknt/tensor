use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

/// Enhanced margin computation that reads prices directly from Sigma
/// shared-oracle PriceFeed and VarianceTracker accounts.
///
/// This replaces the keeper-dependent compute_margin for setups where
/// the oracle infrastructure is live. Prices are read trustlessly from
/// on-chain oracle accounts rather than relying on pushed mark prices.
///
/// remaining_accounts layout (pairs):
///   For each market with active positions:
///     [MarginMarket, PriceFeed, VarianceTracker (optional)]
///   Accounts can be passed in any order; they're matched by market_index.
#[derive(Accounts)]
pub struct ComputeMarginOracle<'info> {
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

    // Remaining accounts: triplets of (MarginMarket, PriceFeed, VarianceTracker)
}

pub fn handler<'info>(
    ctx: Context<'_, '_, 'info, 'info, ComputeMarginOracle<'info>>,
) -> Result<()> {
    let clock = Clock::get()?;
    let config = &ctx.accounts.config;
    let account = &mut ctx.accounts.margin_account;

    let remaining = ctx.remaining_accounts;
    // Process remaining accounts in groups of 3: (MarginMarket, PriceFeed, VarianceTracker)
    // VarianceTracker is optional — if account data is empty/wrong, we skip it
    require!(remaining.len() >= 2, TensorError::InvalidAmount);

    let mut mark_prices = vec![0u64; 256];
    let mut primary_mark_price = 0u64;
    let mut primary_implied_vol = 0u64;

    let mut i = 0;
    while i + 1 < remaining.len() {
        // MarginMarket account
        let market_ai = &remaining[i];
        let market_data = Account::<MarginMarket>::try_from(market_ai)
            .map_err(|_| TensorError::MarketNotActive)?;
        let market_idx = market_data.index as usize;

        // PriceFeed account (sigma shared-oracle)
        let price_feed_ai = &remaining[i + 1];
        let oracle_data = tensor_cpi::sigma::read_price_feed(price_feed_ai)
            .map_err(|_| TensorError::OracleStale)?;

        require!(oracle_data.is_active, TensorError::OracleStale);
        require!(
            !is_oracle_stale(oracle_data.last_sample_time, clock.unix_timestamp),
            TensorError::OracleStale
        );
        require!(oracle_data.last_price > 0, TensorError::InvalidPrice);

        // Use TWAP as mark price (more manipulation-resistant than spot)
        let oracle_price = if oracle_data.twap > 0 {
            oracle_data.twap
        } else {
            oracle_data.last_price
        };

        if market_idx < mark_prices.len() {
            mark_prices[market_idx] = oracle_price;
        }

        // Implied vol from variance (sqrt of annualized variance in bps)
        let implied_vol_bps = if oracle_data.current_variance > 0 {
            // variance is in bps annualized; vol = sqrt(variance)
            // We approximate: sqrt(variance_bps) scaled to bps
            integer_sqrt(oracle_data.current_variance as u128) as u64
        } else {
            0
        };

        // Optional: VarianceTracker (more precise epoch variance)
        let mut epoch_variance = implied_vol_bps;
        if i + 2 < remaining.len() {
            let variance_ai = &remaining[i + 2];
            if let Ok(var_data) = tensor_cpi::sigma::read_variance_tracker(variance_ai) {
                if var_data.current_epoch_variance > 0 {
                    epoch_variance = integer_sqrt(var_data.current_epoch_variance as u128) as u64;
                }
                i += 3; // consumed triplet
            } else {
                i += 2; // only consumed pair
            }
        } else {
            i += 2;
        }

        if primary_mark_price == 0 {
            primary_mark_price = oracle_price;
            primary_implied_vol = epoch_variance;
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

    // Update account
    account.greeks = greeks;
    account.initial_margin_required = initial_margin;
    account.maintenance_margin_required = maint_margin;
    account.equity = equity;
    account.margin_ratio_bps = tensor_math::margin_ratio_bps(equity, maint_margin);
    account.health = tensor_math::compute_health(equity, maint_margin);
    account.last_margin_update = clock.unix_timestamp;

    emit!(OracleMarginComputed {
        owner: account.owner,
        equity,
        initial_margin,
        maintenance_margin: maint_margin,
        health: account.health,
        net_delta: greeks.delta,
        primary_mark_price,
        primary_implied_vol,
    });

    Ok(())
}

/// Check if oracle data is stale (> 5 minutes old)
fn is_oracle_stale(last_sample_time: i64, current_time: i64) -> bool {
    current_time - last_sample_time > 300
}

/// Integer square root via Newton's method
fn integer_sqrt(n: u128) -> u128 {
    if n == 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

#[event]
pub struct OracleMarginComputed {
    pub owner: Pubkey,
    pub equity: i64,
    pub initial_margin: u64,
    pub maintenance_margin: u64,
    pub health: tensor_types::AccountHealth,
    pub net_delta: i64,
    pub primary_mark_price: u64,
    pub primary_implied_vol: u64,
}
