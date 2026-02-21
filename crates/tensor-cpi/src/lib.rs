//! Tensor CPI — Adapters for reading external program state
//!
//! Provides zero-copy account readers for:
//! - Sigma shared-oracle (PriceFeed, VarianceTracker, FundingFeed)
//! - Sovereign identity (SovereignIdentity)
//! - Northtail exchange (Pool spot price)
//! - Percolator (Matcher ABI types)
//!
//! These are NOT full program re-exports. They define minimal structs
//! for reading accounts via try_borrow_data + manual deserialization,
//! avoiding heavy cross-program dependency graphs.

pub mod sigma;
pub mod sovereign;
pub mod northtail;
pub mod percolator;
pub mod zk_credit;
