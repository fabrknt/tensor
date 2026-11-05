use anchor_lang::prelude::*;
use anchor_spl::token::{Token, TokenAccount};
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;
use super::execute_spot_swap::swap_via_northtail;

/// Largest discount to the market mark price a spot liquidation may sell at.
/// The minimum output is computed on chain from `market.mark_price`, so the
/// (permissionless) liquidator cannot choose a worse price than this.
pub const MAX_SPOT_LIQUIDATION_SLIPPAGE_BPS: u64 = 500;

/// Spot step of the liquidation waterfall.
///
/// `liquidate` handles options, perps and lending, which are pure book entries.
/// Spot balances are real tokens held in the protocol base vault, so they are
/// sold here through the same northtail swap CPI that `execute_spot_swap`
/// uses, and the account is credited with the quote that actually arrived.
/// `liquidate` refuses to step past spot while a spot balance can still be
/// sold here; it moves on to lending only when every remaining spot balance
/// is shown to be unsellable (see `spot_is_sellable`). Unsellable balances
/// stay on the books untouched: nothing is written off or credited.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct LiquidateSpotParams {
    /// Base tokens to sell (at most the account's balance for market.base_mint).
    /// A partial amount lets the liquidator split a large sale.
    pub amount_in: u64,
    /// Optional extra floor on the output; the on-chain floor from the mark
    /// price always applies.
    pub min_amount_out: u64,
}

#[derive(Accounts)]
pub struct LiquidateSpot<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, margin_account.owner.as_ref()],
        bump = margin_account.bump,
    )]
    pub margin_account: Box<Account<'info, MarginAccount>>,

    #[account(
        mut,
        seeds = [MarginMarket::SEED, &market.index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, MarginMarket>>,

    #[account(
        mut,
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Box<Account<'info, MarginConfig>>,

    /// CHECK: vault_authority PDA; owns both protocol vaults and signs the swap
    #[account(
        mut,
        seeds = [MarginConfig::VAULT_AUTHORITY_SEED],
        bump,
    )]
    pub vault_authority: AccountInfo<'info>,

    /// Protocol collateral vault (quote side of the pool)
    #[account(
        mut,
        address = config.collateral_vault() @ TensorError::AccountMismatch,
    )]
    pub collateral_vault: Box<Account<'info, TokenAccount>>,

    /// Protocol base vault: the vault_authority ATA for market.base_mint
    #[account(
        mut,
        address = anchor_spl::associated_token::get_associated_token_address(
            &vault_authority.key(),
            &market.base_mint,
        ) @ TensorError::AccountMismatch,
    )]
    pub base_vault: Box<Account<'info, TokenAccount>>,

    /// CHECK: Address derived from stored mints; validated again by northtail
    #[account(
        mut,
        address = tensor_cpi::northtail::market_address(&market.base_mint, &config.collateral_mint) @ TensorError::AccountMismatch,
    )]
    pub exchange_market: AccountInfo<'info>,

    /// CHECK: northtail-exchange pool for (market.base_mint, config.collateral_mint)
    #[account(
        mut,
        owner = tensor_cpi::northtail::EXCHANGE_PROGRAM @ TensorError::AccountMismatch,
        address = tensor_cpi::northtail::pool_address(&market.base_mint, &config.collateral_mint) @ TensorError::AccountMismatch,
    )]
    pub pool: AccountInfo<'info>,

    /// CHECK: Northtail pool authority PDA; northtail signs for it
    pub pool_authority: AccountInfo<'info>,

    /// CHECK: Northtail pool security vault; checked by northtail (pool.has_one)
    #[account(mut)]
    pub pool_security_vault: AccountInfo<'info>,

    /// CHECK: Northtail pool quote vault; checked by northtail (pool.has_one)
    #[account(mut)]
    pub pool_quote_vault: AccountInfo<'info>,

    /// CHECK: northtail-exchange program
    #[account(address = tensor_cpi::northtail::EXCHANGE_PROGRAM @ TensorError::AccountMismatch)]
    pub exchange_program: AccountInfo<'info>,

    pub token_program: Program<'info, Token>,

    #[account(mut)]
    pub liquidator: Signer<'info>,
}

/// On-chain output floor: amount_in * mark_price / 1e6, less the max slippage.
pub fn spot_liquidation_min_out(amount_in: u64, mark_price: u64) -> u64 {
    let fair = amount_in as u128 * mark_price as u128 / 1_000_000;
    let floor = fair * (10_000 - MAX_SPOT_LIQUIDATION_SLIPPAGE_BPS) as u128 / 10_000;
    u64::try_from(floor).unwrap_or(u64::MAX)
}

