use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::common::AgentStatus;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelCreateParams {
    pub name: String,
}

/// `channel.open`: focus a channel's own workspace and repair its
/// two-pane shape (a `channel tail --follow` transcript pane plus a plain
/// interactive shell pane) if either half is missing. Idempotent: a
/// channel that already has both is untouched. The only path that fixes a
/// channel workspace created before the two-pane shape shipped, since
/// nothing else ever re-checks an existing channel's panes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelOpenParams {
    pub name: String,
}

/// `channel.list`: every `#channel` workspace's [`ChannelSummary`]. Unread
/// is per-caller like `channel.history`/`channel.wait` — see
/// [`ChannelHistoryParams::from_pane`]. The CLI defaults `from_pane` to
/// `$HERDR_PANE_ID`; a client with no pane identity (a human shell, the TUI
/// chat view) gets `unread: 0` on every summary, never a room's total
/// message count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_pane: Option<String>,
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
    /// Delivery gate for the agent-member fan-out. Absent/false (the
    /// default): the message is injected into the recipient pane
    /// immediately, even mid-turn — steering semantics, the same default
    /// as `agent prompt`. `true` (`channel send --when-idle`): a Working
    /// target has the message queued instead (deferred receipt) until its
    /// next observed idle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when_idle: Option<bool>,
    /// Trust anchor, exactly like `peer_pid` on agent prompts: never part
    /// of the wire shape (`#[serde(skip)]` drops it during deserialize), set
    /// only in-process by the TUI chat send path. A socket client cannot
    /// claim the human seat.
    #[serde(skip)]
    pub from_human: bool,
}

/// `channel.note`: append-only record with ZERO injection — the cheapest
/// verb, for facts nobody needs to be woken for. Same attribution and
/// per-(sender,channel) rate limit as `channel.send`, but no addressing (no
/// `to`, no leading-mention parsing) and never subject to the burst damper
/// — there is no bell to suppress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelNoteParams {
    pub name: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_pane: Option<String>,
}

/// `channel.ask`: exactly one bell, and blocks the asker until the
/// addressee replies. `to` is mandatory and resolved exactly like a
/// structured `channel.send` — loud `channel_nick_unknown` /
/// `channel_nick_ambiguous` errors before anything is appended. The
/// question always pierces the burst damper. The reply is correlated by
/// `seq`: the server blocks for the first channel message whose
/// `in_reply_to` equals the question's assigned seq (answered via
/// `bora channel send <name> <text> --reply-to SEQ`), bounded by
/// `timeout_ms` (default 300_000, capped at 600_000). A timeout is a clean
/// `answered: false` result, never an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelAskParams {
    pub name: String,
    pub to: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Who authored a channel message: a member agent pane, or the human at the
/// TUI. Lines written before this field existed parse as `Agent`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ChannelSenderKind {
    #[default]
    Agent,
    /// The human at the TUI chat view. `from_pane` is empty for these lines;
    /// `from_name` is the effective chat name (`ui.chat_name`).
    Human,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelHistoryParams {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<u32>,
    /// Calling pane, for read-cursor tracking: when it resolves to a
    /// channel member, this read advances that member's cursor to the
    /// highest seq among the returned messages. The CLI defaults it to
    /// `$HERDR_PANE_ID`. `None` (no pane identity, e.g. a human shell) or a
    /// pane that isn't currently a member reads freely and advances no
    /// cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_pane: Option<String>,
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
    /// Directories `pane` may write in this channel. Write implies read —
    /// do not also list a write dir under `scope_read`. `Some` (even
    /// `Some(vec![])` combined with an empty `scope_read`) replaces any
    /// prior scope entry for `pane` wholesale; `None` on both leaves an
    /// existing entry untouched. See CANAL-ESCOPO.md Shape 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_write: Option<Vec<String>>,
    /// Directories `pane` may read only, beyond its write dirs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope_read: Option<Vec<String>>,
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
    /// Calling pane, for read-cursor tracking — see
    /// [`ChannelHistoryParams::from_pane`]. The CLI defaults it to
    /// `$HERDR_PANE_ID`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_pane: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelSummary {
    /// Channel name including the leading `#`.
    pub name: String,
    pub pane_count: usize,
    pub agent_count: usize,
    /// Monotonic seq of the channel's most recent message (`ChannelMessage`'s
    /// `seq`); `0` when the channel has never been messaged.
    pub last_message_seq: u64,
    /// RFC 3339 timestamp of the most recent message. `None` exactly when
    /// `last_message_seq` is `0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_message_ts: Option<String>,
    /// Messages this summary counts as unread, for the caller identified by
    /// `channel.list`'s `from_pane` (`channel.create`/`channel.open` pass no
    /// pane identity, so they always report `0` here). A resolved member
    /// pane nets its stored cursor against `last_message_seq`, same rule as
    /// `channel.members`' per-member `unread`; a caller with no pane
    /// identity, an unresolvable pane, or a pane that isn't a member of
    /// this channel sees `0` — never the room's full message count.
    pub unread: u64,
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
    /// Messages in the channel this member's stored read cursor has not
    /// reached yet: `last_message_seq - cursor`, floored at zero. A member
    /// with no stored cursor (never read via `channel tail` /
    /// `channel history`) sees the channel's full message count.
    pub unread: u64,
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
    /// Author kind: a member agent pane, or the human at the TUI.
    #[serde(default)]
    pub from_kind: ChannelSenderKind,
    pub text: String,
    /// Seq of the message being replied to, when this was sent as a reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub in_reply_to: Option<u64>,
    /// Pane id of a targeted recipient; `None` = broadcast to every member
    /// agent pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_pane: Option<String>,
    /// Addressed to the human seat rather than a pane: appended to the
    /// transcript, delivered to no pane.
    #[serde(default)]
    pub to_human: bool,
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
