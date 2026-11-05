use anchor_lang::prelude::*;
use anchor_lang::solana_program::instruction::{AccountMeta, Instruction};
use anchor_lang::solana_program::program::invoke_signed;
use anchor_spl::token::{Token, TokenAccount};
use crate::state::*;
use crate::errors::TensorError;

/// Execute a spot swap through Northtail exchange's constant-product AMM.
///
/// The swap is a real CPI into northtail-exchange `swap`, signed by the
/// vault_authority PDA: the input leaves the protocol vault (collateral vault
/// or the base vault for market.base_mint) and the output lands in the other
/// one. The margin account books are updated by the token amounts that
/// actually moved, so they stay in line with the vault balances.
#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SpotSwapParams {
    /// Amount of input tokens to sell
    pub amount_in: u64,
    /// Minimum output tokens to accept (slippage protection)
    pub min_amount_out: u64,
    /// true = selling base token for quote, false = buying base for quote
    pub is_sell: bool,
    /// Market index for the asset being traded; must equal market.index
    pub market_index: u16,
}

#[derive(Accounts)]
pub struct ExecuteSpotSwap<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, authority.key().as_ref()],
        bump = margin_account.bump,
        constraint = margin_account.owner == authority.key() @ TensorError::Unauthorized,
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

    /// Northtail Market for (market.base_mint, config.collateral_mint)
    /// CHECK: Address derived from stored mints; validated again by northtail
    #[account(
        mut,
        address = tensor_cpi::northtail::market_address(&market.base_mint, &config.collateral_mint) @ TensorError::AccountMismatch,
    )]
    pub exchange_market: AccountInfo<'info>,

    /// Northtail Pool account for that market
    /// CHECK: Must be the northtail-exchange pool for (market.base_mint, config.collateral_mint);
    /// deserialized via tensor_cpi::northtail::read_pool
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

    pub authority: Signer<'info>,
}

pub fn handler(ctx: Context<ExecuteSpotSwap>, params: SpotSwapParams) -> Result<()> {
    let config = &ctx.accounts.config;
    require!(!config.is_paused, TensorError::ProtocolPaused);

    let market = &ctx.accounts.market;
    require!(market.is_active, TensorError::MarketNotActive);
    require!(market.spot_enabled, TensorError::ProductNotEnabled);
    require!(params.market_index == market.index, TensorError::AccountMismatch);
    require!(params.amount_in > 0, TensorError::InvalidAmount);

    let clock = Clock::get()?;

    // Check the account can pay the input from its own books before moving tokens
    {
        let account = &ctx.accounts.margin_account;
        if params.is_sell {
            let slot_idx = account.find_spot_by_mint(&market.base_mint)
                .ok_or(TensorError::PositionNotFound)?;
            require!(
                account.spot_balances[slot_idx].balance >= params.amount_in,
                TensorError::InsufficientCollateral
            );
        } else {
            require!(
                account.collateral >= params.amount_in,
                TensorError::InsufficientCollateral
            );
        }
    }

    let pool_data = tensor_cpi::northtail::read_pool(&ctx.accounts.pool)
        .map_err(|_| TensorError::InvalidPrice)?;
    require!(pool_data.is_active, TensorError::MarketNotActive);

    // Output balance before the swap
    let out_before = if params.is_sell {
        ctx.accounts.collateral_vault.amount
    } else {
        ctx.accounts.base_vault.amount
    };

    // CPI: northtail-exchange swap, signed by vault_authority.
    // Accounts follow northtail's Swap context; oracle_feed is omitted (None).
    let exchange_program = ctx.accounts.exchange_program.key();
    let ix = Instruction {
        program_id: exchange_program,
        accounts: vec![
            AccountMeta::new(ctx.accounts.vault_authority.key(), true),
            AccountMeta::new(ctx.accounts.exchange_market.key(), false),
            AccountMeta::new(ctx.accounts.pool.key(), false),
            AccountMeta::new_readonly(ctx.accounts.pool_authority.key(), false),
            AccountMeta::new(ctx.accounts.pool_security_vault.key(), false),
            AccountMeta::new(ctx.accounts.pool_quote_vault.key(), false),
            AccountMeta::new(ctx.accounts.base_vault.key(), false),
            AccountMeta::new(ctx.accounts.collateral_vault.key(), false),
            AccountMeta::new_readonly(ctx.accounts.token_program.key(), false),
            AccountMeta::new_readonly(exchange_program, false),
        ],
        data: tensor_cpi::northtail::build_swap_ix_data(
            params.amount_in,
            params.min_amount_out,
            params.is_sell,
        ),
    };
    let seeds: &[&[u8]] = &[MarginConfig::VAULT_AUTHORITY_SEED, &[ctx.bumps.vault_authority]];
    invoke_signed(
        &ix,
        &[
            ctx.accounts.vault_authority.to_account_info(),
            ctx.accounts.exchange_market.to_account_info(),
            ctx.accounts.pool.to_account_info(),
            ctx.accounts.pool_authority.to_account_info(),
            ctx.accounts.pool_security_vault.to_account_info(),
            ctx.accounts.pool_quote_vault.to_account_info(),
            ctx.accounts.base_vault.to_account_info(),
            ctx.accounts.collateral_vault.to_account_info(),
            ctx.accounts.token_program.to_account_info(),
            ctx.accounts.exchange_program.to_account_info(),
        ],
        &[seeds],
    )?;

    // Book what actually arrived
    ctx.accounts.collateral_vault.reload()?;
    ctx.accounts.base_vault.reload()?;
    let out_after = if params.is_sell {
        ctx.accounts.collateral_vault.amount
    } else {
        ctx.accounts.base_vault.amount
    };
    let amount_out = out_after
        .checked_sub(out_before)
        .ok_or(TensorError::MathOverflow)?;
    require!(amount_out >= params.min_amount_out, TensorError::SlippageExceeded);

    // Spot value at the post-swap pool price
    let pool_after = tensor_cpi::northtail::read_pool(&ctx.accounts.pool)
        .map_err(|_| TensorError::InvalidPrice)?;
    let spot_price = tensor_cpi::northtail::calculate_spot_price(&pool_after);

    let market = &ctx.accounts.market;
    let account = &mut ctx.accounts.margin_account;

    if params.is_sell {
        let slot_idx = account.find_spot_by_mint(&market.base_mint)
            .ok_or(TensorError::PositionNotFound)?;
        account.spot_balances[slot_idx].balance -= params.amount_in;
        account.spot_balances[slot_idx].value =
            (account.spot_balances[slot_idx].balance as u128 * spot_price as u128 / 1_000_000) as u64;

        // Close position if fully sold
        if account.spot_balances[slot_idx].balance == 0 {
            account.spot_balances[slot_idx] = tensor_types::SpotBalance::default();
            account.spot_count = account.spot_count.saturating_sub(1);
        }

        account.collateral = account.collateral
            .checked_add(amount_out)
            .ok_or(TensorError::MathOverflow)?;
    } else {
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

        account.spot_balances[slot_idx].balance = account.spot_balances[slot_idx].balance
            .checked_add(amount_out)
            .ok_or(TensorError::MathOverflow)?;
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
        amount_out,
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
