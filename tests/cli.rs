use std::{fs, path::Path, process::Command};

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
    fs::write(dir.join("input.csv"), input)?;
    let output = Command::new(env!("CARGO_BIN_EXE_simple-ledger"))
        .current_dir(dir)
        .arg("input.csv")
        .output()?;
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
fn domain_records_work_across_runs() -> anyhow::Result<()> {
    let dir = tempfile::tempdir()?;
    let (accounts, errors) = run_at(
        dir.path(),
        "\
type,client,tx,amount
deposit,1,1,2.5000
withdrawal,1,5,1.2501
deposit,2,2,3.0000
dispute,2,2
deposit,3,3,1.2501
dispute,3,3
",
    )?;
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(
        accounts,
        vec![
            account(1, "1.2499", "0.0000", "1.2499", false),
            account(2, "0.0000", "3.0000", "3.0000", false),
            account(3, "0.0000", "1.2501", "1.2501", false),
        ]
    );

    // Stored records use domain fields.
    {
        let db = sled::open(dir.path().join("data"))?;
        let tx = db
            .open_tree("transactions")?
            .get(3_u32.to_le_bytes())?
            .unwrap();
        let dispute = db.open_tree("disputes")?.get(3_u32.to_le_bytes())?.unwrap();
        let withdrawal = db
            .open_tree("transactions")?
            .get(5_u32.to_le_bytes())?
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&tx)?,
            serde_json::json!({"id":3,"account_id":3,"amount":12501})
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&withdrawal)?,
            serde_json::json!({"id":5,"account_id":1,"amount":-12501})
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&dispute)?,
            serde_json::json!({"transaction_id":3,"account_id":3,"stage":"dispute"})
        );
    }

    let (accounts, errors) = run_at(
        dir.path(),
        "\
type,client,tx,amount
resolve,2,2
resolve,3,3
",
    )?;
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(
        accounts,
        vec![
            account(1, "1.2499", "0.0000", "1.2499", false),
            account(2, "3.0000", "0.0000", "3.0000", false),
            account(3, "1.2501", "0.0000", "1.2501", false),
        ]
    );
    Ok(())
}

#[test]
fn empty_input_still_outputs_headers() -> anyhow::Result<()> {
    assert!(run("type,client,tx,amount\n")?.is_empty());
    Ok(())
}
