//! LiteSVM tests for the spot step of the liquidation waterfall.
//!
//! Needs two SBF binaries:
//! - this program at `target/deploy/tensor_margin.so`
//!   (`cd programs/tensor-margin && cargo-build-sbf --tools-version v1.51`);
//! - northtail-exchange, at the path in `NORTHTAIL_EXCHANGE_SO` (build it from
//!   the northtail repo the same way).
//!
//! A missing binary makes the test fail with a message saying which one.
//! To skip these tests on purpose (e.g. a machine without the SBF
//! toolchain), set `TENSOR_SKIP_SVM=1`; the skipped tests then print a note.

use anchor_lang::prelude::*;
use anchor_lang::solana_program::program_pack::Pack;
use anchor_lang::{InstructionData, ToAccountMetas};
use anchor_spl::associated_token::get_associated_token_address;
use anchor_spl::token::spl_token;
use litesvm::LiteSVM;
use solana_sdk::account::Account as SdkAccount;
use solana_sdk::instruction::{AccountMeta, Instruction};
use solana_sdk::signature::{Keypair, Signer};
use solana_sdk::transaction::Transaction;
use tensor_margin::state::*;
use tensor_types::*;

const MARK: u64 = 150_000_000; // $150, 1e6 scaled
const MIN_TRADE: u64 = 1_000;
const NT_FEE_BPS: u16 = 30;

fn skip_requested() -> bool {
    std::env::var("TENSOR_SKIP_SVM").map(|v| v == "1").unwrap_or(false)
}

/// Reads a program binary. Missing binary: None if TENSOR_SKIP_SVM=1, else panic.
fn load_so(what: &str, path: Option<String>) -> Option<Vec<u8>> {
    if let Some(bytes) = path.as_deref().and_then(|p| std::fs::read(p).ok()) {
        return Some(bytes);
    }
    if skip_requested() {
        eprintln!("skipped (TENSOR_SKIP_SVM=1): {what} binary not found at {path:?}");
        return None;
    }
    panic!(
        "{what} SBF binary not found at {path:?}. Build it (see the header of \
         tests/liquidate_spot_svm.rs) or set TENSOR_SKIP_SVM=1 to skip these tests."
    );
}

fn program_so() -> Option<Vec<u8>> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../target/deploy/tensor_margin.so");
    load_so("tensor_margin", Some(path.to_string()))
}

fn northtail_so() -> Option<Vec<u8>> {
    load_so(
        "northtail-exchange (NORTHTAIL_EXCHANGE_SO)",
        std::env::var("NORTHTAIL_EXCHANGE_SO").ok(),
    )
}

fn disc(name: &str) -> [u8; 8] {
    anchor_lang::solana_program::hash::hash(format!("account:{name}").as_bytes()).to_bytes()[..8]
        .try_into()
        .unwrap()
}

struct Env {
    svm: LiteSVM,
    liquidator: Keypair,
    owner: Pubkey,
    config: Pubkey,
    margin_account: Pubkey,
    vault_authority: Pubkey,
    base_mint: Pubkey,
    collateral_mint: Pubkey,
    nt_market: Pubkey,
    pool: Pubkey,
    pool_authority: Pubkey,
    pool_security_vault: Pubkey,
    pool_quote_vault: Pubkey,
}

fn set_anchor<T: AccountSerialize>(svm: &mut LiteSVM, key: Pubkey, owner: Pubkey, value: &T) {
    let mut data = Vec::new();
    value.try_serialize(&mut data).unwrap();
    let lamports = svm.minimum_balance_for_rent_exemption(data.len());
    svm.set_account(key, SdkAccount { lamports, data, owner, executable: false, rent_epoch: 0 })
        .unwrap();
}

fn set_raw(svm: &mut LiteSVM, key: Pubkey, owner: Pubkey, data: Vec<u8>) {
    let lamports = svm.minimum_balance_for_rent_exemption(data.len());
    svm.set_account(key, SdkAccount { lamports, data, owner, executable: false, rent_epoch: 0 })
        .unwrap();
}

