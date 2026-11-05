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
/// `liquidate` refuses to step past spot while any spot balance is left, so
/// lending collateral is only seized once spot has been sold.
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
}
