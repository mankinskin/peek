use std::{
    env,
    io::Write,
    path::{
        Path,
        PathBuf,
    },
    process::{
        Command,
        ExitStatus,
        Stdio,
    },
    time::{
        Duration,
        Instant,
    },
};

use uuid::Uuid;

use crate::{
    error::CompactTerminalError,
    process::{
        ManagedProcess,
        OutputState,
        registry,
        spawn_reader,
    },
    request::{
        PollProcessRequest,
        RunRequest,
        SendInputRequest,
    },
    response::RunResult,
};

/// Default maximum bytes to return inline. Outputs longer than this are spilled to file.
pub const DEFAULT_INLINE_LIMIT: usize = 4096;

/// Default command timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// Default silence duration (seconds) before a still-running process is reported
/// as possibly awaiting input.
pub const DEFAULT_STALL_TIMEOUT_SECS: u64 = 20;

/// Default bound (seconds) for killing and reaping a timed-out process tree.
pub const DEFAULT_KILL_TIMEOUT_SECS: u64 = 10;

/// Default wait window (seconds) for a single `poll_process` call.
pub const DEFAULT_POLL_WAIT_SECS: u64 = 20;

/// Bytes of trailing output surfaced in stall/timeout diagnostics.
const TAIL_BYTES: usize = 2000;

/// Execute a shell command and return the result.
///
/// Short outputs (≤ inline_limit bytes) are returned directly.
/// Long outputs are summarised inline and stored in a transient file.
///
/// For interactive processes, if the process falls silent (no new stdout/stderr)
/// for `stall_timeout_secs` while still running, this returns
/// [`RunResult::AwaitingInput`] instead of blocking until `timeout_secs` — the
/// process is kept alive in the background so a caller can inspect it
/// (`poll_process`) or write to it (`send_input`). Non-interactive processes run
/// until completion or `timeout_secs`.
pub fn execute(
    request: &RunRequest
) -> Result<RunResult, CompactTerminalError> {
    let inline_limit = request.inline_limit.unwrap_or(DEFAULT_INLINE_LIMIT);
    let timeout_secs = request.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let interactive = request.interactive.unwrap_or(false);
    let stall_timeout_secs = if interactive {
        request
            .stall_timeout_secs
            .unwrap_or(DEFAULT_STALL_TIMEOUT_SECS)
            .min(timeout_secs)
    } else {
        timeout_secs
    };
    let spill_dir = request
        .spill_dir
        .clone()
        .unwrap_or_else(|| env::temp_dir().join("compact-terminal-api"));

    let start = Instant::now();

    // Build the command.
    let mut cmd = Command::new("sh");
    #[cfg(windows)]
    let pid_file = env::temp_dir()
        .join(format!("compact-terminal-{}.pid", Uuid::new_v4()));
    #[cfg(windows)]
    {
        cmd.arg("-c")
            .arg("printf '%s' \"$$\" > \"$COMPACT_TERMINAL_PID_FILE\"; eval \"$1\"")
            .arg("sh")
            .arg(&request.command)
            .env("COMPACT_TERMINAL_PID_FILE", &pid_file);
    }
    #[cfg(not(windows))]
    cmd.arg("-c").arg(&request.command);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(ref cwd) = request.cwd {
        cmd.current_dir(cwd);
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Ok(RunResult::LaunchError {
                message: format!("failed to spawn '{}': {e}", request.command),
            });
        },
    };

    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    // Dropping stdin immediately (non-interactive) closes the pipe, sending EOF —
    // functionally identical to Stdio::null() for the child. Interactive callers
    // keep it open so `send_input` can write to it later.
    let stdin = if interactive {
        child.stdin.take()
    } else {
        child.stdin.take();
        None
    };

    let output =
        std::sync::Arc::new(std::sync::Mutex::new(OutputState::new()));
    spawn_reader(stdout, true, output.clone());
    spawn_reader(stderr, false, output.clone());

    let handle_id = Uuid::new_v4().to_string();
    registry().insert(handle_id.clone(), ManagedProcess {
        child,
        command: request.command.clone(),
        started_at: start,
        timeout_secs,
        stall_timeout_secs,
        kill_timeout_secs: DEFAULT_KILL_TIMEOUT_SECS,
        inline_limit,
        spill_dir,
        stdin,
        output,
        #[cfg(windows)]
        pid_file,
    });

    settle_process(&handle_id, None)
}

