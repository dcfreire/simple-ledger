use std::{marker::PhantomData, path::Path};

use serde::{Serialize, de::DeserializeOwned};
use sled::{Transactional, transaction::ConflictableTransactionError};

use crate::types::{Account, Dispute, Transaction};

fn read_stored<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, sled::Error> {
    serde_json::from_slice(bytes)
        .map_err(|error| sled::Error::Unsupported(format!("invalid stored record: {error}")))
}

struct Db<T: Serialize + DeserializeOwned> {
    db: sled::Tree,
    _phantom: PhantomData<T>,
}

impl<T: Serialize + DeserializeOwned> Db<T> {
    fn open(tree: sled::Tree) -> Self {
        Self {
            db: tree,
            _phantom: PhantomData,
        }
    }

    fn iter(&self) -> DbIter<T> {
        DbIter {
            inner: self.db.iter(),
            _phantom: PhantomData,
        }
    }
}

/// Iterator over the values stored in a [`Db<T>`]
struct DbIter<T> {
    inner: sled::Iter,
    _phantom: PhantomData<T>,
}

impl<T: DeserializeOwned> Iterator for DbIter<T> {
    type Item = anyhow::Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|entry| {
            let (_, value) = entry.map_err(anyhow::Error::from)?;
            serde_json::from_slice(&value).map_err(anyhow::Error::from)
        })
    }
}

impl<T: Serialize + DeserializeOwned> IntoIterator for &Db<T> {
    type Item = anyhow::Result<T>;
    type IntoIter = DbIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Operations exposed by the ledger
pub trait Ledger {
    /// Apply a transaction (deposit/withdrawal) to the ledger
    fn transact(&self, tx: Transaction) -> anyhow::Result<()>;

    /// Apply a dispute stage (dispute/resolve/chargeback) to a transaction
    fn dispute(&self, dispute: Dispute) -> anyhow::Result<()>;

    /// Iterate over all accounts in the ledger
    fn accounts(&self) -> Box<dyn Iterator<Item = anyhow::Result<Account>> + '_>;
}

/// Storage implementation for the ledger
pub struct SledLedger {
    /// account's cumulative state
    accounts: Db<Account>,
    /// append-only transaction db
    transactions: Db<Transaction>,
    /// ongoing disputes db
    disputes: Db<Dispute>,
}

impl SledLedger {
    pub fn open() -> anyhow::Result<Self> {
        Self::open_at("./data")
    }

    pub fn open_at(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let db = sled::open(path)?;
        let accounts = Db::open(db.open_tree("accounts")?);
        let transactions = Db::open(db.open_tree("transactions")?);
        let disputes = Db::open(db.open_tree("disputes")?);

        Ok(Self {
            accounts,
            transactions,
            disputes,
        })
    }

    fn transact(&self, tx: Transaction) -> anyhow::Result<()> {
        let account_key = tx.account_id.to_le_bytes();
        let tx_key = tx.id.to_le_bytes();

        (&self.accounts.db, &self.transactions.db).transaction(
            |(tx_accounts, tx_transactions)| {
                if tx_transactions.get(&tx_key)?.is_some() {
                    Err(sled::Error::Unsupported(
                        "a transaction with given id alrady exists".to_string(),
                    ))?;
                }

                let mut account: Account = tx_accounts
                    .get(&account_key)?
                    .map(|v| serde_json::from_slice(&v).unwrap())
                    .unwrap_or(Account {
                        id: tx.account_id,
                        ..Default::default()
                    });

                if account.locked {
                    Err(sled::Error::Unsupported(
                        "locked accounts cannot transact".to_string(),
                    ))?;
                }

                if tx.amount < 0
                    && (account.balance < 0 || account.balance.unsigned_abs() < tx.amount.unsigned_abs())
                {
                    Err(sled::Error::Unsupported(
                        "accounts with negative balance cannot perform a withdrawal and a withdrawal cannot leave the account balance negative".to_string(),
                    ))?;
                }

                account.balance =
                    account
                        .balance
                        .checked_add(tx.amount)
                        .ok_or(sled::Error::Unsupported(
                            "account balance would be < i64::MIN or > i64::MAX".to_string(),
                        ))?;

                let account_bytes = serde_json::to_vec(&account).map_err(|_| {
                    sled::Error::Unsupported("serde_json serialize failed".to_string())
                })?;

                let tx_bytes = serde_json::to_vec(&tx).map_err(|_| {
                    sled::Error::Unsupported("serde_json serialize failed".to_string())
                })?;
                tx_transactions.insert(&tx_key, tx_bytes)?;
                tx_accounts.insert(&account_key, account_bytes)?;

                Ok::<(), ConflictableTransactionError<sled::Error>>(())
            },
        )?;

        Ok(())
    }

