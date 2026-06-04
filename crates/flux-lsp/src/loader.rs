//! File-change detection behind a small [`Handle`] trait.
//!
//! `flux-lsp` learns about on-disk `.flx` changes (a `git checkout`, a codegen
//! run, an external edit) through one of two backends, chosen at `initialize`
//! time by [`WatcherKind::for_client`]:
//!
//! - [`ClientHandle`] — registers a `workspace/didChangeWatchedFiles` capability
//!   and lets the editor's own watcher report changes. Preferred when the client
//!   advertises dynamic-registration support.
//! - [`NotifyHandle`] — a server-side `notify`-crate filesystem watcher, used as
//!   a fallback for clients without that support.
//!
//! Both funnel into `Workspace::on_disk_changed`. The `ClientHandle` delivers
//! changes as ordinary `didChangeWatchedFiles` notifications on the LSP
//! connection; the `NotifyHandle` delivers them out-of-band as [`Message`]s on a
//! channel the server's main loop `select!`s on.

use std::path::{Path, PathBuf};

use crossbeam_channel::Sender;
use lsp_server::{Message as LspMessage, Request, RequestId};
use lsp_types::notification::{DidChangeWatchedFiles, Notification as _};
use lsp_types::request::{RegisterCapability, Request as _};
use lsp_types::{
    DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, InitializeParams,
    Registration, RegistrationParams,
};
use notify::{EventKind, RecursiveMode, Watcher};

/// A batch of on-disk `.flx` paths the server should reload. Sent by the
/// [`NotifyHandle`]'s watcher thread; consumed by the server's main loop.
#[derive(Debug)]
pub enum Message {
    Changed { files: Vec<PathBuf> },
}

/// Strategy for detecting on-disk `.flx` file changes outside the editor.
pub trait Handle {
    /// Begin watching `roots`. Called once, after the client reports
    /// `initialized`.
    fn watch(&mut self, roots: &[PathBuf]);
}

/// Which file-watching backend the server runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatcherKind {
    /// Register a `didChangeWatchedFiles` capability; the editor watches.
    Client,
    /// Run a server-side `notify` filesystem watcher.
    Notify,
}

impl WatcherKind {
    /// Pick a backend from the client's declared capabilities: the editor's
    /// watcher when it supports dynamic registration, the server-side `notify`
    /// watcher otherwise.
    pub fn for_client(params: &InitializeParams) -> Self {
        if client_supports_dynamic_watchers(params) {
            WatcherKind::Client
        } else {
            WatcherKind::Notify
        }
    }
}

/// Whether the client advertised dynamic-registration support for
/// `workspace/didChangeWatchedFiles`. Clients that do (VS Code among them) get
/// the editor-driven [`ClientHandle`]; the rest fall back to [`NotifyHandle`].
pub fn client_supports_dynamic_watchers(params: &InitializeParams) -> bool {
    params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.did_change_watched_files.as_ref())
        .and_then(|d| d.dynamic_registration)
        .unwrap_or(false)
}

/// Editor-driven watching: ask the client to watch every `**/*.flx` file and
/// report changes via `workspace/didChangeWatchedFiles`.
pub struct ClientHandle {
    /// Outgoing LSP messages — used to send the `client/registerCapability`
    /// request.
    sender: Sender<LspMessage>,
    /// Kept alive only so the server's loader channel never disconnects: this
    /// backend delivers changes via `didChangeWatchedFiles`, not the channel.
    _loader_tx: Sender<Message>,
}

impl ClientHandle {
    pub fn new(sender: Sender<LspMessage>, loader_tx: Sender<Message>) -> Self {
        Self {
            sender,
            _loader_tx: loader_tx,
        }
    }
}

impl Handle for ClientHandle {
    /// Send a `client/registerCapability` request for `**/*.flx`. The client's
    /// response is not awaited — the main loop simply drops it. `roots` is
    /// unused: the glob is workspace-relative.
    fn watch(&mut self, _roots: &[PathBuf]) {
        let options = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher {
                glob_pattern: GlobPattern::String("**/*.flx".to_string()),
                kind: None,
            }],
        };
        let registration = Registration {
            id: "flux-lsp-watch-flx".to_string(),
            method: DidChangeWatchedFiles::METHOD.to_string(),
            register_options: serde_json::to_value(options).ok(),
        };
        let params = match serde_json::to_value(RegistrationParams {
            registrations: vec![registration],
        }) {
            Ok(params) => params,
            Err(err) => {
                tracing::warn!(%err, "failed to encode watcher registration");
                return;
            }
        };
        let _ = self.sender.send(LspMessage::Request(Request {
            id: RequestId::from("flux-lsp-register-watchers".to_string()),
            method: RegisterCapability::METHOD.to_string(),
            params,
        }));
    }
}

/// Server-side watching via the `notify` crate. Used when the client cannot
/// watch files itself.
pub struct NotifyHandle {
    /// Cloned into the watcher callback, which runs on `notify`'s own thread.
    loader_tx: Sender<Message>,
    /// Held for the server's lifetime so the OS watch registrations stay live.
    watcher: Option<notify::RecommendedWatcher>,
}

impl NotifyHandle {
    pub fn new(loader_tx: Sender<Message>) -> Self {
        Self {
            loader_tx,
            watcher: None,
        }
    }
}

