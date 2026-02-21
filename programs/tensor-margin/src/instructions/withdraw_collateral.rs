use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct WithdrawCollateral<'info> {
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

    /// CHECK: Vault authority PDA
    #[account(
        seeds = [b"vault_authority"],
        bump,
    )]
    pub vault_authority: AccountInfo<'info>,

    #[account(mut)]
    pub owner: Signer<'info>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<WithdrawCollateral>, amount: u64) -> Result<()> {
    require!(amount > 0, TensorError::InvalidAmount);

    let account = &ctx.accounts.margin_account;
    let available = account.available_collateral();
    require!(amount <= available, TensorError::InsufficientCollateral);

    // Check that withdrawal doesn't put account below maintenance margin
    let new_collateral = account.collateral.saturating_sub(amount);
    let new_equity = account.equity.saturating_sub(amount as i64);
    if account.has_positions() && new_equity > 0 {
        let ratio = tensor_math::margin_ratio_bps(new_equity, account.maintenance_margin_required);
        require!(ratio > 10_000, TensorError::InsufficientMargin);
    }

    // Transfer tokens from vault to user
    let seeds = &[b"vault_authority".as_ref(), &[ctx.bumps.vault_authority]];
    let signer_seeds = &[&seeds[..]];

    let transfer_ctx = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.user_token_account.to_account_info(),
            authority: ctx.accounts.vault_authority.to_account_info(),
        },
        signer_seeds,
    );
    token::transfer(transfer_ctx, amount)?;

    // Update margin account
    let account = &mut ctx.accounts.margin_account;
    account.collateral = new_collateral;
    account.equity = new_equity;

    emit!(CollateralWithdrawn {
        owner: account.owner,
        amount,
        remaining_collateral: account.collateral,
    });

    Ok(())
}

#[event]
pub struct CollateralWithdrawn {
    pub owner: Pubkey,
    pub amount: u64,
    pub remaining_collateral: u64,
}
