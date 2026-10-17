//! Compact terminal MCP server.
//!
//! Exposes a single `run` tool that executes a shell command and returns:
//!
//! - **Short output** (≤ `inline_limit` bytes): returned directly in the MCP response.
//! - **Long output** (> `inline_limit` bytes): truncated inline summary + a transient
//!   file path where the full output is stored. Follow-up inspection should use
//!   bounded reads (`peek --grep`, `peek --start --end`) on the transient file
//!   rather than re-running the full command.
//!
//! # Transient file lifecycle
//!
//! Transient files are written to `<spill_dir>/<uuid>.txt` (default: system temp dir).
//! They are not automatically deleted — callers should clean up when no longer needed,
//! or rely on OS temp cleanup.

use std::{
    env,
    path::PathBuf,
};

use compact_terminal_api::{
    PollProcessRequest,
    ReadSpillRequest,
    RunRequest,
    SendInputRequest,
    execute,
    poll_process,
    read_spill,
    send_input,
};
use rmcp::{
    ErrorData as McpError,
    ServerHandler,
    ServiceExt,
    handler::server::{
        tool::ToolRouter,
        wrapper::Parameters,
    },
    model::{
        CallToolResult,
        Content,
    },
    schemars::{
        self,
        JsonSchema,
    },
    tool,
    tool_handler,
    tool_router,
    transport::stdio,
};
use serde::{
    Deserialize,
    Serialize,
};

