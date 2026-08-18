use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::agents::QueuedAgentPromptDropReason;
use super::common::{AgentStatus, ReadSource};
use super::panes::{PaneInfo, PaneReadResult, PaneScrollInfo};
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;
use super::worktrees::WorktreeInfo;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventsSubscribeParams {
    pub subscriptions: Vec<Subscription>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum Subscription {
    #[serde(rename = "workspace.created")]
    WorkspaceCreated {},
    #[serde(rename = "workspace.updated")]
    WorkspaceUpdated {},
    #[serde(rename = "workspace.metadata_updated")]
    WorkspaceMetadataUpdated {},
    #[serde(rename = "workspace.renamed")]
    WorkspaceRenamed {},
    #[serde(rename = "workspace.moved")]
    WorkspaceMoved {},
    #[serde(rename = "workspace.reordered")]
    WorkspaceReordered {},
    #[serde(rename = "workspace.closed")]
    WorkspaceClosed {},
    #[serde(rename = "workspace.focused")]
    WorkspaceFocused {},
    #[serde(rename = "worktree.created")]
    WorktreeCreated {},
    #[serde(rename = "worktree.opened")]
    WorktreeOpened {},
    #[serde(rename = "worktree.removed")]
    WorktreeRemoved {},
    #[serde(rename = "tab.created")]
    TabCreated {},
    #[serde(rename = "tab.closed")]
    TabClosed {},
    #[serde(rename = "tab.focused")]
    TabFocused {},
    #[serde(rename = "tab.renamed")]
    TabRenamed {},
    #[serde(rename = "tab.moved")]
    TabMoved {},
    #[serde(rename = "pane.created")]
    PaneCreated {},
    #[serde(rename = "pane.closed")]
    PaneClosed {},
    #[serde(rename = "pane.updated")]
    PaneUpdated {},
    #[serde(rename = "pane.focused")]
    PaneFocused {},
    #[serde(rename = "pane.moved")]
    PaneMoved {},
    #[serde(rename = "pane.exited")]
    PaneExited {},
    #[serde(rename = "pane.agent_detected")]
    PaneAgentDetected {},
    #[serde(rename = "pane.result_reported")]
    PaneResultReported {},
    #[serde(rename = "pane.output_matched")]
    PaneOutputMatched {
        pane_id: String,
        source: ReadSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<u32>,
        r#match: OutputMatch,
        #[serde(default = "super::common::default_true")]
        strip_ansi: bool,
    },
    #[serde(rename = "pane.agent_status_changed")]
    PaneAgentStatusChanged {
        pane_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_status: Option<AgentStatus>,
    },
    #[serde(rename = "pane.scroll_changed")]
    PaneScrollChanged { pane_id: String },
    #[serde(rename = "layout.updated")]
    LayoutUpdated {},
    #[serde(rename = "github.prs_refreshed")]
    GithubPrsRefreshed {},
    #[serde(rename = "github.pr_opened")]
    GithubPrOpened {},
    #[serde(rename = "github.issues_refreshed")]
    GithubIssuesRefreshed {},
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventsWaitParams {
    pub match_event: EventMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneWaitForOutputParams {
    pub pane_id: String,
    pub source: ReadSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    pub r#match: OutputMatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default = "super::common::default_true")]
    pub strip_ansi: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutputMatch {
    Substring { value: String },
    Regex { value: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum EventMatch {
    WorkspaceCreated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
    },
    WorkspaceUpdated {
        workspace_id: String,
    },
    WorkspaceClosed {
        workspace_id: String,
    },
    WorkspaceRenamed {
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    WorkspaceMoved {
        workspace_id: String,
    },
    WorkspaceFocused {
        workspace_id: String,
    },
    TabCreated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tab_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
    },
    TabClosed {
        tab_id: String,
    },
    TabRenamed {
        tab_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    TabMoved {
        tab_id: String,
    },
    TabFocused {
        tab_id: String,
    },
    PaneCreated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pane_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
    },
    PaneClosed {
        pane_id: String,
    },
    PaneFocused {
        pane_id: String,
    },
    PaneMoved {
        pane_id: String,
    },
    PaneOutputChanged {
        pane_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min_revision: Option<u64>,
    },
    PaneExited {
        pane_id: String,
    },
    PaneAgentDetected {
        pane_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
    },
    PaneAgentStatusChanged {
        pane_id: String,
        agent_status: AgentStatus,
    },
    PaneResultReported {
        pane_id: String,
    },
    ChannelMessage {
        channel: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    WorkspaceCreated,
    WorkspaceUpdated,
    WorkspaceMetadataUpdated,
    WorkspaceClosed,
    WorkspaceRenamed,
    WorkspaceMoved,
    WorkspaceReordered,
    WorkspaceFocused,
    WorktreeCreated,
    WorktreeOpened,
    WorktreeRemoved,
    TabCreated,
    TabClosed,
    TabRenamed,
    TabMoved,
    TabFocused,
    PaneCreated,
    PaneClosed,
    PaneUpdated,
    PaneFocused,
    PaneMoved,
    PaneOutputChanged,
    PaneExited,
    PaneAgentDetected,
    PaneAgentStatusChanged,
    LayoutUpdated,
    PaneResultReported,
    GithubPrsRefreshed,
    GithubPrOpened,
    GithubIssuesRefreshed,
    QueuedPromptDelivered,
    QueuedPromptDropped,
    ChannelMessage,
}

impl EventKind {
    pub fn dot_name(self) -> &'static str {
        match self {
            EventKind::WorkspaceCreated => "workspace.created",
            EventKind::WorkspaceUpdated => "workspace.updated",
            EventKind::WorkspaceMetadataUpdated => "workspace.metadata_updated",
            EventKind::WorkspaceClosed => "workspace.closed",
            EventKind::WorkspaceRenamed => "workspace.renamed",
            EventKind::WorkspaceMoved => "workspace.moved",
            EventKind::WorkspaceReordered => "workspace.reordered",
            EventKind::WorkspaceFocused => "workspace.focused",
            EventKind::WorktreeCreated => "worktree.created",
            EventKind::WorktreeOpened => "worktree.opened",
            EventKind::WorktreeRemoved => "worktree.removed",
            EventKind::TabCreated => "tab.created",
            EventKind::TabClosed => "tab.closed",
            EventKind::TabRenamed => "tab.renamed",
            EventKind::TabMoved => "tab.moved",
            EventKind::TabFocused => "tab.focused",
            EventKind::PaneCreated => "pane.created",
            EventKind::PaneClosed => "pane.closed",
            EventKind::PaneUpdated => "pane.updated",
            EventKind::PaneFocused => "pane.focused",
            EventKind::PaneMoved => "pane.moved",
            EventKind::PaneOutputChanged => "pane.output_changed",
            EventKind::PaneExited => "pane.exited",
            EventKind::PaneAgentDetected => "pane.agent_detected",
            EventKind::PaneAgentStatusChanged => "pane.agent_status_changed",
            EventKind::LayoutUpdated => "layout.updated",
            EventKind::PaneResultReported => "pane.result_reported",
            EventKind::GithubPrsRefreshed => "github.prs_refreshed",
            EventKind::GithubPrOpened => "github.pr_opened",
            EventKind::GithubIssuesRefreshed => "github.issues_refreshed",
            EventKind::QueuedPromptDelivered => "agent_prompt.delivered",
            EventKind::QueuedPromptDropped => "agent_prompt.dropped",
            EventKind::ChannelMessage => "channel.message",
        }
    }
}

#[cfg(test)]
pub const KNOWN_EVENT_KINDS: &[EventKind] = &[
    EventKind::WorkspaceCreated,
    EventKind::WorkspaceUpdated,
    EventKind::WorkspaceMetadataUpdated,
    EventKind::WorkspaceClosed,
    EventKind::WorkspaceRenamed,
    EventKind::WorkspaceMoved,
    EventKind::WorkspaceReordered,
    EventKind::WorkspaceFocused,
    EventKind::WorktreeCreated,
    EventKind::WorktreeOpened,
    EventKind::WorktreeRemoved,
    EventKind::TabCreated,
    EventKind::TabClosed,
    EventKind::TabRenamed,
    EventKind::TabMoved,
    EventKind::TabFocused,
    EventKind::PaneCreated,
    EventKind::PaneClosed,
    EventKind::PaneUpdated,
    EventKind::PaneFocused,
    EventKind::PaneMoved,
    EventKind::PaneOutputChanged,
    EventKind::PaneExited,
    EventKind::PaneAgentDetected,
    EventKind::PaneAgentStatusChanged,
    EventKind::LayoutUpdated,
    EventKind::PaneResultReported,
    EventKind::GithubPrsRefreshed,
    EventKind::GithubPrOpened,
    EventKind::GithubIssuesRefreshed,
    EventKind::QueuedPromptDelivered,
    EventKind::QueuedPromptDropped,
    EventKind::ChannelMessage,
];

pub const PLUGIN_HOOK_EVENT_KINDS: &[EventKind] = &[
    EventKind::WorkspaceCreated,
    EventKind::WorkspaceUpdated,
    EventKind::WorkspaceClosed,
    EventKind::WorkspaceRenamed,
    EventKind::WorkspaceMoved,
    EventKind::WorkspaceReordered,
    EventKind::WorkspaceFocused,
    EventKind::WorktreeCreated,
    EventKind::WorktreeOpened,
    EventKind::WorktreeRemoved,
    EventKind::TabCreated,
    EventKind::TabClosed,
    EventKind::TabRenamed,
    EventKind::TabMoved,
    EventKind::TabFocused,
    EventKind::PaneCreated,
    EventKind::PaneClosed,
    EventKind::PaneFocused,
    EventKind::PaneMoved,
    EventKind::PaneExited,
    EventKind::PaneAgentDetected,
    EventKind::PaneAgentStatusChanged,
    EventKind::PaneResultReported,
    EventKind::QueuedPromptDelivered,
    EventKind::QueuedPromptDropped,
    EventKind::ChannelMessage,
    EventKind::GithubPrOpened,
];

#[cfg(test)]
pub fn known_event_names() -> Vec<&'static str> {
    KNOWN_EVENT_KINDS
        .iter()
        .copied()
        .map(EventKind::dot_name)
        .collect()
}

/// Event names that manifest `[[events]] on` hooks can reference. This is
/// intentionally narrower than `EventKind` until high-volume output-change hook
/// semantics are implemented.
pub fn plugin_hook_event_names() -> Vec<&'static str> {
    PLUGIN_HOOK_EVENT_KINDS
        .iter()
        .copied()
        .map(EventKind::dot_name)
        .collect()
}

#[cfg(test)]
mod known_event_name_tests {
    use super::*;

    #[test]
    fn known_event_names_stay_in_sync_with_event_kind() {
        let mut from_kind = KNOWN_EVENT_KINDS
            .iter()
            .map(|kind| kind.dot_name())
            .collect::<Vec<_>>();
        from_kind.sort_unstable();
        let mut known = known_event_names();
        known.sort_unstable();
        assert_eq!(
            from_kind, known,
            "known_event_names() out of sync with EventKind"
        );
    }

    #[test]
    fn plugin_hook_event_names_exclude_high_volume_events() {
        let names = plugin_hook_event_names();
        assert!(!names.contains(&"pane.output_changed"));
        assert!(!names.contains(&"layout.updated"));
        assert!(!names.contains(&"workspace.metadata_updated"));
        assert!(!names.contains(&"pane.updated"));
        assert!(names.contains(&"pane.moved"));
    }

    #[test]
    fn github_pr_opened_is_a_plugin_hook_event() {
        assert!(PLUGIN_HOOK_EVENT_KINDS.contains(&EventKind::GithubPrOpened));
        assert_eq!(EventKind::GithubPrOpened.dot_name(), "github.pr_opened");
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EventEnvelope {
    pub event: EventKind,
    pub data: EventData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum SubscriptionEventKind {
    #[serde(rename = "pane.output_matched")]
    PaneOutputMatched,
    #[serde(rename = "pane.agent_status_changed")]
    PaneAgentStatusChanged,
    #[serde(rename = "pane.scroll_changed")]
    ScrollChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SubscriptionEventEnvelope {
    pub event: SubscriptionEventKind,
    pub data: SubscriptionEventData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum SubscriptionEventData {
    PaneOutputMatched(PaneOutputMatchedEvent),
    PaneAgentStatusChanged(PaneAgentStatusChangedEvent),
    ScrollChanged(PaneScrollChangedEvent),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneOutputMatchedEvent {
    pub pane_id: String,
    pub matched_line: String,
    pub read: PaneReadResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneAgentStatusChangedEvent {
    pub pane_id: String,
    pub workspace_id: String,
    pub agent_status: AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_agent: Option<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub state_labels: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PaneScrollChangedEvent {
    pub pane_id: String,
    pub workspace_id: String,
    pub scroll: PaneScrollInfo,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventData {
    WorkspaceCreated {
        workspace: WorkspaceInfo,
    },
    WorkspaceUpdated {
        workspace: WorkspaceInfo,
    },
    WorkspaceMetadataUpdated {
        workspace: WorkspaceInfo,
    },
    WorkspaceClosed {
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<WorkspaceInfo>,
    },
    WorkspaceRenamed {
        workspace_id: String,
        label: String,
    },
    WorkspaceMoved {
        workspace_id: String,
        insert_index: usize,
        workspaces: Vec<WorkspaceInfo>,
    },
    WorkspaceReordered {
        workspace_ids: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before_workspace_id: Option<String>,
        workspaces: Vec<WorkspaceInfo>,
    },
    WorkspaceFocused {
        workspace_id: String,
    },
    WorktreeCreated {
        workspace: WorkspaceInfo,
        worktree: WorktreeInfo,
    },
    WorktreeOpened {
        workspace: WorkspaceInfo,
        worktree: WorktreeInfo,
        already_open: bool,
    },
    WorktreeRemoved {
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace: Option<WorkspaceInfo>,
        worktree: WorktreeInfo,
        forced: bool,
    },
    TabCreated {
        tab: TabInfo,
    },
    TabClosed {
        tab_id: String,
        workspace_id: String,
    },
    TabRenamed {
        tab_id: String,
        workspace_id: String,
        label: String,
    },
    TabMoved {
        tab_id: String,
        workspace_id: String,
        insert_index: usize,
        tabs: Vec<TabInfo>,
    },
    TabFocused {
        tab_id: String,
        workspace_id: String,
    },
    PaneCreated {
        pane: PaneInfo,
    },
    PaneClosed {
        pane_id: String,
        workspace_id: String,
    },
    PaneUpdated {
        pane: PaneInfo,
    },
    PaneFocused {
        pane_id: String,
        workspace_id: String,
    },
    PaneMoved {
        previous_pane_id: String,
        previous_workspace_id: String,
        previous_tab_id: String,
        pane: Box<PaneInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_workspace: Option<WorkspaceInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_tab: Option<TabInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        closed_workspace_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        closed_tab_id: Option<String>,
    },
    PaneOutputChanged {
        pane_id: String,
        workspace_id: String,
        revision: u64,
    },
    PaneExited {
        pane_id: String,
        workspace_id: String,
    },
    PaneAgentDetected {
        pane_id: String,
        workspace_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        released: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        final_status: Option<AgentStatus>,
    },
    PaneAgentStatusChanged {
        pane_id: String,
        workspace_id: String,
        agent_status: AgentStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display_agent: Option<String>,
        #[serde(default, skip_serializing_if = "HashMap::is_empty")]
        state_labels: HashMap<String, String>,
    },
    LayoutUpdated {
        layout: super::panes::PaneLayoutSnapshot,
    },
    PaneResultReported {
        pane_id: String,
        workspace_id: String,
        result: serde_json::Value,
    },
    GithubPrsRefreshed {
        repo_identity: String,
    },
    /// Fires once when bora's own `gh pr create` succeeds for a worktree.
    /// Does not cover PRs opened outside bora, e.g. a `gh` run inside an
    /// agent pane.
    GithubPrOpened {
        branch: String,
        url: String,
        workspace_ids: Vec<String>,
    },
    GithubIssuesRefreshed {
        repo_identity: String,
    },
    /// A `when_idle`-deferred agent prompt was drained and successfully
    /// injected into its target. Terminal, matching `queue_id` from the
    /// original `agent.prompt` deferred receipt.
    QueuedPromptDelivered {
        queue_id: u64,
        target_pane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_pane: Option<String>,
    },
    /// A `when_idle`-deferred agent prompt was dropped without ever reaching
    /// its target — the durable record the deferred receipt promised, since
    /// the queue itself only logs internally. Terminal, matching `queue_id`
    /// from the original `agent.prompt` deferred receipt.
    QueuedPromptDropped {
        queue_id: u64,
        target_pane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workspace_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_pane: Option<String>,
        reason: QueuedAgentPromptDropReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// A message was appended to a `#`-channel transcript (emitted right
    /// after the durable append, before fan-out). `channel` is the
    /// normalized name without the leading `#`; `seq` is the message's
    /// monotonic per-channel id; `from_pane` is `None` for unattributed
    /// senders; `to_pane` is the resolved targeted pane id or `None` for
    /// broadcast.
    ChannelMessage {
        channel: String,
        seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_pane: Option<String>,
        from_name: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_pane: Option<String>,
    },
}

/// Returns whether an emitted `EventEnvelope` satisfies an `events.wait` filter.
pub fn event_matches(filter: &EventMatch, envelope: &EventEnvelope) -> bool {
    match (filter, &envelope.data) {
        (
            EventMatch::WorkspaceCreated { workspace_id },
            EventData::WorkspaceCreated { workspace },
        ) => workspace_id
            .as_ref()
            .is_none_or(|id| *id == workspace.workspace_id),
        (
            EventMatch::WorkspaceUpdated { workspace_id },
            EventData::WorkspaceUpdated { workspace },
        ) => *workspace_id == workspace.workspace_id,
        (
            EventMatch::WorkspaceClosed { workspace_id },
            EventData::WorkspaceClosed {
                workspace_id: actual,
                ..
            },
        ) => workspace_id == actual,
        (
            EventMatch::WorkspaceRenamed {
                workspace_id,
                label,
            },
            EventData::WorkspaceRenamed {
                workspace_id: actual,
                label: actual_label,
            },
        ) => workspace_id == actual && label.as_ref().is_none_or(|l| l == actual_label),
        (
            EventMatch::WorkspaceFocused { workspace_id },
            EventData::WorkspaceFocused {
                workspace_id: actual,
            },
        ) => workspace_id == actual,
        (
            EventMatch::TabCreated {
                tab_id,
                workspace_id,
            },
            EventData::TabCreated { tab },
        ) => {
            tab_id.as_ref().is_none_or(|id| *id == tab.tab_id)
                && workspace_id
                    .as_ref()
                    .is_none_or(|id| *id == tab.workspace_id)
        }
        (EventMatch::TabClosed { tab_id }, EventData::TabClosed { tab_id: actual, .. }) => {
            tab_id == actual
        }
        (
            EventMatch::TabRenamed { tab_id, label },
            EventData::TabRenamed {
                tab_id: actual,
                label: actual_label,
                ..
            },
        ) => tab_id == actual && label.as_ref().is_none_or(|l| l == actual_label),
        (EventMatch::TabFocused { tab_id }, EventData::TabFocused { tab_id: actual, .. }) => {
            tab_id == actual
        }
        (
            EventMatch::PaneCreated {
                pane_id,
                workspace_id,
            },
            EventData::PaneCreated { pane },
        ) => {
            pane_id.as_ref().is_none_or(|id| *id == pane.pane_id)
                && workspace_id
                    .as_ref()
                    .is_none_or(|id| *id == pane.workspace_id)
        }
        (
            EventMatch::PaneClosed { pane_id },
            EventData::PaneClosed {
                pane_id: actual, ..
            },
        ) => pane_id == actual,
        (
            EventMatch::PaneFocused { pane_id },
            EventData::PaneFocused {
                pane_id: actual, ..
            },
        ) => pane_id == actual,
        (EventMatch::PaneMoved { pane_id }, EventData::PaneMoved { pane, .. }) => {
            *pane_id == pane.pane_id
        }
        (
            EventMatch::PaneOutputChanged {
                pane_id,
                min_revision,
            },
            EventData::PaneOutputChanged {
                pane_id: actual,
                revision,
                ..
            },
        ) => pane_id == actual && min_revision.is_none_or(|min| *revision >= min),
        (
            EventMatch::PaneExited { pane_id },
            EventData::PaneExited {
                pane_id: actual, ..
            },
        ) => pane_id == actual,
        (
            EventMatch::PaneAgentDetected { pane_id, agent },
            EventData::PaneAgentDetected {
                pane_id: actual,
                agent: actual_agent,
                ..
            },
        ) => {
            pane_id == actual
                && agent
                    .as_ref()
                    .is_none_or(|a| Some(a) == actual_agent.as_ref())
        }
        (
            EventMatch::PaneAgentStatusChanged {
                pane_id,
                agent_status,
            },
            EventData::PaneAgentStatusChanged {
                pane_id: actual,
                agent_status: actual_status,
                ..
            },
        ) => pane_id == actual && agent_status == actual_status,
        (
            EventMatch::PaneResultReported { pane_id },
            EventData::PaneResultReported {
                pane_id: actual, ..
            },
        ) => pane_id == actual,
        (
            EventMatch::ChannelMessage { channel },
            EventData::ChannelMessage {
                channel: actual, ..
            },
        ) => channel == actual,
        _ => false,
    }
}

#[cfg(test)]
mod event_matches_tests {
    use super::*;

    fn result_envelope(pane_id: &str) -> EventEnvelope {
        EventEnvelope {
            event: EventKind::PaneResultReported,
            data: EventData::PaneResultReported {
                pane_id: pane_id.into(),
                workspace_id: "ws_1".into(),
                result: serde_json::json!({"ok": true}),
            },
        }
    }

    #[test]
    fn result_reported_matches_same_pane() {
        let filter = EventMatch::PaneResultReported {
            pane_id: "pane_1".into(),
        };
        assert!(event_matches(&filter, &result_envelope("pane_1")));
    }

    #[test]
    fn channel_message_matches_same_channel_only() {
        let envelope = EventEnvelope {
            event: EventKind::ChannelMessage,
            data: EventData::ChannelMessage {
                channel: "eng".into(),
                seq: 7,
                from_pane: Some("w1A:p2".into()),
                from_name: "brandos".into(),
                text: "hello".into(),
                to_pane: None,
            },
        };
        let filter = EventMatch::ChannelMessage {
            channel: "eng".into(),
        };
        assert!(event_matches(&filter, &envelope));

        let other_channel = EventMatch::ChannelMessage {
            channel: "ops".into(),
        };
        assert!(!event_matches(&other_channel, &envelope));

        let other_kind = EventMatch::PaneResultReported {
            pane_id: "eng".into(),
        };
        assert!(!event_matches(&other_kind, &envelope));
    }

    #[test]
    fn result_reported_rejects_other_pane_and_kinds() {
        let filter = EventMatch::PaneResultReported {
            pane_id: "pane_1".into(),
        };
        assert!(!event_matches(&filter, &result_envelope("pane_2")));

        let other = EventEnvelope {
            event: EventKind::PaneExited,
            data: EventData::PaneExited {
                pane_id: "pane_1".into(),
                workspace_id: "ws_1".into(),
            },
        };
        assert!(!event_matches(&filter, &other));
    }
}