/// Keep waiting on (or re-check) a process previously started by `execute`.
pub fn poll_process(
    request: &PollProcessRequest
) -> Result<RunResult, CompactTerminalError> {
    if !registry().contains(&request.handle_id) {
        return Ok(RunResult::ProcessNotFound {
            handle_id: request.handle_id.clone(),
        });
    }
    let wait_secs = request.wait_secs.unwrap_or(DEFAULT_POLL_WAIT_SECS);
    settle_process(&request.handle_id, Some(wait_secs))
}

/// Write input to a running process's stdin. Only works for processes started
/// with `interactive: true` — otherwise stdin was already closed at spawn time.
pub fn send_input(
    request: &SendInputRequest
) -> Result<RunResult, CompactTerminalError> {
    let append_newline = request.append_newline.unwrap_or(true);
    let mut payload = request.input.clone();
    if append_newline && !payload.ends_with('\n') {
        payload.push('\n');
    }

    let write_result = registry().with_mut(&request.handle_id, |mp| match mp
        .stdin
        .as_mut()
    {
        Some(stdin) => {
            stdin.write_all(payload.as_bytes()).and_then(|_| stdin.flush())
        },
        None => Err(std::io::Error::other(
            "stdin is not available (process was not started with \
             interactive: true, or stdin already closed)",
        )),
    });

    match write_result {
        None => Ok(RunResult::ProcessNotFound {
            handle_id: request.handle_id.clone(),
        }),
        Some(Ok(())) => {
            // Refresh last_activity so a subsequent poll doesn't immediately
            // re-report AwaitingInput before the process reacts to the input.
            registry().with_mut(&request.handle_id, |mp| {
                mp.output
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .last_activity = Instant::now();
            });
            Ok(RunResult::InputSent {
                handle_id: request.handle_id.clone(),
            })
        },
        Some(Err(e)) => Ok(RunResult::LaunchError {
            message: format!("failed to write input: {e}"),
        }),
    }
}

/// Poll the registered process until it exits, stalls, hits its total timeout, or
/// (when `extra_wait_secs` is set) the caller's own wait window elapses.
fn settle_process(
    handle_id: &str,
    extra_wait_secs: Option<u64>,
) -> Result<RunResult, CompactTerminalError> {
    let poll_started = Instant::now();
    loop {
        let snapshot = registry().with_mut(handle_id, |mp| {
            let status = mp.child.try_wait();
            let last_activity_elapsed = mp
                .output
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .last_activity
                .elapsed();
            (
                status,
                mp.started_at,
                mp.timeout_secs,
                mp.stall_timeout_secs,
                last_activity_elapsed,
            )
        });

        let Some((
            status,
            started_at,
            timeout_secs,
            stall_timeout_secs,
            last_activity_elapsed,
        )) = snapshot
        else {
            return Ok(RunResult::ProcessNotFound {
                handle_id: handle_id.to_string(),
            });
        };

        match status {
            Ok(Some(exit_status)) => {
                return Ok(finalize_completed(handle_id, exit_status));
            },
            Ok(None) => {
                let total_elapsed = started_at.elapsed();
                if total_elapsed >= Duration::from_secs(timeout_secs) {
                    return Ok(kill_and_report(handle_id, timeout_secs));
                }

                if last_activity_elapsed
                    >= Duration::from_secs(stall_timeout_secs)
                {
                    return Ok(build_awaiting_input(
                        handle_id,
                        total_elapsed,
                        stall_timeout_secs,
                    ));
                }

                if let Some(wait_secs) = extra_wait_secs {
                    if poll_started.elapsed() >= Duration::from_secs(wait_secs)
                    {
                        return Ok(build_running(handle_id, total_elapsed));
                    }
                }

                std::thread::sleep(Duration::from_millis(50));
            },
            Err(error) => {
                registry().remove(handle_id);
                return Ok(RunResult::LaunchError {
                    message: format!("failed to wait for command: {error}"),
                });
            },
        }
    }
}

