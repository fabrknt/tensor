use anchor_lang::prelude::*;
use tensor_types::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SubmitIntentArgs {
    pub intent_id: u64,
    pub intent_type: IntentType,
    pub legs: Vec<SubmitIntentLeg>,
    pub max_slippage_bps: u16,
    pub min_fill_ratio_bps: u16,
    pub deadline: i64,
    pub max_total_cost: u64,
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct SubmitIntentLeg {
    pub product_type: ProductType,
    pub market_index: u16,
    pub size: i64,
    pub limit_price: u64,
}

#[derive(Accounts)]
#[instruction(args: SubmitIntentArgs)]
pub struct SubmitIntent<'info> {
    #[account(
        mut,
        seeds = [MarginAccount::SEED, authority.key().as_ref()],
        bump = margin_account.bump,
        constraint = (margin_account.owner == authority.key()
            || margin_account.delegate == authority.key()) @ TensorError::Unauthorized,
    )]
    pub margin_account: Account<'info, MarginAccount>,

    #[account(
        init,
        payer = authority,
        space = 8 + IntentAccount::INIT_SPACE,
        seeds = [
            IntentAccount::SEED,
            margin_account.key().as_ref(),
            &args.intent_id.to_le_bytes(),
        ],
        bump,
    )]
    pub intent_account: Account<'info, IntentAccount>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
    )]
    pub config: Account<'info, MarginConfig>,

    #[account(mut)]
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
}

pub fn handler(ctx: Context<SubmitIntent>, args: SubmitIntentArgs) -> Result<()> {
    require!(!ctx.accounts.config.is_paused, TensorError::ProtocolPaused);

    require!(
        ctx.accounts.margin_account.active_intent_count < MAX_ACTIVE_INTENTS,
        TensorError::TooManyIntents
    );

    let clock = Clock::get()?;

    // Validate deadline is in the future (if set)
    if args.deadline > 0 {
        require!(args.deadline > clock.unix_timestamp, TensorError::DeadlinePassed);
    }

    // Validate legs
    require!(!args.legs.is_empty(), TensorError::InvalidIntentState);
    require!(args.legs.len() <= MAX_INTENT_LEGS, TensorError::InvalidIntentState);

    for leg in &args.legs {
        require!(leg.size != 0, TensorError::InvalidAmount);
    }

    // Copy legs into fixed-size array
    let mut legs = [IntentLeg::default(); MAX_INTENT_LEGS];
    for (i, leg) in args.legs.iter().enumerate() {
        legs[i] = IntentLeg {
            product_type: leg.product_type,
            market_index: leg.market_index,
            size: leg.size,
            limit_price: leg.limit_price,
            is_active: true,
        };
    }

    let margin_account_key = ctx.accounts.margin_account.key();
    let owner = ctx.accounts.margin_account.owner;

    let intent = &mut ctx.accounts.intent_account;
    intent.margin_account = margin_account_key;
    intent.intent_id = args.intent_id;
    intent.intent_type = args.intent_type;
    intent.status = IntentStatus::Pending;
    intent.legs = legs;
    intent.leg_count = args.legs.len() as u8;
    intent.filled_legs = 0;
    intent.max_slippage_bps = args.max_slippage_bps;
    intent.min_fill_ratio_bps = args.min_fill_ratio_bps;
    intent.deadline = args.deadline;
    intent.max_total_cost = args.max_total_cost;
    intent.total_margin_used = 0;
    intent.created_at = clock.unix_timestamp;
    intent.updated_at = clock.unix_timestamp;
    intent.bump = ctx.bumps.intent_account;

    // Increment active intent count
    ctx.accounts.margin_account.active_intent_count += 1;

    emit!(IntentSubmitted {
        owner,
        intent_id: args.intent_id,
        intent_type: args.intent_type,
        leg_count: args.legs.len() as u8,
    });

    Ok(())
}

#[event]
pub struct IntentSubmitted {
    pub owner: Pubkey,
    pub intent_id: u64,
    pub intent_type: IntentType,
    pub leg_count: u8,
}
