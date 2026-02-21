use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct ClosePerp<'info> {
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

pub fn handler(ctx: Context<ClosePerp>, market_index: u16) -> Result<()> {
    let mark_price = ctx.accounts.market.mark_price;
    require!(mark_price > 0, TensorError::InvalidPrice);

    let clock = Clock::get()?;
    let account = &mut ctx.accounts.margin_account;

    let slot_idx = account.find_perp_by_market(market_index)
        .ok_or(TensorError::PositionNotFound)?;

    // Copy values we need before mutating
    let perp_size = account.perp_positions[slot_idx].size;
    let perp_entry = account.perp_positions[slot_idx].entry_price;
    let perp_realized = account.perp_positions[slot_idx].realized_pnl;
    let perp_funding = account.perp_positions[slot_idx].cumulative_funding;

    // Calculate final realized PnL
    let size = perp_size as i128;
    let entry = perp_entry as i128;
    let mark = mark_price as i128;
    let final_pnl = size * (mark - entry) / 1_000_000;
    let total_realized = perp_realized as i128 + final_pnl + perp_funding as i128;

    // Update account realized PnL
    account.total_realized_pnl += final_pnl as i64;

    // Apply realized PnL to collateral
    if total_realized > 0 {
        account.collateral = account.collateral.saturating_add(total_realized as u64);
    } else {
        account.collateral = account.collateral.saturating_sub((-total_realized) as u64);
    }

    // Clear position
    account.perp_positions[slot_idx] = tensor_types::PerpPosition::default();
    account.perp_count = account.perp_count.saturating_sub(1);

    // Update market open interest
    let abs_size = perp_size.unsigned_abs();
    let market = &mut ctx.accounts.market;
    if perp_size > 0 {
        market.open_interest_long = market.open_interest_long.saturating_sub(abs_size);
    } else {
        market.open_interest_short = market.open_interest_short.saturating_sub(abs_size);
    }

    // Recompute equity
    let account = &mut ctx.accounts.margin_account;
    account.equity = account.collateral as i64;
    account.last_margin_update = clock.unix_timestamp;

    emit!(PerpClosed {
        owner: account.owner,
        market_index,
        size: perp_size,
        realized_pnl: final_pnl as i64,
    });

    Ok(())
}

#[event]
pub struct PerpClosed {
    pub owner: Pubkey,
    pub market_index: u16,
    pub size: i64,
    pub realized_pnl: i64,
}