fn finalize_completed(handle_id: &str, exit_status: ExitStatus) -> RunResult {
    let Some(mp) = registry().remove(handle_id) else {
        return RunResult::LaunchError {
            message: "process disappeared before finalization".to_string(),
        };
    };
    #[cfg(windows)]
    let _ = std::fs::remove_file(&mp.pid_file);

    wait_for_readers_done(&mp.output);

    let elapsed_ms = mp.started_at.elapsed().as_millis();
    let exit_code = exit_status.code().unwrap_or(-1);
    let (stdout, stderr, warnings) = {
        let state = mp.output.lock().unwrap_or_else(|e| e.into_inner());
        (
            String::from_utf8_lossy(&state.stdout).into_owned(),
            String::from_utf8_lossy(&state.stderr).into_owned(),
            state.reader_errors.clone(),
        )
    };

    let combined_len = stdout.len() + stderr.len();

    if combined_len <= mp.inline_limit {
        return RunResult::Inline {
            exit_code,
            stdout,
            stderr,
            elapsed_ms,
            warnings,
        };
    }

    let spill_content = format!(
        "=== stdout ===\n{stdout}\n=== stderr ===\n{stderr}\n=== exit_code: {exit_code} ===\n"
    );
    let total_bytes = spill_content.len();
    let total_lines = spill_content.lines().count();

    let spill_file = match write_spill(&mp.spill_dir, &spill_content) {
        Ok(p) => p,
        Err(e) => {
            return RunResult::LaunchError {
                message: format!(
                    "output too large ({combined_len} bytes) and spill failed: {e}"
                ),
            };
        },
    };

    let stdout_preview =
        stdout.chars().take(mp.inline_limit / 2).collect::<String>();
    let stderr_preview =
        stderr.chars().take(mp.inline_limit / 2).collect::<String>();

    let spill_str = spill_file.display().to_string();
    let next_steps = vec![
        format!("peek \"{spill_str}\" --count"),
        format!("peek \"{spill_str}\" --grep \"error\" --window 10"),
        format!("peek \"{spill_str}\" --head 30"),
        format!(
            "Use read_spill with start/end or grep to inspect targeted sections"
        ),
    ];

    RunResult::Spilled {
        exit_code,
        stdout_preview,
        stderr_preview,
        total_bytes,
        total_lines,
        spill_file,
        elapsed_ms,
        next_steps,
        warnings,
    }
}

/// Kill the process (tree) and reap it, both bounded by `kill_timeout_secs` — a
/// hung kill helper (e.g. the Windows process-tree killer subshell) or a child
/// that ignores its kill signal is surfaced as [`RunResult::KillFailed`] instead
/// of blocking the caller forever.
fn kill_and_report(handle_id: &str, timeout_secs: u64) -> RunResult {
    let kill_timeout_secs = registry()
        .with_mut(handle_id, |mp| mp.kill_timeout_secs)
        .unwrap_or(DEFAULT_KILL_TIMEOUT_SECS);
    let bound = Duration::from_secs(kill_timeout_secs);

    let kill_outcome = registry().with_mut(handle_id, |mp| {
        #[cfg(windows)]
        {
            kill_child_tree_bounded(&mut mp.child, &mp.pid_file, bound)
        }
        #[cfg(not(windows))]
        {
            kill_child_tree_bounded(&mut mp.child, bound)
        }
    });

    let Some(kill_outcome) = kill_outcome else {
        return RunResult::ProcessNotFound {
            handle_id: handle_id.to_string(),
        };
    };

    if let Err(message) = kill_outcome {
        return kill_failed(handle_id, kill_timeout_secs, message);
    }

    let reaped = registry()
        .with_mut(handle_id, |mp| {
            let deadline = Instant::now() + bound;
            loop {
                match mp.child.try_wait() {
                    Ok(Some(_)) => break true,
                    Ok(None) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20))
                    },
                    Ok(None) => break false,
                    Err(_) => break false,
                }
            }
        })
        .unwrap_or(false);

    if !reaped {
        return kill_failed(
            handle_id,
            kill_timeout_secs,
            "process was signalled but did not report exit within the kill \
             timeout"
                .to_string(),
        );
    }

    #[cfg(windows)]
    registry().with_mut(handle_id, |mp| {
        let _ = std::fs::remove_file(&mp.pid_file);
    });

    let Some(mp) = registry().remove(handle_id) else {
        return RunResult::ProcessNotFound {
            handle_id: handle_id.to_string(),
        };
    };
    let (stdout_partial, warnings) = {
        let state = mp.output.lock().unwrap_or_else(|e| e.into_inner());
        (
            String::from_utf8_lossy(&state.stdout).into_owned(),
            state.reader_errors.clone(),
        )
    };

    RunResult::TimedOut {
        timeout_secs,
        stdout_partial,
        spill_file: None,
        warnings,
    }
}

