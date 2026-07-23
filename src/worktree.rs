use std::ffi::OsString;
use std::path::{Path, PathBuf};

const DEFAULT_WORKTREE_PREFIX: &str = "worktree";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExistingWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_prunable: bool,
}

pub(crate) fn generated_branch_slug(seed: u64) -> String {
    let adjectives = [
        "brave", "calm", "clear", "green", "lucky", "quiet", "rapid", "silver",
    ];
    let nouns = [
        "river", "cloud", "field", "forest", "harbor", "meadow", "stone", "valley",
    ];
    let adjective = adjectives[(seed as usize) % adjectives.len()];
    let noun = nouns[((seed / adjectives.len() as u64) as usize) % nouns.len()];
    let suffix = seed & 0xffff;
    format!("{DEFAULT_WORKTREE_PREFIX}/{adjective}-{noun}-{suffix:04x}")
}

pub(crate) fn branch_to_path_slug(branch: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for ch in branch.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }

    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        DEFAULT_WORKTREE_PREFIX.to_string()
    } else {
        trimmed
    }
}

pub(crate) fn expand_tilde_path(path: &str) -> PathBuf {
    expand_tilde_path_from_env(path, cfg!(windows), |key| std::env::var_os(key))
}

fn expand_tilde_path_from_env(
    path: &str,
    is_windows: bool,
    env: impl Fn(&str) -> Option<OsString> + Copy,
) -> PathBuf {
    if path == "~" {
        return home_dir_from_env(is_windows, env).unwrap_or_else(|_| PathBuf::from(path));
    }

    let tilde_rest = path.strip_prefix("~/").or_else(|| {
        if is_windows {
            path.strip_prefix("~\\")
        } else {
            None
        }
    });
    if let Some(rest) = tilde_rest {
        return home_dir_from_env(is_windows, env)
            .map(|home| join_tilde_rest(home, rest, is_windows))
            .unwrap_or_else(|_| PathBuf::from(path));
    }

    PathBuf::from(path)
}

fn join_tilde_rest(home: PathBuf, rest: &str, is_windows: bool) -> PathBuf {
    if is_windows {
        rest.split(['/', '\\'])
            .filter(|component| !component.is_empty())
            .fold(home, |path, component| path.join(component))
    } else {
        home.join(rest)
    }
}

fn home_dir_from_env(
    is_windows: bool,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, ()> {
    if !is_windows {
        return env("HOME").map(PathBuf::from).ok_or(());
    }

    if let Some(path) = usable_home_path(env("USERPROFILE")) {
        return Ok(path);
    }
    if let (Some(drive), Some(path)) = (
        usable_home_component(env("HOMEDRIVE")),
        usable_home_component(env("HOMEPATH")),
    ) {
        let path = path.to_string_lossy();
        if !path.starts_with(['\\', '/']) {
            return usable_home_path(env("HOME")).ok_or(());
        }
        let combined = format!("{}{}", drive.to_string_lossy(), path);
        if let Some(path) = usable_home_path(Some(OsString::from(combined))) {
            return Ok(path);
        }
    }

    usable_home_path(env("HOME")).ok_or(())
}

fn usable_home_path(value: Option<OsString>) -> Option<PathBuf> {
    let value = value?;
    if value.is_empty() || value == "~" {
        return None;
    }
    Some(PathBuf::from(value))
}

fn usable_home_component(value: Option<OsString>) -> Option<OsString> {
    let value = value?;
    if value.is_empty() || value == "~" {
        return None;
    }
    Some(value)
}

pub(crate) fn expand_tilde_absolute_path(path: &str) -> PathBuf {
    let path = expand_tilde_path(path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(&path))
            .unwrap_or(path)
    }
}

pub(crate) fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn default_checkout_path(root: &Path, repo_name: &str, branch: &str) -> PathBuf {
    root.join(repo_name).join(branch_to_path_slug(branch))
}

pub(crate) fn build_worktree_remove_command(
    repo_root: &Path,
    path: &Path,
    force: bool,
) -> WorktreeCommand {
    let mut args = vec![
        "-C".to_string(),
        repo_root.display().to_string(),
        "worktree".to_string(),
        "remove".to_string(),
    ];
    if force {
        args.push("--force".to_string());
    }
    args.push(path.display().to_string());

    WorktreeCommand {
        program: "git".to_string(),
        args,
    }
}

pub(crate) fn is_dirty_worktree_remove_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("contains modified or untracked files")
        && lower.contains("use --force to delete it")
}

pub(crate) fn is_not_working_tree_remove_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("is not a working tree") || lower.contains("is not a worktree")
}

