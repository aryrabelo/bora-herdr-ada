use serde::{Deserialize, Serialize};

use super::agents::{AgentInfo, AgentPromptOutcome};
use super::channels::{ChannelDelivery, ChannelMember, ChannelMessage, ChannelSummary};
use super::common::{ClientWindowTitleReason, NotificationShowReason};
use super::events::EventEnvelope;
use super::integrations::{
    IntegrationInstallResult, IntegrationTarget, IntegrationUninstallResult,
};
use super::panes::{
    LayoutDescription, PaneEdgesResult, PaneFocusDirectionResult, PaneInfo, PaneLayoutSnapshot,
    PaneMoveResult, PaneNeighborResult, PaneProcessInfo, PaneReadResult, PaneResizeResult,
    PaneSwapResult, PaneZoomResult,
};
use super::plugins::{
    InstalledPluginInfo, PluginActionInfo, PluginCommandLogInfo, PluginInvocationContext,
    PluginPaneInfo,
};
use super::server::ServerCapabilities;
use super::session::SessionSnapshot;
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;
use super::worktrees::{WorktreeInfo, WorktreeSourceInfo};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SuccessResponse {
    pub id: String,
    pub result: ResponseResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorResponse {
    pub id: String,
    pub error: ErrorBody,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseResult {
    Pong {
        version: String,
        protocol: u32,
        #[serde(default)]
        capabilities: Option<ServerCapabilities>,
    },
    SessionSnapshot {
        snapshot: Box<SessionSnapshot>,
    },
    WorkspaceInfo {
        workspace: WorkspaceInfo,
    },
    WorkspaceCreated {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
    },
    WorkspaceList {
        workspaces: Vec<WorkspaceInfo>,
    },
    WorktreeList {
        source: WorktreeSourceInfo,
        worktrees: Vec<WorktreeInfo>,
    },
    WorktreeCreated {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
        #[serde(default)]
        setup: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        setup_error: Option<String>,
    },
    WorktreeOpened {
        workspace: WorkspaceInfo,
        tab: TabInfo,
        root_pane: PaneInfo,
        worktree: WorktreeInfo,
        already_open: bool,
    },
    WorktreeRemoved {
        workspace_id: String,
        path: String,
        forced: bool,
    },
    TabInfo {
        tab: TabInfo,
    },
    TabCreated {
        tab: TabInfo,
        root_pane: PaneInfo,
    },
    TabList {
        tabs: Vec<TabInfo>,
    },
    AgentInfo {
        agent: AgentInfo,
    },
    AgentStarted {
        agent: AgentInfo,
        argv: Vec<String>,
    },
    AgentPrompted {
        agent: AgentInfo,
        #[serde(default)]
        outcome: AgentPromptOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queue_position: Option<usize>,
        /// Set only when `outcome` is `deferred`: the id of the queued prompt,
        /// stable for its whole lifetime in the pending queue. Correlates this
        /// receipt with its terminal-fate `agent_prompt.delivered` /
        /// `agent_prompt.dropped` event.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queue_id: Option<u64>,
    },
    AgentList {
        agents: Vec<AgentInfo>,
    },
    AgentView {
        active: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    PaneInfo {
        pane: PaneInfo,
    },
    PaneList {
        panes: Vec<PaneInfo>,
    },
    PaneCurrent {
        pane: PaneInfo,
    },
    PaneSwap {
        swap: PaneSwapResult,
    },
    PaneMove {
        move_result: PaneMoveResult,
    },
    PaneZoom {
        zoom: PaneZoomResult,
    },
    PaneLayout {
        layout: PaneLayoutSnapshot,
    },
    PaneProcessInfo {
        process_info: PaneProcessInfo,
    },
    LayoutExport {
        layout: LayoutDescription,
    },
    LayoutApply {
        layout: LayoutDescription,
    },
    LayoutSplitRatioSet {
        layout: LayoutDescription,
    },
    PaneNeighbor {
        neighbor: PaneNeighborResult,
    },
    PaneEdges {
        edges: PaneEdgesResult,
    },
    PaneFocusDirection {
        focus: PaneFocusDirectionResult,
    },
    PaneResize {
        resize: PaneResizeResult,
    },
    PaneRead {
        read: PaneReadResult,
    },
    PaneGraphicsFrameAck {
        sequence: u64,
        revision: u64,
    },
    PaneGraphicsInfo {
        cell_width_px: u32,
        cell_height_px: u32,
        /// True only when this pane is on the currently rendered terminal surface.
        pane_visible: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_frame_directory: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        file_frame_formats: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_frame_max_bytes: Option<usize>,
        /// Accepts damage metadata while still consuming a complete canonical file.
        #[serde(default)]
        file_frame_damage: bool,
        #[serde(default)]
        max_layers_per_pane: usize,
        #[serde(default)]
        pixel_mouse: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_frame_transport: Option<String>,
    },
    AgentExplain {
        explain: serde_json::Value,
    },
    SubscriptionStarted {},
    WaitMatched {
        event: EventEnvelope,
    },
    OutputMatched {
        pane_id: String,
        revision: u64,
        matched_line: Option<String>,
        read: PaneReadResult,
    },
    NotificationShow {
        shown: bool,
        reason: NotificationShowReason,
    },
    ClientWindowTitle {
        changed: bool,
        reason: ClientWindowTitleReason,
    },
    IntegrationInstall {
        target: IntegrationTarget,
        details: IntegrationInstallResult,
    },
    IntegrationUninstall {
        target: IntegrationTarget,
        details: IntegrationUninstallResult,
    },
    AgentManifestReload {
        manifests: Vec<AgentManifestInfo>,
    },
    AgentManifestStatus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_check_unix: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_result: Option<String>,
        manifests: Vec<AgentManifestInfo>,
    },
    PluginLinked {
        plugin: InstalledPluginInfo,
    },
    PluginList {
        plugins: Vec<InstalledPluginInfo>,
    },
    PluginUnlinked {
        plugin_id: String,
        removed: bool,
    },
    PluginEnabled {
        plugin: InstalledPluginInfo,
    },
    PluginDisabled {
        plugin: InstalledPluginInfo,
    },
    PluginActionList {
        actions: Vec<PluginActionInfo>,
    },
    PluginActionInvoked {
        action: PluginActionInfo,
        context: PluginInvocationContext,
        log: PluginCommandLogInfo,
    },
    PluginLogList {
        logs: Vec<PluginCommandLogInfo>,
    },
    PluginPaneOpened {
        plugin_pane: PluginPaneInfo,
    },
    PluginPaneFocused {
        plugin_pane: PluginPaneInfo,
    },
    PluginPaneClosed {
        pane_id: String,
    },
    ConfigReload {
        status: crate::config::ConfigReloadStatus,
        diagnostics: Vec<String>,
    },
    Ok {},
    GithubPullsList {
        repos: Vec<super::github::GithubRepoPrs>,
    },
    GithubIssuesList {
        repos: Vec<super::github::GithubRepoIssues>,
    },
    ChannelCreated {
        channel: ChannelSummary,
    },
    ChannelList {
        channels: Vec<ChannelSummary>,
    },
    ChannelSent {
        deliveries: Vec<ChannelDelivery>,
        /// `true` when the channel was inside an active burst and the bell
        /// (agent injection fan-out) was cut for this send; the message was
        /// still appended to the transcript and eventable as normal. See
        /// `ui.channel_burst_messages` / `ui.channel_burst_window_secs`.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        suppressed: bool,
        /// Monotonic per-channel seq assigned to the appended message —
        /// the correlation key a reply threads back through `in_reply_to`.
        seq: u64,
    },
    ChannelHistory {
        messages: Vec<ChannelMessage>,
    },
    ChannelMembers {
        members: Vec<ChannelMember>,
    },
    /// `channel.join` result. `source` is `joined` when explicit membership
    /// was recorded, and `workspace` when the pane already lived in the
    /// channel's workspace and was a member all along — the join succeeded
    /// but changed nothing.
    ChannelJoined {
        pane_id: String,
        source: super::channels::ChannelMemberSource,
    },
    /// `channel.leave` result. `removed: false` means the pane was not an
    /// explicitly joined member to begin with (never joined, or a
    /// workspace-implicit member that cannot be removed this way).
    ChannelLeft {
        pane_id: String,
        removed: bool,
    },
    /// `channel.wait` result. `messages` are every retained message with
    /// `seq > after_seq` in order — the last one's `seq` is the resume
    /// cursor. `gap: true` means rotation dropped messages between the
    /// caller's cursor and `oldest_seq` (or the history is empty while the
    /// cursor is past 0): continuity is broken, not silent. `timed_out`
    /// means the deadline elapsed with nothing new — a clean no-message,
    /// never an error.
    ChannelWait {
        messages: Vec<ChannelMessage>,
        #[serde(default)]
        gap: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oldest_seq: Option<u64>,
        #[serde(default)]
        timed_out: bool,
    },
    /// `channel.ask` result. `question_seq` is the seq of the appended
    /// question — the correlation key a reply threads back through
    /// `in_reply_to`. `answered: false` means `timeout_ms` elapsed with no
    /// matching reply (`reply: None`); `answered: true` carries the
    /// matching reply message.
    ChannelAsked {
        answered: bool,
        question_seq: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reply: Option<ChannelMessage>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentManifestInfo {
    pub agent: String,
    pub source: String,
    pub source_kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_remote_version: Option<String>,
    pub local_override_shadowing_remote: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_update_result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_update_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_last_checked_unix: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}
