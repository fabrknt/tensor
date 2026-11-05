//! Read-only adapters for Sigma shared-oracle accounts.
//!
//! Instead of importing the full sigma program (which would drag in its
//! entire dependency tree), we define minimal structs that match the
//! on-chain layout and deserialize directly from account data.

use anchor_lang::prelude::*;

/// Sigma shared-oracle program ID (sigma/programs/shared-oracle declare_id!)
pub const SIGMA_ORACLE_PROGRAM_ID: &str = "81SjyEmtwJUeqU9ZEfc7sm9evVpDuGwRSLrFQeCF4j5o";

/// Sigma shared-oracle program ID as a Pubkey
pub const SIGMA_ORACLE_PROGRAM: Pubkey =
    anchor_lang::pubkey!("81SjyEmtwJUeqU9ZEfc7sm9evVpDuGwRSLrFQeCF4j5o");

/// Seed prefix of a Sigma FundingFeed PDA: ["funding_feed", market_symbol]
pub const FUNDING_FEED_SEED: &[u8] = b"funding_feed";

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
///   switchboard_feed: Option<Pubkey> (1 + 32)
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
    require!(str_len <= 16, ErrorCode::AccountDidNotDeserialize);

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

    // switchboard_feed: Option<Pubkey> — 1 byte tag + optional 32 bytes
    let has_switchboard = data[off];
    off += 1;
    if has_switchboard == 1 {
        off += 32;
    }

    // Remaining fields through is_active: 8 + 4 + 8 * 5 + 24 + 1 = 77 bytes
    require!(data.len() >= off + 77, ErrorCode::AccountDidNotDeserialize);

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
    pub is_active: bool,
}

/// Max entries in FundingFeed.rate_history (sigma `#[max_len(168)]`)
pub const FUNDING_HISTORY_MAX: usize = 168;

