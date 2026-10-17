//! Compact terminal execution API.
//!
//! Provides the core logic for executing shell commands with inline/spilled output
//! and reading from spill files. This API is transport-agnostic and designed to be
//! used by CLI, MCP, and HTTP adapters.
//!
//! # Overview
//!
//! - **Short output** (≤ `inline_limit` bytes): returned directly.
//! - **Long output** (> `inline_limit` bytes): truncated inline summary + transient
//!   file path where the full output is stored.
//!
//! # Entry points
//!
//! - [`execute()`]: Execute a shell command and return inline or spilled result.
//! - [`read_spill()`]: Read from a spill file with line range or grep.

pub mod error;
pub mod execute;
pub mod process;
pub mod request;
pub mod response;
pub mod spill_reader;

pub use error::CompactTerminalError;
pub use execute::{
    DEFAULT_INLINE_LIMIT,
    DEFAULT_KILL_TIMEOUT_SECS,
    DEFAULT_POLL_WAIT_SECS,
    DEFAULT_STALL_TIMEOUT_SECS,
    DEFAULT_TIMEOUT_SECS,
    execute,
    poll_process,
    send_input,
};
pub use request::{
    PollProcessRequest,
    ReadSpillRequest,
    RunRequest,
    SendInputRequest,
};
pub use response::{
    ReadSpillResult,
    RunResult,
};
pub use spill_reader::read_spill;
