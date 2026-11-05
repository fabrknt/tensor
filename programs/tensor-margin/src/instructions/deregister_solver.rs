use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct DeregisterSolver<'info> {
    #[account(
        mut,
        seeds = [SolverRegistry::SEED],
        bump = solver_registry.bump,
    )]
    pub solver_registry: Account<'info, SolverRegistry>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, MarginConfig>,

    /// The solver requesting deregistration (must be signer)
    pub solver: Signer<'info>,

    /// The solver's collateral token account (destination for returned stake)
    #[account(
        mut,
        constraint = solver_token_account.mint == config.collateral_mint @ TensorError::InvalidAmount,
        constraint = solver_token_account.owner == solver.key() @ TensorError::Unauthorized,
    )]
    pub solver_token_account: Account<'info, TokenAccount>,

    /// PDA-owned vault token account holding solver stakes (source)
    #[account(
        mut,
        constraint = vault.mint == config.collateral_mint @ TensorError::InvalidAmount,
        address = SolverRegistry::stake_vault(&config.collateral_mint) @ TensorError::AccountMismatch,
    )]
    pub vault: Account<'info, TokenAccount>,

    /// CHECK: Vault authority PDA that owns the vault
    #[account(
        seeds = [SolverRegistry::VAULT_AUTHORITY_SEED],
        bump,
    )]
    pub vault_authority: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<DeregisterSolver>) -> Result<()> {
    let registry = &mut ctx.accounts.solver_registry;
    let solver_key = ctx.accounts.solver.key();

    // Deregister and get remaining stake
    let remaining_stake = registry
        .deregister(&solver_key)
        .ok_or(error!(TensorError::SolverNotFound))?;

    // Transfer remaining stake back to solver
    if remaining_stake > 0 {
        let seeds = &[
            SolverRegistry::VAULT_AUTHORITY_SEED,
            &[ctx.bumps.vault_authority],
        ];
        let signer_seeds = &[&seeds[..]];

        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.solver_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_ctx, remaining_stake)?;
    }

    emit!(SolverDeregistered {
        solver: solver_key,
        returned_stake: remaining_stake,
    });

    Ok(())
}

#[event]
pub struct SolverDeregistered {
    pub solver: Pubkey,
    pub returned_stake: u64,
}
