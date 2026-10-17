use std::path::PathBuf;

use serde::{
    Deserialize,
    Serialize,
};

/// Request to run a shell command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunRequest {
    /// The shell command to execute (passed to `sh -c`).
    pub command: String,

    /// Working directory for the command. Defaults to the current working dir.
    pub cwd: Option<PathBuf>,

    /// Maximum bytes to return inline. Outputs exceeding this are spilled to a
    /// transient file and summarised. Default: 4096.
    pub inline_limit: Option<usize>,

    /// Command timeout in seconds. Default: 60.
    pub timeout_secs: Option<u64>,

    /// Directory where spill files will be written. Defaults to system temp dir.
    pub spill_dir: Option<PathBuf>,

    /// For interactive processes, seconds of silence (no new stdout/stderr bytes)
    /// after which a still-running process is reported as possibly awaiting input.
    /// Default: 20, capped at `timeout_secs`; ignored for non-interactive commands.
    pub stall_timeout_secs: Option<u64>,

    /// Keep stdin open so `send_input` can write to the process later. Default:
    /// false — stdin is closed immediately (EOF), matching plain one-shot execution.
    pub interactive: Option<bool>,
}

/// Request to read from a spill file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadSpillRequest {
    /// Path to the transient spill file returned by a previous `run` call.
    pub spill_file: PathBuf,

    /// First line to read (1-based, inclusive). Defaults to 1.
    pub start: Option<usize>,

    /// Last line to read (1-based, inclusive). Defaults to start + 80.
    pub end: Option<usize>,

    /// Search pattern: returns matching line numbers (1-based) instead of content.
    pub grep: Option<String>,
}

/// Request to keep waiting on (or re-check) a previously started background process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PollProcessRequest {
    /// Handle id returned by a previous `run` (or `poll_process`) call.
    pub handle_id: String,

    /// How long to wait for new activity before returning again. Default: 20s,
    /// bounded by the process's remaining total timeout.
    pub wait_secs: Option<u64>,
}

/// Request to write input to a running process's stdin.
///
/// Only works for processes started with `interactive: true`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendInputRequest {
    /// Handle id returned by a previous `run` call.
    pub handle_id: String,

    /// Text to write to the process's stdin.
    pub input: String,

    /// Append a trailing newline if `input` doesn't already end with one. Default: true.
    pub append_newline: Option<bool>,
}