fn set_mint(svm: &mut LiteSVM, key: Pubkey, decimals: u8) {
    let mut data = vec![0u8; spl_token::state::Mint::LEN];
    spl_token::state::Mint {
        mint_authority: Default::default(),
        supply: u64::MAX / 2,
        decimals,
        is_initialized: true,
        freeze_authority: Default::default(),
    }
    .pack_into_slice(&mut data);
    set_raw(svm, key, spl_token::ID, data);
}

fn set_token(svm: &mut LiteSVM, key: Pubkey, mint: Pubkey, owner: Pubkey, amount: u64) {
    let mut data = vec![0u8; spl_token::state::Account::LEN];
    spl_token::state::Account {
        mint,
        owner,
        amount,
        delegate: Default::default(),
        state: spl_token::state::AccountState::Initialized,
        is_native: Default::default(),
        delegated_amount: 0,
        close_authority: Default::default(),
    }
    .pack_into_slice(&mut data);
    set_raw(svm, key, spl_token::ID, data);
}

fn token_amount(svm: &LiteSVM, key: &Pubkey) -> u64 {
    spl_token::state::Account::unpack(&svm.get_account(key).unwrap().data).unwrap().amount
}

fn market_pda(index: u16) -> (Pubkey, u8) {
    Pubkey::find_program_address(&[MarginMarket::SEED, &index.to_le_bytes()], &tensor_margin::ID)
}

fn margin_market(index: u16, base_mint: Pubkey, bump: u8) -> MarginMarket {
    MarginMarket {
        index,
        symbol: "SOL".to_string(),
        base_mint,
        oracle: Pubkey::default(),
        variance_tracker: Pubkey::default(),
        spot_enabled: true,
        perp_enabled: false,
        options_enabled: false,
        lending_enabled: true,
        initial_margin_bps: 0,
        maintenance_ratio_bps: 0,
        max_position_size: 0,
        mark_price: MARK,
        implied_vol_bps: 0,
        funding_rate_bps: 0,
        cumulative_funding_index: 0,
        last_funding_update: 0,
        open_interest_long: 0,
        open_interest_short: 0,
        total_volume: 0,
        is_active: true,
        aggregate_gamma_long: 0,
        aggregate_gamma_short: 0,
        vol_surface: [[0u64; MAX_VOL_NODES]; MAX_EXPIRY_BUCKETS],
        vol_moneyness_nodes: [0u64; MAX_VOL_NODES],
        vol_expiry_days: [0u16; MAX_EXPIRY_BUCKETS],
        vol_node_count: 0,
        vol_expiry_count: 0,
        bump,
    }
}

/// Northtail Market bytes (layout of northtail-exchange `Market`).
fn nt_market_bytes(base: Pubkey, quote: Pubkey, bump: u8) -> Vec<u8> {
    let mut d = disc("Market").to_vec();
    d.extend_from_slice(Pubkey::new_unique().as_ref()); // authority
    d.extend_from_slice(base.as_ref());
    d.extend_from_slice(quote.as_ref());
    d.push(0); // market_type
    d.push(0); // status Active
    d.extend_from_slice(Pubkey::default().as_ref()); // oracle
    d.extend_from_slice(&NT_FEE_BPS.to_le_bytes());
    d.extend_from_slice(&0u16.to_le_bytes());
    d.extend_from_slice(&MIN_TRADE.to_le_bytes());
    d.extend_from_slice(&0u64.to_le_bytes()); // max_trade_size
    d.extend_from_slice(&[0u8; 32]); // total_volume, total_fees, volume_24h, volume_24h_reset
    for s in ["SOL", "Solana"] {
        d.extend_from_slice(&(s.len() as u32).to_le_bytes());
        d.extend_from_slice(s.as_bytes());
    }
    d.push(0); // isin None
    d.push(1); // is_active
    d.extend_from_slice(&0i64.to_le_bytes());
    d.push(bump);
    d
}

