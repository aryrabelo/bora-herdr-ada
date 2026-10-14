use serde::{Deserialize, Serialize};

use crate::cli::projection::{cli_params, Field, Input};
use crate::cli::workspace_bindings as bindings;

pub(crate) fn workspace() -> Input<Option<String>> {
    super::tabs::workspace_option("ID").configure(|arg| arg.conflicts_with("cwd"))
}

fn json_compatibility() -> Input<bool> {
    Input::flag("json", |_, _| Ok(())).hidden()
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
    pub struct WorktreeListParams => WorktreeListProjection {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub workspace_id: Option<String> => Field::new(None).input(workspace().order(0)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cwd: Option<String> => Field::new(None).input(crate::cli::worktree::bindings::path("cwd").order(1)),
        #[serde(default, skip_serializing_if = "super::is_false")]
        pub trust_repository: bool => Field::new(false)
            .input(bindings::boolean("trust-repository", true).order(2))
            .input(json_compatibility()),
    }
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
    pub struct WorktreeCreateParams => WorktreeCreateProjection {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub workspace_id: Option<String> => Field::new(None).input(workspace().order(0)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cwd: Option<String> => Field::new(None).input(crate::cli::worktree::bindings::path("cwd").order(1)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub branch: Option<String> => Field::new(None).input(bindings::optional_text("branch", "NAME").order(2)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub base: Option<String> => Field::new(None).input(bindings::optional_text("base", "REF").order(3)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub pr: Option<u64> => Field::new(None).input(crate::cli::worktree::bindings::pr().order(4)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub path: Option<String> => Field::new(None).input(crate::cli::worktree::bindings::path("path").order(5)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub label: Option<String> => Field::new(None).input(bindings::optional_text("label", "TEXT").order(6)),
        #[serde(default)]
        pub focus: bool => Field::new(false)
            .input(bindings::boolean("focus", true).order(7))
            .input(bindings::boolean("no-focus", false).order(8)),
        #[serde(default, skip_serializing_if = "super::is_false")]
        pub trust_repository: bool => Field::new(false)
            .input(bindings::boolean("trust-repository", true).order(9))
            .input(json_compatibility()),
    }
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, Default)]
    pub struct WorktreeOpenParams => WorktreeOpenProjection {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub workspace_id: Option<String> => Field::new(None).input(workspace().order(0)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub cwd: Option<String> => Field::new(None).input(crate::cli::worktree::bindings::path("cwd").order(1)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub path: Option<String> => Field::new(None).input(crate::cli::worktree::bindings::path("path").order(2)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub branch: Option<String> => Field::new(None).input(bindings::optional_text("branch", "NAME").order(3)),
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub label: Option<String> => Field::new(None).input(bindings::optional_text("label", "TEXT").order(4)),
        #[serde(default)]
        pub focus: bool => Field::new(false)
            .input(bindings::boolean("focus", true).order(5))
            .input(bindings::boolean("no-focus", false).order(6)),
        #[serde(default, skip_serializing_if = "super::is_false")]
        pub trust_repository: bool => Field::new(false)
            .input(bindings::boolean("trust-repository", true).order(7))
            .input(json_compatibility()),
    }
}

cli_params! {
    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
    pub struct WorktreeRemoveParams => WorktreeRemoveProjection {
        pub workspace_id: String => Field::new(String::new()).input(Input::value("workspace", "ID", |slot, value| {
            *slot = crate::cli::normalize_workspace_id(value);
            Ok(())
        }).required().order(0)),
        #[serde(default)]
        pub force: bool => Field::new(false).input(bindings::boolean("force", true).order(1)),
        #[serde(default, skip_serializing_if = "super::is_false")]
        pub trust_repository: bool => Field::new(false)
            .input(bindings::boolean("trust-repository", true).order(2))
            .input(json_compatibility()),
    }
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
