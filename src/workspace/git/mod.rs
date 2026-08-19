mod change_set;
mod check_status;
mod config;
#[cfg(test)]
mod config_tests;
mod discovery;
mod status;
#[cfg(test)]
mod test_support;

#[cfg(test)]
pub(crate) use self::check_status::PrSummary;
pub use self::{
    change_set::{ChangeSectionKind, ChangeStatus, WorkspaceChangeSet},
    check_status::{fetch_check_status, WorkspaceCheckStatus},
    discovery::{derive_label_from_cwd, git_branch, git_space_metadata, GitSpaceMetadata},
    status::{git_status_cache_key, git_status_snapshot_for_cwd, GitStatusCacheEntry},
};

#[cfg(test)]
pub(super) use self::status::git_ahead_behind;