/// Whether `liquidate_spot` can, structurally, sell some of `balance`: the
/// tensor market is active with a mark price, the northtail market is
/// trading, the pool is active, the balance reaches northtail's
/// min_trade_size, and some legal trade size exists. (Balances above
/// max_trade_size are sellable in chunks.)
///
/// The pool price is deliberately not considered: reserves can be moved
/// within the same transaction, so a price test here would let anyone skip
/// sellable spot. When the pool trades below the floor the spot step waits
/// for the mark price to catch up instead.
pub fn spot_is_sellable(
    tensor_market: &MarginMarket,
    nt_market: &tensor_cpi::northtail::MarketData,
    pool: &tensor_cpi::northtail::PoolData,
    balance: u64,
) -> bool {
    if !tensor_market.is_active || tensor_market.mark_price == 0 {
        return false;
    }
    if !nt_market.is_trading() || !pool.is_active {
        return false;
    }
    let chunk = nt_market.min_trade_size.max(1);
    if balance < chunk {
        return false;
    }
    // No legal trade size at all (max below min)
    if nt_market.max_trade_size > 0 && nt_market.max_trade_size < chunk {
        return false;
    }
    true
}

/// Used by `liquidate` when the waterfall reaches spot. `remaining` holds,
/// for every active spot slot in slot order, the accounts
/// [MarginMarket for slot.market_index, northtail Market, northtail Pool].
/// Returns true only when none of those balances is sellable.
pub(crate) fn all_spot_unsellable(
    remaining: &[AccountInfo],
    account: &MarginAccount,
    collateral_mint: &Pubkey,
) -> Result<bool> {
    let mut i = 0usize;
    for slot in account.spot_balances.iter().filter(|s| s.is_active) {
        let group = remaining
            .get(i..i + 3)
            .ok_or(TensorError::SpotLiquidationRequired)?;
        i += 3;

        let (mm_info, nt_market_info, pool_info) = (&group[0], &group[1], &group[2]);
        require_keys_eq!(*mm_info.owner, crate::ID, TensorError::AccountMismatch);
        let tensor_market = {
            let data = mm_info.try_borrow_data()?;
            Box::new(MarginMarket::try_deserialize(&mut &data[..])?)
        };
        require!(
            tensor_market.index == slot.market_index && tensor_market.base_mint == slot.mint,
            TensorError::AccountMismatch
        );

        require_keys_eq!(
            *nt_market_info.owner,
            tensor_cpi::northtail::EXCHANGE_PROGRAM,
            TensorError::AccountMismatch
        );
        require_keys_eq!(
            nt_market_info.key(),
            tensor_cpi::northtail::market_address(&slot.mint, collateral_mint),
            TensorError::AccountMismatch
        );
        require_keys_eq!(*pool_info.owner, tensor_cpi::northtail::EXCHANGE_PROGRAM, TensorError::AccountMismatch);
        require_keys_eq!(
            pool_info.key(),
            tensor_cpi::northtail::pool_address(&slot.mint, collateral_mint),
            TensorError::AccountMismatch
        );
        let nt_market = tensor_cpi::northtail::read_market(nt_market_info)
            .map_err(|_| TensorError::AccountMismatch)?;
        let pool = tensor_cpi::northtail::read_pool(pool_info)
            .map_err(|_| TensorError::AccountMismatch)?;

        if spot_is_sellable(&tensor_market, &nt_market, &pool, slot.balance) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub fn handler(ctx: Context<LiquidateSpot>, params: LiquidateSpotParams) -> Result<()> {
    let market = &ctx.accounts.market;
    require!(market.is_active, TensorError::MarketNotActive);
    require!(market.mark_price > 0, TensorError::InvalidPrice);
    require!(params.amount_in > 0, TensorError::InvalidAmount);

    let clock = Clock::get()?;

    let slot_idx = {
        let account = &ctx.accounts.margin_account;
        require!(
            matches!(account.health, AccountHealth::Liquidatable | AccountHealth::Bankrupt),
            TensorError::AccountHealthy
        );
        // Follow the waterfall: options and perps go first (via `liquidate`).
        let priority = tensor_math::liquidation_priority(
            &account.perp_positions[..],
            &account.spot_balances[..],
            &account.option_positions[..],
            &account.lending_positions[..],
            clock.unix_timestamp,
        );
        require!(
            priority == Some(ProductType::Spot),
            TensorError::LiquidationOrderViolation
        );
        let idx = account
            .find_spot_by_mint(&market.base_mint)
            .ok_or(TensorError::PositionNotFound)?;
        // The floor is taken from this market, so it must be the slot's market.
        require!(
            account.spot_balances[idx].market_index == market.index,
            TensorError::AccountMismatch
        );
        require!(
            account.spot_balances[idx].balance >= params.amount_in,
            TensorError::InvalidAmount
        );
        idx
    };

    let pool_data = tensor_cpi::northtail::read_pool(&ctx.accounts.pool)
        .map_err(|_| TensorError::InvalidPrice)?;
    require!(pool_data.is_active, TensorError::MarketNotActive);

    let min_out = spot_liquidation_min_out(params.amount_in, market.mark_price)
        .max(params.min_amount_out);

    let amount_out = swap_via_northtail(
        &ctx.accounts.vault_authority,
        ctx.bumps.vault_authority,
        &ctx.accounts.exchange_market,
        &ctx.accounts.pool,
        &ctx.accounts.pool_authority,
        &ctx.accounts.pool_security_vault,
        &ctx.accounts.pool_quote_vault,
        &mut ctx.accounts.base_vault,
        &mut ctx.accounts.collateral_vault,
        &ctx.accounts.token_program.to_account_info(),
        &ctx.accounts.exchange_program,
        params.amount_in,
        min_out,
        true,
    )?;

    let mark_price = ctx.accounts.market.mark_price;
    let liquidation_fee_bps = ctx.accounts.config.liquidation_fee_bps;
    let account = &mut ctx.accounts.margin_account;

    account.spot_balances[slot_idx].balance -= params.amount_in;
    if account.spot_balances[slot_idx].balance == 0 {
        account.spot_balances[slot_idx] = SpotBalance::default();
        account.spot_count = account.spot_count.saturating_sub(1);
    } else {
        account.spot_balances[slot_idx].value = (account.spot_balances[slot_idx].balance as u128
            * mark_price as u128
            / 1_000_000) as u64;
    }

    account.collateral = account
        .collateral
        .checked_add(amount_out)
        .ok_or(TensorError::MathOverflow)?;

    // Same fee split as `liquidate`, on the quote actually received.
    let fee = tensor_math::liquidation_fee(amount_out, liquidation_fee_bps);
    let insurance_portion = fee / 2;
    let liquidator_portion = fee - insurance_portion;

    account.collateral = account.collateral.saturating_sub(fee);
    account.equity = account.collateral as i64;
    account.last_margin_update = clock.unix_timestamp;
    let owner = account.owner;

    let config = &mut ctx.accounts.config;
    config.insurance_fund = config.insurance_fund.saturating_add(insurance_portion);

    let market = &mut ctx.accounts.market;
    market.total_volume = market.total_volume.saturating_add(params.amount_in as u128);

    emit!(super::liquidate::AccountLiquidated {
        owner,
        liquidator: ctx.accounts.liquidator.key(),
        product: ProductType::Spot,
        liquidated_notional: amount_out,
        fee,
        liquidator_reward: liquidator_portion,
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn min_out_is_mark_less_slippage() {
        // 10 base units at $150 (1e6 precision) = 1500 quote; 5% below = 1425.
        assert_eq!(spot_liquidation_min_out(10_000_000, 150_000_000), 1_425_000_000);
        assert_eq!(spot_liquidation_min_out(1, 1), 0);
        assert_eq!(spot_liquidation_min_out(u64::MAX, u64::MAX), u64::MAX);
    }

    use tensor_cpi::northtail::{MarketData, PoolData};

    fn nt_market(min: u64) -> MarketData {
        MarketData {
            security_mint: Pubkey::default(),
            quote_mint: Pubkey::default(),
            status: 0,
            trading_fee_bps: 30,
            min_trade_size: min,
            max_trade_size: 0,
            is_active: true,
        }
    }

    /// Pool at $150 (1e6 scaled), deep enough for small chunks.
    fn pool(price: u64) -> PoolData {
        let security = 1_000_000_000_000u64;
        PoolData {
            market: Pubkey::default(),
            security_liquidity: security,
            quote_liquidity: (security as u128 * price as u128 / 1_000_000) as u64,
            lp_supply: 1,
            twap: 0,
            twap_last_update: 0,
            is_active: true,
        }
    }

    #[test]
    fn sellable_when_everything_is_open() {
        let m = crate::state::market::tests::make_market(); // mark 150e6
        assert!(spot_is_sellable(&m, &nt_market(1_000), &pool(150_000_000), 5_000));
    }

    #[test]
    fn dust_below_min_trade_size_is_unsellable() {
        let m = crate::state::market::tests::make_market();
        assert!(!spot_is_sellable(&m, &nt_market(1_000), &pool(150_000_000), 999));
    }

    #[test]
    fn inactive_or_halted_is_unsellable() {
        let mut m = crate::state::market::tests::make_market();
        let mut nt = nt_market(1);
        nt.status = 1; // paused
        assert!(!spot_is_sellable(&m, &nt, &pool(150_000_000), 5_000));
        let mut p = pool(150_000_000);
        p.is_active = false;
        assert!(!spot_is_sellable(&m, &nt_market(1), &p, 5_000));
        m.is_active = false;
        assert!(!spot_is_sellable(&m, &nt_market(1), &pool(150_000_000), 5_000));
    }

    #[test]
    fn max_below_min_is_unsellable() {
        let m = crate::state::market::tests::make_market();
        let mut nt = nt_market(1_000);
        nt.max_trade_size = 999;
        assert!(!spot_is_sellable(&m, &nt, &pool(150_000_000), 5_000));
        nt.max_trade_size = 1_000; // larger balances sell in chunks
        assert!(spot_is_sellable(&m, &nt, &pool(150_000_000), 5_000));
    }

    #[test]
    fn pool_price_does_not_make_spot_unsellable() {
        let m = crate::state::market::tests::make_market(); // mark 150
        // Far below the floor: still sellable, the spot step waits instead
        assert!(spot_is_sellable(&m, &nt_market(1_000), &pool(100_000_000), 5_000));
        let mut zero_mark = crate::state::market::tests::make_market();
        zero_mark.mark_price = 0;
        assert!(!spot_is_sellable(&zero_mark, &nt_market(1_000), &pool(150_000_000), 5_000));
    }
}
