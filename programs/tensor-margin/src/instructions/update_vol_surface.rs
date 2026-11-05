use anchor_lang::prelude::*;
use crate::state::*;
use crate::errors::TensorError;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct UpdateVolSurfaceParams {
    /// IV in bps for each [expiry_bucket][moneyness_node]
    pub vol_surface: [[u64; MAX_VOL_NODES]; MAX_EXPIRY_BUCKETS],
    /// Moneyness nodes (strike/spot ratio in 1e6)
    pub moneyness_nodes: [u64; MAX_VOL_NODES],
    /// Expiry bucket boundaries in days
    pub expiry_days: [u16; MAX_EXPIRY_BUCKETS],
    /// Number of active moneyness nodes
    pub node_count: u8,
    /// Number of active expiry buckets
    pub expiry_count: u8,
}

#[derive(Accounts)]
pub struct UpdateVolSurface<'info> {
    #[account(
        mut,
        seeds = [MarginMarket::SEED, &market.index.to_le_bytes()],
        bump = market.bump,
    )]
    pub market: Box<Account<'info, MarginMarket>>,

    #[account(
        seeds = [MarginConfig::SEED],
        bump = config.bump,
        constraint = config.authority == authority.key() @ TensorError::Unauthorized,
    )]
    pub config: Box<Account<'info, MarginConfig>>,

    pub authority: Signer<'info>,
}

pub fn handler(ctx: Context<UpdateVolSurface>, params: UpdateVolSurfaceParams) -> Result<()> {
    require!(
        (params.node_count as usize) <= MAX_VOL_NODES,
        TensorError::InvalidAmount
    );
    require!(
        (params.expiry_count as usize) <= MAX_EXPIRY_BUCKETS,
        TensorError::InvalidAmount
    );

    // Validate moneyness nodes are sorted ascending
    for i in 1..params.node_count as usize {
        require!(
            params.moneyness_nodes[i] > params.moneyness_nodes[i - 1],
            TensorError::InvalidPrice
        );
    }

    // Validate expiry days are sorted ascending
    for i in 1..params.expiry_count as usize {
        require!(
            params.expiry_days[i] > params.expiry_days[i - 1],
            TensorError::InvalidPrice
        );
    }

    let market = &mut ctx.accounts.market;
    market.vol_surface = params.vol_surface;
    market.vol_moneyness_nodes = params.moneyness_nodes;
    market.vol_expiry_days = params.expiry_days;
    market.vol_node_count = params.node_count;
    market.vol_expiry_count = params.expiry_count;

    emit!(VolSurfaceUpdated {
        market_index: market.index,
        node_count: params.node_count,
        expiry_count: params.expiry_count,
    });

    Ok(())
}

#[event]
pub struct VolSurfaceUpdated {
    pub market_index: u16,
    pub node_count: u8,
    pub expiry_count: u8,
}
