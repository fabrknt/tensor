use anchor_lang::prelude::*;
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct CancelIntent<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, authority.key().as_ref()],
        bump = margin_account.bump,
        constraint = (margin_account.owner == authority.key()
            || margin_account.delegate == authority.key()) @ TensorError::Unauthorized,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        mut,
        seeds = [
            IntentAccount::SEED,
            margin_account.key().as_ref(),
            &intent_account.intent_id.to_le_bytes(),
        ],
        bump = intent_account.bump,
        constraint = intent_account.margin_account == margin_account.key() @ TensorError::Unauthorized,
    )]
    pub intent_account: Account<'info, IntentAccount>,

    pub authority: Signer<'info>,
}

pub fn handler(ctx: Context<CancelIntent>) -> Result<()> {
    let clock = Clock::get()?;
    let intent = &mut ctx.accounts.intent_account;
    let account = &mut ctx.accounts.margin_account;

    // Only Pending or PartiallyFilled can be cancelled
    require!(
        intent.status == IntentStatus::Pending || intent.status == IntentStatus::PartiallyFilled,
        TensorError::IntentAlreadyResolved
    );

    intent.status = IntentStatus::Cancelled;
    intent.updated_at = clock.unix_timestamp;

    // Decrement active intent count
    account.active_intent_count = account.active_intent_count.saturating_sub(1);

    emit!(IntentCancelled {
        owner: account.owner,
        intent_id: intent.intent_id,
        filled_legs: intent.filled_legs,
        total_legs: intent.leg_count,
    });

    Ok(())
}

#[event]
pub struct IntentCancelled {
    pub owner: Pubkey,
    pub intent_id: u64,
    pub filled_legs: u8,
    pub total_legs: u8,
}
