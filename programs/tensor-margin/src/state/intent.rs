use anchor_lang::prelude::*;
use tensor_types::*;

pub const MAX_ACTIVE_INTENTS: u8 = 4;

#[account]
#[derive(InitSpace)]
pub struct IntentAccount {
    pub margin_account: Pubkey,
    pub intent_id: u64,
    pub intent_type: IntentType,
    pub status: IntentStatus,
    pub legs: [IntentLeg; MAX_INTENT_LEGS],
    pub leg_count: u8,
    pub filled_legs: u8,
    // Constraints
    pub max_slippage_bps: u16,
    pub min_fill_ratio_bps: u16,
    pub deadline: i64,
    pub max_total_cost: u64,
    // Tracking
    pub total_margin_used: u64,
    pub created_at: i64,
    pub updated_at: i64,
    pub bump: u8,
}

impl IntentAccount {
    pub const SEED: &'static [u8] = b"intent";

    pub fn is_expired(&self, current_ts: i64) -> bool {
        self.deadline > 0 && current_ts > self.deadline
    }

    pub fn fill_ratio_bps(&self) -> u16 {
        if self.leg_count == 0 {
            return 0;
        }
        ((self.filled_legs as u32 * 10000) / self.leg_count as u32) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_intent_is_expired_before_deadline() {
        let intent = IntentAccount {
            margin_account: Pubkey::default(),
            intent_id: 0,
            intent_type: IntentType::default(),
            status: IntentStatus::default(),
            legs: [IntentLeg::default(); MAX_INTENT_LEGS],
            leg_count: 0,
            filled_legs: 0,
            max_slippage_bps: 0,
            min_fill_ratio_bps: 0,
            deadline: 1000,
            max_total_cost: 0,
            total_margin_used: 0,
            created_at: 0,
            updated_at: 0,
            bump: 0,
        };
        assert!(!intent.is_expired(500));
    }

    #[test]
    fn test_intent_is_expired_after_deadline() {
        let intent = IntentAccount {
            margin_account: Pubkey::default(),
            intent_id: 0,
            intent_type: IntentType::default(),
            status: IntentStatus::default(),
            legs: [IntentLeg::default(); MAX_INTENT_LEGS],
            leg_count: 0,
            filled_legs: 0,
            max_slippage_bps: 0,
            min_fill_ratio_bps: 0,
            deadline: 1000,
            max_total_cost: 0,
            total_margin_used: 0,
            created_at: 0,
            updated_at: 0,
            bump: 0,
        };
        assert!(intent.is_expired(1500));
    }

    #[test]
    fn test_intent_is_expired_zero_deadline() {
        let intent = IntentAccount {
            margin_account: Pubkey::default(),
            intent_id: 0,
            intent_type: IntentType::default(),
            status: IntentStatus::default(),
            legs: [IntentLeg::default(); MAX_INTENT_LEGS],
            leg_count: 0,
            filled_legs: 0,
            max_slippage_bps: 0,
            min_fill_ratio_bps: 0,
            deadline: 0,
            max_total_cost: 0,
            total_margin_used: 0,
            created_at: 0,
            updated_at: 0,
            bump: 0,
        };
        assert!(!intent.is_expired(999_999));
    }

    #[test]
    fn test_fill_ratio_bps_none_filled() {
        let intent = IntentAccount {
            margin_account: Pubkey::default(),
            intent_id: 0,
            intent_type: IntentType::default(),
            status: IntentStatus::default(),
            legs: [IntentLeg::default(); MAX_INTENT_LEGS],
            leg_count: 4,
            filled_legs: 0,
            max_slippage_bps: 0,
            min_fill_ratio_bps: 0,
            deadline: 0,
            max_total_cost: 0,
            total_margin_used: 0,
            created_at: 0,
            updated_at: 0,
            bump: 0,
        };
        assert_eq!(intent.fill_ratio_bps(), 0);
    }

    #[test]
    fn test_fill_ratio_bps_partial() {
        let intent = IntentAccount {
            margin_account: Pubkey::default(),
            intent_id: 0,
            intent_type: IntentType::default(),
            status: IntentStatus::default(),
            legs: [IntentLeg::default(); MAX_INTENT_LEGS],
            leg_count: 4,
            filled_legs: 2,
            max_slippage_bps: 0,
            min_fill_ratio_bps: 0,
            deadline: 0,
            max_total_cost: 0,
            total_margin_used: 0,
            created_at: 0,
            updated_at: 0,
            bump: 0,
        };
        assert_eq!(intent.fill_ratio_bps(), 5000); // 50%
    }

    #[test]
    fn test_fill_ratio_bps_all_filled() {
        let intent = IntentAccount {
            margin_account: Pubkey::default(),
            intent_id: 0,
            intent_type: IntentType::default(),
            status: IntentStatus::default(),
            legs: [IntentLeg::default(); MAX_INTENT_LEGS],
            leg_count: 4,
            filled_legs: 4,
            max_slippage_bps: 0,
            min_fill_ratio_bps: 0,
            deadline: 0,
            max_total_cost: 0,
            total_margin_used: 0,
            created_at: 0,
            updated_at: 0,
            bump: 0,
        };
        assert_eq!(intent.fill_ratio_bps(), 10000); // 100%
    }

    #[test]
    fn test_fill_ratio_bps_zero_legs() {
        let intent = IntentAccount {
            margin_account: Pubkey::default(),
            intent_id: 0,
            intent_type: IntentType::default(),
            status: IntentStatus::default(),
            legs: [IntentLeg::default(); MAX_INTENT_LEGS],
            leg_count: 0,
            filled_legs: 0,
            max_slippage_bps: 0,
            min_fill_ratio_bps: 0,
            deadline: 0,
            max_total_cost: 0,
            total_margin_used: 0,
            created_at: 0,
            updated_at: 0,
            bump: 0,
        };
        assert_eq!(intent.fill_ratio_bps(), 0);
    }
}
