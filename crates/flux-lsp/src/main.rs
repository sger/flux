use anyhow::Result;
use flux_lsp::{Server, server_capabilities};
use lsp_server::Connection;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    init_tracing();
    tracing::info!("flux-lsp starting");

    let (connection, io_threads) = Connection::stdio();

    let server_capabilities = serde_json::to_value(server_capabilities())?;
    let initialization_params = connection.initialize(server_capabilities)?;
    tracing::debug!(?initialization_params, "initialized");

    let server = Server::new(connection);
    server.run()?;

    io_threads.join()?;
    tracing::info!("flux-lsp shutting down");
    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("FLUX_LSP_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