#[cfg(windows)]
pub(crate) fn worktree_dirty_remove_message(path: &Path) -> String {
    format!(
        "fatal: '{}' contains modified or untracked files, use --force to delete it",
        path.display()
    )
}

pub(crate) fn checkout_has_dirty_files(path: &Path) -> Result<bool, String> {
    let path_arg = path.display().to_string();
    let output = crate::noninteractive_process::command("git")
        .args([
            "-C",
            &path_arg,
            "status",
            "--porcelain",
            "--untracked-files=all",
        ])
        .output()
        .map_err(|err| err.to_string())?;

    if output.status.success() {
        return Ok(!output.stdout.is_empty());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stderr.is_empty() {
        Err(stderr)
    } else if !stdout.is_empty() {
        Err(stdout)
    } else {
        Err(format!("git status failed with status {}", output.status))
    }
}

pub(crate) fn build_worktree_add_new_branch_command(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    base: &str,
) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            repo_root.display().to_string(),
            "worktree".to_string(),
            "add".to_string(),
            "-b".to_string(),
            branch.to_string(),
            path.display().to_string(),
            base.to_string(),
        ],
    }
}

pub(crate) fn build_worktree_add_existing_branch_command(
    repo_root: &Path,
    path: &Path,
    branch: &str,
) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            repo_root.display().to_string(),
            "worktree".to_string(),
            "add".to_string(),
            path.display().to_string(),
            branch.to_string(),
        ],
    }
}

pub(crate) fn local_branch_exists(repo_root: &Path, branch: &str) -> Result<bool, String> {
    let output = crate::noninteractive_process::command("git")
        .arg("-C")
        .arg(repo_root)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .output()
        .map_err(|err| err.to_string())?;

    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(1) {
        return Ok(false);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stderr.is_empty() {
        Err(stderr)
    } else if !stdout.is_empty() {
        Err(stdout)
    } else {
        Err(format!("git show-ref failed with status {}", output.status))
    }
}

pub(crate) fn run_worktree_add_command(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    base: &str,
) -> Result<(), String> {
    let command = if local_branch_exists(repo_root, branch)? {
        build_worktree_add_existing_branch_command(repo_root, path, branch)
    } else {
        build_worktree_add_new_branch_command(repo_root, path, branch, base)
    };
    run_worktree_command(&command)
}

/// Head metadata for a pull request, resolved authoritatively via `gh pr view`
/// in the source checkout (never from the UI cache).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PullRequestHead {
    pub head_ref_name: String,
    pub is_cross_repository: bool,
    pub state: String,
}

pub(crate) fn parse_gh_pr_view_json(json_str: &str) -> Result<PullRequestHead, String> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON from gh: {e}"))?;
    let head_ref_name = value
        .get("headRefName")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if head_ref_name.is_empty() {
        return Err("gh pr view returned no head branch".into());
    }
    let is_cross_repository = value
        .get("isCrossRepository")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let state = value
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Ok(PullRequestHead {
        head_ref_name,
        is_cross_repository,
        state,
    })
}

fn resolve_pull_request_head(
    source_checkout: &Path,
    pr_number: u64,
) -> Result<PullRequestHead, String> {
    let output = std::process::Command::new("gh")
        .current_dir(source_checkout)
        .args([
            "pr",
            "view",
            &pr_number.to_string(),
            "--json",
            "number,headRefName,isCrossRepository,state",
        ])
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "gh CLI not found".to_string()
            } else {
                format!("failed to run gh: {err}")
            }
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(
            if stderr.contains("authentication") || stderr.contains("auth") {
                "gh not authenticated".to_string()
            } else if stderr.is_empty() {
                format!("gh pr view {pr_number} failed")
            } else {
                stderr
            },
        );
    }
    let head = parse_gh_pr_view_json(&String::from_utf8_lossy(&output.stdout))?;
    if !head.state.eq_ignore_ascii_case("open") {
        return Err(format!(
            "PR #{pr_number} is {}; only open PRs can be checked out",
            head.state.to_ascii_lowercase()
        ));
    }
    Ok(head)
}

/// `git fetch origin <branch>` so the tracking worktree add below sees an
/// up-to-date `origin/<branch>`.
pub(crate) fn build_pr_branch_fetch_command(
    source_checkout: &Path,
    head_ref_name: &str,
) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            source_checkout.display().to_string(),
            "fetch".to_string(),
            "origin".to_string(),
            head_ref_name.to_string(),
        ],
    }
}