fn kill_failed(
    handle_id: &str,
    kill_timeout_secs: u64,
    message: String,
) -> RunResult {
    let tail = registry()
        .with_mut(handle_id, |mp| {
            mp.output.lock().unwrap_or_else(|e| e.into_inner()).tail(TAIL_BYTES)
        })
        .unwrap_or_default();
    RunResult::KillFailed {
        handle_id: handle_id.to_string(),
        kill_timeout_secs,
        tail,
        message,
    }
}

fn build_awaiting_input(
    handle_id: &str,
    elapsed: Duration,
    stall_timeout_secs: u64,
) -> RunResult {
    let tail = registry()
        .with_mut(handle_id, |mp| {
            mp.output.lock().unwrap_or_else(|e| e.into_inner()).tail(TAIL_BYTES)
        })
        .unwrap_or_default();
    RunResult::AwaitingInput {
        handle_id: handle_id.to_string(),
        elapsed_ms: elapsed.as_millis(),
        stall_secs: stall_timeout_secs,
        tail,
        message: format!(
            "no output for {stall_timeout_secs}s; the process may be waiting \
             on an interactive input prompt. Use send_input (requires \
             interactive: true at launch) to respond, or poll_process to keep \
             waiting."
        ),
    }
}

fn build_running(handle_id: &str, elapsed: Duration) -> RunResult {
    let tail = registry()
        .with_mut(handle_id, |mp| {
            mp.output.lock().unwrap_or_else(|e| e.into_inner()).tail(TAIL_BYTES)
        })
        .unwrap_or_default();
    RunResult::Running {
        handle_id: handle_id.to_string(),
        elapsed_ms: elapsed.as_millis(),
        tail,
    }
}

fn wait_for_readers_done(
    output: &std::sync::Arc<std::sync::Mutex<OutputState>>
) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let done =
            output.lock().unwrap_or_else(|e| e.into_inner()).readers_done();
        if done {
            return;
        }
        if Instant::now() >= deadline {
            output
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .reader_errors
                .push(
                    "reader threads did not signal completion within 3s \
                     after process exit"
                        .to_string(),
                );
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Kill the process tree, bounded by `bound`.
#[cfg(windows)]
fn kill_child_tree_bounded(
    child: &mut std::process::Child,
    _pid_file: &Path,
    bound: Duration,
) -> Result<(), String> {
    let pid = child.id().to_string();
    let mut killer = Command::new("taskkill.exe")
        .args(["/PID", &pid, "/T", "/F"])
        .spawn()
        .map_err(|e| format!("cannot spawn taskkill helper: {e}"))?;

    let deadline = Instant::now() + bound;
    loop {
        match killer.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            },
            Ok(None) => {
                let _ = killer.kill();
                let _ = killer.wait();
                return Err(format!(
                    "taskkill helper did not exit within {}s",
                    bound.as_secs()
                ));
            },
            Err(_) => break,
        }
    }
    // `taskkill` can report an already-exited child as an error. The direct
    // fallback lets the caller's reap check determine the final state.
    child.kill().map_err(|e| format!("fallback kill failed: {e}"))
}

#[cfg(not(windows))]
fn kill_child_tree_bounded(
    child: &mut std::process::Child,
    _bound: Duration,
) -> Result<(), String> {
    child.kill().map_err(|e| format!("kill failed: {e}"))
}

fn write_spill(
    spill_dir: &PathBuf,
    content: &str,
) -> Result<PathBuf, CompactTerminalError> {
    std::fs::create_dir_all(spill_dir).map_err(|source| {
        CompactTerminalError::CannotCreateSpillDir {
            path: spill_dir.clone(),
            source,
        }
    })?;
    let path = spill_dir.join(format!("{}.txt", Uuid::new_v4()));
    let mut f = std::fs::File::create(&path).map_err(|source| {
        CompactTerminalError::CannotWriteSpillFile {
            path: path.clone(),
            source,
        }
    })?;
    f.write_all(content.as_bytes()).map_err(|source| {
        CompactTerminalError::CannotWriteSpillFile {
            path: path.clone(),
            source,
        }
    })?;
    Ok(path)
}
