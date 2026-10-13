//! `flux-dap`: the Debug Adapter Protocol server for Flux (proposal 0188 S4).
//!
//! The editor talks DAP to this process over stdio. The program runs in a
//! child process, `flux debug <program> --engine <address>`, the VM debugger
//! engine, which this adapter drives over a localhost socket with
//! [`flux::debug_protocol`]. The program's output comes back on the child's
//! stdout and stderr and goes to the editor as output events.
//!
//! - [`transport`]: DAP's `Content-Length` framing.
//! - [`engine`]: starting the child and talking to it.
//! - [`server`]: the session, translating DAP requests for the engine.

pub mod engine;
pub mod server;
pub mod transport;

pub use server::{Input, Session};