    fn dispute(&self, dispute: Dispute) -> anyhow::Result<()> {
        let tx_key = dispute.transaction_id.to_le_bytes();

        (&self.accounts.db, &self.transactions.db, &self.disputes.db).transaction(
            |(tx_accounts, tx_transactions, tx_disputes)| {
                let tx: Transaction = tx_transactions
                    .get(&tx_key)?
                    .map(|bytes| read_stored(&bytes))
                    .transpose()?
                    .ok_or(sled::Error::Unsupported(
                        "disputed transaction does not exist".to_string(),
                    ))?;

                // This was not specified, but assumed for 2 reasons:
                // 1. In practice, disputing a withdrawal is only effective for locking an account,
                // since by the time the withdrawal executed, in most systems, the actual balance would've
                // already been transfered to another bank.
                // 2. The mechanism is not clear, if disputing a withdrawal has the same effects as disputing a deposit then
                // this would mean increasing the client's available balance and decreasing the held balance,
                // which sounds non-sensiscal.
                // (the user could then withdraw the same ammount again since accounts are not locked during disputes for instance)
                if tx.amount < 0 {
                    Err(sled::Error::Unsupported(
                        "withdrawals cannot be disputed".to_string(),
                    ))?;
                }

                let ongoing_dispute: Option<Dispute> = tx_disputes
                    .remove(&tx_key)?
                    .map(|bytes| read_stored(&bytes))
                    .transpose()?;

                let account_key = tx.account_id.to_le_bytes();
                let mut account: Account = tx_accounts
                    .get(&account_key)?
                    .map(|v| serde_json::from_slice(&v).unwrap())
                    .ok_or(sled::Error::Unsupported(
                        "disputed account does not exist".to_string(),
                    ))?;

                match dispute.stage {
                    crate::types::DisputeStage::Dispute => {
                        if ongoing_dispute.is_some() {
                            Err(sled::Error::Unsupported(
                                "this transaction is already being disputed".to_string(),
                            ))?;
                        }

                        account.balance = account.balance.checked_sub(tx.amount).ok_or(
                            sled::Error::Unsupported(
                                "account balance would be < i64::MIN".to_string(),
                            ),
                        )?;
                        account.frozen_balance = account
                            .frozen_balance
                            .checked_add_signed(tx.amount)
                            .ok_or(sled::Error::Unsupported(
                                "account frozen balance would be > u64::MAX".to_string(),
                            ))?;

                        let dispute_bytes = serde_json::to_vec(&dispute).map_err(|_| {
                            sled::Error::Unsupported("serde_json serialize failed".to_string())
                        })?;

                        tx_disputes.insert(&tx_key, dispute_bytes)?;
                    }
                    crate::types::DisputeStage::Resolve => {
                        if ongoing_dispute.is_none() {
                            Err(sled::Error::Unsupported(
                                "there is no dispute to be resolved".to_string(),
                            ))?;
                        }

                        account.balance = account.balance.checked_add(tx.amount).ok_or(
                            sled::Error::Unsupported(
                                "account balance would be > i64::MAX".to_string(),
                            ),
                        )?;
                        account.frozen_balance = account
                            .frozen_balance
                            .checked_add_signed(-tx.amount)
                            .ok_or(sled::Error::Unsupported(
                                "account frozen balance would be < 0".to_string(),
                            ))?;
                    }

                    crate::types::DisputeStage::Chargeback => {
                        if ongoing_dispute.is_none() {
                            Err(sled::Error::Unsupported(
                                "there is no dispute to trigger a chargeback".to_string(),
                            ))?;
                        }

                        account.locked = true;
                        account.frozen_balance = account
                            .frozen_balance
                            .checked_add_signed(-tx.amount)
                            .ok_or(sled::Error::Unsupported(
                                "account frozen balance would be < 0".to_string(),
                            ))?;
                    }
                }

                let account_bytes = serde_json::to_vec(&account).map_err(|_| {
                    sled::Error::Unsupported("serde_json serialize failed".to_string())
                })?;

                tx_accounts.insert(&account_key, account_bytes)?;

                Ok::<(), ConflictableTransactionError<sled::Error>>(())
            },
        )?;

        Ok(())
    }
}

impl Ledger for SledLedger {
    fn transact(&self, tx: Transaction) -> anyhow::Result<()> {
        SledLedger::transact(self, tx)
    }

    fn dispute(&self, dispute: Dispute) -> anyhow::Result<()> {
        SledLedger::dispute(self, dispute)
    }

    fn accounts(&self) -> Box<dyn Iterator<Item = anyhow::Result<Account>> + '_> {
        Box::new(self.accounts.iter())
    }
}
