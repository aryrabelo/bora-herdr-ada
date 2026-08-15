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

/// One pane in a channel's workspace, as reported by `channel.members` — who
/// would receive a `channel.send`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelMember {
    pub pane_id: String,
    /// The pane's display/agent name, when it hosts a detected agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `None` for panes not hosting a detected agent (plain shells).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_status: Option<AgentStatus>,
}

/// A single line of a channel's append-only JSONL transcript. Reused as both
/// the on-disk storage record (`src/persist/channels.rs`) and the wire type,
/// since the two shapes are identical.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelMessage {
    /// RFC 3339 timestamp.
    pub ts: String,
    pub from_pane: String,
    pub from_name: String,
    pub text: String,
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
