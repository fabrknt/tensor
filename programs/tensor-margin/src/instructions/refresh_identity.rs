use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

/// Read a user's Sovereign Identity and update their margin account's
/// investor category based on reputation tier.
///
/// Higher reputation tiers unlock higher leverage limits:
/// - Tier 1-2 (Retail): 5x max leverage
/// - Tier 3 (Qualified): 20x max leverage
/// - Tier 4-5 (Institutional): 50x max leverage
///
/// This can be called by anyone (permissionless) to refresh a user's
/// category. The user benefits from calling it after their reputation
/// improves, as it unlocks more capital efficiency.
#[derive(Accounts)]
pub struct RefreshIdentity<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, margin_account.owner.as_ref()],
        bump = margin_account.bump,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    /// Sovereign Identity PDA for the margin account owner.
    /// CHECK: Deserialized via tensor_cpi::sovereign::read_identity.
    /// Validated to match margin_account.owner.
    #[account()]
    pub identity: AccountInfo<'info>,
}

pub fn handler(ctx: Context<RefreshIdentity>) -> Result<()> {
    let account = &mut ctx.accounts.margin_account;

    // Read identity from sovereign program
    let identity_data = tensor_cpi::sovereign::read_identity(&ctx.accounts.identity)
        .map_err(|_| TensorError::InvalidAmount)?;

    // Verify the identity belongs to this margin account's owner
    require!(
        identity_data.owner == account.owner,
        TensorError::Unauthorized
    );

    // Map tier to investor category
    let new_category = tensor_cpi::sovereign::tier_to_investor_category(identity_data.tier);
    let old_category = account.investor_category;

    account.investor_category = new_category;
    account.identity = ctx.accounts.identity.key();

    emit!(IdentityRefreshed {
        owner: account.owner,
        old_category,
        new_category,
        sovereign_tier: identity_data.tier,
        composite_score: identity_data.composite_score,
        trading_score: identity_data.trading_score,
    });

    // If category changed and positions exist, re-check leverage limits
    if old_category != new_category && account.greeks.total_notional > 0 && account.collateral > 0 {
        let max_leverage = new_category.max_leverage_bps();
        let current_leverage =
            (account.greeks.total_notional as u128 * 10_000) / account.collateral as u128;

        // If downgraded and over-leveraged, emit a warning but don't liquidate.
        // The compute_margin crank will handle the health check.
        if current_leverage > max_leverage as u128 {
            emit!(LeverageWarning {
                owner: account.owner,
                current_leverage_bps: current_leverage as u64,
                max_leverage_bps: max_leverage,
            });
        }
    }

    Ok(())
}

#[event]
pub struct IdentityRefreshed {
    pub owner: Pubkey,
    pub old_category: tensor_types::InvestorCategory,
    pub new_category: tensor_types::InvestorCategory,
    pub sovereign_tier: u8,
    pub composite_score: u16,
    pub trading_score: u16,
}

#[event]
pub struct LeverageWarning {
    pub owner: Pubkey,
    pub current_leverage_bps: u64,
    pub max_leverage_bps: u64,
}
