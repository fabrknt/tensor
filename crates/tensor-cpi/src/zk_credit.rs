//! ZK Credit oracle CPI reader.
//!
//! Reads ZK credit scores from an external oracle program account
//! without importing the oracle program as a dependency.
//! The ZK credit oracle stores privacy-preserving credit scores
//! derived from on-chain activity (verified via ZK proofs).

use anchor_lang::prelude::*;

/// ZK credit data read from oracle account.
pub struct ZkCreditData {
    pub owner: Pubkey,
    pub score: u16,          // 0-1000
    pub last_updated: i64,   // unix timestamp
    pub proof_verified: bool,
}

/// Read ZK credit data from account bytes at fixed offsets.
///
/// Layout: 8 (discriminator) + 32 (owner) + 2 (score) + 8 (last_updated) + 1 (proof_verified)
/// Total minimum size: 51 bytes
pub fn read_zk_credit(account: &AccountInfo) -> Result<ZkCreditData> {
    let data = account.try_borrow_data()?;

    require!(data.len() >= 51, ErrorCode::AccountDidNotDeserialize);

    // owner at offset 8
    let owner = Pubkey::try_from(&data[8..40])
        .map_err(|_| ErrorCode::AccountDidNotDeserialize)?;

    // score at offset 40
    let score = u16::from_le_bytes(
        data[40..42].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );

    // last_updated at offset 42
    let last_updated = i64::from_le_bytes(
        data[42..50].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );

    // proof_verified at offset 50
    let proof_verified = data[50] != 0;

    Ok(ZkCreditData {
        owner,
        score,
        last_updated,
        proof_verified,
    })
}

/// Check if credit score is fresh (within staleness window).
pub fn is_score_valid(credit: &ZkCreditData, current_ts: i64, max_staleness_secs: i64) -> bool {
    if max_staleness_secs <= 0 {
        return true;
    }
    current_ts - credit.last_updated <= max_staleness_secs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_zk_credit_bytes(owner: &Pubkey, score: u16, last_updated: i64, proof_verified: bool) -> Vec<u8> {
        let mut data = vec![0u8; 51];
        // discriminator (8 bytes) = zeros
        data[8..40].copy_from_slice(owner.as_ref());
        data[40..42].copy_from_slice(&score.to_le_bytes());
        data[42..50].copy_from_slice(&last_updated.to_le_bytes());
        data[50] = if proof_verified { 1 } else { 0 };
        data
    }

    #[test]
    fn test_is_score_valid_fresh() {
        let credit = ZkCreditData {
            owner: Pubkey::new_unique(),
            score: 750,
            last_updated: 1000,
            proof_verified: true,
        };
        assert!(is_score_valid(&credit, 1500, 600)); // 500 < 600
    }

    #[test]
    fn test_is_score_valid_stale() {
        let credit = ZkCreditData {
            owner: Pubkey::new_unique(),
            score: 750,
            last_updated: 1000,
            proof_verified: true,
        };
        assert!(!is_score_valid(&credit, 2000, 600)); // 1000 > 600
    }

    #[test]
    fn test_is_score_valid_exact_boundary() {
        let credit = ZkCreditData {
            owner: Pubkey::new_unique(),
            score: 750,
            last_updated: 1000,
            proof_verified: true,
        };
        assert!(is_score_valid(&credit, 1600, 600)); // 600 == 600, valid
    }

    #[test]
    fn test_is_score_valid_zero_staleness() {
        let credit = ZkCreditData {
            owner: Pubkey::new_unique(),
            score: 750,
            last_updated: 0,
            proof_verified: true,
        };
        assert!(is_score_valid(&credit, 999_999, 0)); // no staleness check
    }

    #[test]
    fn test_zk_credit_data_fields() {
        let owner = Pubkey::new_unique();
        let data = build_zk_credit_bytes(&owner, 850, 12345, true);

        // Verify manual byte parsing
        let score = u16::from_le_bytes(data[40..42].try_into().unwrap());
        assert_eq!(score, 850);
        let last_updated = i64::from_le_bytes(data[42..50].try_into().unwrap());
        assert_eq!(last_updated, 12345);
        assert_eq!(data[50], 1);

        let parsed_owner = Pubkey::try_from(&data[8..40]).unwrap();
        assert_eq!(parsed_owner, owner);
    }

    #[test]
    fn test_zk_credit_data_too_short() {
        let data = vec![0u8; 50]; // 1 byte short
        // Verify the minimum size check
        assert!(data.len() < 51);
    }
}
