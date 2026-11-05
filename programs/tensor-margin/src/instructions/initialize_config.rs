use anchor_lang::prelude::*;
use crate::state::*;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct InitializeConfigParams {
    pub initial_margin_bps: u64,
    pub maintenance_ratio_bps: u64,
    pub gamma_margin_bps: u64,
    pub vega_margin_bps: u64,
    pub liquidation_fee_bps: u64,
    pub trading_fee_bps: u64,
    pub kyc_registry: Pubkey,
    pub identity_program: Pubkey,
}

#[derive(Accounts)]
pub struct InitializeConfig<'info> {
    #[account(
        init,
        payer = authority,
        space = 8 + MarginConfig::INIT_SPACE,
        seeds = [MarginConfig::SEED],
        bump
    )]
    pub config: Account<'info, MarginConfig>,

    /// CHECK: Collateral token mint (USDC)
    pub collateral_mint: AccountInfo<'info>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<InitializeConfig>, params: InitializeConfigParams) -> Result<()> {
    let config = &mut ctx.accounts.config;

    config.authority = ctx.accounts.authority.key();
    config.fee_collector = ctx.accounts.authority.key();
    config.collateral_mint = ctx.accounts.collateral_mint.key();

    config.initial_margin_bps = params.initial_margin_bps;
    config.maintenance_ratio_bps = params.maintenance_ratio_bps;
    config.gamma_margin_bps = params.gamma_margin_bps;
    config.vega_margin_bps = params.vega_margin_bps;
    config.liquidation_fee_bps = params.liquidation_fee_bps;
    config.trading_fee_bps = params.trading_fee_bps;

    config.max_margin_mode = tensor_types::MarginMode::Portfolio;
    config.kyc_registry = params.kyc_registry;
    config.identity_program = params.identity_program;

    config.insurance_fund = 0;
    config.total_accounts = 0;
    config.total_markets = 0;
    config.is_paused = false;
    config.bump = ctx.bumps.config;

    emit!(ConfigInitialized {
        authority: config.authority,
        collateral_mint: config.collateral_mint,
        initial_margin_bps: config.initial_margin_bps,
    });

    Ok(())
}

#[event]
pub struct ConfigInitialized {
    pub authority: Pubkey,
    pub collateral_mint: Pubkey,
    pub initial_margin_bps: u64,
}
