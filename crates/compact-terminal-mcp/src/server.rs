//! Compact terminal MCP server.
//!
//! Exposes `run`, `read_spill`, `poll_process`, and `send_input`.
//! The `run` tool executes a shell command and returns:
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

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn advertised_terminal_contracts_explain_spill_and_result_kinds() {
        let tools = CompactTerminalServer::tool_router().list_all();
        assert_eq!(tools.len(), 4);
        let run = tools.iter().find(|tool| tool.name == "run").unwrap();
        assert!(
            run.description
                .as_deref()
                .unwrap()
                .contains("inspect exit_code")
        );
        let spill = tools.iter().find(|tool| tool.name == "read_spill").unwrap();
        assert!(spill.description.as_deref().unwrap().contains("NOT regex"));
    }
}

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
    #[tool(description = "Execute command through sh -c. Combined stdout/stderr at most inline_limit bytes returns inline; otherwise spills to a retained file with stream previews (first inline_limit/2 characters each). Default inline_limit 4096; zero spills any output. Nonzero exit is not a tool error: inspect exit_code. Long-running commands may return a handle for poll_process/send_input; handles are process-local.\n\nUse run() for all terminal commands instead of raw shell execution. This keeps\ntoken consumption bounded by preventing large outputs from flooding the context.\n\nFor interactive processes, if the process falls silent (no new stdout/stderr)\nfor stall_timeout_secs (default 20s), this returns kind: awaiting_input instead\nof hanging until timeout_secs. Non-interactive processes run until completion or\ntimeout_secs. Check the `tail` field for a possible input prompt; use\npoll_process to keep waiting, or send_input (requires interactive: true) to\nrespond.\n\nFollow-up pattern for spilled output:\n  1. Check stdout_preview / stderr_preview for quick diagnosis.\n  2. Use read_spill with start/end or grep to inspect targeted sections.\n  3. Only re-run the full command if the spill file is insufficient.\n")]
    pub async fn run(
        &self,
        Parameters(input): Parameters<RunInput>,
    ) -> Result<CallToolResult, McpError> {
        self.run_tool(input).await
    }

    #[tool(description = "Read a spill path as supplied, not restricted to the spill directory. start/end are 1-based inclusive (end default start+80, clamped); end must be >= start. grep is a case-sensitive literal substring, NOT regex: ignores start/end and returns matching line numbers only. Numbers include stdout/stderr header lines.")]
    pub async fn read_spill(
        &self,
        Parameters(input): Parameters<ReadSpillInput>,
    ) -> Result<CallToolResult, McpError> {
        self.read_spill_tool(input).await
    }

    #[tool(description = "Poll a process-local handle returned by run. Unknown/finished handles return success-shaped kind=process_not_found; inspect kind, running state and exit_code rather than assuming completion.")]
    pub async fn poll_process(
        &self,
        Parameters(input): Parameters<PollProcessInput>,
    ) -> Result<CallToolResult, McpError> {
        self.poll_process_tool(input).await
    }

    #[tool(description = "Send input to a process-local running handle; append_newline defaults true and appends only when absent. Unknown handles return kind=process_not_found, write failures kind=launch_error, successful writes kind=input_sent. This sends terminal input, unlike Session observer tools.")]
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
