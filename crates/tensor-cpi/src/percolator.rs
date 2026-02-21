//! Percolator matcher ABI types and CPI helpers.
//!
//! Percolator uses an inverted CPI model: it calls INTO matcher programs
//! rather than being called BY them. Tensor can act as a matcher that
//! percolator calls, or orchestrate trades by building percolator
//! instructions for end-users.
//!
//! Key ABI:
//! - Matcher call: 67 bytes (sent by percolator to matcher)
//! - Matcher return: 64-byte prefix in context account (written by matcher)
//! - Context account: 320 bytes total, owned by matcher program

/// Matcher ABI version (must equal 1)
pub const MATCHER_ABI_VERSION: u32 = 1;

/// Size of the matcher context account
pub const MATCHER_CONTEXT_LEN: usize = 320;

/// Size of the matcher call instruction data
pub const MATCHER_CALL_LEN: usize = 67;

/// Matcher call tag byte
pub const MATCHER_CALL_TAG: u8 = 0;

/// Return data flags
pub const FLAG_VALID: u32 = 1;
pub const FLAG_PARTIAL_OK: u32 = 2;
pub const FLAG_REJECTED: u32 = 4;

// ---------------------------------------------------------------------------
// Matcher Call (67 bytes, sent by percolator)
// ---------------------------------------------------------------------------

/// Decoded matcher call from percolator.
pub struct MatcherCall {
    pub req_id: u64,
    pub lp_idx: u16,
    pub lp_account_id: u64,
    pub oracle_price_e6: u64,
    pub req_size: i128,
}

impl MatcherCall {
    /// Decode a matcher call from instruction data.
    pub fn decode(data: &[u8]) -> Option<Self> {
        if data.len() < MATCHER_CALL_LEN {
            return None;
        }
        if data[0] != MATCHER_CALL_TAG {
            return None;
        }

        Some(Self {
            req_id: u64::from_le_bytes(data[1..9].try_into().ok()?),
            lp_idx: u16::from_le_bytes(data[9..11].try_into().ok()?),
            lp_account_id: u64::from_le_bytes(data[11..19].try_into().ok()?),
            oracle_price_e6: u64::from_le_bytes(data[19..27].try_into().ok()?),
            req_size: i128::from_le_bytes(data[27..43].try_into().ok()?),
        })
    }
}

// ---------------------------------------------------------------------------
// Matcher Return (64-byte prefix, written to context account)
// ---------------------------------------------------------------------------

/// Matcher return data to write to context account bytes [0..64].
pub struct MatcherReturn {
    pub abi_version: u32,
    pub flags: u32,
    pub exec_price_e6: u64,
    pub exec_size: i128,
    pub req_id: u64,
    pub lp_account_id: u64,
    pub oracle_price_e6: u64,
    pub reserved: u64,
}

impl MatcherReturn {
    /// Encode the return data to write to the context account.
    pub fn encode(&self) -> [u8; 64] {
        let mut buf = [0u8; 64];
        buf[0..4].copy_from_slice(&self.abi_version.to_le_bytes());
        buf[4..8].copy_from_slice(&self.flags.to_le_bytes());
        buf[8..16].copy_from_slice(&self.exec_price_e6.to_le_bytes());
        buf[16..32].copy_from_slice(&self.exec_size.to_le_bytes());
        buf[32..40].copy_from_slice(&self.req_id.to_le_bytes());
        buf[40..48].copy_from_slice(&self.lp_account_id.to_le_bytes());
        buf[48..56].copy_from_slice(&self.oracle_price_e6.to_le_bytes());
        buf[56..64].copy_from_slice(&self.reserved.to_le_bytes());
        buf
    }

    /// Create a valid acceptance response echoing the call fields.
    pub fn accept(
        call: &MatcherCall,
        exec_price_e6: u64,
        exec_size: i128,
    ) -> Self {
        Self {
            abi_version: MATCHER_ABI_VERSION,
            flags: FLAG_VALID,
            exec_price_e6,
            exec_size,
            req_id: call.req_id,
            lp_account_id: call.lp_account_id,
            oracle_price_e6: call.oracle_price_e6,
            reserved: 0,
        }
    }

