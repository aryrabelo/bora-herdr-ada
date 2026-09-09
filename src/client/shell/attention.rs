//! Pane-attention presentation (ceo-bora#302). A pane is "waiting" for
//! attention when it has produced no PTY output for
//! `ui.idle_attention_seconds` (client config; `0` disables the ramp) or is
//! `Blocked` (always attention-worthy, regardless of idle time). The facts
//! this reads -- `ClientShellPane.idle_seconds`, `ClientShellAgent.agent_status`
//! -- are server-side; the threshold and the resulting color/count are a
//! client presentation decision, same split as the rest of `client::shell`.
//!
//! Mirrors the pre-merge fork's `attention_pane_counts` (`ceo-bora` history,
//! pre-0.9.0-sync `src/ui/sidebar.rs`): a plain waiting pane is yellow, one
//! that is also `Blocked` pulls the aggregate to red.

use super::*;

/// Whether a pane counts as "waiting" for the aggregate counter and the
/// idle-ramp dot color.
pub(in crate::client::shell) fn pane_is_waiting(
    status: crate::api::schema::AgentStatus,
    idle_seconds: Option<u64>,
    idle_attention_seconds: u64,
) -> bool {
    status == crate::api::schema::AgentStatus::Blocked
        || (idle_attention_seconds > 0
            && idle_seconds.is_some_and(|seconds| seconds >= idle_attention_seconds))
}

/// Idle-ramp color for one pane's status glyph: the plain status color
/// until the pane crosses the attention threshold, then yellow, then red
/// once it is also `Blocked`.
pub(in crate::client::shell) fn pane_attention_color(
    status: crate::api::schema::AgentStatus,
    idle_seconds: Option<u64>,
    idle_attention_seconds: u64,
    palette: &Palette,
) -> ratatui::style::Color {
    if !pane_is_waiting(status, idle_seconds, idle_attention_seconds) {
        return status_color(status, palette);
    }
    if status == crate::api::schema::AgentStatus::Blocked {
        palette.red
    } else {
        palette.yellow
    }
}

/// Aggregate `(waiting, blocked)` pane counts across the whole snapshot,
/// for the sidebar header's "N waiting" badge. Channel workspaces (label
/// prefixed `#`, ceo-bora#31/#33) are excluded: their panes go quiet by
/// design and carry their own unread badge, not this one. Computed once
/// per sidebar render, never per row.
pub(in crate::client::shell) fn attention_counts(
    snapshot: &ClientShellSnapshot,
    idle_attention_seconds: u64,
) -> (usize, usize) {
    let mut waiting = 0;
    let mut blocked = 0;
    for pane in &snapshot.panes {
        let is_channel_workspace = snapshot
            .workspaces
            .iter()
            .find(|workspace| workspace.workspace_id == pane.workspace_id)
            .is_some_and(|workspace| workspace.label.starts_with('#'));
        if is_channel_workspace {
            continue;
        }
        let status = snapshot
            .agents
            .iter()
            .find(|agent| agent.pane_id == pane.pane_id)
            .map(|agent| agent.agent_status)
            .unwrap_or(crate::api::schema::AgentStatus::Unknown);
        if !pane_is_waiting(status, pane.idle_seconds, idle_attention_seconds) {
            continue;
        }
        waiting += 1;
        if status == crate::api::schema::AgentStatus::Blocked {
            blocked += 1;
        }
    }
    (waiting, blocked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::AgentStatus;

    #[test]
    fn blocked_pane_is_always_waiting_regardless_of_idle_seconds() {
        assert!(pane_is_waiting(AgentStatus::Blocked, None, 300));
        assert!(pane_is_waiting(AgentStatus::Blocked, Some(0), 0));
    }

    #[test]
    fn idle_pane_waits_only_past_threshold() {
        assert!(!pane_is_waiting(AgentStatus::Idle, Some(100), 300));
        assert!(pane_is_waiting(AgentStatus::Idle, Some(300), 300));
        assert!(pane_is_waiting(AgentStatus::Idle, Some(301), 300));
    }

    #[test]
    fn zero_threshold_disables_the_idle_ramp() {
        assert!(!pane_is_waiting(AgentStatus::Idle, Some(99_999), 0));
    }

    #[test]
    fn working_pane_never_waits_on_idle_seconds_alone() {
        // `Working` means active output; idle_seconds tracks silence, so a
        // working pane should never report a stale idle duration in
        // practice, but the predicate itself is status-driven only for
        // Blocked -- everything else is a pure idle-seconds comparison.
        assert!(!pane_is_waiting(AgentStatus::Working, None, 300));
    }
}
