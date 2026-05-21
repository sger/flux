use std::path::PathBuf;

use anyhow::Result;
use flux_lsp::line_index::{PositionEncoding, negotiate_encoding};
use flux_lsp::loader::WatcherKind;
use flux_lsp::vfs::uri_to_path;
use flux_lsp::{Server, server_capabilities_json};
use lsp_server::Connection;
use lsp_types::InitializeParams;
use tracing_subscriber::EnvFilter;

fn main() -> Result<()> {
    init_tracing();
    tracing::info!("flux-lsp starting");

    let (connection, io_threads) = Connection::stdio();

    let (initialize_id, initialize_params) = connection.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(initialize_params)?;
    let encoding = pick_encoding(&params);
    tracing::debug!(?encoding, "negotiated position encoding");

    // `typeHierarchyProvider` is injected here (see `server_capabilities_json`)
    // because the `lsp-types` version in use has no typed field for it.
    let initialize_result = serde_json::json!({
        "capabilities": server_capabilities_json(encoding),
        "serverInfo": {
            "name": "flux-lsp",
            "version": env!("CARGO_PKG_VERSION"),
        },
    });
    connection.initialize_finish(initialize_id, initialize_result)?;

    let roots = workspace_roots(&params);
    tracing::debug!(?roots, "workspace roots");
    // Prefer the editor's own file watcher; fall back to a server-side `notify`
    // watcher for clients without dynamic `didChangeWatchedFiles` registration.
    let watcher = WatcherKind::for_client(&params);
    tracing::debug!(?watcher, "file-watch backend");
    let supports_progress = params
        .capabilities
        .window
        .as_ref()
        .and_then(|w| w.work_done_progress)
        .unwrap_or(false);
    let mut server = Server::new(connection, encoding, watcher);
    server.set_progress_support(supports_progress);
    server.state.set_workspace_folders(roots);
    server.run()?;

    io_threads.join()?;
    tracing::info!("flux-lsp shutting down");
    Ok(())
}

/// Resolve the workspace roots the client advertised. Prefers the modern
/// `workspaceFolders` list; falls back to the deprecated `rootUri`.
fn workspace_roots(params: &InitializeParams) -> Vec<PathBuf> {
    if let Some(folders) = &params.workspace_folders
        && !folders.is_empty()
    {
        let roots: Vec<PathBuf> = folders.iter().filter_map(|f| uri_to_path(&f.uri)).collect();
        if !roots.is_empty() {
            return roots;
        }
    }
    #[allow(deprecated)]
    params
        .root_uri
        .as_ref()
        .and_then(uri_to_path)
        .into_iter()
        .collect()
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
