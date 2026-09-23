use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct WorktreeListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub trust_repository: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct WorktreeCreateParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub focus: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub trust_repository: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct WorktreeOpenParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default)]
    pub focus: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub trust_repository: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorktreeRemoveParams {
    pub workspace_id: String,
    #[serde(default)]
    pub force: bool,
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub trust_repository: bool,
}

/// Where the branch a `worktree.create` landed on came from. Reported so a
/// caller opening an existing PR branch can tell "you are on the remote tip"
/// apart from "I branched you off the trunk" — the two look identical in the
/// response otherwise, and only the second one silently rewrites the PR on the
/// next push.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WorktreeBranchSource {
    /// A local branch of that name already existed and was checked out as-is.
    Local,
    /// The local branch was created at `origin/<branch>` and tracks it.
    Remote,
    /// The branch is new, created from the base ref.
    New,
    /// Nothing was created: a worktree for this branch was already there.
    Existing,
}

impl From<crate::worktree::BranchSource> for WorktreeBranchSource {
    fn from(source: crate::worktree::BranchSource) -> Self {
        match source {
            crate::worktree::BranchSource::Local => Self::Local,
            crate::worktree::BranchSource::Remote => Self::Remote,
            crate::worktree::BranchSource::New => Self::New,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorktreeSourceInfo {
    pub repo_key: String,
    pub repo_name: String,
    pub repo_root: String,
    pub source_checkout_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_workspace_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct WorktreeInfo {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_prunable: bool,
    pub is_linked_worktree: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_workspace_id: Option<String>,
    pub label: String,
}