/// Worktree add creating local `branch` from `origin/<branch>` with upstream
/// tracking so a later push from the checkout works without extra setup.
pub(crate) fn build_worktree_add_tracking_branch_command(
    repo_root: &Path,
    path: &Path,
    branch: &str,
) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            repo_root.display().to_string(),
            "worktree".to_string(),
            "add".to_string(),
            "--track".to_string(),
            "-b".to_string(),
            branch.to_string(),
            path.display().to_string(),
            format!("origin/{branch}"),
        ],
    }
}

/// Local branch name used for a fork (cross-repository) PR checkout.
pub(crate) fn pr_fork_branch_name(pr_number: u64) -> String {
    format!("pr/{pr_number}")
}

/// `git fetch origin +refs/pull/<N>/head:pr/<N>`. The `+` force prefix lets a
/// re-run update an existing `pr/<N>` branch left by an earlier checkout even
/// when the fork force-pushed (non-fast-forward) in between.
pub(crate) fn build_pr_fork_fetch_command(
    source_checkout: &Path,
    pr_number: u64,
) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            source_checkout.display().to_string(),
            "fetch".to_string(),
            "origin".to_string(),
            format!(
                "+refs/pull/{pr_number}/head:{}",
                pr_fork_branch_name(pr_number)
            ),
        ],
    }
}

/// Create a linked worktree at `path` checked out to the head of PR
/// `pr_number`, resolving the PR via `gh` in `source_checkout`. Same-repo PRs
/// check out the head branch (with upstream tracking so push works); fork PRs
/// check out a read-only local `pr/<N>` branch fetched from the PR ref.
pub(crate) fn run_worktree_add_for_pull_request(
    source_checkout: &Path,
    path: &Path,
    pr_number: u64,
) -> Result<(), String> {
    let head = resolve_pull_request_head(source_checkout, pr_number)?;
    let local_branch = if head.is_cross_repository {
        pr_fork_branch_name(pr_number)
    } else {
        head.head_ref_name.clone()
    };
    // Opening a PR whose branch is already checked out in a worktree must reuse
    // that worktree instead of failing to add a second one for the same branch.
    // When it is the requested path, treat as done so the caller attaches and
    // focuses the existing checkout; otherwise report where it lives.
    if let Some(existing) = existing_worktree_path_on_branch(source_checkout, &local_branch)? {
        return if canonical_or_original(&existing) == canonical_or_original(path) {
            Ok(())
        } else {
            Err(format!(
                "PR #{pr_number} branch '{local_branch}' is already checked out at {}",
                existing.display()
            ))
        };
    }
    if head.is_cross_repository {
        run_worktree_command(&build_pr_fork_fetch_command(source_checkout, pr_number))?;
        return run_worktree_command(&build_worktree_add_existing_branch_command(
            source_checkout,
            path,
            &local_branch,
        ));
    }
    run_worktree_command(&build_pr_branch_fetch_command(
        source_checkout,
        &head.head_ref_name,
    ))?;
    let command = if local_branch_exists(source_checkout, &head.head_ref_name)? {
        build_worktree_add_existing_branch_command(source_checkout, path, &head.head_ref_name)
    } else {
        build_worktree_add_tracking_branch_command(source_checkout, path, &head.head_ref_name)
    };
    run_worktree_command(&command)
}

/// Path of an existing worktree currently checked out on `branch` in this
/// repo, if any. Makes opening a PR idempotent when its branch already has a
/// worktree.
fn existing_worktree_path_on_branch(
    source_checkout: &Path,
    branch: &str,
) -> Result<Option<PathBuf>, String> {
    Ok(list_existing_worktrees(source_checkout)?
        .into_iter()
        .find(|entry| entry.branch.as_deref() == Some(branch))
        .map(|entry| entry.path))
}

pub(crate) fn build_worktree_merge_command(repo_root: &Path, branch: &str) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            repo_root.display().to_string(),
            "merge".to_string(),
            "--no-ff".to_string(),
            "--no-edit".to_string(),
            branch.to_string(),
        ],
    }
}

/// Abort an in-progress merge in `repo_root` (used to clean up after a conflict).
pub(crate) fn build_worktree_merge_abort_command(repo_root: &Path) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            repo_root.display().to_string(),
            "merge".to_string(),
            "--abort".to_string(),
        ],
    }
}

