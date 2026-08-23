//! The Project view's entry model: `ViewMode::Project`.
//!
//! Three levels and only three — project → worktree → workspace — plus the
//! section bands that hang off a worktree and the per-pane child rows of a
//! multi-pane workspace.
//!
//! This module is deliberately pure: it reads `AppState` (including the
//! already-refreshed `app.projects`) and returns entries. It performs no
//! filesystem I/O, spawns no process, and takes no lock, because
//! `workspace_list_entries` runs on a multiplicative path — per render, per
//! pane, per attached client (AGENTS.md, "Multiplicative performance paths").
//! The worktree inventory and any check/command data must arrive on
//! `AppState` from the tick, never be fetched from here.
//!
//! One deliberate exception: matching a workspace against a project's
//! declared `members:` calls `Member::resolve()`, which does touch disk (see
//! `workspace_matches_member`'s doc comment) — the set of declared members is
//! config-driven and small, bounded by `projects.yml`, never by the
//! (potentially large, per-render-multiplied) workspace list. Grouping the
//! matched workspaces into worktrees below that uses only already-cached
//! `Workspace` fields (`git_space()`, `identity_cwd`), so nothing here scales
//! with render/pane/client count.
//!
//! The Flat and Repo views are untouched: nothing in this module runs unless
//! the mode is `Project`, and the five variants it emits are produced nowhere
//! else.

use std::collections::{HashMap, HashSet};

use super::{BranchRail, ProjectSection, WorkspaceListEntry};
use crate::app::state::AppState;
use crate::layout::PaneId;
use crate::persist::projects::ResolvedMember;
use crate::terminal::{TerminalId, TerminalState};
use crate::workspace::Workspace;

/// Display name for the trailing implicit group holding workspaces that
/// match no declared project member (rule 4, `WorkspaceListEntry::ProjectRow`
/// doc). Never collides with a user's project `name:` — those come from
/// `projects.yml` and are rendered exactly as the user typed them.
const ORPHANS_NAME: &str = "Ungrouped";
/// Collapse key for the orphans group. Namespaced like every other key this
/// module emits (`proj:`, `wt:`) so it can never collide with a real project
/// slug's `proj:<slug>` key.
const ORPHANS_COLLAPSE_KEY: &str = "proj:__orphans__";

/// Build the Project-view entry list.
///
/// Contract with the row painters in `super`:
///
/// - Every entry is height 1. `entry_row_height` already returns 1 for all
///   five variants and MUST keep doing so; a taller row silently desyncs the
///   three lockstep passes (`sidebar.rs` "Shared row-height").
/// - Order is depth-first and stable: `ProjectRow`, then its `WorktreeRow`s,
///   and under each worktree the COMMANDS band, the CHECKS band, then its
///   `Workspace` rows, each followed by its `PaneRow`s when it has 2+ panes.
/// - `apply_hidden_filter`'s depth ladder (0 project, 1 worktree, 2 section
///   header, 3 section item / workspace, 4 pane) mirrors that nesting. A new
///   level means updating that closure too.
/// - The repo name appears exactly once per row-path: `ProjectRow` omits it, a
///   `WorktreeRow` carries it, and no `Workspace` or `PaneRow` ever repeats
///   it. When a project holds a single repo, `WorktreeRow.repo` is `None` and
///   the column collapses.
/// - Workspaces matching no declared member land in one trailing implicit
///   `ProjectRow` with `declared: false`.
///
/// Not built here (deliberately out of scope, see the task report): COMMANDS
/// / CHECKS section rows (no provider exists yet — rule 5 means they would
/// never render anyway) and `unopened: true` worktree rows (no inventory
/// source exists yet on `AppState`; wiring one is future work, never a
/// disk/git call from this pure builder).
pub(super) fn project_view_entries(
    app: &AppState,
    force_expanded: bool,
) -> Vec<WorkspaceListEntry> {
    let file = app.projects.current();
    let mut entries = Vec::new();
    let mut claimed: HashSet<usize> = HashSet::new();

    // `ProjectsFile::projects` is a `BTreeMap`, so iterating it already
    // yields a stable, deterministic (slug-alphabetical) order across
    // renders — exactly what rule 8 requires for sibling project rows.
    for (slug, project) in &file.projects {
        // Already resolved off the tick: no disk touched on this path.
        let members = app.projects.resolved_members(slug);
        let ws_idxs: Vec<usize> = (0..app.workspaces.len())
            .filter(|idx| !claimed.contains(idx))
            .filter(|idx| {
                members
                    .iter()
                    .any(|member| workspace_matches_member(&app.workspaces[*idx], member))
            })
            .collect();
        for &idx in &ws_idxs {
            claimed.insert(idx);
        }
        let name = project.name.clone().unwrap_or_else(|| slug.clone());
        push_project_group(
            &mut entries,
            app,
            &format!("proj:{slug}"),
            name,
            ws_idxs,
            true,
            sections_of(project).0,
            sections_of(project).1,
            force_expanded,
        );
    }

    let orphan_idxs: Vec<usize> = (0..app.workspaces.len())
        .filter(|idx| !claimed.contains(idx))
        .collect();
    if !orphan_idxs.is_empty() {
        push_project_group(
            &mut entries,
            app,
            ORPHANS_COLLAPSE_KEY,
            ORPHANS_NAME.to_string(),
            orphan_idxs,
            false,
            // An orphan group is not a declared project: it has no sections.
            &[],
            &[],
            force_expanded,
        );
    }

    entries
}

