use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(Accounts)]
pub struct SubmitBid<'info> {
    #[account(
        mut,
        seeds = [
            IntentAccount::SEED,
            intent_account.margin_account.as_ref(),
            &intent_account.intent_id.to_le_bytes(),
        ],
        bump = intent_account.bump,
    )]
    pub intent_account: Box<Account<'info, IntentAccount>>,

    #[account(
        seeds = [SolverRegistry::SEED],
        bump = solver_registry.bump,
    )]
    pub solver_registry: Box<Account<'info, SolverRegistry>>,

    /// The solver submitting a bid
    pub solver: Signer<'info>,
}

pub fn handler(ctx: Context<SubmitBid>, bid_price: u64) -> Result<()> {
    let clock = Clock::get()?;
    let intent = &mut ctx.accounts.intent_account;
    let registry = &ctx.accounts.solver_registry;
    let solver_key = ctx.accounts.solver.key();

    // Must be a registered solver
    require!(registry.is_registered(&solver_key), TensorError::SolverNotRegistered);

    // Intent must be pending or partially filled
    require!(
        intent.status == tensor_types::IntentStatus::Pending
            || intent.status == tensor_types::IntentStatus::PartiallyFilled,
        TensorError::IntentAlreadyResolved
    );

    // Must be within auction window
    require!(
        intent.auction_end > 0 && clock.unix_timestamp <= intent.auction_end,
        TensorError::AuctionEnded
    );

    // Find a bid slot
    let slot = intent
        .bids
        .iter()
        .position(|b| !b.is_active)
        .ok_or(error!(TensorError::MaxSolverCount))?;

    intent.bids[slot] = SolverBid {
        solver: solver_key,
        bid_price,
        bid_timestamp: clock.unix_timestamp,
        is_active: true,
    };
    intent.bid_count += 1;

    emit!(BidSubmitted {
        intent_id: intent.intent_id,
        solver: solver_key,
        bid_price,
    });

    Ok(())
}

#[event]
pub struct BidSubmitted {
    pub intent_id: u64,
    pub solver: Pubkey,
    pub bid_price: u64,
}
