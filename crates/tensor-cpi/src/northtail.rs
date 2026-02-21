//! Read-only adapter for Northtail exchange accounts.
//!
//! Reads Pool state for spot price, TWAP, and liquidity data.
//! Also defines the CPI accounts struct for calling swap().

use anchor_lang::prelude::*;

/// Northtail exchange program ID
pub const EXCHANGE_PROGRAM_ID: &str = "2NueLoUpXxihu4AT6JkEBTHkqpZE8xDgS14ZHX1qPJ1H";

/// Northtail collateral program ID
pub const COLLATERAL_PROGRAM_ID: &str = "4qq4ZPNoLQHzxLZ1SijTsWKgmXmLvED6PXochMTP7EdT";

/// Pool PDA seed prefix
pub const POOL_SEED: &[u8] = b"pool";
/// Market PDA seed prefix
pub const MARKET_SEED: &[u8] = b"market";
/// Pool authority PDA seed prefix
pub const POOL_AUTHORITY_SEED: &[u8] = b"pool_authority";

pub const PRICE_PRECISION: u128 = 1_000_000;

pub struct PoolData {
    pub market: Pubkey,
    pub security_liquidity: u64,
    pub quote_liquidity: u64,
    pub lp_supply: u64,
    pub twap: u64,
    pub twap_last_update: i64,
    pub is_active: bool,
}

