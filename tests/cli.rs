use std::{
    fs,
    path::Path,
    process::{Command, Output, Stdio},
};

use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq)]
struct AccountOutput {
    client: u32,
    available: String,
    held: String,
    total: String,
    locked: bool,
}

fn account(client: u32, available: &str, held: &str, total: &str, locked: bool) -> AccountOutput {
    AccountOutput {
        client,
        available: available.into(),
        held: held.into(),
        total: total.into(),
        locked,
    }
}

fn run(input: &str) -> anyhow::Result<Vec<AccountOutput>> {
    let dir = tempfile::tempdir()?;
    Ok(run_at(dir.path(), input)?.0)
}

fn run_at(dir: &Path, input: &str) -> anyhow::Result<(Vec<AccountOutput>, String)> {
    run_at_with_args(dir, input, &[])
}

fn run_at_with_args(
    dir: &Path,
    input: &str,
    args: &[&str],
) -> anyhow::Result<(Vec<AccountOutput>, String)> {
    fs::write(dir.join("input.csv"), input)?;
    let output = Command::new(env!("CARGO_BIN_EXE_simple-ledger"))
        .current_dir(dir)
        .args(args)
        .arg("input.csv")
        .output()?;
    read_output(output)
}

fn read_output(output: Output) -> anyhow::Result<(Vec<AccountOutput>, String)> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mut reader = csv::Reader::from_reader(output.stdout.as_slice());
    assert_eq!(
        reader.headers()?,
        &vec!["client", "available", "held", "total", "locked"]
    );
    let mut accounts = reader
        .deserialize()
        .collect::<Result<Vec<AccountOutput>, _>>()?;
    accounts.sort_unstable_by_key(|account| account.client);
    Ok((accounts, String::from_utf8(output.stderr)?))
}

#[test]
fn deposits_and_withdrawals_update_each_account() -> anyhow::Result<()> {
    let accounts = run("\
type, client, tx, amount
deposit, 1, 1, 1.2345
deposit, 2, 2, 2.0
withdrawal, 1, 3, 0.2345
")?;
    assert_eq!(
        accounts,
        vec![
            account(1, "1.0000", "0.0000", "1.0000", false),
            account(2, "2.0000", "0.0000", "2.0000", false),
        ]
    );
    Ok(())
}

#[test]
fn rejected_rows_are_skipped_and_processing_continues() -> anyhow::Result<()> {
    let accounts = run("\
type,client,tx,amount
deposit,1,1,1.0
withdrawal,1,2,2.0
deposit,1,3,-10.0
dispute,1,999
deposit,1,4,2.0
withdrawal,1,5,0.5
")?;
    assert_eq!(
        accounts,
        vec![account(1, "2.5000", "0.0000", "2.5000", false)]
    );
    Ok(())
}

#[test]
fn disputes_hold_release_or_remove_funds() -> anyhow::Result<()> {
    let accounts = run("\
type,client,tx,amount
deposit,1,1,10.0
dispute,1,1
resolve,1,1
deposit,2,2,7.0
deposit,2,3,3.0
dispute,2,2,
chargeback,2,2,
deposit,3,4,5.0
dispute,3,4
")?;
    assert_eq!(
        accounts,
        vec![
            account(1, "10.0000", "0.0000", "10.0000", false),
            account(2, "3.0000", "0.0000", "3.0000", true),
            account(3, "0.0000", "5.0000", "5.0000", false),
        ]
    );
    Ok(())
}

#[test]
fn negative_available_and_large_held_amounts_are_exact() -> anyhow::Result<()> {
    let accounts = run("\
type,client,tx,amount
deposit,1,1,1.0001
withdrawal,1,2,1.0001
dispute,1,1
deposit,2,3,922337203685477.5807
dispute,2,3
deposit,2,4,922337203685477.5807
dispute,2,4
deposit,2,5,922337203685477.5807
")?;
    assert_eq!(
        accounts,
        vec![
            account(1, "-1.0001", "1.0001", "0.0000", false),
            account(
                2,
                "922337203685477.5807",
                "1844674407370955.1614",
                "2767011611056432.7421",
                false
            ),
        ]
    );
    Ok(())
}

#[test]
fn repeated_runs_start_with_empty_ledgers() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (first, first_errors) = run_at(dir.path(), "type,client,tx,amount\ndeposit,1,1,2.5000\n")?;
    let (second, second_errors) =
        run_at(dir.path(), "type,client,tx,amount\ndeposit,1,1,3.0000\n")?;
    assert!(first_errors.is_empty(), "{first_errors}");
    assert!(second_errors.is_empty(), "{second_errors}");
    assert_eq!(first, vec![account(1, "2.5000", "0.0000", "2.5000", false)]);
    assert_eq!(
        second,
        vec![account(1, "3.0000", "0.0000", "3.0000", false)]
    );
    Ok(())
}

#[test]
fn persistent_flag_reuses_ledger_across_runs() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (first, first_errors) = run_at_with_args(
        dir.path(),
        "type,client,tx,amount\ndeposit,1,1,2.5000\n",
        &["--persistent"],
    )?;
    let (second, second_errors) = run_at_with_args(
        dir.path(),
        "type,client,tx,amount\ndeposit,1,2,1.5000\ndispute,1,1\n",
        &["--persistent"],
    )?;
    assert!(first_errors.is_empty(), "{first_errors}");
    assert!(second_errors.is_empty(), "{second_errors}");
    assert_eq!(first, vec![account(1, "2.5000", "0.0000", "2.5000", false)]);
    assert_eq!(
        second,
        vec![account(1, "1.5000", "2.5000", "4.0000", false)]
    );
    assert!(dir.path().join("data").is_dir());
    Ok(())
}

#[test]
fn concurrent_runs_ignore_existing_database_lock() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let temp_root = dir.path().join("temp");
    fs::create_dir(&temp_root)?;
    // A database locked at the old default path must not affect either process.
    let _locked_db = sled::open(dir.path().join("data"))?;
    fs::write(
        dir.path().join("first.csv"),
        "type,client,tx,amount\ndeposit,1,1,1.0000\n",
    )?;
    fs::write(
        dir.path().join("second.csv"),
        "type,client,tx,amount\ndeposit,1,1,2.0000\n",
    )?;

    let spawn = |input: &str| {
        Command::new(env!("CARGO_BIN_EXE_simple-ledger"))
            .current_dir(dir.path())
            .env("TMPDIR", &temp_root)
            .arg(input)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    };
    let first = spawn("first.csv")?;
    let second = spawn("second.csv")?;
    let (first, first_errors) = read_output(first.wait_with_output()?)?;
    let (second, second_errors) = read_output(second.wait_with_output()?)?;
    assert!(first_errors.is_empty(), "{first_errors}");
    assert!(second_errors.is_empty(), "{second_errors}");
    assert_eq!(first, vec![account(1, "1.0000", "0.0000", "1.0000", false)]);
    assert_eq!(
        second,
        vec![account(1, "2.0000", "0.0000", "2.0000", false)]
    );
    assert_eq!(
        fs::read_dir(temp_root)?.count(),
        0,
        "temporary databases should be removed"
    );
    Ok(())
}

#[test]
fn empty_input_still_outputs_headers() -> anyhow::Result<()> {
    assert!(run("type,client,tx,amount\n")?.is_empty());
    Ok(())
}
