//! Session persistence — save/restore workspaces, layouts, and working directories.
//!
//! Stored at `~/.config/herdr/session.json`.
//! Optional pane screen history is stored separately at `session-history.json`.
//! Installed plugins are persisted separately at `plugins.json`.
//! Deferred `when_idle` agent prompts are persisted separately at
//! `pending-prompts.json`.

pub mod channels;
mod io;
pub mod pending_prompts;
pub mod plugin_registry;
pub mod projects;
mod restore;
mod snapshot;

pub use self::io::{clear, clear_history, load, load_history, save};
pub use self::restore::restore;
#[cfg(unix)]
pub use self::restore::{handoff_pane_aliases, restore_handoff};
pub use self::snapshot::{
    capture, capture_history, DirectionSnapshot, LayoutSnapshot, SessionHistorySnapshot,
    SessionSnapshot, TabSnapshot, WorkspaceSnapshot,
};