/// Merge `branch` into the parent/main checkout, refusing if either side is
/// dirty and aborting (no changes applied) on conflict. Shared by the
/// merge-to-main and merge-and-remove flows.
pub(crate) fn merge_branch_to_parent(
    repo_root: &Path,
    checkout_path: &Path,
    branch: &str,
) -> Result<(), String> {
    if checkout_has_dirty_files(checkout_path)? {
        return Err("worktree has uncommitted changes; commit them before merging".into());
    }
    if checkout_has_dirty_files(repo_root)? {
        return Err("base checkout has uncommitted changes; commit or stash them first".into());
    }
    let merge = build_worktree_merge_command(repo_root, branch);
    if let Err(err) = run_worktree_command(&merge) {
        let abort = build_worktree_merge_abort_command(repo_root);
        let _ = run_worktree_command(&abort);
        return Err(format!("merge failed (no changes applied): {err}"));
    }
    Ok(())
}

/// Push `branch` to `origin` from the worktree checkout, setting upstream so a
/// subsequent `gh pr create` has a remote head to open a PR from.
pub(crate) fn build_worktree_push_command(checkout_path: &Path, branch: &str) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            checkout_path.display().to_string(),
            "push".to_string(),
            "-u".to_string(),
            "origin".to_string(),
            branch.to_string(),
        ],
    }
}

/// Push the worktree branch and open a GitHub PR via `gh pr create --fill`,
/// returning the PR URL printed on stdout. `gh` runs inside the checkout so it
/// resolves the repo from the worktree's remote.
pub(crate) fn open_pull_request(checkout_path: &Path, branch: &str) -> Result<String, String> {
    run_worktree_command(&build_worktree_push_command(checkout_path, branch))?;
    let output = std::process::Command::new("gh")
        .current_dir(checkout_path)
        .args(["pr", "create", "--fill"])
        .output()
        .map_err(|err| format!("failed to run gh: {err}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            "gh pr create failed".to_string()
        } else {
            stderr
        })
    }
}

/// A `git -C <checkout_path> <args...>` command.
fn build_git_in_checkout_command(checkout_path: &Path, args: &[&str]) -> WorktreeCommand {
    let mut full = vec!["-C".to_string(), checkout_path.display().to_string()];
    full.extend(args.iter().map(std::string::ToString::to_string));
    WorktreeCommand {
        program: "git".to_string(),
        args: full,
    }
}

/// Sync the checkout's branch with its upstream: fast-forward in remote commits
/// (`pull --ff-only`, never creating merge commits or conflicts), then push any
/// local commits. A diverged branch fails the `--ff-only` pull cleanly without
/// touching the tree.
pub(crate) fn sync_branch_with_upstream(checkout_path: &Path) -> Result<(), String> {
    run_worktree_command(&build_git_in_checkout_command(
        checkout_path,
        &["pull", "--ff-only"],
    ))?;
    run_worktree_command(&build_git_in_checkout_command(checkout_path, &["push"]))
}

pub(crate) fn run_worktree_command(command: &WorktreeCommand) -> Result<(), String> {
    let output = crate::noninteractive_process::command(&command.program)
        .args(&command.args)
        .output()
        .map_err(|err| err.to_string())?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = if stderr.is_empty() { stdout } else { stderr };
    Err(if message.is_empty() {
        format!("{} failed with status {}", command.program, output.status)
    } else {
        message
    })
}

