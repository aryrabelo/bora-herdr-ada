use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Opaque identity for a server-owned terminal.
///
/// During the pane-backed transition this is stored one-to-one beside panes,
/// but callers must not derive it from a pane id or layout position.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct TerminalId(String);

static NEXT_TERMINAL_ID: AtomicU64 = AtomicU64::new(1);

impl TerminalId {
    pub fn alloc() -> Self {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_micros())
            .unwrap_or(0);
        let counter = NEXT_TERMINAL_ID.fetch_add(1, Ordering::Relaxed);
        Self(format!("term_{micros:x}{counter:x}"))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TerminalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Durable identity for the *agent* that lives in a terminal.
///
/// This exists because every other id in the system fails at least one of the
/// four properties a channel registry needs. `pane_id` is reallocated on cold
/// restore (`persist/restore.rs`, and the test
/// `restore_preserves_public_id_mapping_after_pane_id_remap`). [`TerminalId`]
/// is re-minted unconditionally on every restore. `agent_session_id` names a
/// *conversation*, not an agent, and rolls over whenever the harness starts a
/// new session. `agent_name` is globally unique but optional, and its restore
/// is conditional on `managed_agent_kind` — the test
/// `cold_restore_with_gapped_public_tab_numbers_drops_unmanaged_agent_name`
/// exists precisely because it gets dropped.
///
/// So: minted once per terminal at birth, persisted in `PaneSnapshot`, and
/// restored **unconditionally**. The unconditional part is the whole point —
/// every conditional restore path in this file is a way to silently lose
/// identity, and channel membership keyed on lost identity disappears without
/// an error.
///
/// Never derive one from a pane id, a layout position, or a terminal id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AgentId(String);

static NEXT_AGENT_ID: AtomicU64 = AtomicU64::new(1);

impl AgentId {
    /// Mints a fresh id. Same shape as [`TerminalId::alloc`] — wall-clock
    /// micros for cross-process uniqueness plus a process-local counter for
    /// within-process uniqueness — so no new dependency is needed for this.
    pub fn alloc() -> Self {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_micros())
            .unwrap_or(0);
        let counter = NEXT_AGENT_ID.fetch_add(1, Ordering::Relaxed);
        Self(format!("agent_{micros:x}{counter:x}"))
    }

    /// Rebuilds an id read back from a snapshot. Deliberately infallible and
    /// lossless: a persisted id is authoritative even if a future version
    /// changes the minting format, because rejecting it would silently orphan
    /// the channel membership it keys.
    pub fn from_persisted(raw: String) -> Self {
        Self(raw)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_agent_ids_are_unique() {
        let a = AgentId::alloc();
        let b = AgentId::alloc();
        assert_ne!(a, b, "two freshly minted agent ids must differ");
        assert!(a.as_str().starts_with("agent_"));
    }

    #[test]
    fn persisted_agent_id_round_trips_verbatim() {
        // A snapshot written by any version must come back byte-identical:
        // this id is a registry key, so "almost the same" means "lost".
        let raw = "agent_deadbeef1".to_string();
        assert_eq!(AgentId::from_persisted(raw.clone()).as_str(), raw);
    }
}
