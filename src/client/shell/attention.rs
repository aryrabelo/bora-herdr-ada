//! Pane-attention presentation (ceo-bora#302). A pane is "waiting" for
//! attention when it has been idle (time since the agent's last move to
//! Idle, `ClientShellPane.idle_seconds`) for at least
//! `ui.idle_attention_seconds` (client config; `0` disables the ramp) or is
//! `Blocked` (always attention-worthy, regardless of idle time) -- unless
//! its workspace is the one currently focused, in which case the operator
//! is already looking at it and neither the dot nor the aggregate counter
//! needs to nag them (cubic review on ceo-bora#302's PR: a focused
//! workspace's own quiet panes were counting as waiting). The facts this
//! reads -- `ClientShellPane.idle_seconds`, `ClientShellAgent.agent_status`,
//! `ClientShellWorkspace.focused` -- are server-side; the threshold and the
//! resulting color/count are a client presentation decision, same split as
//! the rest of `client::shell`.
//!
//! Mirrors the pre-merge fork's `attention_pane_counts` (`ceo-bora` history,
//! pre-0.9.0-sync `src/ui/sidebar.rs`): a plain waiting pane is yellow, one
//! that is also `Blocked` pulls the aggregate to red.
//!
//! `IdleTier`/`idle_tier` (ceo-bora#352) is a second, independent reading of
//! the same `idle_seconds` fact, for the agent panel's name color rather
//! than the Folders dots/badge: Fresh, Plain, or Stale by how long an
//! `Idle`/`Done` pane has sat, gated by `ui.idle_fresh_minutes` and
//! `ui.idle_stale_minutes`. It does not move the waiting counter. The
//! Stale tier also dims the whole agent row and sinks it in the panel
//! order (ceo-bora#353, `agent_sidebar::agent_is_idle_stale`), gated by
//! `ui.dim_stale_agents`.

use super::*;

/// Whether a pane counts as "waiting" for the aggregate counter and the
/// idle-ramp dot color. `workspace_focused` short-circuits to `false`: the
/// operator is looking at that workspace right now, so nothing in it needs
/// to compete for attention, blocked or not.
pub(in crate::client::shell) fn pane_is_waiting(
    status: crate::api::schema::AgentStatus,
    idle_seconds: Option<u64>,
    idle_attention_seconds: u64,
    workspace_focused: bool,
) -> bool {
    if workspace_focused {
        return false;
    }
    status == crate::api::schema::AgentStatus::Blocked
        || (idle_attention_seconds > 0
            && idle_seconds.is_some_and(|seconds| seconds >= idle_attention_seconds))
}

/// Idle-ramp color for one pane's status glyph: the plain status color
/// until the pane crosses the attention threshold, then yellow, then red
/// once it is also `Blocked`. Always the plain status color while its
/// workspace is focused (see module doc).
pub(in crate::client::shell) fn pane_attention_color(
    status: crate::api::schema::AgentStatus,
    idle_seconds: Option<u64>,
    idle_attention_seconds: u64,
    workspace_focused: bool,
    palette: &Palette,
) -> ratatui::style::Color {
    if !pane_is_waiting(
        status,
        idle_seconds,
        idle_attention_seconds,
        workspace_focused,
    ) {
        return status_color(status, palette);
    }
    if status == crate::api::schema::AgentStatus::Blocked {
        palette.red
    } else {
        palette.yellow
    }
}

/// Idle-duration tier for an `Idle`/`Done` pane's name color in the agent
/// panel (ceo-bora#352), a second, independent reading of
/// `ClientShellPane.idle_seconds` from the Folders-dot/badge ramp above:
/// `Fresh` just went idle, `Stale` has sat unseen a long time, `Plain` is
/// in between.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::client::shell) enum IdleTier {
    Fresh,
    Plain,
    Stale,
}

/// Map a pane's status and idle duration to its agent-panel name tier, if
/// any. Only `Idle` and `Done` panes get a tier -- `idle_seconds` tracks
/// time since the last move to `Idle`, which says nothing about a
/// `Working`/`Blocked`/`Unknown` pane. `None` idle_seconds (just
/// transitioned, no duration yet) also gets no tier.
///
/// `fresh_seconds == 0` disables the Fresh edge; `stale_seconds == 0`
/// disables the Stale edge. The Stale edge is checked first: `seconds >=
/// stale_seconds` (when nonzero) wins over the Fresh check, so a config
/// with `fresh_seconds > stale_seconds` (flagged as a diagnostic by
/// `Config::invalid_idle_tier_bounds_diagnostic`, never a panic) still
/// composes -- the stale edge simply catches everything past it, and
/// nothing past the stale edge is ever read as fresh instead.
pub(in crate::client::shell) fn idle_tier(
    status: crate::api::schema::AgentStatus,
    idle_seconds: Option<u64>,
    fresh_seconds: u64,
    stale_seconds: u64,
) -> Option<IdleTier> {
    use crate::api::schema::AgentStatus;
    if !matches!(status, AgentStatus::Idle | AgentStatus::Done) {
        return None;
    }
    let seconds = idle_seconds?;
    if stale_seconds > 0 && seconds >= stale_seconds {
        return Some(IdleTier::Stale);
    }
    if fresh_seconds > 0 && seconds < fresh_seconds {
        return Some(IdleTier::Fresh);
    }
    Some(IdleTier::Plain)
}

