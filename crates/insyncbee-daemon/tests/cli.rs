//! Smoke tests for the `insyncbee` CLI binary.
//!
//! Each test runs the real built binary in an isolated `HOME` (and
//! `XDG_DATA_HOME`) so it never touches the developer's database.

use assert_cmd::prelude::*;
use predicates::str::contains;
use std::process::Command;
use tempfile::TempDir;

/// Build a `Command` for the `insyncbee` binary with isolated data dirs.
/// Returns the tempdir so callers can keep it alive for the command's lifetime.
fn cmd() -> (Command, TempDir) {
    let tmp = TempDir::new().expect("tempdir");
    let mut c = Command::cargo_bin("insyncbee").expect("binary built");
    c.env("HOME", tmp.path());
    c.env("XDG_DATA_HOME", tmp.path());
    // macOS dirs::data_dir() reads from $HOME, but env_remove'ing OAuth env
    // vars is good hygiene so a developer running tests with their own creds
    // doesn't accidentally trigger network auth.
    c.env_remove("INSYNCBEE_CLIENT_ID");
    c.env_remove("INSYNCBEE_CLIENT_SECRET");
    (c, tmp)
}

#[test]
fn help_lists_subcommands() {
    let (mut c, _t) = cmd();
    c.arg("--help")
        .assert()
        .success()
        .stdout(contains("Add a new sync pair"))
        .stdout(contains("Run a sync cycle now"))
        .stdout(contains("Run as a background daemon"));
}

#[test]
fn version_includes_a_semver_string() {
    let (mut c, _t) = cmd();
    c.arg("--version")
        .assert()
        .success()
        .stdout(predicates::str::is_match(r"insyncbee \d+\.\d+\.\d+").unwrap());
}

#[test]
fn list_with_empty_db_says_no_pairs() {
    let (mut c, _t) = cmd();
    c.arg("list")
        .assert()
        .success()
        .stdout(contains("No sync pairs configured"));
}

#[test]
fn status_with_empty_db_says_no_pairs() {
    let (mut c, _t) = cmd();
    c.arg("status")
        .assert()
        .success()
        .stdout(contains("No sync pairs configured"));
}

#[test]
fn add_then_list_then_pause_then_remove() {
    let tmp = TempDir::new().unwrap();
    let local = tmp.path().join("local-root");
    let make_cmd = || {
        let mut c = Command::cargo_bin("insyncbee").unwrap();
        c.env("HOME", tmp.path());
        c.env("XDG_DATA_HOME", tmp.path());
        c.env_remove("INSYNCBEE_CLIENT_ID");
        c.env_remove("INSYNCBEE_CLIENT_SECRET");
        c
    };

    // The `add` subcommand inserts a sync_pair that references an account
    // via FK, so seed an account in the same on-disk DB the binary will use.
    let db_path = tmp.path().join("insyncbee/insyncbee.db");
    std::fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let db = insyncbee_core::db::Database::open(&db_path).unwrap();
    let acc = insyncbee_core::db::models::Account {
        id: "cli-acct".into(),
        email: "cli@example.com".into(),
        display_name: None,
        access_token: "a".into(),
        refresh_token: "r".into(),
        token_expiry: "2030-01-01T00:00:00Z".into(),
        change_token: None,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    db.with_conn(|c| acc.insert(c)).unwrap();
    drop(db);

    make_cmd()
        .arg("add")
        .args(["--name", "Pictures"])
        .args(["--local", local.to_str().unwrap()])
        .args(["--remote-id", "remote-root"])
        .args(["--account", "cli-acct"])
        .args(["--mode", "two-way"])
        .assert()
        .success()
        .stdout(contains("Sync pair 'Pictures' created"));

    let listed = make_cmd().arg("list").output().unwrap();
    let stdout = String::from_utf8_lossy(&listed.stdout);
    assert!(stdout.contains("Pictures"), "list should show new pair, got: {stdout}");

    // Extract pair UUID from list output.
    let pair_id = stdout
        .lines()
        .next()
        .unwrap()
        .split('(')
        .nth(1)
        .and_then(|s| s.split(')').next())
        .unwrap();

    make_cmd()
        .args(["pause", pair_id])
        .assert()
        .success()
        .stdout(contains("paused"));

    make_cmd()
        .args(["resume", pair_id])
        .assert()
        .success()
        .stdout(contains("resumed"));

    make_cmd()
        .args(["remove", pair_id])
        .assert()
        .success()
        .stdout(contains("removed"));

    make_cmd()
        .arg("list")
        .assert()
        .success()
        .stdout(contains("No sync pairs configured"));
}

#[test]
fn unknown_subcommand_exits_nonzero() {
    let (mut c, _t) = cmd();
    c.arg("definitely-not-a-real-subcommand").assert().failure();
}
