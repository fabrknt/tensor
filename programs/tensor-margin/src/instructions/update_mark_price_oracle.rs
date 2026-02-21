use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

/// Permissionless keeper instruction: update mark price and implied vol
/// for a market by reading directly from Sigma shared-oracle accounts.
///
/// Unlike `update_mark_price` (which requires authority), this is
/// permissionless because data comes trustlessly from oracle accounts.
#[derive(Accounts)]
pub struct UpdateMarkPriceOracle<'info> {
    #[account(
        mut,
        seeds = [MarginMarket::SEED, &market.index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Account<'info, MarginMarket>,

    /// Sigma PriceFeed account linked to this market's oracle field
    /// CHECK: Validated by reading and checking it matches market.oracle
    #[account()]
    pub price_feed: AccountInfo<'info>,

    /// Optional: Sigma FundingFeed for this market
    /// CHECK: Deserialized via tensor_cpi::sigma::read_funding_feed
    #[account()]
    pub funding_feed: AccountInfo<'info>,

    /// Anyone can crank this
    pub cranker: Signer<'info>,
}

pub fn handler(ctx: Context<UpdateMarkPriceOracle>) -> Result<()> {
    let clock = Clock::get()?;
    let market = &mut ctx.accounts.market;

    // Validate oracle account matches market's configured oracle
    require!(
        ctx.accounts.price_feed.key() == market.oracle,
        TensorError::OracleStale
    );

    // Read price from sigma oracle
    let oracle_data = tensor_cpi::sigma::read_price_feed(&ctx.accounts.price_feed)
        .map_err(|_| TensorError::OracleStale)?;

    require!(oracle_data.is_active, TensorError::OracleStale);
    require!(oracle_data.last_price > 0, TensorError::InvalidPrice);

    // Check staleness (5 minute max)
    require!(
        clock.unix_timestamp - oracle_data.last_sample_time <= 300,
        TensorError::OracleStale
    );

    // Use TWAP as mark price (more manipulation-resistant)
    let new_mark_price = if oracle_data.twap > 0 {
        oracle_data.twap
    } else {
        oracle_data.last_price
    };

    // Derive implied vol from variance
    let new_implied_vol = if oracle_data.current_variance > 0 {
        integer_sqrt(oracle_data.current_variance as u128) as u64
    } else {
        market.implied_vol_bps // keep existing if oracle has no variance data
    };

    market.mark_price = new_mark_price;
    market.implied_vol_bps = new_implied_vol;

    // Try reading funding feed
    let funding_feed_ai = &ctx.accounts.funding_feed;
    if let Ok(funding_data) = tensor_cpi::sigma::read_funding_feed(funding_feed_ai) {
        let new_rate = funding_data.current_rate_bps;
        if new_rate != market.funding_rate_bps {
            let old_update = market.last_funding_update;
            let elapsed = clock.unix_timestamp - old_update;

            // Accumulate funding index before updating rate
            if elapsed > 0 && old_update > 0 {
                let funding_increment =
                    market.funding_rate_bps as i128 * elapsed as i128 * 1_000_000_000
                        / (28_800 * 10_000);
                market.cumulative_funding_index += funding_increment;
            }

            market.funding_rate_bps = new_rate;
            market.last_funding_update = clock.unix_timestamp;
        }
    }

    emit!(OracleMarkPriceUpdated {
        market_index: market.index,
        mark_price: new_mark_price,
        implied_vol_bps: new_implied_vol,
        funding_rate_bps: market.funding_rate_bps,
        oracle_source: ctx.accounts.price_feed.key(),
    });

    Ok(())
}

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
pub struct OracleMarkPriceUpdated {
    pub market_index: u16,
    pub mark_price: u64,
    pub implied_vol_bps: u64,
    pub funding_rate_bps: i64,
    pub oracle_source: Pubkey,
}