/// Client-side ageing of one cached snapshot's `ClientShellPane.idle_seconds`
/// (ceo-bora#352 follow-up). The server stamps `idle_seconds` when it builds
/// a snapshot and sends a new one only when something else changes, so on a
/// quiet screen the fact froze and neither an idle tier nor the
/// `idle_attention_seconds` ramp could cross its edge. The client ages it
/// instead: `advance(now)` returns the whole seconds elapsed since the last
/// call, measured from a baseline taken on the first call after the snapshot
/// was cached. The clock is passed in, never read here, so tests drive it
/// deterministically (same rule as the frame-counter spinner).
#[derive(Debug, Clone, Copy, Default)]
pub(in crate::client::shell) struct IdleAge {
    baseline: Option<std::time::Instant>,
    applied_seconds: u64,
}

impl IdleAge {
    pub(in crate::client::shell) fn advance(&mut self, now: std::time::Instant) -> u64 {
        let Some(baseline) = self.baseline else {
            self.baseline = Some(now);
            return 0;
        };
        let elapsed = now.saturating_duration_since(baseline).as_secs();
        let seconds = elapsed.saturating_sub(self.applied_seconds);
        self.applied_seconds = elapsed;
        seconds
    }
}

/// Add `seconds` to every pane's known `idle_seconds` and report whether any
/// pane crossed one of `edges` (the attention ramp and both tier edges; `0`
/// edges are disabled) -- the only case where the aged value changes what a
/// frame draws, so the caller repaints on `true` and stays idle otherwise.
pub(in crate::client::shell) fn age_idle_seconds(
    snapshot: &mut ClientShellSnapshot,
    seconds: u64,
    edges: [u64; 3],
) -> bool {
    let mut crossed = false;
    for pane in &mut snapshot.panes {
        let Some(before) = pane.idle_seconds else {
            continue;
        };
        let after = before.saturating_add(seconds);
        pane.idle_seconds = Some(after);
        crossed |= edges
            .iter()
            .any(|&edge| edge > 0 && before < edge && after >= edge);
    }
    crossed
}