impl Handle for NotifyHandle {
    /// Spawn a recursive `notify` watcher over each root. Its callback (on
    /// `notify`'s thread) filters to `.flx` files and forwards a [`Message`]
    /// over the loader channel.
    fn watch(&mut self, roots: &[PathBuf]) {
        let tx = self.loader_tx.clone();
        let mut watcher =
            match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                let files = flx_paths_of_event(&event);
                if !files.is_empty() {
                    // Unbounded channel — never blocks the watcher thread.
                    let _ = tx.send(Message::Changed { files });
                }
            }) {
                Ok(watcher) => watcher,
                Err(err) => {
                    tracing::warn!(%err, "failed to create filesystem watcher");
                    return;
                }
            };
        for root in roots {
            if let Err(err) = watcher.watch(root, RecursiveMode::Recursive) {
                tracing::warn!(%err, root = %root.display(), "failed to watch root");
            }
        }
        self.watcher = Some(watcher);
    }
}

/// Extract the `.flx` files touched by a `notify` event, dropping paths inside
/// build/VCS directories. `Access` events (file reads) and paths that are not
/// `.flx` files yield an empty result.
pub fn flx_paths_of_event(event: &notify::Event) -> Vec<PathBuf> {
    if matches!(event.kind, EventKind::Access(_)) {
        return Vec::new();
    }
    event
        .paths
        .iter()
        .filter(|p| is_watchable_flx(p))
        .cloned()
        .collect()
}

/// Whether `path` is a `.flx` file outside a skipped (build/VCS) directory.
fn is_watchable_flx(path: &Path) -> bool {
    if path.extension().is_none_or(|ext| ext != "flx") {
        return false;
    }
    !path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        crate::workspace::SKIP_DIRS.contains(&name.as_ref())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        ClientCapabilities, DidChangeWatchedFilesClientCapabilities, WorkspaceClientCapabilities,
    };

    fn params_with_dynamic_registration(value: Option<bool>) -> InitializeParams {
        InitializeParams {
            capabilities: ClientCapabilities {
                workspace: Some(WorkspaceClientCapabilities {
                    did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                        dynamic_registration: value,
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn capability_detection() {
        assert!(client_supports_dynamic_watchers(
            &params_with_dynamic_registration(Some(true))
        ));
        assert!(!client_supports_dynamic_watchers(
            &params_with_dynamic_registration(Some(false))
        ));
        assert!(!client_supports_dynamic_watchers(
            &params_with_dynamic_registration(None)
        ));
        // No `workspace` capabilities at all.
        assert!(!client_supports_dynamic_watchers(
            &InitializeParams::default()
        ));
    }

    #[test]
    fn watcher_kind_falls_back_when_unsupported() {
        assert_eq!(
            WatcherKind::for_client(&params_with_dynamic_registration(Some(true))),
            WatcherKind::Client
        );
        assert_eq!(
            WatcherKind::for_client(&InitializeParams::default()),
            WatcherKind::Notify
        );
    }

    /// Build a `notify::Event` over `paths` with a `Modify` kind.
    fn modify_event(paths: &[&str]) -> notify::Event {
        notify::Event {
            kind: EventKind::Modify(notify::event::ModifyKind::Any),
            paths: paths.iter().map(PathBuf::from).collect(),
            attrs: Default::default(),
        }
    }

    #[test]
    fn event_keeps_only_flx_files() {
        let event = modify_event(&["/proj/a.flx", "/proj/b.txt", "/proj/sub/c.flx"]);
        let files = flx_paths_of_event(&event);
        assert_eq!(
            files,
            vec![
                PathBuf::from("/proj/a.flx"),
                PathBuf::from("/proj/sub/c.flx")
            ]
        );
    }

    #[test]
    fn event_excludes_build_and_vcs_dirs() {
        let event = modify_event(&[
            "/proj/target/gen.flx",
            "/proj/node_modules/dep.flx",
            "/proj/.git/x.flx",
            "/proj/keep.flx",
        ]);
        assert_eq!(
            flx_paths_of_event(&event),
            vec![PathBuf::from("/proj/keep.flx")]
        );
    }

    #[test]
    fn access_events_are_ignored() {
        let event = notify::Event {
            kind: EventKind::Access(notify::event::AccessKind::Read),
            paths: vec![PathBuf::from("/proj/a.flx")],
            attrs: Default::default(),
        };
        assert!(flx_paths_of_event(&event).is_empty());
    }

    /// End-to-end watcher check: real filesystem events are timing-dependent
    /// and OS-specific, so this is run on demand (`cargo test -- --ignored`)
    /// rather than in CI.
    #[test]
    #[ignore = "depends on real filesystem-watch event delivery"]
    fn notify_handle_reports_flx_changes() {
        use std::io::Write;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("watched.flx");
        std::fs::write(&file, "x = 1\n").unwrap();

        let (tx, rx) = crossbeam_channel::unbounded::<Message>();
        let mut handle = NotifyHandle::new(tx);
        handle.watch(&[dir.path().to_path_buf()]);

        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap();
        writeln!(f, "y = 2").unwrap();
        f.sync_all().unwrap();

        let Message::Changed { files } = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("expected a Changed message");
        assert!(files.iter().any(|p| p.ends_with("watched.flx")));
    }
}
