### Added
- **`Flow.Tcp.local_port` — read the OS-assigned port of a bound listener.**
  Bind with `listen(host, 0)` for an OS-chosen *ephemeral* port, then call
  `local_port(listener)` to learn which one — so same-process server/client pairs
  and tests can agree on a port without hard-coding one (and without racing a
  fixed port's `TIME_WAIT`). New `CorePrimOp::TcpLocalPort` wired end to end on
  both backends, mirroring `TcpAccept`: fiber-suspending, answered immediately
  from the listener's `local_addr()` (mio backend), through VM dispatch and the
  native C-runtime/LLVM path (`flux_tcp_local_port`). Parity fixture
  `tests/parity/tcp_local_port.flx`.

### Changed
- `examples/async/29_deep_io_in_both.flx` now binds an **ephemeral** port and
  hands it to the client over a channel (via `local_port`), instead of a
  hard-coded port with a `sleep`-ordered startup. Removes both the fixed-port
  collision on rapid reruns and the server/client startup race; the channel
  handshake also means the client never connects before the server is bound.
