//! ARGUS terminal user interface (Ratatui + Crossterm).
//!
//! Presentation logic only. The TUI must not perform privileged operations
//! directly; it communicates with `argusd` through `argus-ipc`.

mod app;
mod data;

use std::path::Path;

use anyhow::Result;

/// Runs the interactive TUI, fetching runtime state from `argusd`.
pub async fn run(socket_path: &Path) -> Result<()> {
    let data = match data::fetch(socket_path).await {
        Ok(d) => d,
        // The TUI still runs when the daemon is unreachable (e.g. the `init`
        // setup flow before `argusd` is running).
        Err(e) => data::Data::unavailable(&e.to_string()),
    };
    app::run(data)
}
