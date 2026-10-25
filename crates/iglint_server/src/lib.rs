//! The iglint language server.
//!
//! Built on `lsp-server` and `lsp-types`, the synchronous stack of
//! rust-analyzer: the linter is CPU-bound and already parallel (`rayon`), so
//! an async runtime would add a scheduler and `Send + 'static` plumbing
//! without making anything faster. A single loop owns all state; requests are
//! answered in order, and edits are coalesced by a short debounce before the
//! affected files are analysed.
//!
//! State per workspace folder ([`folder`]): the resolved configuration and an
//! incremental [`iglint_linter::live::LiveWorkspace`] that keeps every file's
//! text and every resource's summary. An edit re-models only the edited
//! resource; the files whose findings depend on its summary (for example the
//! views that embed a view whose parameters changed) are re-checked with it.

mod actions;
mod convert;
mod folder;
mod fresh;
mod options;
mod server;

pub use options::{RunTrigger, Scope, ServerOptions};
pub use server::{COMMAND_FIX_ALL, COMMAND_RULE_DOCS, FIX_ALL_KIND};

use lsp_server::Connection;

/// Runs the language server on stdin and stdout until the client exits.
pub fn run() -> anyhow::Result<()> {
    let (connection, io_threads) = Connection::stdio();
    run_connection(connection)?;
    io_threads.join()?;
    Ok(())
}

/// Runs the server on an established connection (stdio, or in memory for
/// tests), from the `initialize` handshake until `exit`.
pub fn run_connection(connection: Connection) -> anyhow::Result<()> {
    server::Server::start(connection)
}
