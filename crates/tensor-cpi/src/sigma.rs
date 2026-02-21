//! Read-only adapters for Sigma shared-oracle accounts.
//!
//! Instead of importing the full sigma program (which would drag in its
//! entire dependency tree), we define minimal structs that match the
//! on-chain layout and deserialize directly from account data.

use anchor_lang::prelude::*;

/// Sigma shared-oracle program ID
pub const SIGMA_ORACLE_PROGRAM_ID: &str = "SgmaHBxiGbVj9sFBFQhRMgij3aApxVZbGtfCjRPB3Nk";

// ---------------------------------------------------------------------------
// PriceFeed — minimal reader
// ---------------------------------------------------------------------------

/// Reads price data from a Sigma PriceFeed account.
///
/// On-chain layout (after 8-byte Anchor discriminator):
///   authority: Pubkey (32)
///   asset_symbol: String (4 + up to 16)
///   asset_mint: Pubkey (32)
///   pyth_feed: Option<Pubkey> (1 + 32)
///   sample_interval_seconds: u64 (8)
///   max_samples: u16 (2)
///   sample_count: u16 (2)
///   last_sample_time: i64 (8)
///   last_price: u64 (8)
///   twap: u64 (8)
///   ema: u64 (8)
///   current_variance: u64 (8)
///   period_high: u64 (8)
///   period_low: u64 (8)
///   created_at: i64 (8)
///   is_active: bool (1)
///   max_staleness_seconds: u64 (8)
///   bump: u8 (1)
pub struct OraclePriceData {
    pub last_price: u64,
    pub twap: u64,
    pub ema: u64,
    pub current_variance: u64,
    pub last_sample_time: i64,
    pub is_active: bool,
}

/// Reads a Sigma PriceFeed account and extracts key price data.
///
/// The caller must pass the PriceFeed account as an `AccountInfo`.
/// This function borrows the data and reads fields at known offsets.
pub fn read_price_feed(account: &AccountInfo) -> Result<OraclePriceData> {
    let data = account.try_borrow_data()?;

    // We need at least enough bytes for all fields through is_active
    require!(data.len() >= 200, ErrorCode::AccountDidNotDeserialize);

    // Walk the Borsh layout:
    // Discriminator: 8
    // authority: 32
    // asset_symbol String: 4 (len) + variable bytes (up to 16)
    // We need to read the string length to find the actual offset

    let str_len = u32::from_le_bytes(
        data[40..44].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    ) as usize;

    // Base offset after string
    let mut off = 44 + str_len;

    // asset_mint: 32
    off += 32;

    // pyth_feed: Option<Pubkey> — 1 byte tag + optional 32 bytes
    let has_pyth = data[off];
    off += 1;
    if has_pyth == 1 {
        off += 32;
    }

    // sample_interval_seconds: 8
    off += 8;

    // max_samples: 2, sample_count: 2
    off += 4;

    // last_sample_time: i64 (8)
    let last_sample_time = i64::from_le_bytes(
        data[off..off + 8].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    off += 8;

    // last_price: u64 (8)
    let last_price = u64::from_le_bytes(
        data[off..off + 8].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    off += 8;

    // twap: u64 (8)
    let twap = u64::from_le_bytes(
        data[off..off + 8].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    off += 8;

    // ema: u64 (8)
    let ema = u64::from_le_bytes(
        data[off..off + 8].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    off += 8;

    // current_variance: u64 (8)
    let current_variance = u64::from_le_bytes(
        data[off..off + 8].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    off += 8;

    // period_high: 8, period_low: 8, created_at: 8
    off += 24;

    // is_active: bool (1)
    let is_active = data[off] != 0;

    Ok(OraclePriceData {
        last_price,
        twap,
        ema,
        current_variance,
        last_sample_time,
        is_active,
    })
}

// ---------------------------------------------------------------------------
// VarianceTracker — minimal reader
// ---------------------------------------------------------------------------

pub struct VarianceData {
    pub current_epoch_variance: u64,
    pub current_epoch: u64,
    pub epoch_start_time: i64,
}

/// Reads variance data from a Sigma VarianceTracker account.
///
/// On-chain layout (after 8-byte discriminator):
///   price_feed: Pubkey (32)
///   authority: Pubkey (32)
///   current_epoch: u64 (8)
///   epoch_duration_seconds: u64 (8)
///   epoch_start_time: i64 (8)
///   current_epoch_variance: u64 (8) ← offset 96
pub fn read_variance_tracker(account: &AccountInfo) -> Result<VarianceData> {
    let data = account.try_borrow_data()?;
    require!(data.len() >= 104, ErrorCode::AccountDidNotDeserialize);

    let current_epoch = u64::from_le_bytes(
        data[72..80].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let epoch_start_time = i64::from_le_bytes(
        data[88..96].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let current_epoch_variance = u64::from_le_bytes(
        data[96..104].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );

    Ok(VarianceData {
        current_epoch_variance,
        current_epoch,
        epoch_start_time,
    })
}

// ---------------------------------------------------------------------------
// FundingFeed — minimal reader
// ---------------------------------------------------------------------------

pub struct FundingData {
    pub current_rate_bps: i64,
    pub cumulative_funding: i128,
    pub last_update: i64,
}

/// Reads funding rate data from a Sigma FundingFeed account.
///
/// On-chain layout (after 8-byte discriminator):
///   authority: Pubkey (32)
///   market_symbol: String (4 + up to 16)
///   current_rate_bps: i64 (8)
///   cumulative_funding: i128 (16)
///   funding_interval_seconds: u64 (8)
///   last_update: i64 (8)
pub fn read_funding_feed(account: &AccountInfo) -> Result<FundingData> {
    let data = account.try_borrow_data()?;
    require!(data.len() >= 100, ErrorCode::AccountDidNotDeserialize);

    // Discriminator(8) + authority(32) + string_len(4) = 44
    let str_len = u32::from_le_bytes(
        data[40..44].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    ) as usize;

    let mut off = 44 + str_len;

    // current_rate_bps: i64
    let current_rate_bps = i64::from_le_bytes(
        data[off..off + 8].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    off += 8;

    // cumulative_funding: i128
    let cumulative_funding = i128::from_le_bytes(
        data[off..off + 16].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    off += 16;

    // funding_interval_seconds: u64 (skip)
    off += 8;

    // last_update: i64
    let last_update = i64::from_le_bytes(
        data[off..off + 8].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );

    Ok(FundingData {
        current_rate_bps,
        cumulative_funding,
        last_update,
    })
}
