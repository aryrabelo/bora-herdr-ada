use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::common::AgentStatus;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelCreateParams {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelSendParams {
    pub name: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_pane: Option<String>,
    /// Structured addressing: a nick or raw pane id resolved server-side
    /// against the channel's member panes. The primary addressing path for
    /// model senders — never depend on in-body `@nick` parsing. Unique match
    /// -> delivered to that pane only; 2+ matches -> `channel_nick_ambiguous`;
    /// no match -> `channel_nick_unknown` (the send fails before append).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Seq of the channel message this one replies to. Threading metadata
    /// only — recorded on the message, never used for delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelHistoryParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelMembersParams {
    pub name: String,
}

/// `channel.join`: make `pane` an explicit member of a channel it does not
/// live in, so `channel.send` fan-out and `@nick` addressing reach it. The
/// pane keeps living in its own workspace; membership is the only thing
/// recorded, and it survives a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelJoinParams {
    pub name: String,
    /// Public pane id to add. The CLI defaults it to `$HERDR_PANE_ID`.
    pub pane: String,
}

/// `channel.leave`: drop an explicitly joined pane from a channel. Panes that
/// live in the channel's own workspace are members by construction and
/// cannot be removed this way.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelLeaveParams {
    pub name: String,
    pub pane: String,
}

/// `channel.wait`: cursor-based tail follow. Returns every retained message
/// with `seq > after_seq` (backlog first), then blocks until a new message
/// is appended or `timeout_ms` elapses. `None` timeout waits forever;
/// `Some(0)` never blocks (backlog-only snapshot). Timeout is a clean
/// `timed_out: true` response, never an error. When `after_seq` predates
/// the oldest retained line (rotation dropped messages in between), the
/// response says so via `gap: true` + `oldest_seq` instead of pretending
/// continuity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelWaitParams {
    pub name: String,
    /// Resume cursor: only messages with a strictly greater seq are
    /// returned. `0` skips pre-seq history (old lines default to seq 0).
    #[serde(default)]
    pub after_seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelSummary {
    /// Channel name including the leading `#`.
    pub name: String,
    pub pane_count: usize,
    pub agent_count: usize,
    /// Member panes' agent status (`"idle"`, `"working"`, ...) mapped to how
    /// many panes are currently in that status. Panes not hosting a detected
    /// agent are excluded, so this can undercount `pane_count`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub member_status_counts: HashMap<String, usize>,
}

/// How a pane came to be a channel member.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelMemberSource {
    /// The pane lives in the channel's `#name` workspace.
    Workspace,
    /// The pane lives elsewhere and joined explicitly (`channel.join`).
    Joined,
}

/// One member pane of a channel, as reported by `channel.members` — who would
/// receive a `channel.send`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelMember {
    pub pane_id: String,
    /// The pane's display/agent name, when it hosts a detected agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `None` for panes not hosting a detected agent (plain shells).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<AgentStatus>,
    /// Workspace-implicit or explicitly joined membership.
    pub source: ChannelMemberSource,
}

/// A single line of a channel's append-only JSONL transcript. Reused as both
/// the on-disk storage record (`src/persist/channels.rs`) and the wire type,
/// since the two shapes are identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelMessage {
    /// RFC 3339 timestamp.
    pub ts: String,
    /// Monotonic per-channel sequence id, assigned server-side at append
    /// time. Survives log rotation (`next = last + 1`, never a line count).
    /// `0` marks pre-seq history written before this field existed.
    #[serde(default)]
    pub seq: u64,
    pub from_pane: String,
    pub from_name: String,
    pub text: String,
    /// Seq of the message being replied to, when this was sent as a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<u64>,
    /// Pane id of a targeted recipient; `None` = broadcast to every member
    /// agent pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_pane: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ChannelDeliveryStatus {
    Delivered,
    Deferred,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelDelivery {
    pub pane_id: String,
    pub status: ChannelDeliveryStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}
