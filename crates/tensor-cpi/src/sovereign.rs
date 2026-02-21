//! Read-only adapter for Sovereign identity accounts.
//!
//! Reads the 5-dimension reputation scores and tier from a
//! SovereignIdentity PDA without importing the full sovereign program.

use anchor_lang::prelude::*;

/// Sovereign program ID
pub const SOVEREIGN_PROGRAM_ID: &str = "2UAZc1jj4QTSkgrC8U9d4a7EM9AQunxMvW5g7rX7Af9T";

/// Identity PDA seed
pub const IDENTITY_SEED: &[u8] = b"identity";

pub struct IdentityData {
    pub owner: Pubkey,
    pub trading_score: u16,
    pub civic_score: u16,
    pub developer_score: u16,
    pub infra_score: u16,
    pub creator_score: u16,
    pub composite_score: u16,
    pub tier: u8,
    pub last_updated: i64,
}

/// Reads a SovereignIdentity account.
///
/// On-chain layout (after 8-byte discriminator):
///   owner: Pubkey (32)
///   created_at: i64 (8)
///   trading_authority: Pubkey (32)
///   civic_authority: Pubkey (32)
///   developer_authority: Pubkey (32)
///   infra_authority: Pubkey (32)
///   creator_authority: Pubkey (32)
///   trading_score: u16 (2)
///   civic_score: u16 (2)
///   developer_score: u16 (2)
///   infra_score: u16 (2)
///   creator_score: u16 (2)
///   composite_score: u16 (2)
///   tier: u8 (1)
///   last_updated: i64 (8)
///   bump: u8 (1)
///
/// Scores offset = 8 + 32 + 8 + (5 * 32) = 208
pub fn read_identity(account: &AccountInfo) -> Result<IdentityData> {
    let data = account.try_borrow_data()?;

    // SovereignIdentity::SIZE = 236 bytes total (8 discriminator + 228 fields)
    require!(data.len() >= 236, ErrorCode::AccountDidNotDeserialize);

    // owner at offset 8
    let owner = Pubkey::try_from(&data[8..40])
        .map_err(|_| ErrorCode::AccountDidNotDeserialize)?;

    // Scores start at offset 208
    let off = 208;

    let trading_score = u16::from_le_bytes(
        data[off..off + 2].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let civic_score = u16::from_le_bytes(
        data[off + 2..off + 4].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let developer_score = u16::from_le_bytes(
        data[off + 4..off + 6].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let infra_score = u16::from_le_bytes(
        data[off + 6..off + 8].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let creator_score = u16::from_le_bytes(
        data[off + 8..off + 10].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let composite_score = u16::from_le_bytes(
        data[off + 10..off + 12].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let tier = data[off + 12];

    let last_updated = i64::from_le_bytes(
        data[off + 13..off + 21].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );

    Ok(IdentityData {
        owner,
        trading_score,
        civic_score,
        developer_score,
        infra_score,
        creator_score,
        composite_score,
        tier,
        last_updated,
    })
}

/// Map sovereign tier to Tensor InvestorCategory.
///
/// Tier 1-2 → Retail (5x leverage)
/// Tier 3   → Qualified (20x leverage)
/// Tier 4-5 → Institutional (50x leverage)
pub fn tier_to_investor_category(tier: u8) -> tensor_types::InvestorCategory {
    match tier {
        4..=5 => tensor_types::InvestorCategory::Institutional,
        3 => tensor_types::InvestorCategory::Qualified,
        _ => tensor_types::InvestorCategory::Retail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_0_is_retail() {
        assert_eq!(
            tier_to_investor_category(0),
            tensor_types::InvestorCategory::Retail
        );
    }

    #[test]
    fn test_tier_1_is_retail() {
        assert_eq!(
            tier_to_investor_category(1),
            tensor_types::InvestorCategory::Retail
        );
    }

    #[test]
    fn test_tier_2_is_retail() {
        assert_eq!(
            tier_to_investor_category(2),
            tensor_types::InvestorCategory::Retail
        );
    }

    #[test]
    fn test_tier_3_is_qualified() {
        assert_eq!(
            tier_to_investor_category(3),
            tensor_types::InvestorCategory::Qualified
        );
    }

    #[test]
    fn test_tier_4_is_institutional() {
        assert_eq!(
            tier_to_investor_category(4),
            tensor_types::InvestorCategory::Institutional
        );
    }

    #[test]
    fn test_tier_5_is_institutional() {
        assert_eq!(
            tier_to_investor_category(5),
            tensor_types::InvestorCategory::Institutional
        );
    }

    #[test]
    fn test_tier_255_is_retail() {
        // Out-of-range tier defaults to Retail
        assert_eq!(
            tier_to_investor_category(255),
            tensor_types::InvestorCategory::Retail
        );
    }

    #[test]
    fn test_tier_leverage_progression() {
        let retail_lev = tier_to_investor_category(1).max_leverage_bps();
        let qualified_lev = tier_to_investor_category(3).max_leverage_bps();
        let institutional_lev = tier_to_investor_category(5).max_leverage_bps();

        assert!(retail_lev < qualified_lev);
        assert!(qualified_lev < institutional_lev);
        assert_eq!(retail_lev, 50_000);        // 5x
        assert_eq!(qualified_lev, 200_000);    // 20x
        assert_eq!(institutional_lev, 500_000); // 50x
    }
}