/// Reads funding rate data from a Sigma FundingFeed account.
///
/// On-chain layout (after 8-byte discriminator):
///   authority: Pubkey (32)
///   market_symbol: String (4 + up to 16)
///   current_rate_bps: i64 (8)
///   cumulative_funding: i128 (16)
///   funding_interval_seconds: u64 (8)
///   last_update: i64 (8)
///   total_periods: u64 (8)
///   rate_history: Vec<FundingRateSample> (4 + 16 * up to 168)
///   is_active: bool (1)
///   bump: u8 (1)
pub fn read_funding_feed(account: &AccountInfo) -> Result<FundingData> {
    let data = account.try_borrow_data()?;
    require!(data.len() >= 100, ErrorCode::AccountDidNotDeserialize);

    // Discriminator(8) + authority(32) + string_len(4) = 44
    let str_len = u32::from_le_bytes(
        data[40..44].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    ) as usize;
    require!(str_len <= 16, ErrorCode::AccountDidNotDeserialize);

    let mut off = 44 + str_len;
    // current_rate_bps, cumulative_funding, funding_interval_seconds,
    // last_update, total_periods, rate_history length: 8 + 16 + 8 + 8 + 8 + 4
    require!(data.len() >= off + 52, ErrorCode::AccountDidNotDeserialize);

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
    off += 8;

    // total_periods: u64 (skip)
    off += 8;

    // rate_history: Vec<FundingRateSample { rate_bps: i64, timestamp: i64 }>
    let history_len = u32::from_le_bytes(
        data[off..off + 4].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    ) as usize;
    require!(history_len <= FUNDING_HISTORY_MAX, ErrorCode::AccountDidNotDeserialize);
    off += 4 + 16 * history_len;

    // is_active: bool
    require!(data.len() > off, ErrorCode::AccountDidNotDeserialize);
    let is_active = data[off] != 0;

    Ok(FundingData {
        current_rate_bps,
        cumulative_funding,
        last_update,
        is_active,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PriceFeed bytes in the sigma shared-oracle layout.
    fn price_feed_bytes(symbol: &str, pyth: bool, switchboard: bool, price: u64, active: bool) -> Vec<u8> {
        let mut d = vec![0u8; 8];
        d.extend_from_slice(&[1u8; 32]); // authority
        d.extend_from_slice(&(symbol.len() as u32).to_le_bytes());
        d.extend_from_slice(symbol.as_bytes());
        d.extend_from_slice(&[2u8; 32]); // asset_mint
        for present in [pyth, switchboard] {
            d.push(present as u8);
            if present {
                d.extend_from_slice(&[3u8; 32]);
            }
        }
        d.extend_from_slice(&60u64.to_le_bytes()); // sample_interval_seconds
        d.extend_from_slice(&100u16.to_le_bytes()); // max_samples
        d.extend_from_slice(&5u16.to_le_bytes()); // sample_count
        d.extend_from_slice(&1_000i64.to_le_bytes()); // last_sample_time
        d.extend_from_slice(&price.to_le_bytes()); // last_price
        d.extend_from_slice(&(price + 1).to_le_bytes()); // twap
        d.extend_from_slice(&(price + 2).to_le_bytes()); // ema
        d.extend_from_slice(&42u64.to_le_bytes()); // current_variance
        d.extend_from_slice(&[0u8; 24]); // period_high, period_low, created_at
        d.push(active as u8);
        d.extend_from_slice(&300u64.to_le_bytes()); // max_staleness_seconds
        d.push(255); // bump
        d.resize(d.len().max(200), 0);
        d
    }

    fn read(data: &mut [u8]) -> Result<OraclePriceData> {
        let key = Pubkey::new_unique();
        let owner = SIGMA_ORACLE_PROGRAM;
        let mut lamports = 0u64;
        let ai = AccountInfo::new(&key, false, false, &mut lamports, data, &owner, false, 0);
        read_price_feed(&ai)
    }

    #[test]
    fn reads_price_feed_with_and_without_optional_feeds() {
        for (pyth, sb) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut d = price_feed_bytes("SOL", pyth, sb, 150_000_000, true);
            let p = read(&mut d).unwrap();
            assert_eq!(p.last_sample_time, 1_000);
            assert_eq!(p.last_price, 150_000_000);
            assert_eq!(p.twap, 150_000_001);
            assert_eq!(p.ema, 150_000_002);
            assert_eq!(p.current_variance, 42);
            assert!(p.is_active);
        }
    }

    fn funding_feed_bytes(symbol: &str, history: u32, active: bool) -> Vec<u8> {
        let mut d = vec![0u8; 8];
        d.extend_from_slice(&[1u8; 32]); // authority
        d.extend_from_slice(&(symbol.len() as u32).to_le_bytes());
        d.extend_from_slice(symbol.as_bytes());
        d.extend_from_slice(&(-7i64).to_le_bytes()); // current_rate_bps
        d.extend_from_slice(&99i128.to_le_bytes()); // cumulative_funding
        d.extend_from_slice(&3_600u64.to_le_bytes()); // funding_interval_seconds
        d.extend_from_slice(&500i64.to_le_bytes()); // last_update
        d.extend_from_slice(&2u64.to_le_bytes()); // total_periods
        d.extend_from_slice(&history.to_le_bytes());
        for _ in 0..history {
            d.extend_from_slice(&[9u8; 16]);
        }
        d.push(active as u8);
        d.push(254); // bump
        d.resize(d.len().max(100), 0);
        d
    }

    fn read_funding(data: &mut [u8]) -> Result<FundingData> {
        let key = Pubkey::new_unique();
        let owner = SIGMA_ORACLE_PROGRAM;
        let mut lamports = 0u64;
        let ai = AccountInfo::new(&key, false, false, &mut lamports, data, &owner, false, 0);
        read_funding_feed(&ai)
    }

    #[test]
    fn reads_funding_feed_and_is_active() {
        for (history, active) in [(0, true), (3, false), (168, true)] {
            let mut d = funding_feed_bytes("SOL-PERP", history, active);
            let f = read_funding(&mut d).unwrap();
            assert_eq!(f.current_rate_bps, -7);
            assert_eq!(f.cumulative_funding, 99);
            assert_eq!(f.last_update, 500);
            assert_eq!(f.is_active, active);
        }
    }

    #[test]
    fn rejects_bad_funding_feed_lengths() {
        let mut d = funding_feed_bytes("SOL-PERP", 0, true);
        d[40..44].copy_from_slice(&1_000u32.to_le_bytes());
        assert!(read_funding(&mut d).is_err());

        let mut d = funding_feed_bytes("SOL-PERP", 169, true);
        assert!(read_funding(&mut d).is_err());
    }

    #[test]
    fn rejects_oversized_symbol() {
        let mut d = price_feed_bytes("SOL", false, false, 1, true);
        d[40..44].copy_from_slice(&1_000u32.to_le_bytes());
        assert!(read(&mut d).is_err());
    }
}
