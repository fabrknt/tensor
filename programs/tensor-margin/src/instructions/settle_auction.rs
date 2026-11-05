use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

/// Permissionless crank: settle the solver auction for an intent.
/// Selects the best bid (lowest price for buys, highest for sells).
#[derive(Accounts)]
pub struct SettleAuction<'info> {
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
}

pub fn handler(ctx: Context<SettleAuction>) -> Result<()> {
    let clock = Clock::get()?;
    let intent = &mut ctx.accounts.intent_account;

    // Auction must have ended
    require!(
        intent.auction_end > 0 && clock.unix_timestamp > intent.auction_end,
        TensorError::AuctionStillOpen
    );

    // Must not already be settled (winning_solver == default means unsettled)
    require!(
        intent.winning_solver == Pubkey::default(),
        TensorError::IntentAlreadyResolved
    );

    // Find the first active leg to determine buy/sell direction
    let first_active_leg = intent
        .legs
        .iter()
        .find(|l| l.is_active);

    let is_buy = match first_active_leg {
        Some(leg) => leg.size > 0,
        None => return Err(error!(TensorError::IntentAlreadyResolved)),
    };

    // Select best bid: lowest price for buys, highest for sells
    let mut best_idx: Option<usize> = None;
    let mut best_price: u64 = if is_buy { u64::MAX } else { 0 };

    for (i, bid) in intent.bids.iter().enumerate() {
        if !bid.is_active {
            continue;
        }
        let is_better = if is_buy {
            bid.bid_price < best_price
        } else {
            bid.bid_price > best_price
        };
        if is_better {
            best_price = bid.bid_price;
            best_idx = Some(i);
        }
    }

    if let Some(idx) = best_idx {
        intent.winning_solver = intent.bids[idx].solver;

        emit!(AuctionSettled {
            intent_id: intent.intent_id,
            winning_solver: intent.winning_solver,
            winning_price: best_price,
            bid_count: intent.bid_count,
        });
    }
    // If no bids, winning_solver stays default — any registered solver can fill (fallback)

    Ok(())
}

#[event]
pub struct AuctionSettled {
    pub intent_id: u64,
    pub winning_solver: Pubkey,
    pub winning_price: u64,
    pub bid_count: u8,
}
