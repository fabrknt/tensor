use anchor_lang::prelude::*;
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct CreateMarginAccount<'info> {
    #[account(
        init,
        payer = owner,
        space = 8 + MarginAccount::INIT_SPACE,
        seeds = [MarginAccount::SEED, owner.key().as_ref()],
        bump
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        mut,
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, MarginConfig>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(
    ctx: Context<CreateMarginAccount>,
    margin_mode: MarginMode,
    investor_category: InvestorCategory,
) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(!config.is_paused, TensorError::ProtocolPaused);

    let clock = Clock::get()?;
    let account = &mut ctx.accounts.margin_account;

    account.owner = ctx.accounts.owner.key();
    account.delegate = Pubkey::default();
    account.collateral = 0;
    account.locked_collateral = 0;

    account.perp_positions = [PerpPosition::default(); MAX_PERP_POSITIONS];
    account.perp_count = 0;
    account.spot_balances = [SpotBalance::default(); MAX_SPOT_BALANCES];
    account.spot_count = 0;
    account.option_positions = [OptionPosition::default(); MAX_OPTION_POSITIONS];
    account.option_count = 0;
    account.lending_positions = [LendingPosition::default(); MAX_LENDING_POSITIONS];
    account.lending_count = 0;

    account.greeks = PortfolioGreeks::default();
    account.initial_margin_required = 0;
    account.maintenance_margin_required = 0;
    account.equity = 0;
    account.margin_ratio_bps = u16::MAX;
    account.health = AccountHealth::Healthy;

    account.margin_mode = margin_mode;
    account.investor_category = investor_category;
    account.identity = Pubkey::default();

    account.created_at = clock.unix_timestamp;
    account.last_margin_update = clock.unix_timestamp;
    account.total_trades = 0;
    account.total_realized_pnl = 0;
    account.bump = ctx.bumps.margin_account;

    // Increment global account count
    let config = &mut ctx.accounts.config;
    config.total_accounts += 1;

    emit!(MarginAccountCreated {
        owner: account.owner,
        margin_mode,
        investor_category,
    });

    Ok(())
}

#[event]
pub struct MarginAccountCreated {
    pub owner: Pubkey,
    pub margin_mode: MarginMode,
    pub investor_category: InvestorCategory,
}
