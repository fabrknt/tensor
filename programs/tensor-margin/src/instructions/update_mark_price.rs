use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

/// Keeper instruction: update mark price and implied vol for a market.
///
/// In production, this would read from sigma shared-oracle or northtail-oracle.
/// For MVP, the keeper pushes prices directly (authority-gated).
#[derive(Accounts)]
pub struct UpdateMarkPrice<'info> {
    #[account(
        mut,
        seeds = [MarginMarket::SEED, &market.index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Account<'info, MarginMarket>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
        constraint = config.authority == authority.key() @ TensorError::Unauthorized,
    )]
    pub config: Account<'info, MarginConfig>,

    pub authority: Signer<'info>,
}

pub fn handler(
    ctx: Context<UpdateMarkPrice>,
    mark_price: u64,
    implied_vol_bps: u64,
    funding_rate_bps: i64,
) -> Result<()> {
    require!(mark_price > 0, TensorError::InvalidPrice);

    let clock = Clock::get()?;
    let market = &mut ctx.accounts.market;

    market.mark_price = mark_price;
    market.implied_vol_bps = implied_vol_bps;

    // Update funding
    if funding_rate_bps != market.funding_rate_bps {
        market.funding_rate_bps = funding_rate_bps;
        market.last_funding_update = clock.unix_timestamp;

        // Accumulate funding index (1e9 precision)
        // funding_index += rate_bps * elapsed / (8h * BPS)
        let elapsed = clock.unix_timestamp - market.last_funding_update;
        if elapsed > 0 {
            let funding_increment = funding_rate_bps as i128 * elapsed as i128 * 1_000_000_000
                / (28_800 * 10_000); // 8h = 28800s
            market.cumulative_funding_index += funding_increment;
        }
    }

    emit!(MarkPriceUpdated {
        market_index: market.index,
        mark_price,
        implied_vol_bps,
        funding_rate_bps,
    });

    Ok(())
}

#[event]
pub struct MarkPriceUpdated {
    pub market_index: u16,
    pub mark_price: u64,
    pub implied_vol_bps: u64,
    pub funding_rate_bps: i64,
}
