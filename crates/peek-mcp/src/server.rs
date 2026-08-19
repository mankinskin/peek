use std::path::PathBuf;

use peek_api::{
    InspectMode,
    InspectRequest,
    PeekError,
    PeekRequest,
    execute,
};
use rmcp::{
    ErrorData as McpError,
    ServerHandler,
    ServiceExt,
    handler::server::{
        tool::ToolRouter,
        wrapper::Parameters,
    },
    model::*,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeekReadInput {
    pub path: String,
    #[serde(default)]
    pub start: usize,
    #[serde(default)]
    pub end: Option<usize>,
    #[serde(default)]
    pub window: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeekGrepInput {
    pub path: String,
    pub pattern: String,
    #[serde(default)]
    pub window: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeekCountInput {
    pub path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PeekSkeletonInput {
    pub path: String,
}

#[derive(Clone)]
pub struct PeekServer {
    tool_router: ToolRouter<Self>,
}

impl PeekServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    fn json_result<T: Serialize>(
        value: &T
    ) -> Result<CallToolResult, McpError> {
        let text = serde_json::to_string(value).map_err(|err| {
            McpError::internal_error(format!("serialization: {err}"), None)
        })?;
        Ok(CallToolResult::success(vec![Content::text(text)]))
    }

    fn peek_err(err: PeekError) -> McpError {
        match err {
            PeekError::RepoMapEncode(message) =>
                McpError::internal_error(message, None),
            other => McpError::invalid_params(other.to_string(), None),
        }
    }

    fn run_request(request: PeekRequest) -> Result<CallToolResult, McpError> {
        let response = execute(&request).map_err(Self::peek_err)?;
        Self::json_result(&response)
    }
}

impl Default for PeekServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl PeekServer {
    #[tool(
        name = "peek_read",
        description = "Read a bounded line window from a file using shared peek-api validation and formatting."
    )]
    pub async fn peek_read(
        &self,
        Parameters(input): Parameters<PeekReadInput>,
    ) -> Result<CallToolResult, McpError> {
        Self::run_request(PeekRequest::Inspect(InspectRequest {
            path: PathBuf::from(input.path),
            mode: InspectMode::Range {
                start: input.start,
                end: input.end,
                window: input.window,
            },
        }))
    }

    #[tool(
        name = "peek_grep",
        description = "Search a file with a regex pattern and optionally return a bounded context window."
    )]
    pub async fn peek_grep(
        &self,
        Parameters(input): Parameters<PeekGrepInput>,
    ) -> Result<CallToolResult, McpError> {
        Self::run_request(PeekRequest::Inspect(InspectRequest {
            path: PathBuf::from(input.path),
            mode: InspectMode::Grep {
                pattern: input.pattern,
                window: input.window,
            },
        }))
    }

    #[tool(
        name = "peek_count",
        description = "Count total lines in a file before choosing bounded read coordinates."
    )]
    pub async fn peek_count(
        &self,
        Parameters(input): Parameters<PeekCountInput>,
    ) -> Result<CallToolResult, McpError> {
        Self::run_request(PeekRequest::Inspect(InspectRequest {
            path: PathBuf::from(input.path),
            mode: InspectMode::Count,
        }))
    }

    #[tool(
        name = "peek_skeleton",
        description = "Render a structural skeleton for a file or directory using peek-api."
    )]
    pub async fn peek_skeleton(
        &self,
        Parameters(input): Parameters<PeekSkeletonInput>,
    ) -> Result<CallToolResult, McpError> {
        Self::run_request(PeekRequest::Skeleton {
            path: PathBuf::from(input.path),
        })
    }
}

#[tool_handler]
impl ServerHandler for PeekServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            server_info: Implementation {
                name: env!("CARGO_PKG_NAME").to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                ..Default::default()
            },
            instructions: Some(
                "peek-mcp provides named token-bounded inspection tools backed by peek-api. Use peek_read, peek_grep, peek_count, and peek_skeleton instead of reimplementing file inspection in transport code."
                    .to_string(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

pub async fn run_mcp_server()
-> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = PeekServer::new();

    tracing::info!("Starting peek-mcp server on stdio");

    let service = server.serve(stdio()).await.inspect_err(|err| {
        eprintln!("Server error: {err:?}");
    })?;

    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write_file(
        path: &std::path::Path,
        content: &str,
    ) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(path, content).expect("write file");
    }

    #[tokio::test]
    async fn count_tool_succeeds() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("sample.rs");
        write_file(&file, "one\ntwo\nthree\n");

        let server = PeekServer::new();
        let result = server
            .peek_count(Parameters(PeekCountInput {
                path: file.to_string_lossy().to_string(),
            }))
            .await
            .expect("count tool should succeed");

        assert!(!result.is_error.unwrap_or(false));
    }

    #[tokio::test]
    async fn grep_tool_rejects_invalid_regex() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("sample.rs");
        write_file(&file, "one\ntwo\nthree\n");

        let server = PeekServer::new();
        let result = server
            .peek_grep(Parameters(PeekGrepInput {
                path: file.to_string_lossy().to_string(),
                pattern: "[".to_string(),
                window: None,
            }))
            .await;

        assert!(result.is_err());
    }
}