/// Whether `ws`'s cached git identity falls under `member`'s resolved
/// checkout. Resolved-identity comparison — `repo_identity`, `checkout_key`,
/// and path-component subdir containment — never a raw string prefix, so
/// `/a/foo-bar` is never treated as living under a `/a/foo` member even
/// though the string starts with it (bora-1le.1 precedent,
/// `app::agents::member_covers`, which this mirrors for the render path:
/// that function resolves the *workspace* side too via disk I/O, which is
/// forbidden here — `ws.git_space()`/`ws.identity_cwd` are already cached on
/// `AppState`, so this reads zero bytes from disk).
fn workspace_matches_member(ws: &Workspace, member: &ResolvedMember) -> bool {
    let Some(space) = ws.git_space() else {
        return false;
    };
    if space.repo_identity != member.repo_identity || space.checkout_key != member.checkout_key {
        return false;
    }
    let ws_subdir = ws
        .identity_cwd
        .strip_prefix(&space.repo_root)
        .unwrap_or(ws.identity_cwd.as_path());
    ws_subdir.starts_with(&member.subdir)
}

/// Push one `ProjectRow` (declared project or the trailing orphans group)
/// and its `WorktreeRow`/`Workspace`/`PaneRow` descendants.
fn push_project_group(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    collapse_key: &str,
    name: String,
    ws_idxs: Vec<usize>,
    declared: bool,
    commands: &[String],
    checks: &[String],
    force_expanded: bool,
) {
    // Group by `checkout_key` — the level the Repo view does not have (rule
    // 2 / `WorktreeRow` doc). First-seen order over `ws_idxs`, which is
    // already `app.workspaces` insertion order, keeps this stable across
    // renders without an extra sort.
    let mut order: Vec<String> = Vec::new();
    let mut by_checkout: HashMap<String, Vec<usize>> = HashMap::new();
    for &idx in &ws_idxs {
        let key = checkout_group_key(&app.workspaces[idx]);
        if !by_checkout.contains_key(&key) {
            order.push(key.clone());
        }
        by_checkout.entry(key).or_default().push(idx);
    }

    // Rule 3: the repo column collapses to `None` when every worktree in
    // this project shares one `repo_name`. No git-backed worktree at all
    // (an orphan with no identity) trivially collapses too — there is
    // nothing to disambiguate.
    let repo_names: HashSet<&str> = order
        .iter()
        .filter_map(|key| by_checkout[key].first())
        .filter_map(|&idx| app.workspaces[idx].git_space())
        .map(|space| space.repo_name.as_str())
        .collect();
    let single_repo = repo_names.len() <= 1;

    // Rule 7: live/total. There is no "dead but tracked" workspace in this
    // data model yet — every matched workspace is, by definition, open — so
    // both sides are the matched count until an unopened-worktree inventory
    // (out of scope here) can widen `total`.
    let total = ws_idxs.len();
    let live = ws_idxs.len();

    entries.push(WorkspaceListEntry::ProjectRow {
        name,
        collapse_key: collapse_key.to_string(),
        live,
        total,
        declared,
    });
    if !force_expanded && app.collapsed_space_keys.contains(collapse_key) {
        return;
    }

    for checkout_key in &order {
        push_worktree(
            entries,
            app,
            checkout_key,
            &by_checkout[checkout_key],
            single_repo,
            commands,
            checks,
            force_expanded,
        );
    }
}

