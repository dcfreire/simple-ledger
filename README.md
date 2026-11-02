# Simple ledger

Process transactions and disputes from a CSV file. Requires Rust and Cargo.

## Usage

Run `cargo run --release -- transactions.csv > accounts.csv`.

Input columns are `type,client,tx,amount`. Supported types are `deposit`, `withdrawal`, `dispute`, `resolve`, and `chargeback`. Amounts are unsigned decimals with up to four fractional digits; dispute, resolve, and chargeback rows leave the amount empty. Transaction IDs must be unique.

```csv
type,client,tx,amount
deposit,1,1,2.0000
dispute,1,1,
resolve,1,1,
withdrawal,1,2,0.5000
```

Output columns are `client,available,held,total,locked`. Progress and skipped-row errors are written to stderr.

The ledger persists in `./data`. Later runs reuse those accounts and transaction IDs.

Tests and this README were generated with pi.dev using GPT 6 Terra.