pub(crate) fn run_worktree_remove_command_with_recovery(
    command: &WorktreeCommand,
    repo_root: &Path,
    path: &Path,
    force: bool,
) -> Result<(), String> {
    match run_worktree_command(command) {
        Ok(()) => Ok(()),
        Err(err) if force && is_not_working_tree_remove_error(&err) => {
            if worktree_list_contains_path(repo_root, path)? {
                return Err(err);
            }
            if path.exists() {
                if !leftover_worktree_checkout_matches_repo(repo_root, path) {
                    return Err(err);
                }
                std::fs::remove_dir_all(path).map_err(|remove_err| {
                    format!(
                        "{err}; failed to remove leftover checkout {}: {remove_err}",
                        path.display()
                    )
                })?;
            }
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn leftover_worktree_checkout_matches_repo(repo_root: &Path, path: &Path) -> bool {
    let git_file = path.join(".git");
    let Ok(content) = std::fs::read_to_string(&git_file) else {
        return false;
    };
    let Some(gitdir) = content.trim().strip_prefix("gitdir:") else {
        return false;
    };
    let gitdir = PathBuf::from(gitdir.trim());
    let gitdir = if gitdir.is_absolute() {
        gitdir
    } else {
        path.join(gitdir)
    };
    let Some(worktrees_dir) = git_common_worktrees_dir(repo_root) else {
        return false;
    };
    canonical_or_original(&gitdir).starts_with(canonical_or_original(&worktrees_dir))
}

fn git_common_worktrees_dir(repo_root: &Path) -> Option<PathBuf> {
    let output = crate::noninteractive_process::command("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "--git-common-dir"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let common_dir = stdout.trim();
    if common_dir.is_empty() {
        None
    } else {
        let common_dir = PathBuf::from(common_dir);
        let common_dir = if common_dir.is_absolute() {
            common_dir
        } else {
            repo_root.join(common_dir)
        };
        Some(common_dir.join("worktrees"))
    }
}

pub(crate) fn parse_worktree_list_porcelain(output: &str) -> Vec<ExistingWorktree> {
    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch = None;
    let mut is_bare = false;
    let mut is_detached = false;
    let mut is_prunable = false;

    let finish = |entries: &mut Vec<ExistingWorktree>,
                  path: &mut Option<PathBuf>,
                  branch: &mut Option<String>,
                  is_bare: &mut bool,
                  is_detached: &mut bool,
                  is_prunable: &mut bool| {
        if let Some(path) = path.take() {
            entries.push(ExistingWorktree {
                path,
                branch: branch.take(),
                is_bare: *is_bare,
                is_detached: *is_detached,
                is_prunable: *is_prunable,
            });
        }
        *is_bare = false;
        *is_detached = false;
        *is_prunable = false;
    };

    for line in output.lines() {
        if line.trim().is_empty() {
            finish(
                &mut entries,
                &mut path,
                &mut branch,
                &mut is_bare,
                &mut is_detached,
                &mut is_prunable,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(
                value
                    .strip_prefix("refs/heads/")
                    .unwrap_or(value)
                    .to_string(),
            );
        } else if line == "detached" {
            is_detached = true;
        } else if line == "bare" {
            is_bare = true;
        } else if line.starts_with("prunable") {
            is_prunable = true;
        }
    }

    finish(
        &mut entries,
        &mut path,
        &mut branch,
        &mut is_bare,
        &mut is_detached,
        &mut is_prunable,
    );
    entries
}

pub(crate) fn list_existing_worktrees(repo_root: &Path) -> Result<Vec<ExistingWorktree>, String> {
    let output = crate::noninteractive_process::command("git")
        .arg("-C")
        .arg(repo_root)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .map_err(|err| err.to_string())?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Ok(parse_worktree_list_porcelain(&stdout));
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(if stderr.is_empty() {
        format!("git worktree list failed with status {}", output.status)
    } else {
        stderr
    })
}

pub(crate) fn worktree_list_contains_path(repo_root: &Path, path: &Path) -> Result<bool, String> {
    let expected = canonical_or_original(path);
    Ok(list_existing_worktrees(repo_root)?
        .into_iter()
        .any(|entry| canonical_or_original(&entry.path) == expected))
}

/// Copy files listed in `.worktreeinclude` from the source repo root to a new
/// worktree checkout. Each line is a relative path; missing source files are
/// silently skipped. Parent directories are created as needed.
pub(crate) fn copy_worktree_includes(repo_root: &Path, checkout_path: &Path) {
    let include_path = repo_root.join(".worktreeinclude");
    let content = match std::fs::read_to_string(&include_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let src = repo_root.join(line);
        let dst = checkout_path.join(line);
        if !src.is_file() {
            tracing::debug!(path = %src.display(), "worktreeinclude: source file not found, skipping");
            continue;
        }
        if let Some(parent) = dst.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                tracing::warn!(path = %parent.display(), "worktreeinclude: failed to create parent dir: {err}");
                continue;
            }
        }
        if let Err(err) = std::fs::copy(&src, &dst) {
            tracing::warn!(src = %src.display(), dst = %dst.display(), "worktreeinclude: copy failed: {err}");
        } else {
            tracing::info!(file = line, "worktreeinclude: copied to new worktree");
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("herdr-{name}-{}-{nanos}", std::process::id()))
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .status()
            .unwrap();
        assert!(
            status.success(),
            "git command failed: git -C {} {}",
            repo.display(),
            args.join(" ")
        );
    }

    fn create_committed_repo(name: &str) -> PathBuf {
        let repo = unique_temp_path(name);
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "--quiet"]);
        run_git(&repo, &["config", "user.email", "herdr@example.invalid"]);
        run_git(&repo, &["config", "user.name", "Herdr Test"]);
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "--quiet", "-m", "initial"]);
        repo
    }

    #[test]
    fn generated_branch_slug_is_worktree_namespaced_and_stable() {
        assert_eq!(generated_branch_slug(0), "worktree/brave-river-0000");
        assert_eq!(generated_branch_slug(9), "worktree/calm-cloud-0009");
    }

    #[test]
    fn parses_git_worktree_list_porcelain() {
        let output = "\
worktree /repo/main
HEAD abc
branch refs/heads/main

worktree /repo/issue
HEAD def
branch refs/heads/worktree/issue

worktree /repo/detached
HEAD fed
detached
prunable stale

";

        assert_eq!(
            parse_worktree_list_porcelain(output),
            vec![
                ExistingWorktree {
                    path: PathBuf::from("/repo/main"),
                    branch: Some("main".into()),
                    is_bare: false,
                    is_detached: false,
                    is_prunable: false,
                },
                ExistingWorktree {
                    path: PathBuf::from("/repo/issue"),
                    branch: Some("worktree/issue".into()),
                    is_bare: false,
                    is_detached: false,
                    is_prunable: false,
                },
                ExistingWorktree {
                    path: PathBuf::from("/repo/detached"),
                    branch: None,
                    is_bare: false,
                    is_detached: true,
                    is_prunable: true,
                },
            ]
        );
    }

    #[test]
    fn branch_to_path_slug_makes_branch_safe_folder_name() {
        assert_eq!(
            branch_to_path_slug("worktree/brave-river"),
            "worktree-brave-river"
        );
        assert_eq!(
            branch_to_path_slug("issue/137 Worktree Spaces"),
            "issue-137-worktree-spaces"
        );
        assert_eq!(branch_to_path_slug("///"), "worktree");
    }

    #[test]
    fn expand_tilde_path_uses_home_when_available() {
        assert_eq!(
            expand_tilde_path_from_env("~/.herdr/worktrees", false, |key| match key {
                "HOME" => Some("/home/me".into()),
                _ => None,
            }),
            PathBuf::from("/home/me/.herdr/worktrees")
        );
        assert_eq!(
            expand_tilde_path_from_env("/tmp/worktrees", false, |_| None),
            PathBuf::from("/tmp/worktrees")
        );
    }

    #[test]
    fn home_dir_uses_windows_profile_before_literal_home() {
        assert_eq!(
            home_dir_from_env(true, |key| match key {
                "HOME" => Some("~".into()),
                "USERPROFILE" => Some(r"C:\Users\herdr".into()),
                _ => None,
            }),
            Ok(PathBuf::from(r"C:\Users\herdr"))
        );
    }

    #[test]
    fn home_dir_uses_windows_drive_and_path_when_profile_is_missing() {
        assert_eq!(
            home_dir_from_env(true, |key| match key {
                "HOMEDRIVE" => Some("C:".into()),
                "HOMEPATH" => Some(r"\Users\herdr".into()),
                _ => None,
            }),
            Ok(PathBuf::from(r"C:\Users\herdr"))
        );
    }

    #[test]
    fn home_dir_rejects_incomplete_windows_drive_and_path() {
        assert_eq!(
            home_dir_from_env(true, |key| match key {
                "HOMEDRIVE" => Some("C:".into()),
                "HOMEPATH" => Some("".into()),
                _ => None,
            }),
            Err(())
        );
        assert_eq!(
            home_dir_from_env(true, |key| match key {
                "HOMEDRIVE" => Some("C:".into()),
                "HOMEPATH" => Some("Users\\herdr".into()),
                _ => None,
            }),
            Err(())
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_tilde_expansion_keeps_windows_separator_literal() {
        assert_eq!(
            expand_tilde_path_from_env(r"~\.herdr\worktrees", false, |key| match key {
                "HOME" => Some("/home/me".into()),
                _ => None,
            }),
            PathBuf::from(r"~\.herdr\worktrees")
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_tilde_expansion_normalizes_separators() {
        fn env(key: &str) -> Option<OsString> {
            match key {
                "HOME" => Some("~".into()),
                "USERPROFILE" => Some(r"C:\Users\herdr".into()),
                _ => None,
            }
        }

        let default_path = expand_tilde_path_from_env("~/.herdr/worktrees", true, env);
        assert_eq!(
            default_path,
            PathBuf::from(r"C:\Users\herdr\.herdr\worktrees")
        );
        assert_eq!(
            default_path.display().to_string(),
            r"C:\Users\herdr\.herdr\worktrees"
        );
        assert_eq!(
            expand_tilde_path_from_env(r"~\.herdr\worktrees", true, env),
            PathBuf::from(r"C:\Users\herdr\.herdr\worktrees")
        );
    }

    #[test]
    fn default_checkout_path_appends_repo_and_branch_slug() {
        assert_eq!(
            default_checkout_path(
                Path::new("/home/me/.herdr/worktrees"),
                "herdr",
                "worktree/brave-river",
            ),
            PathBuf::from("/home/me/.herdr/worktrees/herdr/worktree-brave-river")
        );
    }

    #[test]
    fn checkout_dirty_detection_reports_clean_and_dirty_worktrees() {
        let repo = create_committed_repo("worktree-dirty-detection-repo");
        let checkout = unique_temp_path("worktree-dirty-detection-checkout");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                "worktree/dirty-detection",
                checkout.to_str().unwrap(),
                "HEAD",
            ],
        );

        assert_eq!(checkout_has_dirty_files(&checkout), Ok(false));
        std::fs::write(checkout.join("README.md"), "dirty\n").unwrap();
        assert_eq!(checkout_has_dirty_files(&checkout), Ok(true));

        let remove = build_worktree_remove_command(&repo, &checkout, true);
        run_worktree_command(&remove).unwrap();
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn worktree_remove_command_preserves_branch_by_not_deleting_it() {
        let command = build_worktree_remove_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/issue-137"),
            false,
        );
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "remove",
                "/w/herdr/issue-137"
            ]
        );
    }

    #[test]
    fn forced_worktree_remove_command_uses_git_force_flag() {
        let command = build_worktree_remove_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/issue-137"),
            true,
        );
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "remove",
                "--force",
                "/w/herdr/issue-137"
            ]
        );
    }

    #[test]
    fn dirty_remove_error_detection_matches_git_force_hint() {
        assert!(is_dirty_worktree_remove_error(
            "fatal: '/w/herdr' contains modified or untracked files, use --force to delete it"
        ));
        assert!(!is_dirty_worktree_remove_error(
            "fatal: '/w/herdr' is a missing but already registered worktree"
        ));
        assert!(!is_dirty_worktree_remove_error(
            "fatal: '/w/herdr' contains a locked worktree, use --force only if you know why"
        ));
    }

    #[test]
    fn worktree_add_command_creates_new_branch_from_base() {
        let command = build_worktree_add_new_branch_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/worktree-brave-river"),
            "worktree/brave-river",
            "HEAD",
        );
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "add",
                "-b",
                "worktree/brave-river",
                "/w/herdr/worktree-brave-river",
                "HEAD"
            ]
        );
    }

    #[test]
    fn worktree_add_command_checks_out_existing_branch() {
        let command = build_worktree_add_existing_branch_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/worktree-brave-river"),
            "worktree/brave-river",
        );
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "add",
                "/w/herdr/worktree-brave-river",
                "worktree/brave-river"
            ]
        );
    }

    #[test]
    fn pr_branch_fetch_command_fetches_head_from_origin() {
        let command = build_pr_branch_fetch_command(Path::new("/repo/herdr"), "feature/pr-head");
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec!["-C", "/repo/herdr", "fetch", "origin", "feature/pr-head"]
        );
    }

    #[test]
    fn worktree_add_tracking_branch_command_tracks_origin_branch() {
        let command = build_worktree_add_tracking_branch_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/pr-42"),
            "feature/pr-head",
        );
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "add",
                "--track",
                "-b",
                "feature/pr-head",
                "/w/herdr/pr-42",
                "origin/feature/pr-head"
            ]
        );
    }

    #[test]
    fn pr_fork_fetch_command_force_fetches_pull_ref_into_local_branch() {
        let command = build_pr_fork_fetch_command(Path::new("/repo/herdr"), 42);
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "fetch",
                "origin",
                "+refs/pull/42/head:pr/42"
            ]
        );
        assert_eq!(pr_fork_branch_name(42), "pr/42");
    }

    #[test]
    fn parse_gh_pr_view_json_reads_head_metadata() {
        let head = parse_gh_pr_view_json(
            r#"{"number":42,"headRefName":"feature/pr-head","isCrossRepository":false,"state":"OPEN"}"#,
        )
        .unwrap();
        assert_eq!(
            head,
            PullRequestHead {
                head_ref_name: "feature/pr-head".into(),
                is_cross_repository: false,
                state: "OPEN".into(),
            }
        );

        let fork = parse_gh_pr_view_json(
            r#"{"number":7,"headRefName":"fork-branch","isCrossRepository":true,"state":"MERGED"}"#,
        )
        .unwrap();
        assert!(fork.is_cross_repository);
        assert_eq!(fork.state, "MERGED");
    }

    #[test]
    fn parse_gh_pr_view_json_rejects_missing_head_and_invalid_json() {
        let missing = parse_gh_pr_view_json(r#"{"number":42,"state":"OPEN"}"#);
        assert!(missing.unwrap_err().contains("no head branch"));

        let invalid = parse_gh_pr_view_json("not json");
        assert!(invalid.unwrap_err().contains("invalid JSON"));
    }

    #[test]
    fn run_worktree_add_and_remove_create_and_delete_checkout() {
        let repo = create_committed_repo("worktree-run-repo");
        let checkout = unique_temp_path("worktree-run-checkout");
        let branch = "worktree/test-create-remove";

        let add = build_worktree_add_new_branch_command(&repo, &checkout, branch, "HEAD");
        run_worktree_command(&add).unwrap();

        assert!(checkout.join("README.md").exists());
        let branch_name = std::process::Command::new("git")
            .arg("-C")
            .arg(&checkout)
            .args(["branch", "--show-current"])
            .output()
            .unwrap();
        assert!(branch_name.status.success());
        assert_eq!(
            String::from_utf8(branch_name.stdout).unwrap().trim(),
            branch
        );

        let remove = build_worktree_remove_command(&repo, &checkout, false);
        run_worktree_command(&remove).unwrap();
        assert!(!checkout.exists());

        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn existing_worktree_path_on_branch_finds_checked_out_branch() {
        let repo = create_committed_repo("worktree-branch-lookup-repo");
        let checkout = unique_temp_path("worktree-branch-lookup-checkout");
        let branch = "flow/lookup-branch";

        let add = build_worktree_add_new_branch_command(&repo, &checkout, branch, "HEAD");
        run_worktree_command(&add).unwrap();

        let found = existing_worktree_path_on_branch(&repo, branch)
            .unwrap()
            .expect("worktree on branch should be found");
        assert_eq!(
            canonical_or_original(&found),
            canonical_or_original(&checkout)
        );
        assert!(existing_worktree_path_on_branch(&repo, "no/such-branch")
            .unwrap()
            .is_none());

        let remove = build_worktree_remove_command(&repo, &checkout, false);
        run_worktree_command(&remove).unwrap();
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn forced_worktree_remove_recovers_leftover_unregistered_checkout() {
        let repo = create_committed_repo("worktree-recovery-repo");
        let checkout = unique_temp_path("worktree-recovery-checkout");
        let branch = "worktree/recovery";

        let add = build_worktree_add_new_branch_command(&repo, &checkout, branch, "HEAD");
        run_worktree_command(&add).unwrap();
        let remove = build_worktree_remove_command(&repo, &checkout, true);
        run_worktree_command(&remove).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();
        let stale_admin_dir = git_common_worktrees_dir(&repo).unwrap().join("stale");
        std::fs::write(
            checkout.join(".git"),
            format!("gitdir: {}\n", stale_admin_dir.display()),
        )
        .unwrap();
        std::fs::write(checkout.join("leftover"), "leftover\n").unwrap();

        run_worktree_remove_command_with_recovery(&remove, &repo, &checkout, true).unwrap();

        assert!(!checkout.exists());
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn forced_worktree_remove_recovery_keeps_unrelated_replacement_directory() {
        let repo = create_committed_repo("worktree-recovery-unrelated-repo");
        let checkout = unique_temp_path("worktree-recovery-unrelated-checkout");
        let branch = "worktree/recovery-unrelated";

        let add = build_worktree_add_new_branch_command(&repo, &checkout, branch, "HEAD");
        run_worktree_command(&add).unwrap();
        let remove = build_worktree_remove_command(&repo, &checkout, true);
        run_worktree_command(&remove).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join("unrelated"), "do not delete\n").unwrap();

        let err = run_worktree_remove_command_with_recovery(&remove, &repo, &checkout, true)
            .expect_err("unrelated replacement directory should not be removed");

        assert!(is_not_working_tree_remove_error(&err));
        assert!(checkout.join("unrelated").exists());
        let _ = std::fs::remove_dir_all(checkout);
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn copy_worktree_includes_copies_listed_files() {
        let src = unique_temp_path("worktree-include-src");
        let dst = unique_temp_path("worktree-include-dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();

        // Create .worktreeinclude and source files.
        std::fs::write(
            src.join(".worktreeinclude"),
            ".env\nsub/config.toml\nmissing\n",
        )
        .unwrap();
        std::fs::write(src.join(".env"), "SECRET=42\n").unwrap();
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub/config.toml"), "[app]\nport = 3000\n").unwrap();

        copy_worktree_includes(&src, &dst);

        assert_eq!(
            std::fs::read_to_string(dst.join(".env")).unwrap(),
            "SECRET=42\n"
        );
        assert_eq!(
            std::fs::read_to_string(dst.join("sub/config.toml")).unwrap(),
            "[app]\nport = 3000\n"
        );
        // Missing source file is silently skipped.
        assert!(!dst.join("missing").exists());

        let _ = std::fs::remove_dir_all(src);
        let _ = std::fs::remove_dir_all(dst);
    }
}
