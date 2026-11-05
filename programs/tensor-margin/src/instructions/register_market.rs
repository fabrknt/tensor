use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct RegisterMarketParams {
    pub symbol: String,
    pub base_mint: Pubkey,
    pub oracle: Pubkey,
    pub variance_tracker: Pubkey,
    pub spot_enabled: bool,
    pub perp_enabled: bool,
    pub options_enabled: bool,
    pub lending_enabled: bool,
    pub initial_margin_bps: u64,
    pub maintenance_ratio_bps: u64,
    pub max_position_size: u64,
}

#[derive(Accounts)]
#[instruction(params: RegisterMarketParams)]
pub struct RegisterMarket<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + MarginMarket::INIT_SPACE,
        seeds = [MarginMarket::SEED, &config.total_markets.to_le_bytes()],
        bump
    )]
    pub market: Account<'info, MarginMarket>,

    #[account(
        mut,
        seeds = [MarginConfig::SEED],
        bump = config.bump,
        constraint = config.authority == authority.key() @ TensorError::Unauthorized,
    )]
    pub config: Account<'info, MarginConfig>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<RegisterMarket>, params: RegisterMarketParams) -> Result<()> {
    let config = &mut ctx.accounts.config;
    let market = &mut ctx.accounts.market;

    market.index = config.total_markets;
    market.symbol = params.symbol;
    market.base_mint = params.base_mint;
    market.oracle = params.oracle;
    market.variance_tracker = params.variance_tracker;

    market.spot_enabled = params.spot_enabled;
    market.perp_enabled = params.perp_enabled;
    market.options_enabled = params.options_enabled;
    market.lending_enabled = params.lending_enabled;

    market.initial_margin_bps = params.initial_margin_bps;
    market.maintenance_ratio_bps = params.maintenance_ratio_bps;
    market.max_position_size = params.max_position_size;

    market.mark_price = 0;
    market.implied_vol_bps = 0;
    market.funding_rate_bps = 0;
    market.cumulative_funding_index = 0;
    market.last_funding_update = 0;
    market.open_interest_long = 0;
    market.open_interest_short = 0;
    market.total_volume = 0;
    market.is_active = true;
    market.bump = ctx.bumps.market;

    config.total_markets += 1;

    emit!(MarketRegistered {
        index: market.index,
        symbol: market.symbol.clone(),
        base_mint: market.base_mint,
    });

    Ok(())
}

#[event]
pub struct MarketRegistered {
    pub index: u16,
    pub symbol: String,
    pub base_mint: Pubkey,
}
