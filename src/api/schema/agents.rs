use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::common::{AgentStatus, ReadFormat, ReadSource};
use crate::cli::agent::bindings;
use crate::cli::projection::{cli_params, Field};

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct AgentReadParams => AgentReadProjection {
        pub target: String => Field::new(String::new()).input(bindings::target()),
        pub source: ReadSource => Field::new(ReadSource::Recent).input(bindings::source()),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub lines: Option<u32> => Field::new(None).input(bindings::lines()),
        #[serde(default)]
        pub format: ReadFormat => Field::new(ReadFormat::Text)
            .input(bindings::format()).input(bindings::ansi()),
        #[serde(default = "super::common::default_true")]
        pub strip_ansi: bool => Field::new(true),
    }
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct AgentSendKeysParams => AgentSendKeysProjection {
        pub target: String => Field::new(String::new()).input(bindings::target()),
        pub keys: Vec<String> => Field::new(Vec::new()).input(bindings::keys()),
    }
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct AgentWaitParams => AgentWaitProjection {
        pub target: String => Field::new(String::new()).input(bindings::target()),
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub until: Vec<AgentStatus> => Field::new(Vec::new()).input(bindings::until()
            .help("State to match; repeat for more than one state")),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub timeout_ms: Option<u64> => Field::new(None).input(bindings::timeout()
            .help("Fail after this many milliseconds")),
    }
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct AgentPromptWaitOptions => AgentPromptWaitProjection {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub until: Vec<AgentStatus> => Field::new(Vec::new()).input(bindings::until()
            .help("State to match after --wait; repeat for more than one state")),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub timeout_ms: Option<u64> => Field::new(None).input(bindings::timeout()
            .help("Fail after this many milliseconds")),
        #[serde(skip)]
        #[schemars(skip)]
        pub(crate) submission_deadline: Option<std::time::Instant> => Field::new(None::<std::time::Instant>),
    }
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct AgentRenameParams => AgentRenameProjection {
        pub target: String => Field::new(String::new()).input(bindings::target()),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub name: Option<String> => Field::new(None)
            .input(bindings::rename_name()).input(bindings::clear_name()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentViewSetParams {
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<AgentViewFilter>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sort: Vec<AgentViewSort>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct AgentViewClearParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum AgentViewFilter {
    All {
        filters: Vec<AgentViewFilter>,
    },
    Any {
        filters: Vec<AgentViewFilter>,
    },
    Not {
        filter: Box<AgentViewFilter>,
    },
    Eq {
        field: AgentViewField,
        value: AgentViewValue,
    },
    In {
        field: AgentViewField,
        values: Vec<AgentViewValue>,
    },
    Exists {
        field: AgentViewField,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum AgentViewField {
    Builtin(AgentViewBuiltinField),
    Token { token: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentViewBuiltinField {
    Status,
    WorkspaceId,
    TabId,
    PaneId,
    Agent,
    Seen,
    StateChangeSeq,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum AgentViewValue {
    String(String),
    Bool(bool),
    Number(u64),
    Context { context: AgentViewContext },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentViewContext {
    CurrentWorkspaceId,
    CurrentTabId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentViewSort {
    pub field: AgentViewSortField,
    #[serde(default)]
    pub order: AgentViewSortOrder,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum AgentViewSortField {
    Builtin(AgentViewBuiltinSortField),
    Token { token: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentViewBuiltinSortField {
    WorkspaceOrder,
    TabOrder,
    PaneOrder,
    Attention,
    Status,
    Agent,
    Seen,
    StateChangeSeq,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentViewSortOrder {
    #[default]
    Asc,
    Desc,
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct AgentStartParams => AgentStartProjection {
        pub name: String => Field::new(String::new()).input(bindings::text_position("name", "NAME", 0)),
        pub kind: String => Field::new(String::new()).input(bindings::kind()),
        pub pane_id: String => Field::new(String::new()).input(bindings::pane()),
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub args: Vec<String> => Field::new(Vec::new()).input(bindings::agent_args()),
        /// Startup timeout in milliseconds. Values must be greater than 3000 and at most 300000.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub timeout_ms: Option<u64> => Field::new(None).input(bindings::timeout()
            .help("Wait for interactive readiness (default: 30000; max: 300000)")),
    }
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct AgentPromptParams => AgentPromptProjection {
        pub target: String => Field::new(String::new()).input(bindings::target()),
        pub text: String => Field::new(String::new()).input(bindings::text_position("text", "TEXT", 1).verbatim()),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub wait: Option<AgentPromptWaitOptions> => Field::new(None).input(bindings::wait_presence()),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub from_pane: Option<String> => Field::new(None)
            .input(bindings::from_pane()).input(bindings::no_from()),
        /// When true, and the target is currently `working`, block server-side until the
        /// target leaves the working state (bounded by `when_idle_timeout_ms`) before
        /// injecting the prompt. Absent/false: byte-identical to always-inject behavior.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub when_idle: Option<bool> => Field::new(None).input(bindings::when_idle()),
        /// Idle-gate timeout in milliseconds; only meaningful with `when_idle: true`.
        /// Defaults to 60000ms when omitted.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub when_idle_timeout_ms: Option<u64> => Field::<Option<u64>>::new(None),
        /// Caller PID from OS-level socket peer credentials, set by the connection accept
        /// loop before dispatch. `#[serde(skip)]`: never accepted from a client, never
        /// echoed back; a client-supplied value in the wire JSON is silently ignored.
        /// Used to verify `from_pane` actually descends from the claimed pane's shell
        /// process instead of trusting the caller-supplied string at face value.
        #[serde(skip)]
        pub peer_pid: Option<u32> => Field::<Option<u32>>::new(None),
        /// Channel name (without leading `#`) when this prompt was sent as a
        /// `channel.send` fan-out delivery, set internally by
        /// `handle_channel_send`. `#[serde(skip)]`: never accepted from or echoed
        /// back to a client. Threaded onto `PendingAgentPrompt` so a later terminal
        /// drop of this queued delivery (cap eviction, pane close, replay failure)
        /// can append an honest system line to that channel's history.
        #[serde(skip)]
        pub origin_channel: Option<String> => Field::<Option<String>>::new(None),
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default,
)]
#[serde(rename_all = "snake_case")]
pub enum AgentPromptOutcome {
    /// The prompt was injected into the target pane immediately.
    #[default]
    Injected,
    /// The target was `working`; the prompt was queued (see the receipt's
    /// `queue_id`) and will be replayed once it leaves `working` (see
    /// `App::drain_pending_agent_prompts`). This outcome only reports that the
    /// prompt was accepted into the queue, not that it was delivered: its
    /// terminal fate (delivered or dropped) is observable via the
    /// `agent_prompt.delivered` / `agent_prompt.dropped` events carrying the
    /// same `queue_id`.
    Deferred,
}

/// Why a queued (`when_idle`-deferred) agent prompt never reached its target.
/// Carried on `EventData::QueuedPromptDropped` so a sender that only sees the
/// `deferred` receipt can still learn the terminal outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueuedAgentPromptDropReason {
    /// The per-target queue was full; this was the oldest entry, evicted to
    /// make room for a newer one.
    Capacity,
    /// The target pane closed before the queue could be drained to it.
    PaneClosed,
    /// Replaying the prompt failed for a reason a retry cannot fix (rate
    /// limited, target pane gone, or the agent hosting it changed).
    AgentChanged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentInfo {
    pub terminal_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_title_stripped: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_agent: Option<String>,
    pub agent_status: AgentStatus,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub screen_detection_skipped: bool,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub state_labels: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    #[schemars(schema_with = "super::common::metadata_token_values_schema")]
    pub tokens: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session: Option<AgentSessionInfo>,
    pub workspace_id: String,
    pub tab_id: String,
    pub pane_id: String,
    pub focused: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub launch_pending: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub interactive_ready: bool,
    #[serde(default)]
    pub state_change_seq: u64,
    /// The current idle transition completed work, independently of who has viewed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completion_seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground_cwd: Option<String>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AgentSessionInfo {
    pub source: String,
    pub agent: String,
    pub kind: crate::agent_resume::AgentSessionRefKind,
    pub value: String,
}