/// Northtail Pool bytes (layout of northtail-exchange `Pool`).
#[allow(clippy::too_many_arguments)]
fn pool_bytes(
    market: Pubkey,
    security: u64,
    quote: u64,
    authority: Pubkey,
    security_vault: Pubkey,
    quote_vault: Pubkey,
    bump: u8,
    authority_bump: u8,
) -> Vec<u8> {
    let mut d = disc("Pool").to_vec();
    d.extend_from_slice(market.as_ref());
    d.extend_from_slice(&security.to_le_bytes());
    d.extend_from_slice(&quote.to_le_bytes());
    d.extend_from_slice(Pubkey::new_unique().as_ref()); // lp_mint
    d.extend_from_slice(&1u64.to_le_bytes()); // lp_supply
    d.extend_from_slice(authority.as_ref());
    d.extend_from_slice(security_vault.as_ref());
    d.extend_from_slice(quote_vault.as_ref());
    d.extend_from_slice(&[0u8; 16]); // accumulated fees
    d.extend_from_slice(&0u64.to_le_bytes()); // twap
    d.extend_from_slice(&0i64.to_le_bytes()); // twap_last_update
    d.extend_from_slice(&[0u8; 32]); // cumulative_price, k_last
    d.push(1); // is_active
    d.extend_from_slice(&0i64.to_le_bytes()); // created_at
    d.push(bump);
    d.push(authority_bump);
    d
}

