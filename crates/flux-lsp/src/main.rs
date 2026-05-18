use anyhow::Result;
use flux_lsp::line_index::{PositionEncoding, negotiate_encoding};
use flux_lsp::{Server, server_capabilities};
use lsp_server::Connection;
use lsp_types::{InitializeParams, InitializeResult, ServerInfo};
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    init_tracing();
    tracing::info!("flux-lsp starting");

    let (connection, io_threads) = Connection::stdio();

    let (initialize_id, initialize_params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(initialize_params)?;
    let encoding = pick_encoding(&params);
    tracing::debug!(?encoding, "negotiated position encoding");

    let initialize_result = InitializeResult {
        capabilities: server_capabilities(encoding),
        server_info: Some(ServerInfo {
            name: "flux-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    connection.initialize_finish(initialize_id, serde_json::to_value(initialize_result)?)?;

    let server = Server::new(connection, encoding);
    server.run()?;

    io_threads.join()?;
    tracing::info!("flux-lsp shutting down");
    Ok(())
}

fn pick_encoding(params: &InitializeParams) -> PositionEncoding {
    let supported = params
        .capabilities
        .general
        .as_ref()
        .and_then(|g| g.position_encodings.as_deref());
    negotiate_encoding(supported)
}

fn init_tracing() {
    let filter = EnvFilter::try_from_env("FLUX_LSP_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .try_init();
}