/// Aggregate `(waiting, blocked)` counts across the whole snapshot, for the
/// sidebar header's "N waiting" badge. Channel workspaces (label prefixed
/// `#`, ceo-bora#31/#33) contribute no panes: their panes go quiet by
/// design and carry their own unread badge, not this one. What a channel
/// does contribute is each open question to the human
/// (`ClientShellWorkspace.open_human_asks`, ceo-bora#347), counted in BOTH
/// numbers so the badge turns red like `Blocked`: an agent asked the
/// operator something and is held on the answer. Only the count changes;
/// the view never switches to that channel by itself. The focused
/// workspace is excluded for panes and asks alike (see module doc) -- its
/// promoted state clears itself on the very next render once focus moves
/// away, since this is recomputed fresh every frame, never cached.
/// Computed once per sidebar render, never per row.
pub(in crate::client::shell) fn attention_counts(
    snapshot: &ClientShellSnapshot,
    idle_attention_seconds: u64,
) -> (usize, usize) {
    let workspaces_by_id = snapshot
        .workspaces
        .iter()
        .map(|workspace| (workspace.workspace_id.as_str(), workspace))
        .collect::<std::collections::HashMap<_, _>>();
    let status_by_pane_id = snapshot
        .agents
        .iter()
        .map(|agent| (agent.pane_id.as_str(), agent.agent_status))
        .collect::<std::collections::HashMap<_, _>>();
    let mut waiting = 0;
    let mut blocked = 0;
    for pane in &snapshot.panes {
        let Some(workspace) = workspaces_by_id.get(pane.workspace_id.as_str()) else {
            continue;
        };
        if workspace.label.starts_with('#') {
            continue;
        }
        let status = status_by_pane_id
            .get(pane.pane_id.as_str())
            .copied()
            .unwrap_or(crate::api::schema::AgentStatus::Unknown);
        if !pane_is_waiting(
            status,
            pane.idle_seconds,
            idle_attention_seconds,
            workspace.focused,
        ) {
            continue;
        }
        waiting += 1;
        if status == crate::api::schema::AgentStatus::Blocked {
            blocked += 1;
        }
    }
    for workspace in &snapshot.workspaces {
        if workspace.focused || !workspace.label.starts_with('#') {
            continue;
        }
        waiting += workspace.open_human_asks;
        blocked += workspace.open_human_asks;
    }
    (waiting, blocked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::AgentStatus;

    #[test]
    fn blocked_pane_is_always_waiting_regardless_of_idle_seconds() {
        assert!(pane_is_waiting(AgentStatus::Blocked, None, 300, false));
        assert!(pane_is_waiting(AgentStatus::Blocked, Some(0), 0, false));
    }

    #[test]
    fn idle_pane_waits_only_past_threshold() {
        assert!(!pane_is_waiting(AgentStatus::Idle, Some(100), 300, false));
        assert!(pane_is_waiting(AgentStatus::Idle, Some(300), 300, false));
        assert!(pane_is_waiting(AgentStatus::Idle, Some(301), 300, false));
    }

    #[test]
    fn zero_threshold_disables_the_idle_ramp() {
        assert!(!pane_is_waiting(AgentStatus::Idle, Some(99_999), 0, false));
    }

    #[test]
    fn working_pane_never_waits_on_idle_seconds_alone() {
        // `Working` means active output; idle_seconds tracks silence, so a
        // working pane should never report a stale idle duration in
        // practice, but the predicate itself is status-driven only for
        // Blocked -- everything else is a pure idle-seconds comparison.
        assert!(!pane_is_waiting(AgentStatus::Working, None, 300, false));
    }

    #[test]
    fn focused_workspace_never_waits_even_when_blocked() {
        // cubic review, ceo-bora#302: the operator is already looking at
        // the focused workspace, so it never competes for attention.
        assert!(!pane_is_waiting(AgentStatus::Blocked, None, 300, true));
        assert!(!pane_is_waiting(AgentStatus::Idle, Some(9_999), 300, true));
    }

    #[test]
    fn idle_tier_fresh_before_the_fresh_edge() {
        assert_eq!(
            idle_tier(AgentStatus::Idle, Some(0), 900, 7_200),
            Some(IdleTier::Fresh)
        );
        assert_eq!(
            idle_tier(AgentStatus::Idle, Some(899), 900, 7_200),
            Some(IdleTier::Fresh)
        );
    }

    #[test]
    fn idle_tier_plain_between_the_two_edges() {
        assert_eq!(
            idle_tier(AgentStatus::Idle, Some(900), 900, 7_200),
            Some(IdleTier::Plain)
        );
        assert_eq!(
            idle_tier(AgentStatus::Idle, Some(7_199), 900, 7_200),
            Some(IdleTier::Plain)
        );
    }

    #[test]
    fn idle_tier_stale_at_and_past_the_stale_edge() {
        assert_eq!(
            idle_tier(AgentStatus::Idle, Some(7_200), 900, 7_200),
            Some(IdleTier::Stale)
        );
        assert_eq!(
            idle_tier(AgentStatus::Idle, Some(99_999), 900, 7_200),
            Some(IdleTier::Stale)
        );
    }

    #[test]
    fn idle_tier_zero_fresh_edge_disables_the_fresh_tier() {
        assert_eq!(
            idle_tier(AgentStatus::Idle, Some(0), 0, 7_200),
            Some(IdleTier::Plain)
        );
    }

    #[test]
    fn idle_tier_zero_stale_edge_disables_the_stale_tier() {
        assert_eq!(
            idle_tier(AgentStatus::Idle, Some(99_999), 900, 0),
            Some(IdleTier::Plain)
        );
    }

    #[test]
    fn idle_tier_working_and_blocked_panes_get_no_tier() {
        assert_eq!(
            idle_tier(AgentStatus::Working, Some(99_999), 900, 7_200),
            None
        );
        assert_eq!(
            idle_tier(AgentStatus::Blocked, Some(99_999), 900, 7_200),
            None
        );
        assert_eq!(idle_tier(AgentStatus::Idle, None, 900, 7_200), None);
    }

    #[test]
    fn idle_tier_age_baselines_first_then_counts_whole_seconds() {
        let start = std::time::Instant::now();
        let mut age = IdleAge::default();
        assert_eq!(age.advance(start), 0);
        assert_eq!(
            age.advance(start + std::time::Duration::from_millis(900)),
            0
        );
        assert_eq!(
            age.advance(start + std::time::Duration::from_millis(2_500)),
            2
        );
        assert_eq!(age.advance(start + std::time::Duration::from_secs(3)), 1);
    }

    #[test]
    fn idle_tier_ageing_reports_only_an_edge_crossing() {
        let mut snapshot = crate::client::shell::tests::snapshot();
        snapshot.panes[0].idle_seconds = Some(898);
        assert!(!age_idle_seconds(&mut snapshot, 1, [300, 900, 7_200]));
        assert_eq!(snapshot.panes[0].idle_seconds, Some(899));
        assert!(age_idle_seconds(&mut snapshot, 1, [300, 900, 7_200]));
        assert_eq!(snapshot.panes[0].idle_seconds, Some(900));
        assert!(!age_idle_seconds(&mut snapshot, 10_000, [0, 0, 0]));

        snapshot.panes[0].idle_seconds = None;
        assert!(!age_idle_seconds(&mut snapshot, 10_000, [300, 900, 7_200]));
        assert_eq!(snapshot.panes[0].idle_seconds, None);
    }

    #[test]
    fn idle_tier_done_pane_gets_a_tier() {
        assert_eq!(
            idle_tier(AgentStatus::Done, Some(0), 900, 7_200),
            Some(IdleTier::Fresh)
        );
        assert_eq!(
            idle_tier(AgentStatus::Done, Some(7_200), 900, 7_200),
            Some(IdleTier::Stale)
        );
    }
}
