//! Wire shapes for the `todo.*` verbs (bora-s3y.2) over the
//! `persist::todos` append-only log (bora-s3y.1): params, plus wire mirrors
//! of the store's record types. The mirrors exist because the store types
//! deliberately stay persist-layer (no `schemars` derive); the conversion
//! is field-for-field, so the wire never drifts from what the log holds.

use serde::{Deserialize, Serialize};

/// `todo.create`: append a todo to the project's shared log. `origin`
/// records where the todo came from (e.g. `beads:bora-s3y`, `mcp`,
/// `channel:#eng`) — free text, interpreted by writers, never the store.
/// `blockers` are ids of other todos that must reach `done` before this
/// one is actionable; every id must name a live todo at create time (the
/// verb layer checks — the store deliberately does not).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TodoCreateParams {
    pub project: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blockers: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    pub origin: String,
}

/// `todo.complete`: flip one todo to `done`. Completing an already-done
/// todo is a clean no-op (no new append, no event), never an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TodoCompleteParams {
    pub project: String,
    pub id: u64,
}

/// `todo.list`: every live todo in the project, or only the actionable
/// ones (`actionable: true`) — open todos whose blockers are all done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TodoListParams {
    pub project: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actionable: Option<bool>,
}

/// Wire mirror of `persist::todos::TodoState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum TodoStateInfo {
    Open,
    Done,
}

impl From<crate::persist::todos::TodoState> for TodoStateInfo {
    fn from(state: crate::persist::todos::TodoState) -> Self {
        match state {
            crate::persist::todos::TodoState::Open => Self::Open,
            crate::persist::todos::TodoState::Done => Self::Done,
        }
    }
}

/// Wire mirror of `persist::todos::Todo`: the five contract fields plus
/// the store's `id`/`seq`. `seq` is the follower's replay cursor
/// (`persist::todos::read_since`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct TodoInfo {
    pub id: u64,
    pub seq: u64,
    pub title: String,
    pub state: TodoStateInfo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    pub origin: String,
}

impl From<crate::persist::todos::Todo> for TodoInfo {
    fn from(todo: crate::persist::todos::Todo) -> Self {
        Self {
            id: todo.id,
            seq: todo.seq,
            title: todo.title,
            state: todo.state.into(),
            blockers: todo.blockers,
            assignee: todo.assignee,
            origin: todo.origin,
        }
    }
}
