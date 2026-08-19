use peek_mcp::run_mcp_server;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("peek_mcp=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    eprintln!("peek-mcp starting");

    if let Err(err) = run_mcp_server().await {
        eprintln!("Fatal error: {err}");
        std::process::exit(1);
    }
}
