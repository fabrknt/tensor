use anchor_lang::prelude::*;
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

/// Refresh ZK credit score from an external oracle account.
///
/// Follows the same pattern as `refresh_identity`: reads an external
/// program's account via zero-copy CPI reader, validates ownership,
/// and updates the margin account's credit tier.
///
/// Higher credit tiers unlock margin discounts and leverage bonuses.
#[derive(Accounts)]
pub struct RefreshZkCredit<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, margin_account.owner.as_ref()],
        bump = margin_account.bump,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    /// ZK Credit oracle account for the margin account owner.
    /// CHECK: Deserialized via tensor_cpi::zk_credit::read_zk_credit.
    /// Validated to match margin_account.owner.
    #[account()]
    pub zk_credit_oracle: AccountInfo<'info>,
}

/// Max staleness for ZK credit scores: 24 hours
const ZK_CREDIT_MAX_STALENESS: i64 = 86_400;

pub fn handler(ctx: Context<RefreshZkCredit>) -> Result<()> {
    let clock = Clock::get()?;
    let account = &mut ctx.accounts.margin_account;

    // Read ZK credit data from oracle
    let credit_data = tensor_cpi::zk_credit::read_zk_credit(&ctx.accounts.zk_credit_oracle)
        .map_err(|_| TensorError::CreditScoreInvalid)?;

    // Verify the credit belongs to this margin account's owner
    require!(
        credit_data.owner == account.owner,
        TensorError::CreditOracleMismatch
    );

    // Verify proof was verified
    require!(credit_data.proof_verified, TensorError::CreditScoreInvalid);

    // Verify score is in valid range
    require!(credit_data.score <= 1000, TensorError::CreditScoreInvalid);

    // Check staleness
    require!(
        tensor_cpi::zk_credit::is_score_valid(
            &credit_data,
            clock.unix_timestamp,
            ZK_CREDIT_MAX_STALENESS,
        ),
        TensorError::CreditScoreStale
    );

    // Update margin account
    let old_tier = account.zk_credit_tier;
    let new_tier = ZkCreditTier::from_score(credit_data.score);

    account.zk_credit_score = credit_data.score;
    account.zk_credit_tier = new_tier;
    account.zk_score_updated_at = clock.unix_timestamp;
    account.zk_credit_oracle = ctx.accounts.zk_credit_oracle.key();

    emit!(ZkCreditRefreshed {
        owner: account.owner,
        score: credit_data.score,
        old_tier,
        new_tier,
        margin_discount_bps: new_tier.margin_discount_bps(),
        leverage_bonus_bps: new_tier.leverage_bonus_bps(),
    });

    Ok(())
}

#[event]
pub struct ZkCreditRefreshed {
    pub owner: Pubkey,
    pub score: u16,
    pub old_tier: ZkCreditTier,
    pub new_tier: ZkCreditTier,
    pub margin_discount_bps: u64,
    pub leverage_bonus_bps: u64,
}
