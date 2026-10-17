//! Integration tests for compact-terminal CLI.

use std::path::Path;

use assert_cmd::Command;
use serde_json::Value;
use tempfile::TempDir;

#[test]
fn test_run_short_output() {
    let mut cmd = Command::cargo_bin("compact-terminal").unwrap();
    cmd.arg("run").arg("echo hello");

    let output = cmd.output().unwrap();
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["kind"], "inline");
    assert_eq!(json["exit_code"], 0);
    assert!(json["stdout"].as_str().unwrap().contains("hello"));
}

#[test]
fn test_run_with_custom_inline_limit() {
    let mut cmd = Command::cargo_bin("compact-terminal").unwrap();
    cmd.arg("run")
        .arg("echo hello")
        .arg("--inline-limit")
        .arg("10");

    let output = cmd.output().unwrap();
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    // With such a small limit, it should be spilled
    assert!(json["kind"] == "spilled" || json["kind"] == "inline");
}

#[test]
fn test_run_with_timeout() {
    let mut cmd = Command::cargo_bin("compact-terminal").unwrap();
    cmd.arg("run").arg("echo quick").arg("--timeout").arg("5");

    let output = cmd.output().unwrap();
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["kind"], "inline");
    assert_eq!(json["exit_code"], 0);
}

#[test]
fn test_run_creates_spill_file_for_long_output() {
    let temp_dir = TempDir::new().unwrap();
    let spill_dir = temp_dir.path();

    let mut cmd = Command::cargo_bin("compact-terminal").unwrap();
    cmd.arg("run")
        .arg("seq 1 1000")
        .arg("--inline-limit")
        .arg("100")
        .arg("--spill-dir")
        .arg(spill_dir);

    let output = cmd.output().unwrap();
    assert!(output.status.success());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["kind"], "spilled");

    let spill_file = json["spill_file"].as_str().unwrap();
    assert!(Path::new(spill_file).exists());
}

#[test]
fn test_read_spill() {
    // First run a command that generates a spill file
    let temp_dir = TempDir::new().unwrap();
    let spill_dir = temp_dir.path();

    let mut run_cmd = Command::cargo_bin("compact-terminal").unwrap();
    run_cmd
        .arg("run")
        .arg("seq 1 100")
        .arg("--inline-limit")
        .arg("50")
        .arg("--spill-dir")
        .arg(spill_dir);

    let run_output = run_cmd.output().unwrap();
    assert!(run_output.status.success());

    let run_json: Value = serde_json::from_slice(&run_output.stdout).unwrap();
    assert_eq!(run_json["kind"], "spilled");
    let spill_file = run_json["spill_file"].as_str().unwrap();

    // Now read from the spill file
    let mut read_cmd = Command::cargo_bin("compact-terminal").unwrap();
    read_cmd
        .arg("read-spill")
        .arg(spill_file)
        .arg("--start")
        .arg("1")
        .arg("--end")
        .arg("5");

    let read_output = read_cmd.output().unwrap();
    assert!(read_output.status.success());

    let content = String::from_utf8(read_output.stdout).unwrap();
    assert!(content.contains("1\n"));
    assert!(content.contains("2\n"));
}

#[test]
fn test_read_spill_with_grep() {
    // First run a command that generates a spill file
    let temp_dir = TempDir::new().unwrap();
    let spill_dir = temp_dir.path();

    let mut run_cmd = Command::cargo_bin("compact-terminal").unwrap();
    run_cmd
        .arg("run")
        .arg("seq 1 100")
        .arg("--inline-limit")
        .arg("50")
        .arg("--spill-dir")
        .arg(spill_dir);

    let run_output = run_cmd.output().unwrap();
    assert!(run_output.status.success());

    let run_json: Value = serde_json::from_slice(&run_output.stdout).unwrap();
    assert_eq!(run_json["kind"], "spilled");
    let spill_file = run_json["spill_file"].as_str().unwrap();

    // Search for pattern
    let mut read_cmd = Command::cargo_bin("compact-terminal").unwrap();
    read_cmd
        .arg("read-spill")
        .arg(spill_file)
        .arg("--grep")
        .arg("10");

    let read_output = read_cmd.output().unwrap();
    assert!(read_output.status.success());

    let content = String::from_utf8(read_output.stdout).unwrap();
    // Grep returns line numbers, and "10" should match lines 10, 100
    assert!(content.contains("matches (line numbers):"));
    assert!(content.contains("10"));
}