/// One unhealthy account holding `spot_balance` base tokens (market 0) and a
/// supplied lending position; a pool priced at `pool_price`.
fn setup(spot_balance: u64, pool_price: u64, with_northtail: bool) -> Option<Env> {
    let so = program_so()?;
    let mut svm = LiteSVM::new();
    svm.add_program(tensor_margin::ID, &so);
    if with_northtail {
        svm.add_program(tensor_cpi::northtail::EXCHANGE_PROGRAM, &northtail_so()?);
    }

    let liquidator = Keypair::new();
    svm.airdrop(&liquidator.pubkey(), 10_000_000_000).unwrap();
    let owner = Pubkey::new_unique();
    let base_mint = Pubkey::new_unique();
    let collateral_mint = Pubkey::new_unique();
    set_mint(&mut svm, base_mint, 6);
    set_mint(&mut svm, collateral_mint, 6);

    let (config, config_bump) =
        Pubkey::find_program_address(&[MarginConfig::SEED], &tensor_margin::ID);
    set_anchor(
        &mut svm,
        config,
        tensor_margin::ID,
        &MarginConfig {
            authority: liquidator.pubkey(),
            fee_collector: Pubkey::new_unique(),
            collateral_mint,
            initial_margin_bps: 1_000,
            maintenance_ratio_bps: 5_000,
            gamma_margin_bps: 0,
            vega_margin_bps: 0,
            liquidation_fee_bps: 100,
            trading_fee_bps: 0,
            max_margin_mode: MarginMode::Portfolio,
            kyc_registry: Pubkey::default(),
            identity_program: Pubkey::default(),
            insurance_fund: 0,
            total_accounts: 1,
            total_markets: 2,
            is_paused: false,
            max_account_gamma_notional: 0,
            max_market_gamma_notional: 0,
            auction_duration_seconds: 0,
            min_solver_stake: 0,
            solver_slash_rate_bps: 0,
            bump: config_bump,
        },
    );

    // Markets 0 and 1 share the base mint.
    for index in [0u16, 1] {
        let (key, bump) = market_pda(index);
        set_anchor(&mut svm, key, tensor_margin::ID, &margin_market(index, base_mint, bump));
    }

    let (margin_account, ma_bump) =
        Pubkey::find_program_address(&[MarginAccount::SEED, owner.as_ref()], &tensor_margin::ID);
    let mut spot_balances = Box::new([SpotBalance::default(); MAX_SPOT_BALANCES]);
    spot_balances[2] = SpotBalance {
        mint: base_mint,
        balance: spot_balance,
        value: (spot_balance as u128 * MARK as u128 / 1_000_000) as u64,
        market_index: 0,
        is_active: true,
    };
    let mut lending_positions = Box::new([LendingPosition::default(); MAX_LENDING_POSITIONS]);
    lending_positions[0] = LendingPosition {
        mint: Pubkey::new_unique(),
        market_index: 0,
        side: LendingSide::Supply,
        principal: 7_000,
        accrued_interest: 0,
        rate_bps: 0,
        haircut_bps: 0,
        effective_value: 7_000,
        last_accrual: 0,
        is_active: true,
    };
    set_anchor(
        &mut svm,
        margin_account,
        tensor_margin::ID,
        &MarginAccount {
            owner,
            delegate: Pubkey::default(),
            collateral: 1_000,
            locked_collateral: 0,
            perp_positions: Box::new([PerpPosition::default(); MAX_PERP_POSITIONS]),
            perp_count: 0,
            spot_balances,
            spot_count: 1,
            option_positions: Box::new([OptionPosition::default(); MAX_OPTION_POSITIONS]),
            option_count: 0,
            lending_positions,
            lending_count: 1,
            greeks: PortfolioGreeks::default(),
            initial_margin_required: 0,
            maintenance_margin_required: 0,
            equity: 0,
            margin_ratio_bps: 0,
            health: AccountHealth::Liquidatable,
            margin_mode: MarginMode::Portfolio,
            investor_category: InvestorCategory::Retail,
            identity: Pubkey::default(),
            zk_credit_score: 0,
            zk_credit_tier: ZkCreditTier::None,
            zk_score_updated_at: 0,
            zk_credit_oracle: Pubkey::default(),
            active_intent_count: 0,
            created_at: 0,
            last_margin_update: 0,
            total_trades: 0,
            total_realized_pnl: 0,
            bump: ma_bump,
        },
    );

    let (vault_authority, _) =
        Pubkey::find_program_address(&[MarginConfig::VAULT_AUTHORITY_SEED], &tensor_margin::ID);
    set_token(
        &mut svm,
        get_associated_token_address(&vault_authority, &collateral_mint),
        collateral_mint,
        vault_authority,
        1_000,
    );
    set_token(
        &mut svm,
        get_associated_token_address(&vault_authority, &base_mint),
        base_mint,
        vault_authority,
        spot_balance,
    );

    let exch = tensor_cpi::northtail::EXCHANGE_PROGRAM;
    let (nt_market, nt_bump) = Pubkey::find_program_address(
        &[b"market", base_mint.as_ref(), collateral_mint.as_ref()],
        &exch,
    );
    let (pool, pool_bump) = Pubkey::find_program_address(&[b"pool", nt_market.as_ref()], &exch);
    let (pool_authority, pa_bump) =
        Pubkey::find_program_address(&[b"pool_authority", nt_market.as_ref()], &exch);
    let pool_security_vault = Pubkey::new_unique();
    let pool_quote_vault = Pubkey::new_unique();
    let security_reserve = 1_000_000_000u64;
    let quote_reserve = (security_reserve as u128 * pool_price as u128 / 1_000_000) as u64;
    set_token(&mut svm, pool_security_vault, base_mint, pool_authority, security_reserve);
    set_token(&mut svm, pool_quote_vault, collateral_mint, pool_authority, quote_reserve);
    set_raw(&mut svm, nt_market, exch, nt_market_bytes(base_mint, collateral_mint, nt_bump));
    set_raw(
        &mut svm,
        pool,
        exch,
        pool_bytes(
            nt_market,
            security_reserve,
            quote_reserve,
            pool_authority,
            pool_security_vault,
            pool_quote_vault,
            pool_bump,
            pa_bump,
        ),
    );

    Some(Env {
        svm,
        liquidator,
        owner,
        config,
        margin_account,
        vault_authority,
        base_mint,
        collateral_mint,
        nt_market,
        pool,
        pool_authority,
        pool_security_vault,
        pool_quote_vault,
    })
}

