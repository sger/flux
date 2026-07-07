### Fixed
- **VS Code extension: `flux.autoRestartOnBinaryChange` no longer throws when the
  server binary appears after activation.** When the resolved `flux-lsp` binary
  was missing at activation (e.g. a fresh checkout before the first
  `cargo build`), the initial `client.start()` rejected and left the language
  client in the internal `startFailed` state. When the binary was then built, the
  binary watcher fired and called `client.restart()`, whose implicit `stop()`
  throws *"Client is not running and can't be stopped. It's current state is:
  startFailed"* — so the server never came up even though the binary now existed.
  The auto-restart handler now inspects `client.state` and issues a plain
  `start()` when the client is not `Running`, reserving `restart()` for a live
  client.
