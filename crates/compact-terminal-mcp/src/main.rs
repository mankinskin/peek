use compact_terminal_mcp::server;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("compact_terminal_mcp=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let spill_dir = std::env::var("COMPACT_TERMINAL_SPILL_DIR")
        .map(PathBuf::from)
        .ok();

    eprintln!(
        "compact-terminal-mcp starting (spill_dir: {})",
        spill_dir
            .as_deref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<system temp>".to_string())
    );

    if let Err(err) = server::run_mcp_server(spill_dir).await {
        eprintln!("Fatal error: {err}");
        std::process::exit(1);
    }
}