fn send(env: &mut Env, ix: Instruction) -> std::result::Result<Vec<String>, Vec<String>> {
    send_all(env, &[ix])
}

fn send_all(env: &mut Env, ixs: &[Instruction]) -> std::result::Result<Vec<String>, Vec<String>> {
    let tx = Transaction::new_signed_with_payer(
        ixs,
        Some(&env.liquidator.pubkey()),
        &[&env.liquidator],
        env.svm.latest_blockhash(),
    );
    env.svm.expire_blockhash();
    match env.svm.send_transaction(tx) {
        Ok(meta) => Ok(meta.logs),
        Err(failed) => Err(failed.meta.logs),
    }
}

fn liquidate_spot_ix(env: &Env, market_index: u16, amount_in: u64) -> Instruction {
    let accounts = tensor_margin::accounts::LiquidateSpot {
        margin_account: env.margin_account,
        market: market_pda(market_index).0,
        config: env.config,
        vault_authority: env.vault_authority,
        collateral_vault: get_associated_token_address(&env.vault_authority, &env.collateral_mint),
        base_vault: get_associated_token_address(&env.vault_authority, &env.base_mint),
        exchange_market: env.nt_market,
        pool: env.pool,
        pool_authority: env.pool_authority,
        pool_security_vault: env.pool_security_vault,
        pool_quote_vault: env.pool_quote_vault,
        exchange_program: tensor_cpi::northtail::EXCHANGE_PROGRAM,
        token_program: spl_token::ID,
        liquidator: env.liquidator.pubkey(),
    };
    Instruction {
        program_id: tensor_margin::ID,
        accounts: accounts.to_account_metas(None),
        data: tensor_margin::instruction::LiquidateSpot {
            params: tensor_margin::instructions::LiquidateSpotParams { amount_in, min_amount_out: 0 },
        }
        .data(),
    }
}

fn liquidate_ix(env: &Env, with_spot_proof: bool) -> Instruction {
    let mut accounts = tensor_margin::accounts::Liquidate {
        margin_account: env.margin_account,
        market: market_pda(0).0,
        config: env.config,
        liquidator: env.liquidator.pubkey(),
    }
    .to_account_metas(None);
    if with_spot_proof {
        accounts.push(AccountMeta::new_readonly(market_pda(0).0, false));
        accounts.push(AccountMeta::new_readonly(env.nt_market, false));
        accounts.push(AccountMeta::new_readonly(env.pool, false));
    }
    Instruction {
        program_id: tensor_margin::ID,
        accounts,
        data: tensor_margin::instruction::Liquidate {}.data(),
    }
}

fn margin(env: &Env) -> MarginAccount {
    let data = env.svm.get_account(&env.margin_account).unwrap().data;
    MarginAccount::try_deserialize(&mut &data[..]).unwrap()
}

fn has(logs: &[String], needle: &str) -> bool {
    logs.iter().any(|l| l.contains(needle))
}

macro_rules! env_or_skip {
    ($e:expr) => {
        match $e {
            Some(env) => env,
            None => {
                // only reached with TENSOR_SKIP_SVM=1 (load_so panics otherwise)
                return;
            }
        }
    };
}

#[test]
fn liquidate_refuses_sellable_spot() {
    let mut env = env_or_skip!(setup(10_000, MARK, false));
    let logs = { let ix = liquidate_ix(&env, false); send(&mut env, ix) }.unwrap_err();
    assert!(has(&logs, "SpotLiquidationRequired"), "{logs:#?}");
    let logs = { let ix = liquidate_ix(&env, true); send(&mut env, ix) }.unwrap_err();
    assert!(has(&logs, "SpotLiquidationRequired"), "{logs:#?}");
    assert!(margin(&env).lending_positions[0].is_active);
}