/// Reads a Northtail Pool account.
///
/// On-chain layout (after 8-byte discriminator):
///   market: Pubkey (32)
///   security_liquidity: u64 (8)
///   quote_liquidity: u64 (8)
///   lp_mint: Pubkey (32)
///   lp_supply: u64 (8)
///   authority: Pubkey (32)
///   security_vault: Pubkey (32)
///   quote_vault: Pubkey (32)
///   accumulated_fees_security: u64 (8)
///   accumulated_fees_quote: u64 (8)
///   twap: u64 (8)
///   twap_last_update: i64 (8)
///   cumulative_price: u128 (16)
///   k_last: u128 (16)
///   is_active: bool (1)
pub fn read_pool(account: &AccountInfo) -> Result<PoolData> {
    let data = account.try_borrow_data()?;
    require!(data.len() >= 250, ErrorCode::AccountDidNotDeserialize);

    let market = Pubkey::try_from(&data[8..40])
        .map_err(|_| ErrorCode::AccountDidNotDeserialize)?;

    let security_liquidity = u64::from_le_bytes(
        data[40..48].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let quote_liquidity = u64::from_le_bytes(
        data[48..56].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    // skip lp_mint (32)
    let lp_supply = u64::from_le_bytes(
        data[88..96].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    // skip authority (32), security_vault (32), quote_vault (32)
    // skip accumulated_fees_security (8), accumulated_fees_quote (8)
    // offset = 96 + 32 + 32 + 32 + 8 + 8 = 208
    let twap = u64::from_le_bytes(
        data[208..216].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    let twap_last_update = i64::from_le_bytes(
        data[216..224].try_into().map_err(|_| ErrorCode::AccountDidNotDeserialize)?,
    );
    // skip cumulative_price (16), k_last (16)
    // offset = 224 + 16 + 16 = 256
    let is_active = data[256] != 0;

    Ok(PoolData {
        market,
        security_liquidity,
        quote_liquidity,
        lp_supply,
        twap,
        twap_last_update,
        is_active,
    })
}

/// Calculate spot price from pool reserves: quote / security * 1e6
pub fn calculate_spot_price(pool: &PoolData) -> u64 {
    if pool.security_liquidity == 0 {
        return 0;
    }
    (pool.quote_liquidity as u128 * PRICE_PRECISION / pool.security_liquidity as u128) as u64
}

/// Calculate AMM output for a swap (constant-product with fee).
///
/// Returns (output_amount, fee_amount)
pub fn calculate_swap_output(
    input_amount: u64,
    input_reserve: u64,
    output_reserve: u64,
    fee_bps: u16,
) -> Option<(u64, u64)> {
    if input_reserve == 0 || output_reserve == 0 {
        return None;
    }
    let fee = (input_amount as u128 * fee_bps as u128) / 10_000;
    let input_with_fee = input_amount as u128 - fee;
    let numerator = input_with_fee * output_reserve as u128;
    let denominator = input_reserve as u128 + input_with_fee;
    Some(((numerator / denominator) as u64, fee as u64))
}

// ---------------------------------------------------------------------------
// Swap CPI instruction data
// ---------------------------------------------------------------------------

/// Build the instruction data for northtail-exchange `swap`.
///
/// Anchor discriminator for "swap" = first 8 bytes of sha256("global:swap")
pub fn build_swap_ix_data(amount_in: u64, min_amount_out: u64, is_security_input: bool) -> Vec<u8> {
    // Anchor discriminator: sha256("global:swap")[0..8]
    let discriminator: [u8; 8] = anchor_lang::solana_program::hash::hash(b"global:swap").to_bytes()[..8]
        .try_into()
        .unwrap();

    let mut data = Vec::with_capacity(8 + 8 + 8 + 1);
    data.extend_from_slice(&discriminator);
    data.extend_from_slice(&amount_in.to_le_bytes());
    data.extend_from_slice(&min_amount_out.to_le_bytes());
    data.push(if is_security_input { 1 } else { 0 });
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Spot price
    // -----------------------------------------------------------------------

    #[test]
    fn test_spot_price_basic() {
        let pool = PoolData {
            market: Pubkey::default(),
            security_liquidity: 1_000_000, // 1 SOL
            quote_liquidity: 150_000_000,  // $150
            lp_supply: 1_000_000,
            twap: 0,
            twap_last_update: 0,
            is_active: true,
        };
        // price = 150_000_000 * 1_000_000 / 1_000_000 = 150_000_000
        assert_eq!(calculate_spot_price(&pool), 150_000_000);
    }

    #[test]
    fn test_spot_price_zero_security() {
        let pool = PoolData {
            market: Pubkey::default(),
            security_liquidity: 0,
            quote_liquidity: 150_000_000,
            lp_supply: 0,
            twap: 0,
            twap_last_update: 0,
            is_active: true,
        };
        assert_eq!(calculate_spot_price(&pool), 0);
    }

    #[test]
    fn test_spot_price_large_pool() {
        let pool = PoolData {
            market: Pubkey::default(),
            security_liquidity: 10_000_000_000, // 10000 SOL
            quote_liquidity: 1_500_000_000_000, // $1.5M
            lp_supply: 1_000_000,
            twap: 0,
            twap_last_update: 0,
            is_active: true,
        };
        // price = 1.5T * 1e6 / 10B = 150_000_000
        assert_eq!(calculate_spot_price(&pool), 150_000_000);
    }

    // -----------------------------------------------------------------------
    // Swap output (constant-product)
    // -----------------------------------------------------------------------

    #[test]
    fn test_swap_output_basic() {
        // Pool: 1000 SOL / $150,000
        // Swap 10 SOL in, 30 bps fee
        let result = calculate_swap_output(
            10_000_000,       // 10 SOL
            1_000_000_000,    // 1000 SOL reserve
            150_000_000_000,  // $150k reserve
            30,               // 30 bps
        );
        let (output, fee) = result.unwrap();

        // Fee: 10_000_000 * 30 / 10000 = 30_000
        assert_eq!(fee, 30_000);
        // After fee: 10_000_000 - 30_000 = 9_970_000
        // Output: 9_970_000 * 150B / (1B + 9_970_000)
        //       = 1_495_500_000_000_000 / 1_009_970_000 ≈ 1_480_758_641
        assert!(output > 0);
        assert!(output < 1_500_000_000); // must be less than full price
    }

    #[test]
    fn test_swap_output_zero_fee() {
        let result = calculate_swap_output(
            1_000_000,
            100_000_000,
            100_000_000,
            0,
        );
        let (output, fee) = result.unwrap();
        assert_eq!(fee, 0);
        // With equal reserves and no fee: output = 1M * 100M / (100M + 1M) ≈ 990_099
        assert!(output > 0);
        assert!(output < 1_000_000);
    }

    #[test]
    fn test_swap_output_zero_reserve() {
        assert_eq!(calculate_swap_output(1_000, 0, 100_000, 30), None);
        assert_eq!(calculate_swap_output(1_000, 100_000, 0, 30), None);
    }

    #[test]
    fn test_swap_output_small_amount() {
        let result = calculate_swap_output(1, 100_000_000, 100_000_000, 30);
        let (output, _fee) = result.unwrap();
        // Very small swap should still produce output (or 0 due to rounding)
        assert!(output <= 1);
    }

    #[test]
    fn test_swap_output_preserves_k_approximately() {
        // Constant product: (x + dx) * (y - dy) ≈ x * y
        let x = 1_000_000_000u64;
        let y = 150_000_000_000u64;
        let k_before = x as u128 * y as u128;

        let dx = 10_000_000u64;
        let (dy, _fee) = calculate_swap_output(dx, x, y, 0).unwrap();

        let k_after = (x as u128 + dx as u128) * (y as u128 - dy as u128);
        // k should be preserved (with no fee)
        assert!(k_after >= k_before); // k_after >= k_before due to integer rounding
    }

    // -----------------------------------------------------------------------
    // Swap IX data
    // -----------------------------------------------------------------------

    #[test]
    fn test_build_swap_ix_data_format() {
        let data = build_swap_ix_data(100, 50, true);
        assert_eq!(data.len(), 8 + 8 + 8 + 1); // discriminator + 2 u64 + 1 bool

        // Check amount_in at offset 8
        let amount = u64::from_le_bytes(data[8..16].try_into().unwrap());
        assert_eq!(amount, 100);

        // Check min_amount_out at offset 16
        let min_out = u64::from_le_bytes(data[16..24].try_into().unwrap());
        assert_eq!(min_out, 50);

        // Check is_security_input at offset 24
        assert_eq!(data[24], 1);
    }

    #[test]
    fn test_build_swap_ix_data_sell() {
        let data = build_swap_ix_data(1000, 900, false);
        assert_eq!(data[24], 0); // is_security_input = false
    }
}
