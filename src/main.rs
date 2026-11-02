use std::{fs::File, io, path::PathBuf};

use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rust_decimal::Decimal;

use simple_ledger::csv::CsvRow;
use simple_ledger::{Ledger, SledLedger};

/// Ingest a ledger CSV (transactions and disputes) into the sled-backed ledger.
#[derive(Parser)]
struct Args {
    /// Path to the CSV file with `type,client,tx,amount` rows to ingest
    input: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let ledger = SledLedger::open()?;
    let input = File::open(&args.input)?;
    let progress = ProgressBar::new(input.metadata()?.len());

    progress.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({percent}%) {msg}",
        )?
        .progress_chars("=>-"),
    );

    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .flexible(true)
        .from_reader(input);
    let mut rows = reader.deserialize::<CsvRow>();

    let mut index = 0;
    while let Some(row) = rows.next() {
        index += 1;
        let result = match row {
            Ok(CsvRow::Transaction(tx)) => ledger.transact(tx),
            Ok(CsvRow::Dispute(dispute)) => ledger.dispute(dispute),
            Err(err) if err.is_io_error() => {
                progress.abandon_with_message("ingestion failed");
                return Err(err.into());
            }
            Err(err) => Err(err.into()),
        };
        if let Err(err) = result {
            progress.suspend(|| eprintln!("skipping record {index}: {err}"));
        }
        progress.set_position(rows.reader().position().byte());
    }
    progress.finish_with_message("done");

    let mut writer = csv::WriterBuilder::new()
        .has_headers(false)
        .from_writer(io::stdout());
    writer.write_record(["client", "available", "held", "total", "locked"])?;
    for account in ledger.accounts() {
        let account = account?;
        let available = i128::from(account.balance);
        let held = i128::from(account.frozen_balance);
        writer.serialize((
            account.id,
            Decimal::from_i128_with_scale(available, 4).to_string(),
            Decimal::from_i128_with_scale(held, 4).to_string(),
            Decimal::from_i128_with_scale(available + held, 4).to_string(),
            account.locked,
        ))?;
    }
    writer.flush()?;
    Ok(())
}
