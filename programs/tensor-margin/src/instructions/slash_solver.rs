use anchor_lang::prelude::*;
use anchor_spl::token::{self, Token, TokenAccount, Transfer};
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

/// Permissionless slashing: anyone can call when a solver wins an auction
/// but fails to execute before the intent deadline.
#[derive(Accounts)]
pub struct SlashSolver<'info> {
    /// The intent account to verify winning_solver and deadline
    #[account(
        mut,
        seeds = [
            IntentAccount::SEED,
            intent_account.margin_account.as_ref(),
            &intent_account.intent_id.to_le_bytes(),
        ],
        bump = intent_account.bump,
    )]
    pub intent_account: Account<'info, IntentAccount>,

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

    /// PDA-owned vault token account holding solver stakes (source of slashed tokens)
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

    /// Destination for slashed tokens (fee collector)
    #[account(
        mut,
        constraint = fee_collector_token_account.mint == config.collateral_mint @ TensorError::InvalidAmount,
        constraint = fee_collector_token_account.owner == config.fee_collector @ TensorError::AccountMismatch,
    )]
    pub fee_collector_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
}

pub fn handler(ctx: Context<SlashSolver>) -> Result<()> {
    let clock = Clock::get()?;
    let intent = &mut ctx.accounts.intent_account;
    let config = &ctx.accounts.config;
    let registry = &mut ctx.accounts.solver_registry;

    // Verify winning_solver is set (auction was settled)
    require!(
        intent.winning_solver != Pubkey::default(),
        TensorError::InvalidIntentState
    );

    // Verify intent is still pending or partially filled (solver failed to complete it)
    require!(
        intent.status == IntentStatus::Pending || intent.status == IntentStatus::PartiallyFilled,
        TensorError::IntentAlreadyResolved
    );

    // Verify deadline has passed
    require!(
        intent.deadline > 0 && clock.unix_timestamp > intent.deadline,
        TensorError::DeadlinePassed
    );

    let winning_solver = intent.winning_solver;

    // Apply slash to the solver in registry
    let slash_amount = registry
        .slash(&winning_solver, config.solver_slash_rate_bps, config.min_solver_stake)
        .ok_or(error!(TensorError::SolverNotFound))?;

    // Transfer slashed tokens from vault to fee collector
    if slash_amount > 0 {
        let seeds = &[
            SolverRegistry::VAULT_AUTHORITY_SEED,
            &[ctx.bumps.vault_authority],
        ];
        let signer_seeds = &[&seeds[..]];

        let transfer_ctx = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.fee_collector_token_account.to_account_info(),
                authority: ctx.accounts.vault_authority.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(transfer_ctx, slash_amount)?;
    }

    // Mark intent as cancelled since the solver failed
    intent.status = IntentStatus::Cancelled;
    intent.updated_at = clock.unix_timestamp;

    emit!(SolverSlashed {
        solver: winning_solver,
        intent_id: intent.intent_id,
        slash_amount,
    });

    Ok(())
}

#[event]
pub struct SolverSlashed {
    pub solver: Pubkey,
    pub intent_id: u64,
    pub slash_amount: u64,
}
