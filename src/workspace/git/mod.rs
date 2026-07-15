mod branches;
mod change_set;
mod check_status;
mod config;
#[cfg(test)]
mod config_tests;
mod discovery;
mod issues;
mod open_prs;
mod status;
#[cfg(test)]
mod test_support;

#[cfg(test)]
pub(crate) use self::check_status::PrSummary;
pub use self::{
    branches::{fetch_local_branches, RepoBranch, RepoBranches},
    change_set::{ChangeSectionKind, ChangeStatus, WorkspaceChangeSet},
    check_status::{
        checks_rollup, fetch_check_status, CheckRun, ChecksRollup, WorkspaceCheckStatus,
    },
    discovery::{derive_label_from_cwd, git_branch, git_space_metadata, GitSpaceMetadata},
    issues::{fetch_my_issues, RepoIssue, RepoIssues},
    open_prs::{fetch_my_open_prs, OpenPr, RepoOpenPrs},
    status::{git_status_cache_key, git_status_snapshot_for_cwd, GitStatusCacheEntry},
};

#[cfg(test)]
pub(super) use self::status::git_ahead_behind;
