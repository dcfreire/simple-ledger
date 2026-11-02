//! CSV adapters

use anyhow::bail;
use rust_decimal::Decimal;
use serde::Deserialize;

use crate::types::{Dispute, DisputeStage, Transaction};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "CsvRecord")]
pub enum CsvRow {
    Transaction(Transaction),
    Dispute(Dispute),
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Deposit,
    Withdrawal,
    Dispute,
    Resolve,
    Chargeback,
}

#[derive(Deserialize)]
pub(crate) struct CsvRecord {
    #[serde(rename = "type")]
    kind: Kind,
    client: u32,
    tx: u32,
    #[serde(default)]
    amount: Option<String>,
}

impl TryFrom<CsvRecord> for CsvRow {
    type Error = anyhow::Error;

    fn try_from(row: CsvRecord) -> Result<Self, Self::Error> {
        match row.kind {
            Kind::Deposit | Kind::Withdrawal => Transaction::try_from(row).map(Self::Transaction),
            _ => Dispute::try_from(row).map(Self::Dispute),
        }
    }
}

impl TryFrom<CsvRecord> for Transaction {
    type Error = anyhow::Error;

    fn try_from(row: CsvRecord) -> Result<Self, Self::Error> {
        let absolute_amount = i128::from(parse_amount(row.amount.as_deref().unwrap_or_default())?);

        let amount = match row.kind {
            Kind::Deposit => absolute_amount,
            Kind::Withdrawal => -absolute_amount,
            _ => bail!("expected deposit or withdrawal"),
        };

        Ok(Self {
            id: row.tx,
            account_id: row.client,
            amount: amount.try_into()?,
        })
    }
}

impl TryFrom<CsvRecord> for Dispute {
    type Error = anyhow::Error;

    fn try_from(row: CsvRecord) -> Result<Self, Self::Error> {
        let stage = match row.kind {
            Kind::Dispute => DisputeStage::Dispute,
            Kind::Resolve => DisputeStage::Resolve,
            Kind::Chargeback => DisputeStage::Chargeback,
            _ => bail!("expected dispute, resolve or chargeback"),
        };

        Ok(Self {
            transaction_id: row.tx,
            account_id: row.client,
            stage,
        })
    }
}

fn parse_amount(raw: &str) -> anyhow::Result<u64> {
    let raw = raw.trim();
    let mut value = Decimal::from_str_exact(raw)?;
    value.rescale(4);

    Ok(value.mantissa().try_into()?)
}