/// Grouping key for a workspace's worktree row: its real `checkout_key` when
/// it has git identity, else a synthetic per-workspace key so an
/// identity-less orphan still gets its own row rather than silently
/// vanishing or colliding with another identity-less workspace.
fn checkout_group_key(ws: &Workspace) -> String {
    ws.git_space()
        .map(|space| space.checkout_key.clone())
        .unwrap_or_else(|| format!("ws-no-space:{}", ws.id))
}

/// Push one `WorktreeRow` and, unless it is collapsed, its section bands
/// (none exist yet, see module doc) and its `Workspace`/`PaneRow` children.
fn push_worktree(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    checkout_key: &str,
    ws_idxs: &[usize],
    single_repo: bool,
    commands: &[String],
    checks: &[String],
    force_expanded: bool,
) {
    let Some(&first_idx) = ws_idxs.first() else {
        return;
    };
    let first = &app.workspaces[first_idx];
    let space = first.git_space();
    let repo = if single_repo {
        None
    } else {
        space.map(|s| s.repo_name.clone())
    };
    let branch = first.branch().unwrap_or_default();
    let (ahead, behind) = first.git_ahead_behind().unwrap_or((0, 0));
    let pr = first
        .cached_check_status
        .as_ref()
        .and_then(|status| status.pr.as_ref())
        .map(|pr| pr.number);
    let collapse_key = format!("wt:{checkout_key}");

    entries.push(WorkspaceListEntry::WorktreeRow {
        checkout_key: checkout_key.to_string(),
        repo,
        branch,
        ahead,
        behind,
        pr,
        collapse_key: collapse_key.clone(),
        unopened: false,
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }

    // Rule 5: a band renders only when non-empty, COMMANDS before CHECKS. The
    // names come from the project's declared `sections:` in `projects.yml` —
    // the one source that exists today and is already resolved off the tick.
    // `done` is 0 on both: nothing executes commands or collects check runs
    // yet (design section 5/6 is a later bead), and rule 6 defines COMMANDS as
    // running/declared, so `0/2` is the honest count, not a placeholder.
    push_section(
        entries,
        app,
        ProjectSection::Commands,
        commands,
        force_expanded,
    );
    push_section(entries, app, ProjectSection::Checks, checks, force_expanded);

    for &idx in ws_idxs {
        push_workspace(entries, app, idx);
    }
}

/// Push one declared band and its items. Empty or absent -> nothing at all
/// (rule 5), which is why a project with no `sections:` renders exactly the
/// tree it renders today.
/// A project's declared band names. Absent `sections:` is an empty slice, so
/// rule 5 keeps both bands unemitted.
fn sections_of(project: &crate::persist::projects::Project) -> (&[String], &[String]) {
    let Some(sections) = project.sections.as_ref() else {
        return (&[], &[]);
    };
    (
        sections.commands.as_deref().unwrap_or(&[]),
        sections.checks.as_deref().unwrap_or(&[]),
    )
}

fn push_section(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    kind: ProjectSection,
    names: &[String],
    force_expanded: bool,
) {
    if names.is_empty() {
        return;
    }
    let collapse_key = format!(
        "sec:{}:{}",
        match kind {
            ProjectSection::Commands => "commands",
            ProjectSection::Checks => "checks",
        },
        names.len()
    );
    entries.push(WorkspaceListEntry::SectionHeader {
        kind,
        collapse_key: collapse_key.clone(),
        done: 0,
        total: names.len(),
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for name in names {
        entries.push(WorkspaceListEntry::SectionItem {
            kind,
            label: name.clone(),
            detail: None,
            running: false,
        });
    }
}

/// Push one `Workspace` row and — only when it has 2+ live panes — one
/// `PaneRow` per pane, in ascending public-pane-number order (rule 6,
/// `PaneRow` doc). A single-pane workspace stays exactly the shape it has
/// today: one `Workspace` row, no `PaneRow` at all.
fn push_workspace(entries: &mut Vec<WorkspaceListEntry>, app: &AppState, ws_idx: usize) {
    entries.push(WorkspaceListEntry::Workspace {
        ws_idx,
        indented: true,
        rail: BranchRail::None,
    });

    let ws = &app.workspaces[ws_idx];
    if ws.public_pane_numbers.len() < 2 {
        return;
    }
    let mut panes: Vec<(PaneId, usize)> = ws
        .public_pane_numbers
        .iter()
        .map(|(&id, &number)| (id, number))
        .collect();
    panes.sort_by_key(|(_, number)| *number);
    for (pane_id, number) in panes {
        entries.push(WorkspaceListEntry::PaneRow {
            ws_idx,
            pane_id: pane_address(ws, number),
            label: pane_row_label(ws, &app.terminals, pane_id, number),
        });
    }
}

/// The stable, addressable pane id — the same `wNpN` form
/// `bora agent prompt` / `orc channel send` accept (`workspace_agent_label`'s
/// doc comment).
fn pane_address(ws: &Workspace, number: usize) -> String {
    format!(
        "{}p{}",
        ws.id,
        crate::workspace::encode_public_number(number)
    )
}

/// What a `PaneRow` shows: a registered `bora agent rename` name for that
/// specific pane when one exists, else its addressable id. Mirrors
/// `workspace_agent_label` (`sidebar.rs`) but per-pane rather than only the
/// workspace's first pane — the whole reason `PaneRow` exists (module doc).
fn pane_row_label(
    ws: &Workspace,
    terminals: &HashMap<TerminalId, TerminalState>,
    pane_id: PaneId,
    number: usize,
) -> String {
    let registered = ws
        .tabs
        .iter()
        .find_map(|tab| tab.panes.get(&pane_id))
        .and_then(|pane| terminals.get(&pane.attached_terminal_id))
        .and_then(|terminal| terminal.agent_name.clone());
    registered.unwrap_or_else(|| pane_address(ws, number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Direction;

    use crate::config::IsolatedDirs;
    use crate::persist::projects::{Member, Project, ProjectsFile, ProjectsStore, WorktreesScope};

    fn temp_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "bora-project-view-fixture-{name}-{}-{nanos}",
            std::process::id(),
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Minimal on-disk `.git` layout `git_space_metadata` can walk without a
    /// real `git` binary — same fixture style already used by
    /// `persist::projects`'s own resolver tests and `workspace::git::discovery`'s.
    fn init_fake_git_repo(root: &std::path::Path, origin_url: Option<&str>) {
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        if let Some(url) = origin_url {
            std::fs::write(
                root.join(".git/config"),
                format!("[remote \"origin\"]\n\turl = {url}\n"),
            )
            .unwrap();
        }
    }

    fn member(dir: &std::path::Path) -> Member {
        Member {
            dir: dir.display().to_string(),
            worktrees: WorktreesScope::default(),
            template: None,
        }
    }

    fn project(members: Vec<Member>) -> Project {
        Project {
            name: None,
            channel: None,
            members,
            orchestrator: None,
            sections: None,
            auto_join: true,
        }
    }

    /// Writes `file` into an isolated `XDG_CONFIG_HOME` and loads it back
    /// through the real `ProjectsStore::load()` path — same idiom the rest
    /// of the codebase uses (`app::agents`, `app::api::projects` tests) so
    /// `AppState::test_new()` never touches the operator's real
    /// `~/.config/bora/projects.yml`. Returns the `IsolatedDirs` guard,
    /// which the caller must keep alive for the test's duration.
    fn store_with(file: ProjectsFile) -> (IsolatedDirs, ProjectsStore) {
        let isolated = IsolatedDirs::new("project-view");
        crate::persist::projects::write_projects_file(&file).expect("write projects.yml");
        (isolated, ProjectsStore::load())
    }

    /// A workspace whose cached git identity is `dir` (real `git_space_metadata`
    /// call, so it matches whatever a `Member::resolve()` on the same `dir`
    /// independently derives).
    fn ws_at(dir: &std::path::Path) -> Workspace {
        let mut ws = Workspace::test_new("t");
        ws.identity_cwd = dir.to_path_buf();
        ws.cached_git_space = crate::workspace::git_space_metadata(dir);
        ws
    }

    fn worktree_rows(entries: &[WorkspaceListEntry]) -> Vec<&str> {
        entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::WorktreeRow { checkout_key, .. } => Some(checkout_key.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The `ws_idx` of every `Workspace` row directly under the `WorktreeRow`
    /// at `worktree_pos` (up to the next container row).
    fn workspace_children(entries: &[WorkspaceListEntry], worktree_pos: usize) -> Vec<usize> {
        entries[worktree_pos + 1..]
            .iter()
            .take_while(|e| {
                matches!(
                    e,
                    WorkspaceListEntry::Workspace { .. } | WorkspaceListEntry::PaneRow { .. }
                )
            })
            .filter_map(|e| match e {
                WorkspaceListEntry::Workspace { ws_idx, .. } => Some(*ws_idx),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn same_checkout_shares_one_worktree_row_distinct_checkouts_of_one_repo_split() {
        let checkout_a = temp_test_dir("dual-checkout-a");
        let checkout_b = temp_test_dir("dual-checkout-b");
        let origin = "git@github.com:owner/repo.git";
        init_fake_git_repo(&checkout_a, Some(origin));
        init_fake_git_repo(&checkout_b, Some(origin));

        let mut file = ProjectsFile::default();
        file.projects.insert(
            "proj".to_string(),
            project(vec![member(&checkout_a), member(&checkout_b)]),
        );
        let (_isolated, store) = store_with(file);

        let mut app = AppState::test_new();
        app.projects = store;
        // ws0 and ws1 share checkout_a; ws2 is checkout_b — same repo
        // identity (shared `origin`), different checkout_key.
        app.workspaces = vec![ws_at(&checkout_a), ws_at(&checkout_a), ws_at(&checkout_b)];

        let entries = project_view_entries(&app, false);
        let keys = worktree_rows(&entries);
        assert_eq!(
            keys.len(),
            2,
            "two distinct checkouts of the same repo must not collapse into one \
             WorktreeRow just because they share repo_identity — that's exactly what \
             the Repo view already does and this level exists to not do: {entries:?}"
        );

        let checkout_a_pos = entries
            .iter()
            .position(|e| matches!(e, WorkspaceListEntry::WorktreeRow { checkout_key, .. } if checkout_key == keys[0]))
            .unwrap();
        let checkout_b_pos = entries
            .iter()
            .position(|e| matches!(e, WorkspaceListEntry::WorktreeRow { checkout_key, .. } if checkout_key == keys[1]))
            .unwrap();
        let mut a_children = workspace_children(&entries, checkout_a_pos);
        a_children.sort_unstable();
        assert_eq!(
            a_children,
            vec![0, 1],
            "both workspaces on checkout_a must land under checkout_a's single WorktreeRow"
        );
        assert_eq!(
            workspace_children(&entries, checkout_b_pos),
            vec![2],
            "checkout_b's workspace must land under its own separate WorktreeRow"
        );

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_b).unwrap();
    }

    #[test]
    fn repo_column_collapses_to_none_when_every_worktree_shares_one_repo() {
        // `repo_name` (rule 3's actual signal) is the checkout root's own
        // basename — same leaf name, different parents, so `checkout_key`
        // still differs while `repo_name` genuinely matches, the way two
        // real worktrees of one repo would.
        let container = temp_test_dir("single-repo-container");
        let checkout_a = container.join("worktree-a").join("myrepo");
        let checkout_b = container.join("worktree-b").join("myrepo");
        std::fs::create_dir_all(&checkout_a).unwrap();
        std::fs::create_dir_all(&checkout_b).unwrap();
        init_fake_git_repo(&checkout_a, None);
        init_fake_git_repo(&checkout_b, None);

        let mut file = ProjectsFile::default();
        file.projects.insert(
            "proj".to_string(),
            project(vec![member(&checkout_a), member(&checkout_b)]),
        );
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&checkout_a), ws_at(&checkout_b)];

        let entries = project_view_entries(&app, false);
        let repos: Vec<Option<String>> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::WorktreeRow { repo, .. } => Some(repo.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            repos,
            vec![None, None],
            "a project whose worktrees all belong to one repo must collapse the repo column: {entries:?}"
        );

        std::fs::remove_dir_all(&container).unwrap();
    }

    #[test]
    fn repo_column_shows_the_repo_name_when_a_project_spans_two_repos() {
        let repo_x = temp_test_dir("multi-repo-x");
        let repo_y = temp_test_dir("multi-repo-y");
        init_fake_git_repo(&repo_x, None);
        init_fake_git_repo(&repo_y, None);

        let mut file = ProjectsFile::default();
        file.projects.insert(
            "proj".to_string(),
            project(vec![member(&repo_x), member(&repo_y)]),
        );
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo_x), ws_at(&repo_y)];

        let entries = project_view_entries(&app, false);
        let repos: Vec<Option<String>> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::WorktreeRow { repo, .. } => Some(repo.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(repos.len(), 2);
        assert!(
            repos.iter().all(Option::is_some),
            "a project spanning two distinct repos must show the repo column: {entries:?}"
        );
        assert_ne!(
            repos[0], repos[1],
            "the two repo names must actually distinguish the worktrees, not both fall back \
             to the same value: {entries:?}"
        );

        std::fs::remove_dir_all(&repo_x).unwrap();
        std::fs::remove_dir_all(&repo_y).unwrap();
    }

    #[test]
    fn two_pane_workspace_emits_two_pane_rows_one_pane_emits_zero() {
        let repo = temp_test_dir("panes");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let single_pane = ws_at(&repo);
        let mut multi_pane = ws_at(&repo);
        multi_pane.test_split(Direction::Horizontal);
        app.workspaces = vec![single_pane, multi_pane];

        let entries = project_view_entries(&app, false);
        let pane_rows_for = |ws_idx: usize| -> usize {
            entries
                .iter()
                .filter(|e| matches!(e, WorkspaceListEntry::PaneRow { ws_idx: idx, .. } if *idx == ws_idx))
                .count()
        };
        assert_eq!(
            pane_rows_for(0),
            0,
            "a single-pane workspace must emit zero PaneRow entries: {entries:?}"
        );
        assert_eq!(
            pane_rows_for(1),
            2,
            "a 2-pane workspace must emit one PaneRow per pane: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn sibling_directory_with_a_colliding_string_prefix_is_not_treated_as_a_member() {
        let base = temp_test_dir("prefix-guard-base");
        let member_dir = base.join("foo");
        // Deliberately a raw string-prefix collision: the sibling's path
        // literally starts with the member directory's path.
        let sibling_dir = std::path::PathBuf::from(format!("{}-bar", member_dir.display()));
        std::fs::create_dir_all(&member_dir).unwrap();
        std::fs::create_dir_all(&sibling_dir).unwrap();
        init_fake_git_repo(&member_dir, None);
        init_fake_git_repo(&sibling_dir, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&member_dir)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&sibling_dir)];

        let entries = project_view_entries(&app, false);
        let proj_row = entries
            .iter()
            .find(|e| matches!(e, WorkspaceListEntry::ProjectRow { declared: true, .. }))
            .expect("declared project row must still exist, just empty");
        let WorkspaceListEntry::ProjectRow { total, .. } = proj_row else {
            unreachable!()
        };
        assert_eq!(
            *total, 0,
            "a workspace under a raw-string sibling of a member dir must NOT match it: {entries:?}"
        );
        let orphan_row = entries
            .iter()
            .find(|e| {
                matches!(
                    e,
                    WorkspaceListEntry::ProjectRow {
                        declared: false,
                        ..
                    }
                )
            })
            .expect("the unmatched workspace must land in the orphans group");
        let WorkspaceListEntry::ProjectRow { total, .. } = orphan_row else {
            unreachable!()
        };
        assert_eq!(*total, 1);

        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn unclaimed_workspaces_land_in_one_orphan_project_row_with_declared_false() {
        // `AppState::test_new()` uses `ProjectsStore::empty()` — no members at all.
        let mut app = AppState::test_new();
        app.workspaces = vec![Workspace::test_new("a"), Workspace::test_new("b")];

        let entries = project_view_entries(&app, false);
        let orphan_rows: Vec<&WorkspaceListEntry> = entries
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    WorkspaceListEntry::ProjectRow {
                        declared: false,
                        ..
                    }
                )
            })
            .collect();
        assert_eq!(
            orphan_rows.len(),
            1,
            "every unclaimed workspace must land in exactly ONE orphan ProjectRow: {entries:?}"
        );
        let WorkspaceListEntry::ProjectRow { total, .. } = orphan_rows[0] else {
            unreachable!()
        };
        assert_eq!(*total, 2);
        assert!(
            !entries
                .iter()
                .any(|e| matches!(e, WorkspaceListEntry::ProjectRow { declared: true, .. })),
            "no declared project exists, so no declared ProjectRow may appear: {entries:?}"
        );
    }

    #[test]
    fn no_workspace_or_pane_row_ever_repeats_the_repo_name() {
        let repo = temp_test_dir("no-repeat-zzreponame");
        init_fake_git_repo(&repo, None);
        let repo_name = crate::workspace::git_space_metadata(&repo)
            .expect("fixture must resolve")
            .repo_name;

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.test_split(Direction::Horizontal);
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, WorkspaceListEntry::PaneRow { .. })),
            "fixture must actually exercise a PaneRow: {entries:?}"
        );
        for entry in &entries {
            if let WorkspaceListEntry::PaneRow { pane_id, label, .. } = entry {
                assert!(
                    !pane_id.contains(repo_name.as_str()) && !label.contains(repo_name.as_str()),
                    "PaneRow must never repeat the repo name ({repo_name:?}): {entry:?}"
                );
            }
        }

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn every_emitted_entry_has_row_height_one() {
        let checkout_a = temp_test_dir("height-a");
        let checkout_b = temp_test_dir("height-b");
        init_fake_git_repo(&checkout_a, None);
        init_fake_git_repo(&checkout_b, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&checkout_a)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut multi_pane = ws_at(&checkout_a);
        multi_pane.test_split(Direction::Horizontal);
        // Include an orphan (unmatched) workspace too, so every variant this
        // module can emit is present in one pass.
        app.workspaces = vec![multi_pane, ws_at(&checkout_b)];

        let entries = project_view_entries(&app, false);
        assert!(
            entries.len() >= 5,
            "fixture too small to exercise every variant: {entries:?}"
        );
        for (idx, entry) in entries.iter().enumerate() {
            assert_eq!(
                crate::ui::sidebar::entry_row_height(entry, &entries, idx),
                1,
                "every Project-view row must stay height 1, entry {idx}: {entry:?}"
            );
        }

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_b).unwrap();
    }
}
