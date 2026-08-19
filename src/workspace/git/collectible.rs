//! "Collectible" status for a linked worktree: `HEAD` fully merged into the
//! repo's default branch, with nothing left to lose. The sidebar renders this
//! as a discreet marker meaning "safe to close".
//!
//! `merge-base --is-ancestor` is the one call here worth caching — it walks
//! commit history and gets slower as a repo ages. `git rev-parse` is a plain
//! ref lookup and cheap enough to run every refresh; callers gate the
//! expensive call by comparing against the last (head, default) pair.

use std::path::Path;

use super::discovery::{git_rev_parse_verify, git_trimmed_stdout};

/// Repo's default-branch commit, resolved via `origin/HEAD` when configured,
/// falling back to a local `main` or `master`. Works from any worktree
/// checkout of the repo: `refs/remotes/*` and `refs/heads/*` live in the
/// shared common git dir, not the per-worktree one.
fn default_branch_oid(repo_root: &Path) -> Option<String> {
    if let Some(short) = git_trimmed_stdout(
        repo_root,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    ) {
        if let Some(oid) = git_rev_parse_verify(repo_root, &short) {
            return Some(oid);
        }
    }
    ["main", "master"]
        .into_iter()
        .find_map(|candidate| git_rev_parse_verify(repo_root, candidate))
}

fn is_ancestor(repo_root: &Path, head_oid: &str, default_oid: &str) -> bool {
    crate::noninteractive_process::command("git")
        .arg("-C")
        .arg(repo_root)
        .args(["merge-base", "--is-ancestor", head_oid, default_oid])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// `(head_sha, default_sha, head_is_ancestor_of_default)` for `repo_root` at
/// `head_oid`. `prev` is the last computed triple; the ancestor check is
/// skipped and `prev`'s result reused when neither sha moved. Returns `None`
/// when the repo has no resolvable default branch (no `origin/HEAD`, no
/// local `main`/`master`).
pub(super) fn compute(
    repo_root: &Path,
    head_oid: &str,
    prev: Option<(&str, &str, bool)>,
) -> Option<(String, String, bool)> {
    let default_oid = default_branch_oid(repo_root)?;
    let ancestor = match prev {
        Some((prev_head, prev_default, prev_ancestor))
            if prev_head == head_oid && prev_default == default_oid =>
        {
            prev_ancestor
        }
        _ => is_ancestor(repo_root, head_oid, &default_oid),
    };
    Some((head_oid.to_string(), default_oid, ancestor))
}
