use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct DepositCollateral<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, owner.key().as_ref()],
        bump = margin_account.bump,
        constraint = margin_account.owner == owner.key() @ TensorError::Unauthorized,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, MarginConfig>,

    #[account(
        mut,
        constraint = user_token_account.mint == config.collateral_mint @ TensorError::InvalidAmount,
        constraint = user_token_account.owner == owner.key() @ TensorError::Unauthorized,
    )]
    pub user_token_account: Account<'info, TokenAccount>,

    #[account(
        mut,
        constraint = vault.mint == config.collateral_mint @ TensorError::InvalidAmount,
    )]
    pub vault: Account<'info, TokenAccount>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<DepositCollateral>, amount: u64) -> Result<()> {
    require!(amount > 0, TensorError::InvalidAmount);

    let config = &ctx.accounts.config;
    require!(!config.is_paused, TensorError::ProtocolPaused);

    // Transfer tokens from user to vault
    let transfer_ctx = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        Transfer {
            from: ctx.accounts.user_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.owner.to_account_info(),
        },
    );
    token::transfer(transfer_ctx, amount)?;

    // Update margin account
    let account = &mut ctx.accounts.margin_account;
    account.collateral = account.collateral.checked_add(amount)
        .ok_or(TensorError::MathOverflow)?;

    // Update equity (simple case: collateral increase)
    account.equity = account.equity.checked_add(amount as i64)
        .ok_or(TensorError::MathOverflow)?;

    emit!(CollateralDeposited {
        owner: account.owner,
        amount,
        total_collateral: account.collateral,
    });

    Ok(())
}

#[event]
pub struct CollateralDeposited {
    pub owner: Pubkey,
    pub amount: u64,
    pub total_collateral: u64,
}
