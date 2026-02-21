use anchor_lang::prelude::*;

#[error_code]
pub enum TensorError {
    #[msg("Protocol is paused")]
    ProtocolPaused,

    #[msg("Unauthorized")]
    Unauthorized,

    #[msg("Market not active")]
    MarketNotActive,

    #[msg("Insufficient collateral")]
    InsufficientCollateral,

    #[msg("Insufficient margin")]
    InsufficientMargin,

    #[msg("Position slot full")]
    PositionSlotFull,

    #[msg("Position not found")]
    PositionNotFound,

    #[msg("Position still active")]
    PositionStillActive,

    #[msg("Account has open positions")]
    AccountHasPositions,

    #[msg("Account is healthy, cannot liquidate")]
    AccountHealthy,

    #[msg("Account is bankrupt")]
    AccountBankrupt,

    #[msg("Invalid margin mode")]
    InvalidMarginMode,

    #[msg("Invalid amount")]
    InvalidAmount,

    #[msg("Invalid price")]
    InvalidPrice,

    #[msg("Oracle stale")]
    OracleStale,

    #[msg("Product not enabled for this market")]
    ProductNotEnabled,

    #[msg("Exceeds position limit")]
    ExceedsPositionLimit,

    #[msg("Exceeds leverage limit")]
    ExceedsLeverageLimit,

    #[msg("Option expired")]
    OptionExpired,

    #[msg("Math overflow")]
    MathOverflow,

    #[msg("Market already exists")]
    MarketAlreadyExists,

    #[msg("Collateral locked")]
    CollateralLocked,

    #[msg("KYC verification required")]
    KycRequired,

    // Phase 3: Intents
    #[msg("Intent not found")]
    IntentNotFound,

    #[msg("Intent already resolved")]
    IntentAlreadyResolved,

    #[msg("Intent has expired")]
    IntentExpired,

    #[msg("Invalid intent state")]
    InvalidIntentState,

    #[msg("Intent not fully filled")]
    IntentNotFilled,

    #[msg("Constraint violation")]
    ConstraintViolation,

    #[msg("Too many active intents")]
    TooManyIntents,

    // Phase 3: ZK Credit
    #[msg("Credit score is stale")]
    CreditScoreStale,

    #[msg("Credit score is invalid")]
    CreditScoreInvalid,

    #[msg("Credit oracle mismatch")]
    CreditOracleMismatch,

    // Phase 3: Execution constraints
    #[msg("Slippage exceeded")]
    SlippageExceeded,

    #[msg("Fill ratio too low")]
    FillRatioTooLow,

    #[msg("Deadline has passed")]
    DeadlinePassed,

    #[msg("Max cost exceeded")]
    MaxCostExceeded,
}
