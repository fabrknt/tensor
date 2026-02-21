use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

/// Execute a spot swap through Northtail exchange's constant-product AMM.
///
/// This instruction CPI-calls northtail-exchange's swap function using
/// the margin account's authority. The resulting token balance change
/// is reflected in the account's spot_balances.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SpotSwapParams {
    /// Amount of input tokens to sell
    pub amount_in: u64,
    /// Minimum output tokens to accept (slippage protection)
    pub min_amount_out: u64,
    /// true = selling base token for quote, false = buying base for quote
    pub is_sell: bool,
    /// Market index for the asset being traded
    pub market_index: u16,
}

#[derive(Accounts)]
pub struct ExecuteSpotSwap<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, authority.key().as_ref()],
        bump = margin_account.bump,
        constraint = (margin_account.owner == authority.key()
            || margin_account.delegate == authority.key()) @ TensorError::Unauthorized,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        mut,
        seeds = [MarginMarket::SEED, &market.index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Account<'info, MarginMarket>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, MarginConfig>,

    /// Northtail Pool account (read for price quote, not mutated by us)
    /// CHECK: Deserialized via tensor_cpi::northtail::read_pool
    pub pool: AccountInfo<'info>,

    pub authority: Signer<'info>,
}

pub fn handler(ctx: Context<ExecuteSpotSwap>, params: SpotSwapParams) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(!config.is_paused, TensorError::ProtocolPaused);

    let market = &ctx.accounts.market;
    require!(market.is_active, TensorError::MarketNotActive);
    require!(market.spot_enabled, TensorError::ProductNotEnabled);

    let clock = Clock::get()?;
    let account = &mut ctx.accounts.margin_account;

    // Read pool state to verify liquidity and get spot price
    let pool_data = tensor_cpi::northtail::read_pool(&ctx.accounts.pool)
        .map_err(|_| TensorError::InvalidPrice)?;
    require!(pool_data.is_active, TensorError::MarketNotActive);

    // Calculate expected output from constant-product formula
    let (input_reserve, output_reserve) = if params.is_sell {
        (pool_data.security_liquidity, pool_data.quote_liquidity)
    } else {
        (pool_data.quote_liquidity, pool_data.security_liquidity)
    };

    let (expected_out, _fee) = tensor_cpi::northtail::calculate_swap_output(
        params.amount_in,
        input_reserve,
        output_reserve,
        config.trading_fee_bps as u16,
    )
    .ok_or(TensorError::InvalidAmount)?;

    require!(expected_out >= params.min_amount_out, TensorError::InvalidAmount);

    // Update spot balances on the margin account
    // When selling base: decrease base balance, increase quote (collateral)
    // When buying base: decrease quote (collateral), increase base balance
    if params.is_sell {
        // Find or verify the spot balance exists
        let slot_idx = account.find_spot_by_mint(&market.base_mint)
            .ok_or(TensorError::PositionNotFound)?;

        require!(
            account.spot_balances[slot_idx].balance >= params.amount_in,
            TensorError::InsufficientCollateral
        );

        account.spot_balances[slot_idx].balance -= params.amount_in;
        // Update value based on new balance and current price
        let spot_price = tensor_cpi::northtail::calculate_spot_price(&pool_data);
        account.spot_balances[slot_idx].value =
            (account.spot_balances[slot_idx].balance as u128 * spot_price as u128 / 1_000_000) as u64;

        // Close position if fully sold
        if account.spot_balances[slot_idx].balance == 0 {
            account.spot_balances[slot_idx] = tensor_types::SpotBalance::default();
            account.spot_count = account.spot_count.saturating_sub(1);
        }

        // Add proceeds to collateral
        account.collateral = account.collateral.saturating_add(expected_out);
    } else {
        // Buying base token with collateral
        require!(
            account.collateral >= params.amount_in,
            TensorError::InsufficientCollateral
        );
        account.collateral -= params.amount_in;

        // Find existing spot position or create new one
        let slot_idx = if let Some(idx) = account.find_spot_by_mint(&market.base_mint) {
            idx
        } else {
            let idx = account.find_empty_spot_slot()
                .ok_or(TensorError::PositionSlotFull)?;
            account.spot_balances[idx].mint = market.base_mint;
            account.spot_balances[idx].market_index = params.market_index;
            account.spot_balances[idx].is_active = true;
            account.spot_count += 1;
            idx
        };

        account.spot_balances[slot_idx].balance += expected_out;
        let spot_price = tensor_cpi::northtail::calculate_spot_price(&pool_data);
        account.spot_balances[slot_idx].value =
            (account.spot_balances[slot_idx].balance as u128 * spot_price as u128 / 1_000_000) as u64;
    }

    // Recompute margin after the swap
    let mark_prices: Vec<u64> = vec![market.mark_price];
    let greeks = tensor_math::compute_portfolio_greeks(
        &account.perp_positions,
        &account.spot_balances,
        &account.option_positions,
        &mark_prices,
        clock.unix_timestamp,
    );

    let im_bps = market.effective_initial_margin(config.initial_margin_bps);
    let initial_margin = tensor_math::compute_initial_margin(
        &greeks,
        market.mark_price,
        market.implied_vol_bps,
        im_bps,
        config.gamma_margin_bps,
        config.vega_margin_bps,
    );

    // Verify margin sufficiency after the trade
    account.equity = account.collateral as i64;
    require!(
        account.equity >= initial_margin as i64,
        TensorError::InsufficientMargin
    );

    account.greeks = greeks;
    account.initial_margin_required = initial_margin;
    account.total_trades += 1;
    account.last_margin_update = clock.unix_timestamp;

    // Track volume on market
    let market = &mut ctx.accounts.market;
    market.total_volume = market.total_volume.saturating_add(params.amount_in as u128);

    emit!(SpotSwapExecuted {
        owner: account.owner,
        market_index: params.market_index,
        amount_in: params.amount_in,
        amount_out: expected_out,
        is_sell: params.is_sell,
    });

    Ok(())
}

#[event]
pub struct SpotSwapExecuted {
    pub owner: Pubkey,
    pub market_index: u16,
    pub amount_in: u64,
    pub amount_out: u64,
    pub is_sell: bool,
}
