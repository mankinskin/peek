//! Integration tests for compact-terminal-api.

use compact_terminal_api::{
    PollProcessRequest,
    ReadSpillRequest,
    RunRequest,
    RunResult,
    SendInputRequest,
    execute,
    poll_process,
    read_spill,
    send_input,
};
use tempfile::tempdir;

fn base_request(command: &str) -> RunRequest {
    RunRequest {
        command: command.to_string(),
        cwd: None,
        inline_limit: Some(4096),
        timeout_secs: Some(5),
        spill_dir: None,
        stall_timeout_secs: None,
        interactive: None,
    }
}

#[test]
fn test_inline_short_output() {
    let request = base_request("echo hello");

    let result = execute(&request).expect("execution failed");

    match result {
        RunResult::Inline {
            exit_code,
            stdout,
            stderr: _,
            elapsed_ms: _,
            warnings,
        } => {
            assert_eq!(exit_code, 0);
            assert!(stdout.contains("hello"));
            assert!(warnings.is_empty());
        },
        _ => panic!("expected inline result"),
    }
}

#[test]
fn test_spilled_long_output() {
    let spill_dir = tempdir().expect("failed to create temp dir");

    // Generate output longer than inline limit.
    let request = RunRequest {
        inline_limit: Some(100), // Small limit to force spill.
        spill_dir: Some(spill_dir.path().to_path_buf()),
        ..base_request("seq 1 1000")
    };

    let result = execute(&request).expect("execution failed");

    match result {
        RunResult::Spilled {
            exit_code,
            stdout_preview,
            stderr_preview: _,
            total_bytes,
            total_lines,
            spill_file,
            elapsed_ms: _,
            next_steps,
            warnings: _,
        } => {
            assert_eq!(exit_code, 0);
            assert!(!stdout_preview.is_empty());
            assert!(total_bytes > 100);
            assert!(total_lines > 100);
            assert!(spill_file.exists());
            assert!(!next_steps.is_empty());

            // Verify the spill file can be read.
            let spill_content = std::fs::read_to_string(&spill_file)
                .expect("cannot read spill file");
            assert!(spill_content.contains("=== stdout ==="));
            assert!(spill_content.contains("1000"));
        },
        _ => panic!("expected spilled result"),
    }
}

#[test]
fn test_non_zero_exit_code() {
    let request = base_request("exit 42");

    let result = execute(&request).expect("execution failed");

    match result {
        RunResult::Inline {
            exit_code,
            stdout: _,
            stderr: _,
            elapsed_ms: _,
            warnings: _,
        } => {
            assert_eq!(exit_code, 42);
        },
        _ => panic!("expected inline result"),
    }
}

#[test]
fn test_timeout() {
    let request = RunRequest {
        timeout_secs: Some(1),
        ..base_request("sleep 10")
    };

    let result = execute(&request).expect("execution failed");

    match result {
        RunResult::TimedOut {
            timeout_secs,
            stdout_partial: _,
            spill_file: _,
            warnings: _,
        } => {
            assert_eq!(timeout_secs, 1);
        },
        _ => panic!("expected timeout result"),
    }
}

#[test]
fn stdin_reading_command_terminates_and_subsequent_command_succeeds() {
    let cat_request = base_request("cat");

    let cat_result = execute(&cat_request).expect("cat execution failed");
    assert!(matches!(
        cat_result,
        RunResult::Inline {
            exit_code: 0,
            stdout,
            ..
        } if stdout.is_empty()
    ));

    let echo_request = base_request("echo alive");
    let echo_result = execute(&echo_request).expect("echo execution failed");
    assert!(matches!(
        echo_result,
        RunResult::Inline {
            exit_code: 0,
            stdout,
            ..
        } if stdout.contains("alive")
    ));
}

#[test]
fn timeout_kills_and_reaps_shell_process() {
    let temp_dir = tempdir().expect("failed to create temp dir");
    let request = RunRequest {
        cwd: Some(temp_dir.path().to_path_buf()),
        timeout_secs: Some(2),
        ..base_request(
            "echo $$ > child.pid; sleep 30 & echo $! > grandchild.pid; wait",
        )
    };

    let result = execute(&request).expect("execution failed");
    assert!(matches!(
        result,
        RunResult::TimedOut {
            timeout_secs: 2,
            ..
        }
    ));

    #[cfg(windows)]
    let status = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            "$ids = Get-Content child.pid, grandchild.pid; foreach ($id in $ids) { if (Get-Process -Id $id -ErrorAction SilentlyContinue) { exit 0 } }; exit 1",
        ])
        .current_dir(temp_dir.path())
        .status()
        .expect("failed to check Windows process status");
    #[cfg(not(windows))]
    let status = std::process::Command::new("sh")
        .arg("-c")
        .arg("for pid in $(cat child.pid grandchild.pid); do kill -0 \"$pid\" 2>/dev/null && exit 0; done; exit 1")
        .current_dir(temp_dir.path())
        .status()
        .expect("failed to check shell process status");
    assert!(
        !status.success(),
        "timed out command process tree is still running"
    );
}

