use std::path::PathBuf;

use serde::{
    Deserialize,
    Serialize,
};

/// Result of a run command execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunResult {
    /// Short output returned inline.
    Inline {
        exit_code: i32,
        stdout: String,
        stderr: String,
        elapsed_ms: u128,
        /// Diagnostics from reader threads (I/O errors, panics); empty when clean.
        #[serde(default)]
        warnings: Vec<String>,
    },
    /// Long output spilled to a transient file.
    Spilled {
        exit_code: i32,
        /// First `inline_limit` bytes of stdout for quick scanning.
        stdout_preview: String,
        /// First `inline_limit` bytes of stderr.
        stderr_preview: String,
        /// Total bytes of combined output stored in the spill file.
        total_bytes: usize,
        /// Total lines in the spill file.
        total_lines: usize,
        /// Path to the transient file containing the full output.
        spill_file: PathBuf,
        elapsed_ms: u128,
        /// Suggested follow-up inspection commands.
        next_steps: Vec<String>,
        /// Diagnostics from reader threads (I/O errors, panics); empty when clean.
        #[serde(default)]
        warnings: Vec<String>,
    },
    /// Command timed out and the process (tree) was killed.
    TimedOut {
        timeout_secs: u64,
        /// Partial stdout captured before timeout.
        stdout_partial: String,
        spill_file: Option<PathBuf>,
        /// Diagnostics from reader threads (I/O errors, panics); empty when clean.
        #[serde(default)]
        warnings: Vec<String>,
    },
    /// Command could not be launched.
    LaunchError { message: String },
    /// No new stdout/stderr activity for `stall_secs`; the process is still running
    /// but is likely blocked on an interactive input prompt. It is kept alive —
    /// use `send_input` (if started with `interactive: true`) or `poll_process` to
    /// keep waiting.
    AwaitingInput {
        handle_id: String,
        elapsed_ms: u128,
        stall_secs: u64,
        /// Last bytes of combined stdout+stderr, to help spot a prompt.
        tail: String,
        message: String,
    },
    /// Still running normally; returned when a `poll_process` wait window elapsed
    /// without completion or a new stall being detected.
    Running {
        handle_id: String,
        elapsed_ms: u128,
        tail: String,
    },
    /// The kill signal was sent (or attempted) but the process (tree) did not exit
    /// within `kill_timeout_secs`. The process may still be registered and running;
    /// this is surfaced instead of hanging silently.
    KillFailed {
        handle_id: String,
        kill_timeout_secs: u64,
        tail: String,
        message: String,
    },
    /// Input was written to the process's stdin.
    InputSent { handle_id: String },
    /// No running process registered under this handle id — it may have already
    /// completed, been reaped, or never existed.
    ProcessNotFound { handle_id: String },
}


/// Result of reading a spill file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadSpillResult {
    pub content: String,
}
