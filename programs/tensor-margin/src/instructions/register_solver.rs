use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct RegisterSolver<'info> {
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

    /// The solver's signing authority
    pub solver: Signer<'info>,

    /// The solver's collateral token account (source of stake tokens)
    #[account(
        mut,
        constraint = solver_token_account.mint == config.collateral_mint @ TensorError::InvalidAmount,
        constraint = solver_token_account.owner == solver.key() @ TensorError::Unauthorized,
    )]
    pub solver_token_account: Account<'info, TokenAccount>,

    /// PDA-owned vault token account for holding solver stakes
    #[account(
        mut,
        constraint = vault.mint == config.collateral_mint @ TensorError::InvalidAmount,
        address = SolverRegistry::stake_vault(&config.collateral_mint) @ TensorError::AccountMismatch,
    )]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<RegisterSolver>, stake: u64) -> Result<()> {
    let config = &ctx.accounts.config;
    let registry = &mut ctx.accounts.solver_registry;
    let solver_key = ctx.accounts.solver.key();

    require!(stake >= config.min_solver_stake, TensorError::InsufficientSolverStake);
    require!(!registry.is_registered(&solver_key), TensorError::SolverNotActive);

    let slot = registry
        .find_empty_slot()
        .ok_or(error!(TensorError::MaxSolverCount))?;

    // Transfer stake tokens from solver to vault
    let transfer_ctx = CpiContext::new(
        ctx.accounts.token_program.to_account_info(),
        Transfer {
            from: ctx.accounts.solver_token_account.to_account_info(),
            to: ctx.accounts.vault.to_account_info(),
            authority: ctx.accounts.solver.to_account_info(),
        },
    );
    token::transfer(transfer_ctx, stake)?;

    let clock = Clock::get()?;

    registry.solvers[slot] = SolverEntry {
        solver: solver_key,
        stake,
        total_fills: 0,
        total_volume: 0,
        slash_count: 0,
        is_active: true,
        registered_at: clock.unix_timestamp,
    };
    registry.solver_count += 1;

    emit!(SolverRegistered {
        solver: solver_key,
        stake,
    });

    Ok(())
}

#[event]
pub struct SolverRegistered {
    pub solver: Pubkey,
    pub stake: u64,
}