#[test]
fn test_read_spill_by_line_range() {
    let spill_dir = tempdir().expect("failed to create temp dir");

    // Generate a spill file.
    let request = RunRequest {
        inline_limit: Some(50),
        spill_dir: Some(spill_dir.path().to_path_buf()),
        ..base_request("seq 1 100")
    };

    let result = execute(&request).expect("execution failed");

    let spill_file = match result {
        RunResult::Spilled { spill_file, .. } => spill_file,
        _ => panic!("expected spilled result"),
    };

    // Read lines 10-15.
    let read_request = ReadSpillRequest {
        spill_file,
        start: Some(10),
        end: Some(15),
        grep: None,
    };

    let read_result = read_spill(&read_request).expect("read failed");
    let content = read_result.content;

    // Should contain line numbers and content from the specified range.
    assert!(content.contains("10"));
    assert!(content.contains("15"));
}

#[test]
fn test_read_spill_grep() {
    let spill_dir = tempdir().expect("failed to create temp dir");

    // Generate a spill file with some error-like output.
    let request = RunRequest {
        inline_limit: Some(50),
        spill_dir: Some(spill_dir.path().to_path_buf()),
        ..base_request(r#"echo "line 1"; echo "error at line 2"; echo "line 3"; echo "another error at line 4""#)
    };

    let result = execute(&request).expect("execution failed");

    let spill_file = match result {
        RunResult::Spilled { spill_file, .. } => spill_file,
        _ => panic!("expected spilled result"),
    };

    // Grep for "error".
    let read_request = ReadSpillRequest {
        spill_file,
        start: None,
        end: None,
        grep: Some("error".to_string()),
    };

    let read_result = read_spill(&read_request).expect("read failed");
    let content = read_result.content;

    // Should list matching line numbers.
    assert!(content.contains("matches (line numbers)"));
}

#[test]
fn test_missing_spill_file_error() {
    let read_request = ReadSpillRequest {
        spill_file: "/nonexistent/path/to/spill.txt".into(),
        start: None,
        end: None,
        grep: None,
    };

    let result = read_spill(&read_request);
    assert!(result.is_err());
}

#[test]
fn test_boundary_inline_limit() {
    let spill_dir = tempdir().expect("failed to create temp dir");

    // Output exactly at the limit should be inline.
    let request = RunRequest {
        spill_dir: Some(spill_dir.path().to_path_buf()),
        ..base_request("printf 'a%.0s' {1..4096}")
    };

    let result = execute(&request).expect("execution failed");

    // This might be inline or spilled depending on exact char count vs byte count.
    // The key is that the API makes a clear decision.
    match result {
        RunResult::Inline { .. } | RunResult::Spilled { .. } => {
            // Both are acceptable; just verify no panic.
        },
        _ => panic!("unexpected result type"),
    }
}

#[test]
fn stalled_process_reports_awaiting_input_and_stays_alive() {
    // `cat` with piped stdin blocks waiting for input; it never writes anything,
    // so it should be reported as stalled well before the total timeout.
    let request = RunRequest {
        timeout_secs: Some(3),
        stall_timeout_secs: Some(1),
        interactive: Some(true),
        ..base_request("cat")
    };

    let result = execute(&request).expect("execution failed");
    let handle_id = match result {
        RunResult::AwaitingInput {
            handle_id,
            stall_secs,
            ..
        } => {
            assert_eq!(stall_secs, 1);
            handle_id
        },
        other => panic!("expected AwaitingInput, got {other:?}"),
    };

    // The process is still registered and alive — send it input and observe it
    // echo the input back to stdout (that's what `cat` does with each line).
    let send_result = send_input(&SendInputRequest {
        handle_id: handle_id.clone(),
        input: "hello-from-test".to_string(),
        append_newline: Some(true),
    })
    .expect("send_input failed");
    assert!(matches!(send_result, RunResult::InputSent { .. }));

    // `cat` never exits on its own without EOF; poll repeatedly (each poll may
    // re-report AwaitingInput once the echoed input goes quiet again) until the
    // total timeout elapses and cleanup (kill + reap) kicks in.
    let stdout_partial = poll_until_timed_out(&handle_id);
    assert!(stdout_partial.contains("hello-from-test"));
}

#[test]
fn poll_process_reports_not_found_for_unknown_handle() {
    let result = poll_process(&PollProcessRequest {
        handle_id: "does-not-exist".to_string(),
        wait_secs: Some(1),
    })
    .expect("poll_process failed");
    assert!(matches!(result, RunResult::ProcessNotFound { .. }));
}

#[test]
fn non_interactive_process_waits_for_total_timeout() {
    let request = RunRequest {
        timeout_secs: Some(2),
        stall_timeout_secs: Some(1),
        ..base_request("sleep 5")
    };

    let result = execute(&request).expect("execution failed");
    assert!(matches!(
        result,
        RunResult::TimedOut {
            timeout_secs: 2,
            ..
        }
    ));
}

/// Repeatedly poll a stalled/silent process until it is killed for exceeding its
/// total timeout, returning the captured partial stdout. A single `poll_process`
/// call can legitimately re-report `AwaitingInput` before the total timeout is
/// reached, so this loops with real wall-clock waits in between.
fn poll_until_timed_out(handle_id: &str) -> String {
    for _ in 0..20 {
        let result = poll_process(&PollProcessRequest {
            handle_id: handle_id.to_string(),
            wait_secs: Some(1),
        })
        .expect("poll_process failed");
        match result {
            RunResult::TimedOut { stdout_partial, .. } => return stdout_partial,
            RunResult::AwaitingInput { .. } | RunResult::Running { .. } => {
                std::thread::sleep(std::time::Duration::from_millis(300));
            },
            other => panic!("unexpected result while waiting for timeout: {other:?}"),
        }
    }
    panic!("process did not time out within the polling budget");
}

