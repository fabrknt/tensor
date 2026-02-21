use anchor_lang::prelude::*;
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

/// Liquidation instruction. Called when an account's health is Liquidatable.
///
/// Follows the waterfall priority:
/// 1. Close near-expiry options (lowest time value)
/// 2. Reduce perp positions (most liquid)
/// 3. Close remaining options
/// 4. Sell spot balances
/// 5. Seize lending collateral
///
/// The liquidator receives a fee for performing the liquidation.
#[derive(Accounts)]
pub struct Liquidate<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, margin_account.owner.as_ref()],
        bump = margin_account.bump,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        mut,
        seeds = [MarginMarket::SEED, &market.index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Account<'info, MarginMarket>,

    #[account(
        mut,
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, MarginConfig>,

    #[account(mut)]
    pub liquidator: Signer<'info>,
}

pub fn handler(ctx: Context<Liquidate>) -> Result<()> {
    let liquidation_fee_bps = ctx.accounts.config.liquidation_fee_bps;
    let market_index = ctx.accounts.market.index;
    let mark_price = ctx.accounts.market.mark_price;
    let account = &mut ctx.accounts.margin_account;

    // Verify account is liquidatable
    require!(
        matches!(account.health, AccountHealth::Liquidatable | AccountHealth::Bankrupt),
        TensorError::AccountHealthy
    );

    let clock = Clock::get()?;

    // Determine what to liquidate
    let priority = tensor_math::liquidation_priority(
        &account.perp_positions,
        &account.spot_balances,
        &account.option_positions,
        &account.lending_positions,
        clock.unix_timestamp,
    );

    let mut liquidated_notional: u64 = 0;

    match priority {
        Some(ProductType::Perpetual) => {
            // Find the largest perp position in this market
            if let Some(idx) = account.perp_positions
                .iter()
                .enumerate()
                .filter(|(_, p)| p.is_active && p.market_index == market_index)
                .max_by_key(|(_, p)| p.size.unsigned_abs())
                .map(|(i, _)| i)
            {
                // Copy values before mutating
                let perp_size = account.perp_positions[idx].size;
                let perp_entry = account.perp_positions[idx].entry_price;

                let size = perp_size as i128;
                let entry = perp_entry as i128;
                let mark = mark_price as i128;
                let pnl = size * (mark - entry) / 1_000_000;

                let abs_size = perp_size.unsigned_abs() as u128;
                liquidated_notional = (abs_size * mark_price as u128 / 1_000_000) as u64;

                // Apply PnL to collateral
                if pnl > 0 {
                    account.collateral = account.collateral.saturating_add(pnl as u64);
                } else {
                    account.collateral = account.collateral.saturating_sub((-pnl) as u64);
                }

                // Clear position
                account.perp_positions[idx] = PerpPosition::default();
                account.perp_count = account.perp_count.saturating_sub(1);

                // Update OI
                let market = &mut ctx.accounts.market;
                if perp_size > 0 {
                    market.open_interest_long = market.open_interest_long
                        .saturating_sub(perp_size.unsigned_abs());
                } else {
                    market.open_interest_short = market.open_interest_short
                        .saturating_sub(perp_size.unsigned_abs());
                }
            }
        }
        Some(ProductType::Option) => {
            // Close the nearest-expiry option
            if let Some(idx) = account.option_positions
                .iter()
                .enumerate()
                .filter(|(_, o)| o.is_active)
                .min_by_key(|(_, o)| o.expiry)
                .map(|(i, _)| i)
            {
                liquidated_notional = account.option_positions[idx].notional();
                account.option_positions[idx] = OptionPosition::default();
                account.option_count = account.option_count.saturating_sub(1);
            }
        }
        Some(ProductType::Spot) => {
            // Sell the largest spot balance
            if let Some(idx) = account.spot_balances
                .iter()
                .enumerate()
                .filter(|(_, s)| s.is_active)
                .max_by_key(|(_, s)| s.value)
                .map(|(i, _)| i)
            {
                let spot_value = account.spot_balances[idx].value;
                liquidated_notional = spot_value;
                account.collateral = account.collateral.saturating_add(spot_value);
                account.spot_balances[idx] = SpotBalance::default();
                account.spot_count = account.spot_count.saturating_sub(1);
            }
        }
        Some(ProductType::Lending) => {
            if let Some(idx) = account.lending_positions
                .iter()
                .enumerate()
                .filter(|(_, l)| l.is_active && l.side == LendingSide::Supply)
                .max_by_key(|(_, l)| l.effective_value)
                .map(|(i, _)| i)
            {
                let eff_val = account.lending_positions[idx].effective_value;
                liquidated_notional = eff_val;
                account.collateral = account.collateral.saturating_add(eff_val);
                account.lending_positions[idx] = LendingPosition::default();
                account.lending_count = account.lending_count.saturating_sub(1);
            }
        }
        _ => {}
    }

    // Calculate and distribute liquidation fee
    let fee = tensor_math::liquidation_fee(liquidated_notional, liquidation_fee_bps);
    let insurance_portion = fee / 2;
    let liquidator_portion = fee - insurance_portion;

    account.collateral = account.collateral.saturating_sub(fee);
    account.equity = account.collateral as i64;
    account.last_margin_update = clock.unix_timestamp;

    let config = &mut ctx.accounts.config;
    config.insurance_fund = config.insurance_fund.saturating_add(insurance_portion);

    emit!(AccountLiquidated {
        owner: ctx.accounts.margin_account.owner,
        liquidator: ctx.accounts.liquidator.key(),
        product: priority.unwrap_or(ProductType::Spot),
        liquidated_notional,
        fee,
        liquidator_reward: liquidator_portion,
    });

    Ok(())
}

#[event]
pub struct AccountLiquidated {
    pub owner: Pubkey,
    pub liquidator: Pubkey,
    pub product: ProductType,
    pub liquidated_notional: u64,
    pub fee: u64,
    pub liquidator_reward: u64,
}