// ── Input types ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunInput {
    /// The shell command to execute (passed to `sh -c`).
    pub command: String,

    /// Working directory for the command. Defaults to the server's working dir.
    #[serde(default)]
    pub cwd: Option<PathBuf>,

    /// Maximum bytes to return inline. Outputs exceeding this are spilled to a
    /// transient file and summarised. Default: 4096.
    #[serde(default)]
    pub inline_limit: Option<usize>,

    /// Command timeout in seconds. Default: 60.
    #[serde(default)]
    pub timeout_secs: Option<u64>,

    /// For interactive processes, seconds of silence (no new stdout/stderr) after
    /// which a still-running process is reported as possibly awaiting input.
    /// Default: 20, capped at timeout_secs; ignored for non-interactive commands.
    #[serde(default)]
    pub stall_timeout_secs: Option<u64>,

    /// Keep stdin open so send_input can write to the process later. Default:
    /// false — stdin is closed immediately, matching plain one-shot execution.
    #[serde(default)]
    pub interactive: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReadSpillInput {
    /// Path to the transient spill file returned by a previous `run` call.
    pub spill_file: PathBuf,

    /// First line to read (1-based, inclusive). Defaults to 1.
    #[serde(default)]
    pub start: Option<usize>,

    /// Last line to read (1-based, inclusive). Defaults to start + 80.
    #[serde(default)]
    pub end: Option<usize>,

    /// Search pattern: returns matching line numbers (1-based) instead of content.
    #[serde(default)]
    pub grep: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PollProcessInput {
    /// Handle id returned by a previous run (or poll_process) call.
    pub handle_id: String,

    /// How long to wait for new activity before returning again. Default: 20s,
    /// bounded by the process's remaining total timeout.
    #[serde(default)]
    pub wait_secs: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SendInputInput {
    /// Handle id returned by a previous run call.
    pub handle_id: String,

    /// Text to write to the process's stdin.
    pub input: String,

    /// Append a trailing newline if input doesn't already end with one. Default: true.
    #[serde(default)]
    pub append_newline: Option<bool>,
}

// ── Server ────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct CompactTerminalServer {
    spill_dir: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl CompactTerminalServer {
    pub fn new(spill_dir: Option<PathBuf>) -> Self {
        let spill_dir = spill_dir
            .unwrap_or_else(|| env::temp_dir().join("compact-terminal-mcp"));
        Self {
            spill_dir,
            tool_router: Self::tool_router(),
        }
    }

    fn json_result<T: Serialize>(
        value: &T
    ) -> Result<CallToolResult, McpError> {
        let text = serde_json::to_string(value).map_err(|e| {
            McpError::internal_error(format!("serialization: {e}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    async fn run_tool(
        &self,
        input: RunInput,
    ) -> Result<CallToolResult, McpError> {
        let request = RunRequest {
            command: input.command,
            cwd: input.cwd,
            inline_limit: input.inline_limit,
            timeout_secs: input.timeout_secs,
            spill_dir: Some(self.spill_dir.clone()),
            stall_timeout_secs: input.stall_timeout_secs,
            interactive: input.interactive,
        };

        let result = tokio::task::spawn_blocking(move || execute(&request))
            .await
            .map_err(|e| {
                McpError::internal_error(format!("task error: {e}"), None)
            })?
            .map_err(|e| {
                McpError::internal_error(format!("execution error: {e}"), None)
            })?;

        Self::json_result(&result)
    }

    async fn read_spill_tool(
        &self,
        input: ReadSpillInput,
    ) -> Result<CallToolResult, McpError> {
        let request = ReadSpillRequest {
            spill_file: input.spill_file,
            start: input.start,
            end: input.end,
            grep: input.grep,
        };

        let result = tokio::task::spawn_blocking(move || read_spill(&request))
            .await
            .map_err(|e| {
                McpError::internal_error(format!("task error: {e}"), None)
            })?
            .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(result.content)]))
    }

    async fn poll_process_tool(
        &self,
        input: PollProcessInput,
    ) -> Result<CallToolResult, McpError> {
        let request = PollProcessRequest {
            handle_id: input.handle_id,
            wait_secs: input.wait_secs,
        };

        let result =
            tokio::task::spawn_blocking(move || poll_process(&request))
                .await
                .map_err(|e| {
                    McpError::internal_error(format!("task error: {e}"), None)
                })?
                .map_err(|e| {
                    McpError::internal_error(
                        format!("execution error: {e}"),
                        None,
                    )
                })?;

        Self::json_result(&result)
    }

    async fn send_input_tool(
        &self,
        input: SendInputInput,
    ) -> Result<CallToolResult, McpError> {
        let request = SendInputRequest {
            handle_id: input.handle_id,
            input: input.input,
            append_newline: input.append_newline,
        };

        let result = tokio::task::spawn_blocking(move || send_input(&request))
            .await
            .map_err(|e| {
                McpError::internal_error(format!("task error: {e}"), None)
            })?
            .map_err(|e| {
                McpError::internal_error(format!("execution error: {e}"), None)
            })?;

        Self::json_result(&result)
    }
}

// ── MCP tool surface (delegates to impl methods above) ────────────────────────

#[tool_router]
impl CompactTerminalServer {
    #[tool(description = "
Run a shell command. Short outputs (≤ inline_limit bytes) are returned directly.
Long outputs are summarised inline and stored in a transient file for targeted
follow-up inspection using read_spill or peek.

Use run() for all terminal commands instead of raw shell execution. This keeps
token consumption bounded by preventing large outputs from flooding the context.

For interactive processes, if the process falls silent (no new stdout/stderr)
for stall_timeout_secs (default 20s), this returns kind: awaiting_input instead
of hanging until timeout_secs. Non-interactive processes run until completion or
timeout_secs. Check the `tail` field for a possible input prompt; use
poll_process to keep waiting, or send_input (requires interactive: true) to
respond.

Follow-up pattern for spilled output:
  1. Check stdout_preview / stderr_preview for quick diagnosis.
  2. Use read_spill with start/end or grep to inspect targeted sections.
  3. Only re-run the full command if the spill file is insufficient.
")]
    pub async fn run(
        &self,
        Parameters(input): Parameters<RunInput>,
    ) -> Result<CallToolResult, McpError> {
        self.run_tool(input).await
    }

    #[tool(description = "
Read a bounded window from a transient spill file returned by run().

Use this instead of re-running the full command when you need to inspect
specific sections of long output. Prefer grep for pattern search and
start/end for targeted slices.

Patterns:
  - grep: 'error'       → returns matching line numbers
  - start: 1, end: 30  → first 30 lines
  - start: 100, end: 130 → specific slice
")]
    pub async fn read_spill(
        &self,
        Parameters(input): Parameters<ReadSpillInput>,
    ) -> Result<CallToolResult, McpError> {
        self.read_spill_tool(input).await
    }

    #[tool(description = "
Keep waiting on (or re-check) a process previously started by run(), identified
by the handle_id from its result.

Use this after a run() call returns kind: awaiting_input or kind: running to
continue watching a long-lived or possibly-stuck process without re-running it.
Returns the same result shapes as run(): inline/spilled once it completes,
awaiting_input if it stalls again, running if it's still busy after wait_secs,
or timed_out once the process's total timeout is finally exceeded (it is
killed at that point).
")]
    pub async fn poll_process(
        &self,
        Parameters(input): Parameters<PollProcessInput>,
    ) -> Result<CallToolResult, McpError> {
        self.poll_process_tool(input).await
    }

    #[tool(description = "
Write input to a running process's stdin, identified by the handle_id from a
previous run() result.

Only works for processes started with interactive: true — otherwise stdin was
already closed at spawn time and this returns a launch_error explaining that.
Useful after run() (or poll_process) reports kind: awaiting_input, to respond
to an interactive prompt instead of letting the process time out.
")]
    pub async fn send_input(
        &self,
        Parameters(input): Parameters<SendInputInput>,
    ) -> Result<CallToolResult, McpError> {
        self.send_input_tool(input).await
    }
}

#[tool_handler]
impl ServerHandler for CompactTerminalServer {
    fn get_info(&self) -> rmcp::model::ServerInfo {
        rmcp::model::ServerInfo {
            server_info: rmcp::model::Implementation {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "Compact terminal MCP. Use run() for all shell commands. \
                 Long outputs are truncated inline and stored in a transient file. \
                 Use read_spill() for targeted follow-up inspection. If a process \
                 stalls (kind: awaiting_input), use poll_process() to keep \
                 watching it and send_input() to respond to prompts (requires \
                 interactive: true at launch)."
                    .into(),
            ),
            capabilities: rmcp::model::ServerCapabilities::builder()
                .enable_tools()
                .build(),
            ..Default::default()
        }
    }
}

pub async fn run_mcp_server(spill_dir: Option<PathBuf>) -> anyhow::Result<()> {
    let server = CompactTerminalServer::new(spill_dir);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