#[test]
fn liquidate_moves_past_dust_spot_to_lending() {
    let mut env = env_or_skip!(setup(MIN_TRADE - 1, MARK, false));
    { let ix = liquidate_ix(&env, true); send(&mut env, ix) }.unwrap();
    let acc = margin(&env);
    assert!(!acc.lending_positions[0].is_active, "lending was seized");
    assert_eq!(acc.spot_balances[2].balance, MIN_TRADE - 1, "dust stays on the books");
    // 1_000 + 7_000 seized - 1% fee on 7_000
    assert_eq!(acc.collateral, 1_000 + 7_000 - 70);
    let _ = env.owner;
}

#[test]
fn liquidate_waits_on_spot_when_pool_is_below_the_floor() {
    // A low pool price is not a reason to skip spot: the step waits for the mark.
    let mut env = env_or_skip!(setup(10_000, MARK * 90 / 100, false));
    let logs = { let ix = liquidate_ix(&env, true); send(&mut env, ix) }.unwrap_err();
    assert!(has(&logs, "SpotLiquidationRequired"), "{logs:#?}");
    assert!(margin(&env).lending_positions[0].is_active);
    assert_eq!(margin(&env).spot_balances[2].balance, 10_000);
}

#[test]
fn liquidate_spot_rejects_another_market_with_the_same_mint() {
    let mut env = env_or_skip!(setup(10_000, MARK, false));
    let logs = { let ix = liquidate_spot_ix(&env, 1, 5_000); send(&mut env, ix) }.unwrap_err();
    assert!(has(&logs, "AccountMismatch"), "{logs:#?}");
}

#[test]
fn liquidate_spot_sells_a_chunk_through_northtail() {
    let mut env = env_or_skip!(setup(10_000, MARK, true));
    let base_vault = get_associated_token_address(&env.vault_authority, &env.base_mint);
    let coll_vault = get_associated_token_address(&env.vault_authority, &env.collateral_mint);
    let coll_before = token_amount(&env.svm, &coll_vault);

    { let ix = liquidate_spot_ix(&env, 0, 4_000); send(&mut env, ix) }.unwrap();

    let received = token_amount(&env.svm, &coll_vault) - coll_before;
    // 4_000 atoms at $150 less the 0.3% northtail fee and a little impact
    assert!(received > 4_000 * 150 * 99 / 100 && received <= 4_000 * 150, "{received}");
    assert_eq!(token_amount(&env.svm, &base_vault), 6_000);
    let acc = margin(&env);
    assert_eq!(acc.spot_balances[2].balance, 6_000);
    let fee = received / 100;
    assert_eq!(acc.collateral, 1_000 + received - fee);
    assert!(acc.lending_positions[0].is_active, "lending waits until spot is done");
}

#[test]
fn liquidate_spot_fails_below_the_floor() {
    let mut env = env_or_skip!(setup(10_000, MARK * 90 / 100, true));
    let logs = { let ix = liquidate_spot_ix(&env, 0, 4_000); send(&mut env, ix) }.unwrap_err();
    assert!(has(&logs, "SlippageExceeded"), "{logs:#?}");
    assert_eq!(margin(&env).spot_balances[2].balance, 10_000);
}

fn register_market_ix(env: &Env, base_mint: Pubkey) -> Instruction {
    let mut accounts = tensor_margin::accounts::RegisterMarket {
        market: market_pda(2).0,
        config: env.config,
        authority: env.liquidator.pubkey(),
        system_program: anchor_lang::system_program::ID,
    }
    .to_account_metas(None);
    accounts.push(AccountMeta::new_readonly(base_mint, false));
    accounts.push(AccountMeta::new_readonly(env.collateral_mint, false));
    Instruction {
        program_id: tensor_margin::ID,
        accounts,
        data: tensor_margin::instruction::RegisterMarket {
            params: tensor_margin::instructions::RegisterMarketParams {
                symbol: "ETH".to_string(),
                base_mint,
                oracle: Pubkey::default(),
                variance_tracker: Pubkey::default(),
                spot_enabled: true,
                perp_enabled: false,
                options_enabled: false,
                lending_enabled: false,
                initial_margin_bps: 0,
                maintenance_ratio_bps: 0,
                max_position_size: 0,
            },
        }
        .data(),
    }
}