    /// Create a partial-fill acceptance (exec_size can be less than req_size).
    pub fn partial(
        call: &MatcherCall,
        exec_price_e6: u64,
        exec_size: i128,
    ) -> Self {
        Self {
            abi_version: MATCHER_ABI_VERSION,
            flags: FLAG_VALID | FLAG_PARTIAL_OK,
            exec_price_e6,
            exec_size,
            req_id: call.req_id,
            lp_account_id: call.lp_account_id,
            oracle_price_e6: call.oracle_price_e6,
            reserved: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Percolator instruction builders
// ---------------------------------------------------------------------------

/// Build instruction data for percolator's TradeCpi instruction.
///
/// Percolator instruction enum tag for TradeCpi = 8 (varies by build).
/// The actual tag must match the deployed percolator program.
pub struct TradeCpiParams {
    pub user_idx: u16,
    pub lp_idx: u16,
    pub size: i128,
}

/// Slab account PDA seeds for percolator.
/// Percolator slabs are raw accounts (no PDA derivation — allocated directly).
/// The slab account key is stored in the percolator config.

/// LP PDA derivation: ["lp", slab_key, lp_idx_le_bytes]
pub fn derive_lp_pda(
    program_id: &anchor_lang::prelude::Pubkey,
    slab_key: &anchor_lang::prelude::Pubkey,
    lp_idx: u16,
) -> (anchor_lang::prelude::Pubkey, u8) {
    anchor_lang::prelude::Pubkey::find_program_address(
        &[b"lp", slab_key.as_ref(), &lp_idx.to_le_bytes()],
        program_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_matcher_call(
        req_id: u64,
        lp_idx: u16,
        lp_account_id: u64,
        oracle_price_e6: u64,
        req_size: i128,
    ) -> Vec<u8> {
        let mut data = vec![0u8; MATCHER_CALL_LEN];
        data[0] = MATCHER_CALL_TAG;
        data[1..9].copy_from_slice(&req_id.to_le_bytes());
        data[9..11].copy_from_slice(&lp_idx.to_le_bytes());
        data[11..19].copy_from_slice(&lp_account_id.to_le_bytes());
        data[19..27].copy_from_slice(&oracle_price_e6.to_le_bytes());
        data[27..43].copy_from_slice(&req_size.to_le_bytes());
        data
    }

    #[test]
    fn test_matcher_call_decode() {
        let data = build_matcher_call(42, 7, 123, 150_000_000, -5_000_000);
        let call = MatcherCall::decode(&data).unwrap();

        assert_eq!(call.req_id, 42);
        assert_eq!(call.lp_idx, 7);
        assert_eq!(call.lp_account_id, 123);
        assert_eq!(call.oracle_price_e6, 150_000_000);
        assert_eq!(call.req_size, -5_000_000);
    }

    #[test]
    fn test_matcher_call_wrong_tag() {
        let mut data = build_matcher_call(1, 0, 0, 0, 0);
        data[0] = 1; // wrong tag
        assert!(MatcherCall::decode(&data).is_none());
    }

    #[test]
    fn test_matcher_call_too_short() {
        let data = vec![0u8; 10]; // way too short
        assert!(MatcherCall::decode(&data).is_none());
    }

    #[test]
    fn test_matcher_return_encode_decode() {
        let ret = MatcherReturn {
            abi_version: MATCHER_ABI_VERSION,
            flags: FLAG_VALID,
            exec_price_e6: 150_500_000,
            exec_size: 3_000_000,
            req_id: 42,
            lp_account_id: 123,
            oracle_price_e6: 150_000_000,
            reserved: 0,
        };

        let buf = ret.encode();
        assert_eq!(buf.len(), 64);

        // Verify fields
        assert_eq!(u32::from_le_bytes(buf[0..4].try_into().unwrap()), MATCHER_ABI_VERSION);
        assert_eq!(u32::from_le_bytes(buf[4..8].try_into().unwrap()), FLAG_VALID);
        assert_eq!(u64::from_le_bytes(buf[8..16].try_into().unwrap()), 150_500_000);
        assert_eq!(i128::from_le_bytes(buf[16..32].try_into().unwrap()), 3_000_000);
        assert_eq!(u64::from_le_bytes(buf[32..40].try_into().unwrap()), 42);
        assert_eq!(u64::from_le_bytes(buf[40..48].try_into().unwrap()), 123);
        assert_eq!(u64::from_le_bytes(buf[48..56].try_into().unwrap()), 150_000_000);
    }

    #[test]
    fn test_accept_echoes_call_fields() {
        let data = build_matcher_call(99, 3, 456, 200_000_000, 10_000_000);
        let call = MatcherCall::decode(&data).unwrap();

        let ret = MatcherReturn::accept(&call, 200_100_000, 10_000_000);
        assert_eq!(ret.abi_version, MATCHER_ABI_VERSION);
        assert_eq!(ret.flags, FLAG_VALID);
        assert_eq!(ret.req_id, 99);
        assert_eq!(ret.lp_account_id, 456);
        assert_eq!(ret.oracle_price_e6, 200_000_000);
        assert_eq!(ret.exec_price_e6, 200_100_000);
        assert_eq!(ret.exec_size, 10_000_000);
    }

    #[test]
    fn test_partial_has_correct_flags() {
        let data = build_matcher_call(1, 0, 0, 100_000_000, 50_000_000);
        let call = MatcherCall::decode(&data).unwrap();

        let ret = MatcherReturn::partial(&call, 100_000_000, 25_000_000);
        assert_eq!(ret.flags, FLAG_VALID | FLAG_PARTIAL_OK);
        assert_eq!(ret.exec_size, 25_000_000); // partial fill
    }

    #[test]
    fn test_lp_pda_deterministic() {
        let program = anchor_lang::prelude::Pubkey::new_unique();
        let slab = anchor_lang::prelude::Pubkey::new_unique();

        let (pda1, bump1) = derive_lp_pda(&program, &slab, 0);
        let (pda2, bump2) = derive_lp_pda(&program, &slab, 0);

        assert_eq!(pda1, pda2);
        assert_eq!(bump1, bump2);
    }

    #[test]
    fn test_lp_pda_different_indices() {
        let program = anchor_lang::prelude::Pubkey::new_unique();
        let slab = anchor_lang::prelude::Pubkey::new_unique();

        let (pda0, _) = derive_lp_pda(&program, &slab, 0);
        let (pda1, _) = derive_lp_pda(&program, &slab, 1);

        assert_ne!(pda0, pda1);
    }

    #[test]
    fn test_constants() {
        assert_eq!(MATCHER_ABI_VERSION, 1);
        assert_eq!(MATCHER_CONTEXT_LEN, 320);
        assert_eq!(MATCHER_CALL_LEN, 67);
        assert_eq!(MATCHER_CALL_TAG, 0);
        assert_eq!(FLAG_VALID, 1);
        assert_eq!(FLAG_PARTIAL_OK, 2);
        assert_eq!(FLAG_REJECTED, 4);
    }
}
