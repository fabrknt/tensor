use anchor_lang::prelude::*;

pub const MAX_SOLVERS: usize = 16;
pub const MAX_BIDS_PER_INTENT: usize = 8;

/// Registry of approved solvers that can execute intents.
#[account]
#[derive(InitSpace)]
pub struct SolverRegistry {
    /// Admin who can add/remove solvers
    pub authority: Pubkey,

    /// Registered solvers
    pub solvers: [SolverEntry; MAX_SOLVERS],

    /// Number of active solvers
    pub solver_count: u8,

    /// PDA bump
    pub bump: u8,
}

impl SolverRegistry {
    pub const SEED: &'static [u8] = b"solver_registry";
    pub const VAULT_SEED: &'static [u8] = b"solver_vault";
    pub const VAULT_AUTHORITY_SEED: &'static [u8] = b"solver_vault_authority";

    /// The solver stake vault: the associated token account of the
    /// solver_vault_authority PDA for the collateral mint.
    pub fn stake_vault(collateral_mint: &Pubkey) -> Pubkey {
        let (authority, _) = Pubkey::find_program_address(&[Self::VAULT_AUTHORITY_SEED], &crate::ID);
        anchor_spl::associated_token::get_associated_token_address(&authority, collateral_mint)
    }

    /// Find a solver entry by pubkey. Returns the index if found and active.
    pub fn find_solver(&self, solver: &Pubkey) -> Option<usize> {
        self.solvers
            .iter()
            .position(|s| s.is_active && s.solver == *solver)
    }

    /// Find an empty slot for a new solver.
    pub fn find_empty_slot(&self) -> Option<usize> {
        self.solvers.iter().position(|s| !s.is_active)
    }

    /// Check if a pubkey is a registered and active solver.
    pub fn is_registered(&self, solver: &Pubkey) -> bool {
        self.find_solver(solver).is_some()
    }

    /// Deregister a solver: mark inactive and decrement count.
    /// Returns the solver's remaining stake.
    pub fn deregister(&mut self, solver: &Pubkey) -> Option<u64> {
        if let Some(idx) = self.find_solver(solver) {
            let remaining = self.solvers[idx].stake;
            self.solvers[idx].is_active = false;
            self.solvers[idx].stake = 0;
            self.solver_count = self.solver_count.saturating_sub(1);
            Some(remaining)
        } else {
            None
        }
    }

    /// Apply a slash to a solver. Returns the slash amount actually applied.
    /// If remaining stake drops below `min_stake`, the solver is deactivated.
    pub fn slash(&mut self, solver: &Pubkey, slash_rate_bps: u16, min_stake: u64) -> Option<u64> {
        if let Some(idx) = self.find_solver(solver) {
            let entry = &mut self.solvers[idx];
            let slash_amount = (entry.stake as u128)
                .checked_mul(slash_rate_bps as u128)
                .unwrap_or(0)
                / 10_000;
            let slash_amount = slash_amount as u64;
            entry.stake = entry.stake.saturating_sub(slash_amount);
            entry.slash_count = entry.slash_count.saturating_add(1);
            if entry.stake < min_stake {
                entry.is_active = false;
                self.solver_count = self.solver_count.saturating_sub(1);
            }
            Some(slash_amount)
        } else {
            None
        }
    }
}

/// A registered solver's on-chain record.
#[derive(Clone, Copy, Default, Debug, AnchorSerialize, AnchorDeserialize, InitSpace)]
pub struct SolverEntry {
    /// Solver's signing authority
    pub solver: Pubkey,
    /// Stake deposited (in collateral units)
    pub stake: u64,
    /// Lifetime fills completed
    pub total_fills: u64,
    /// Lifetime volume executed (1e6 scaled)
    pub total_volume: u128,
    /// Number of times slashed
    pub slash_count: u16,
    /// Whether this solver is currently active
    pub is_active: bool,
    /// Registration timestamp
    pub registered_at: i64,
}

/// A solver's bid on an intent leg during the auction period.
#[derive(Clone, Copy, Default, Debug, AnchorSerialize, AnchorDeserialize, InitSpace)]
pub struct SolverBid {
    /// Solver pubkey
    pub solver: Pubkey,
    /// The price the solver commits to fill at (1e6 precision)
    pub bid_price: u64,
    /// Timestamp of bid submission
    pub bid_timestamp: i64,
    /// Whether this bid is still active
    pub is_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solver_registry_find_empty() {
        let registry = SolverRegistry {
            authority: Pubkey::default(),
            solvers: [SolverEntry::default(); MAX_SOLVERS],
            solver_count: 0,
            bump: 0,
        };
        assert_eq!(registry.find_empty_slot(), Some(0));
    }