#[test]
fn register_spot_market_requires_equal_decimals() {
    let mut env = env_or_skip!(setup(10_000, MARK, false));
    let nine = Pubkey::new_unique();
    set_mint(&mut env.svm, nine, 9);
    let logs = { let ix = register_market_ix(&env, nine); send(&mut env, ix) }.unwrap_err();
    assert!(has(&logs, "MintDecimalsMismatch"), "{logs:#?}");

    let six = Pubkey::new_unique();
    set_mint(&mut env.svm, six, 6);
    { let ix = register_market_ix(&env, six); send(&mut env, ix) }.unwrap();
    let data = env.svm.get_account(&market_pda(2).0).unwrap().data;
    assert_eq!(MarginMarket::try_deserialize(&mut &data[..]).unwrap().base_mint, six);
}

/// Gives the liquidator base tokens and returns a northtail swap that sells
/// `amount` of them into the pool, pushing its price down.
fn pool_push_ix(env: &mut Env, amount: u64) -> Instruction {
    let attacker = env.liquidator.pubkey();
    let attacker_base = Pubkey::new_unique();
    let attacker_quote = Pubkey::new_unique();
    let (base_mint, collateral_mint) = (env.base_mint, env.collateral_mint);
    set_token(&mut env.svm, attacker_base, base_mint, attacker, amount);
    set_token(&mut env.svm, attacker_quote, collateral_mint, attacker, 0);
    Instruction {
        program_id: tensor_cpi::northtail::EXCHANGE_PROGRAM,
        accounts: vec![
            AccountMeta::new(attacker, true),
            AccountMeta::new(env.nt_market, false),
            AccountMeta::new(env.pool, false),
            AccountMeta::new_readonly(env.pool_authority, false),
            AccountMeta::new(env.pool_security_vault, false),
            AccountMeta::new(env.pool_quote_vault, false),
            AccountMeta::new(attacker_base, false),
            AccountMeta::new(attacker_quote, false),
            AccountMeta::new_readonly(spl_token::ID, false),
            // oracle_feed: None
            AccountMeta::new_readonly(tensor_cpi::northtail::EXCHANGE_PROGRAM, false),
        ],
        data: tensor_cpi::northtail::build_swap_ix_data(amount, 0, true),
    }
}

fn pool_price(env: &Env) -> u64 {
    let data = env.svm.get_account(&env.pool).unwrap().data;
    let s = u64::from_le_bytes(data[40..48].try_into().unwrap()) as u128;
    let q = u64::from_le_bytes(data[48..56].try_into().unwrap()) as u128;
    (q * 1_000_000 / s) as u64
}

/// Selling 4% of the base reserve drops the pool price about 8%.
const PUSH: u64 = 40_000_000;

#[test]
fn pool_push_alone_goes_below_the_floor() {
    let mut env = env_or_skip!(setup(10_000, MARK, true));
    let push = pool_push_ix(&mut env, PUSH);
    send(&mut env, push).unwrap();
    let price = pool_price(&env);
    assert!(price < MARK * 95 / 100, "push should go below the floor: {price}");
}

/// A swap that pushes the pool below the floor, then liquidate in the same
/// transaction: the spot step must still be required, so lending is not seized.
#[test]
fn pool_push_then_liquidate_in_one_tx_is_refused() {
    let mut env = env_or_skip!(setup(10_000, MARK, true));
    let push = pool_push_ix(&mut env, PUSH);
    let liq = liquidate_ix(&env, true);
    let logs = send_all(&mut env, &[push, liq]).unwrap_err();
    assert!(has(&logs, "SpotLiquidationRequired"), "{logs:#?}");
    let acc = margin(&env);
    assert!(acc.lending_positions[0].is_active, "lending not seized");
    assert_eq!(acc.spot_balances[2].balance, 10_000);
    assert_eq!(pool_price(&env), MARK, "the whole transaction rolled back");
}
