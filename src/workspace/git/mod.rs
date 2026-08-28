mod branches;
mod change_set;
mod check_provider;
mod check_status;
mod collectible;
mod config;
#[cfg(test)]
mod config_tests;
mod discovery;
mod issues;
mod open_prs;
mod status;
#[cfg(test)]
pub(super) mod test_support;

// Test-only re-export: production code reads the sentinel through
// `WorkspaceCheckStatus::is_not_applicable`.
#[cfg(test)]
pub(crate) use self::check_status::NOT_APPLICABLE_ERROR;
pub(crate) use self::discovery::automatic_workspace_label;

#[cfg(test)]
pub(crate) use self::check_status::PrSummary;
pub use self::{
    branches::{fetch_local_branches, RepoBranch, RepoBranches},
    change_set::{ChangeSectionKind, ChangeStatus, WorkspaceChangeSet},
    check_status::{
        checks_counts, checks_rollup, fetch_check_status, CheckRun, ChecksRollup,
        WorkspaceCheckStatus,
    },
    discovery::{
        derive_label_from_cwd, fallback_label_from_cwd, git_branch, git_space_metadata,
        GitSpaceMetadata,
    },
    issues::{fetch_my_issues, RepoIssue, RepoIssues},
    open_prs::{fetch_my_open_prs, OpenPr, RepoOpenPrs},
    status::{
        git_status_cache_key, git_status_cache_key_for_space,
        git_status_snapshot_for_cwd_with_demand, GitStatusCacheEntry, GitStatusRefreshDemand,
    },
};

/// Test-support: the sidebar capture fixture names these via
/// `crate::workspace::`; no production consumer exists yet.
#[cfg(test)]
pub use self::change_set::{ChangeSection, ChangedFile};

#[cfg(test)]
pub(super) use self::status::git_ahead_behind;
