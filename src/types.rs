use fake::Dummy;
use serde::{Deserialize, Serialize};

/// Amounts are stored in ten-thousandths, without floating-point rounding.
pub const AMOUNT_SCALE: i64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Dummy)]
#[serde(rename_all = "lowercase")]
pub enum DisputeStage {
    Dispute,
    Resolve,
    Chargeback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Dummy)]
pub struct Transaction {
    #[dummy(faker = "1..1_000_000")]
    pub id: u32,
    /// Deposits are positive, withdrawals negative; scaled by [`AMOUNT_SCALE`].
    #[dummy(faker = "-1_000_000..1_000_000")]
    pub amount: i64,
    #[dummy(faker = "1..100")]
    pub account_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Dummy)]
pub struct Dispute {
    #[dummy(faker = "1..1_000_000")]
    pub transaction_id: u32,
    #[dummy(faker = "1..100")]
    pub account_id: u32,
    pub stage: DisputeStage,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Account {
    pub id: u32,
    pub balance: i64,
    pub frozen_balance: u64,
    pub locked: bool,
}