    #[test]
    fn test_solver_registry_find_solver() {
        let mut registry = SolverRegistry {
            authority: Pubkey::default(),
            solvers: [SolverEntry::default(); MAX_SOLVERS],
            solver_count: 1,
            bump: 0,
        };
        let solver_key = Pubkey::new_unique();
        registry.solvers[0] = SolverEntry {
            solver: solver_key,
            stake: 1_000_000,
            is_active: true,
            ..Default::default()
        };
        assert_eq!(registry.find_solver(&solver_key), Some(0));
        assert!(registry.is_registered(&solver_key));
        assert!(!registry.is_registered(&Pubkey::new_unique()));
    }

    #[test]
    fn test_deregister_solver_clears_entry() {
        let mut registry = SolverRegistry {
            authority: Pubkey::default(),
            solvers: [SolverEntry::default(); MAX_SOLVERS],
            solver_count: 1,
            bump: 0,
        };
        let solver_key = Pubkey::new_unique();
        registry.solvers[0] = SolverEntry {
            solver: solver_key,
            stake: 1_000_000,
            is_active: true,
            ..Default::default()
        };
        assert!(registry.is_registered(&solver_key));

        let returned = registry.deregister(&solver_key);
        assert_eq!(returned, Some(1_000_000));
        assert!(!registry.is_registered(&solver_key));
        assert!(!registry.solvers[0].is_active);
        assert_eq!(registry.solvers[0].stake, 0);
        assert_eq!(registry.solver_count, 0);

        // Deregistering again returns None
        assert_eq!(registry.deregister(&solver_key), None);
    }

    #[test]
    fn test_slash_reduces_stake() {
        let mut registry = SolverRegistry {
            authority: Pubkey::default(),
            solvers: [SolverEntry::default(); MAX_SOLVERS],
            solver_count: 1,
            bump: 0,
        };
        let solver_key = Pubkey::new_unique();
        registry.solvers[0] = SolverEntry {
            solver: solver_key,
            stake: 1_000_000,
            is_active: true,
            ..Default::default()
        };

        // Slash 10% (1000 bps)
        let slash_amount = registry.slash(&solver_key, 1000, 500_000);
        assert_eq!(slash_amount, Some(100_000));
        assert_eq!(registry.solvers[0].stake, 900_000);
        assert_eq!(registry.solvers[0].slash_count, 1);
        assert!(registry.solvers[0].is_active); // still above min

        // Slash again: 10% of 900_000 = 90_000 -> 810_000, still active
        let slash_amount = registry.slash(&solver_key, 1000, 500_000);
        assert_eq!(slash_amount, Some(90_000));
        assert_eq!(registry.solvers[0].stake, 810_000);
        assert_eq!(registry.solvers[0].slash_count, 2);
        assert!(registry.solvers[0].is_active);
    }

    #[test]
    fn test_slash_deactivates_below_min() {
        let mut registry = SolverRegistry {
            authority: Pubkey::default(),
            solvers: [SolverEntry::default(); MAX_SOLVERS],
            solver_count: 1,
            bump: 0,
        };
        let solver_key = Pubkey::new_unique();
        registry.solvers[0] = SolverEntry {
            solver: solver_key,
            stake: 500_000,
            is_active: true,
            ..Default::default()
        };

        // Slash 50% (5000 bps): 250_000 remaining < min of 500_000
        let slash_amount = registry.slash(&solver_key, 5000, 500_000);
        assert_eq!(slash_amount, Some(250_000));
        assert_eq!(registry.solvers[0].stake, 250_000);
        assert!(!registry.solvers[0].is_active);
        assert_eq!(registry.solver_count, 0);
    }

    #[test]
    fn test_solver_registry_full() {
        let mut registry = SolverRegistry {
            authority: Pubkey::default(),
            solvers: [SolverEntry::default(); MAX_SOLVERS],
            solver_count: MAX_SOLVERS as u8,
            bump: 0,
        };
        for s in registry.solvers.iter_mut() {
            s.is_active = true;
            s.solver = Pubkey::new_unique();
        }
        assert_eq!(registry.find_empty_slot(), None);
    }
}
