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
use crate::persist::projects::{ResolvedMember, WorktreesScope};
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
/// `unopened: true` worktree rows are built here too (bora-qdi): a declared
/// project's `WorktreesScope::All` members each contribute every
/// `AppState.worktree_inventory` entry for their `repo_identity` that has no
/// matching open `WorktreeRow` in this project, appended after the
/// project's open worktrees (`push_project_group`'s
/// `unopened_worktrees_for_project` call). `ProjectRow.total` counts open +
/// unopened; `.live` stays the open count — there is still no "dead but
/// tracked" *workspace* in this data model, only a worktree with none open
/// on it. The CHECKS band IS built here too, from the representative
/// workspace's `cached_check_status` (see `push_checks_section`).
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
            Some(slug),
            name,
            ws_idxs,
            true,
            resolve_section_order(declared_section_order(project)),
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
            None,
            ORPHANS_NAME.to_string(),
            orphan_idxs,
            false,
            // An orphan group is not a declared project: no order, no
            // sections.
            ProjectSection::ALL,
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
///
/// `member.worktrees` (bora-qdi) decides how strict the checkout compare
/// is: `This` keeps today's behavior exactly — `repo_identity` AND
/// `checkout_key` must both match the declared checkout. `All` compares
/// `repo_identity` only, so every checkout of that repo (the main one plus
/// every linked worktree) qualifies; `ws_subdir` is still computed relative
/// to `ws`'s OWN `repo_root` (never the member's), so a member declared at
/// `<checkout>/packages/landing` keeps matching that same relative subdir
/// inside any worktree of the repo. That generalization — one `members:`
/// entry covering every worktree of a repo — is deliberate, not a
/// loosened bug: it is what lets `push_project_group` attach an
/// `unopened: true` row for a worktree the project never explicitly
/// declared a member for.
fn workspace_matches_member(ws: &Workspace, member: &ResolvedMember) -> bool {
    let Some(space) = ws.git_space() else {
        return false;
    };
    if space.repo_identity != member.repo_identity {
        return false;
    }
    if member.worktrees == WorktreesScope::This && space.checkout_key != member.checkout_key {
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
    // The projects.yml key when this is a declared project (`proj:{slug}`),
    // `None` for the orphans group — only declared projects carry the
    // project-level TODOS/NOTES bands (bora-s3y.3).
    slug: Option<&str>,
    name: String,
    ws_idxs: Vec<usize>,
    declared: bool,
    section_order: [ProjectSection; 5],
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

    // bora-qdi: worktrees on disk for this project's `WorktreesScope::All`
    // members with no open workspace. Empty for the orphans group
    // (`slug: None`) — an "unopened worktree" is defined relative to a
    // declared member's repo, and the orphans group declares none.
    let already_open: HashSet<&str> = order.iter().map(String::as_str).collect();
    let unopened = slug
        .map(|slug| unopened_worktrees_for_project(app, slug, &already_open))
        .unwrap_or_default();

    // Rule 7: live/total. `live` stays the open workspace count — there is
    // still no "dead but tracked" *workspace* in this data model, only a
    // worktree with none open on it. `total` now widens by the unopened
    // count (bora-qdi).
    let total = ws_idxs.len() + unopened.len();
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

    // Project-level shared memory (bora-s3y.3): declared order, default
    // TODOS, NOTES, then PULL REQUESTS (bora-5ia, bora-yw6.2).
    if let Some(slug) = slug {
        // Branches with a local worktree — open or on-disk-unopened — for
        // this project's repos: C3's "no local worktree" filter for the
        // PULL REQUESTS band below. Zero I/O: `order`/`by_checkout` are
        // already resolved above from cached `Workspace` fields, and
        // `unopened` from the tick-refreshed inventory (bora-qdi).
        let local_branches: HashSet<String> = order
            .iter()
            .filter_map(|key| by_checkout[key].first())
            .map(|&idx| app.workspaces[idx].branch().unwrap_or_default())
            .chain(unopened.iter().map(|u| u.branch.clone()))
            .collect();
        for section in project_section_order(&section_order) {
            match section {
                ProjectSection::Todos => push_todos_section(entries, app, slug, force_expanded),
                ProjectSection::Notes => push_notes_section(entries, app, slug, force_expanded),
                ProjectSection::PullRequests => {
                    push_pull_requests_section(entries, app, slug, &local_branches, force_expanded)
                }
                ProjectSection::Commands | ProjectSection::Checks => {
                    unreachable!("project_section_order only returns project-level sections")
                }
            }
        }
    }

    for checkout_key in &order {
        push_worktree(
            entries,
            app,
            checkout_key,
            &by_checkout[checkout_key],
            single_repo,
            section_order,
            commands,
            checks,
            force_expanded,
        );
    }

    // bora-qdi: unopened rows go after the project's live worktrees, no
    // section bands, no children — `sidebar.rs`'s
    // `worktree_row_unopened_renders_dimmed_branch` /
    // `project_view_geometry_unopened_worktree_targets_open_worktree` own
    // their rendering and hit-testing; this builder only decides which ones
    // exist and in what order (sorted deterministically inside
    // `unopened_worktrees_for_project`, so this loop stays stable).
    for entry in &unopened {
        entries.push(WorkspaceListEntry::WorktreeRow {
            checkout_key: entry.checkout_key.clone(),
            // ponytail: reuses the same `single_repo` flag the open
            // worktrees use above rather than widening it with unopened
            // repos' names too, so a project with ZERO open workspaces
            // spanning 2+ repos briefly shows a collapsed repo column
            // until one checkout opens. Fold `repo_name_for_identity`'s
            // lookups into the `single_repo` computation if that edge case
            // matters.
            repo: if single_repo {
                None
            } else {
                repo_name_for_identity(app, &entry.repo_identity)
            },
            branch: entry.branch.clone(),
            ahead: 0,
            behind: 0,
            pr: None,
            collapse_key: format!("wt:{}", entry.checkout_key),
            unopened: true,
        });
    }
}

/// One inventory worktree eligible to render as an `unopened: true` row for
/// a project (bora-qdi).
struct UnopenedWorktree {
    checkout_key: String,
    branch: String,
    repo_identity: String,
}

/// Worktrees on disk for `slug`'s `WorktreesScope::All` members with no
/// currently-open workspace, sorted by branch then checkout key so render
/// order is stable across ticks (rule 8). `already_open` is the project's
/// own worktree-row keys (`push_project_group`'s `order`, built from
/// `checkout_group_key`) — an inventory entry whose canonicalized
/// `checkout_key` is already one of those is a live `WorktreeRow` already
/// and must not double up. Skips bare and prunable entries
/// (`InventoryWorktree.is_bare`/`.is_prunable`) — neither is a worktree a
/// user would "open". `WorktreesScope::This` members are excluded: a `This`
/// member owns exactly one checkout by definition and can never contribute
/// an unopened peer. Zero I/O: reads only `app.projects.resolved_members`
/// (resolved off the tick) and `app.worktree_inventory` (refreshed off the
/// tick, already canonicalized there — see `InventoryWorktree`'s doc).
fn unopened_worktrees_for_project(
    app: &AppState,
    slug: &str,
    already_open: &HashSet<&str>,
) -> Vec<UnopenedWorktree> {
    let mut seen_repo_identities: HashSet<&str> = HashSet::new();
    let mut rows = Vec::new();
    for member in app.projects.resolved_members(slug) {
        if member.worktrees != WorktreesScope::All {
            continue;
        }
        if !seen_repo_identities.insert(member.repo_identity.as_str()) {
            // Already covered by an earlier `All`-scope member on the same
            // repo in this project — eligibility only needs one match.
            continue;
        }
        let Some(inventory) = app.worktree_inventory.get(&member.repo_identity) else {
            continue;
        };
        for entry in &inventory.worktrees {
            if entry.is_bare || entry.is_prunable {
                continue;
            }
            if already_open.contains(entry.checkout_key.as_str()) {
                continue;
            }
            rows.push(UnopenedWorktree {
                checkout_key: entry.checkout_key.clone(),
                branch: entry.branch.clone().unwrap_or_default(),
                repo_identity: member.repo_identity.clone(),
            });
        }
    }
    rows.sort_by(|a, b| {
        a.branch
            .cmp(&b.branch)
            .then_with(|| a.checkout_key.cmp(&b.checkout_key))
    });
    rows
}

/// Best-effort repo display name for `repo_identity`, read from ANY
/// currently-open workspace sharing that identity anywhere in the app (not
/// just the current project) — the only zero-I/O source available on the
/// render path. `None` when no open workspace anywhere shares this identity
/// yet; the `WorktreeRow` then renders with `repo: None`, same treatment a
/// project whose repo column has nothing to disambiguate already gets.
fn repo_name_for_identity(app: &AppState, repo_identity: &str) -> Option<String> {
    app.workspaces
        .iter()
        .find_map(|ws| {
            ws.git_space()
                .filter(|space| space.repo_identity == repo_identity)
        })
        .map(|space| space.repo_name.clone())
}

/// Index of any currently-open workspace sharing `repo_identity`, for naming
/// which repo a PR's worktree should be created in. Mirrors
/// `repo_name_for_identity`'s zero-I/O lookup and its "any open workspace
/// anywhere" scope: every clone and worktree of one repository shares an
/// identity, so any of them names the right repo.
///
/// `None` when no open workspace shares the identity yet. The PR row then
/// renders but stays un-clickable, which is the honest outcome — the
/// alternative, falling back to the active workspace the way the right
/// panel's PR menu does, would create the worktree in whatever repo happened
/// to be focused.
///
/// Called once per project member while building the band, never per row and
/// never from the geometry walk: this is a scan over `app.workspaces`, and
/// the render path is per render x per pane x per client.
fn ws_idx_for_identity(app: &AppState, repo_identity: &str) -> Option<usize> {
    app.workspaces.iter().position(|ws| {
        ws.git_space()
            .is_some_and(|space| space.repo_identity == repo_identity)
    })
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
/// (COMMANDS from the declaration, CHECKS from the cached provider outcome)
/// and its `Workspace`/`PaneRow` children.
fn push_worktree(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    checkout_key: &str,
    ws_idxs: &[usize],
    single_repo: bool,
    section_order: [ProjectSection; 5],
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

    // Rule 5: a band renders only when non-empty, in declared order,
    // default COMMANDS before CHECKS (bora-5ia).
    for section in worktree_section_order(&section_order) {
        if section == ProjectSection::Commands {
            push_commands_section(
                entries,
                app,
                checkout_key,
                ws_idxs,
                commands,
                force_expanded,
            );
        } else {
            push_checks_section(entries, app, checkout_key, first, checks, force_expanded);
        }
    }

    for &idx in ws_idxs {
        push_workspace(entries, app, idx);
    }
}

/// Push the live COMMANDS band for one worktree (bora-55c.3). Items are the
/// repo-declared pane-mode commands the project selects
/// (`sections.commands`); the header's `n/m` counts distinct selected
/// commands with at least one live tagged pane (bora-55c.2's
/// `PaneState.command_label`) over the selected total — the mockup's
/// running/declared. Shell-mode commands never appear (fire-and-forget is
/// unobservable by construction). Reads `Workspace.cached_commands`
/// (refreshed on the runtime tick) and pane tags only — the loader never
/// runs here. The collapse key is namespaced per worktree
/// (`sec:commands:{checkout_key}`), and item rows carry the representative
/// workspace (the same one CHECKS reads) so a click launches there.
fn push_commands_section(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    checkout_key: &str,
    ws_idxs: &[usize],
    selection: &[String],
    force_expanded: bool,
) {
    if selection.is_empty() {
        return;
    }
    let Some(&first) = ws_idxs.first() else {
        return;
    };
    let declared: Vec<&crate::bora_config::BoraCommand> = app.workspaces[first]
        .cached_commands
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|cmd| {
            cmd.mode == crate::bora_config::BoraCommandMode::Pane && selection.contains(&cmd.label)
        })
        .collect();
    if declared.is_empty() {
        return;
    }
    let is_running = |label: &str| {
        ws_idxs.iter().any(|&idx| {
            app.workspaces[idx]
                .panes
                .values()
                .any(|pane| pane.command_label.as_deref() == Some(label))
        })
    };
    let collapse_key = format!("sec:commands:{checkout_key}");
    entries.push(WorkspaceListEntry::SectionHeader {
        kind: ProjectSection::Commands,
        collapse_key: collapse_key.clone(),
        done: declared.iter().filter(|cmd| is_running(&cmd.label)).count(),
        total: declared.len(),
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for cmd in declared {
        entries.push(WorkspaceListEntry::SectionItem {
            kind: ProjectSection::Commands,
            label: cmd.label.clone(),
            // ponytail: port badge (wt port block) deferred — resolve_port
            // runs per click already; a render-time badge wants a
            // tick-refreshed port cache nobody needs yet.
            detail: None,
            running: is_running(&cmd.label),
            ws_idx: Some(first),
        });
    }
}

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

/// A project's declared `sections.order:`, `None` when undeclared — the
/// input to `resolve_section_order`.
fn declared_section_order(project: &crate::persist::projects::Project) -> Option<&[String]> {
    project.sections.as_ref()?.order.as_deref()
}

/// Resolves a project's declared `sections.order:` names into the five
/// `ProjectSection` variants in render priority (bora-5ia, bora-yw6.2).
/// Contract:
///
/// - Names are matched case-insensitively (`ProjectSection::from_name`).
/// - An unknown name is ignored, never an error — a future bora writing a
///   sixth section name into `projects.yml` must not break an older
///   binary's sidebar.
/// - A declared-but-unlisted section still renders: it is appended after
///   the listed ones, in `ProjectSection::ALL` order. Ordering decides
///   sequence, never visibility.
/// - A duplicate name is honored once, at its first position.
/// - Absent or empty `order` resolves to exactly `ProjectSection::ALL`, so
///   behavior is unchanged for every project that does not opt in.
///
/// Always returns all five variants (a permutation of `ProjectSection::ALL`)
/// as a fixed-size array, not a `Vec` — no allocation on the per-render,
/// per-pane, per-client path (AGENTS.md, "Multiplicative performance
/// paths"). `push_project_group`/`push_worktree` then read the relevant
/// group out of it via `project_section_order`/`worktree_section_order` —
/// project-level and worktree-level bands never interleave with each
/// other (module doc), only reorder within their own group.
fn resolve_section_order(order: Option<&[String]>) -> [ProjectSection; 5] {
    let mut resolved = ProjectSection::ALL;
    let mut len = 0usize;
    if let Some(names) = order {
        for name in names {
            let Some(section) = ProjectSection::from_name(name) else {
                continue;
            };
            if resolved[..len].contains(&section) {
                continue;
            }
            resolved[len] = section;
            len += 1;
        }
    }
    for section in ProjectSection::ALL {
        if len == resolved.len() {
            break;
        }
        if resolved[..len].contains(&section) {
            continue;
        }
        resolved[len] = section;
        len += 1;
    }
    resolved
}

/// Filters a resolved order down to the worktree-level pair (COMMANDS,
/// CHECKS), preserving their relative sequence.
fn worktree_section_order(resolved: &[ProjectSection; 5]) -> [ProjectSection; 2] {
    let mut out = [ProjectSection::Commands, ProjectSection::Checks];
    let mut i = 0;
    for &section in resolved {
        if matches!(section, ProjectSection::Commands | ProjectSection::Checks) {
            out[i] = section;
            i += 1;
        }
    }
    out
}

/// Filters a resolved order down to the project-level pair (TODOS, NOTES),
/// preserving their relative sequence. See `worktree_section_order`.
/// Filters a resolved order down to the project-level trio (TODOS, NOTES,
/// PULL REQUESTS), preserving their relative sequence. See
/// `worktree_section_order`.
fn project_section_order(resolved: &[ProjectSection; 5]) -> [ProjectSection; 3] {
    let mut out = [
        ProjectSection::Todos,
        ProjectSection::Notes,
        ProjectSection::PullRequests,
    ];
    let mut i = 0;
    for &section in resolved {
        if matches!(
            section,
            ProjectSection::Todos | ProjectSection::Notes | ProjectSection::PullRequests
        ) {
            out[i] = section;
            i += 1;
        }
    }
    out
}

/// Push the CHECKS band for one worktree, from the representative workspace's
/// `cached_check_status` (the same workspace the WorktreeRow's PR badge reads
/// — one fetch covers every workspace on the branch). Eligibility mirrors the
/// provider contract (`ProviderOutcome`):
///
/// - The project must declare the section (`sections.checks` non-empty) —
///   the declaration is the design's "toggle CHECKS" knob. Undeclared renders
///   nothing, exactly like today.
/// - Not-applicable (the `WorkspaceCheckStatus::is_not_applicable` sentinel,
///   e.g. no PR for this branch) renders nothing at all.
/// - No cached outcome yet (first fetch still in flight) renders nothing —
///   rule 5, the band would be empty.
/// - A provider error renders the header plus one error row: a failure is
///   never silently empty.
/// - Rows render the header with `n/m` = `checks_counts` (passing/total) and
///   one item per failing check; passing/pending checks have no row. A PR
///   with zero check runs renders no band (rule 5).
///
/// The collapse key is namespaced per worktree (`sec:checks:{checkout_key}`)
/// so two worktrees' bands never share collapse state.
fn push_checks_section(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    checkout_key: &str,
    ws: &Workspace,
    declared: &[String],
    force_expanded: bool,
) {
    if declared.is_empty() {
        return;
    }
    let Some(status) = ws.cached_check_status.as_ref() else {
        return;
    };
    if status.is_not_applicable() {
        return;
    }
    let collapse_key = format!("sec:checks:{checkout_key}");
    if let Some(error) = status.error.as_deref() {
        entries.push(WorkspaceListEntry::SectionHeader {
            kind: ProjectSection::Checks,
            collapse_key: collapse_key.clone(),
            done: 0,
            total: 0,
        });
        if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
            return;
        }
        entries.push(WorkspaceListEntry::SectionItem {
            kind: ProjectSection::Checks,
            label: error.to_string(),
            detail: None,
            running: false,
            ws_idx: None,
        });
        return;
    }
    let (passing, total) = crate::workspace::checks_counts(&status.checks);
    if total == 0 {
        return;
    }
    entries.push(WorkspaceListEntry::SectionHeader {
        kind: ProjectSection::Checks,
        collapse_key: collapse_key.clone(),
        done: passing,
        total,
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for run in status.checks.iter().filter(|run| run.is_failing()) {
        entries.push(WorkspaceListEntry::SectionItem {
            kind: ProjectSection::Checks,
            label: run.name.clone(),
            detail: None,
            running: false,
            ws_idx: None,
        });
    }
}

/// Push the project-level TODOS band (bora-s3y.3): header `n/m` = done/total
/// and one row per ACTIONABLE open todo (blocked todos get no row — the
/// section is the swarm's "what can I pick up next" list). Reads the
/// AppState snapshot the verb handlers refresh; never the stores. Renders
/// only when the project has todos at all (rule 5). Collapse key is
/// namespaced per project (`sec:todos:{slug}`).
fn push_todos_section(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    slug: &str,
    force_expanded: bool,
) {
    let Some(summary) = app.project_todos.get(slug) else {
        return;
    };
    if summary.total == 0 {
        return;
    }
    let collapse_key = format!("sec:todos:{slug}");
    entries.push(WorkspaceListEntry::SectionHeader {
        kind: ProjectSection::Todos,
        collapse_key: collapse_key.clone(),
        done: summary.done,
        total: summary.total,
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for title in &summary.actionable {
        entries.push(WorkspaceListEntry::SectionItem {
            kind: ProjectSection::Todos,
            label: title.clone(),
            detail: None,
            running: false,
            ws_idx: None,
        });
    }
}

/// Push the project-level NOTES band (bora-s3y.3): one row per scratchpad
/// doc name, from the same refresh discipline as TODOS. Renders only when
/// the project has docs.
fn push_notes_section(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    slug: &str,
    force_expanded: bool,
) {
    let Some(names) = app.project_notes.get(slug) else {
        return;
    };
    if names.is_empty() {
        return;
    }
    let collapse_key = format!("sec:notes:{slug}");
    entries.push(WorkspaceListEntry::SectionHeader {
        kind: ProjectSection::Notes,
        collapse_key: collapse_key.clone(),
        done: 0,
        total: names.len(),
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for name in names {
        entries.push(WorkspaceListEntry::SectionItem {
            kind: ProjectSection::Notes,
            label: name.clone(),
            detail: None,
            running: false,
            ws_idx: None,
        });
    }
}

/// Push the project-level PULL REQUESTS band (bora-yw6.2): one row per open
/// PR authored by the user, for a `WorktreesScope::All` member's repo, whose
/// head branch has no local worktree — open or unopened-on-disk (C3, the
/// exact analogue of the dimmed unopened-worktree row: it is what keeps a PR
/// from appearing twice once opened). Reads `AppState.repo_open_prs` only —
/// no fetch, no I/O, same discipline as TODOS/NOTES. Eligibility:
///
/// - No `WorktreesScope::All` member's repo has ANY cached `repo_open_prs`
///   entry yet, or a relevant repo's cache has zero PRs after the
///   local-worktree filter below -> no band at all (rule 5, mirrors "a PR
///   with zero check runs renders no band").
/// - A relevant repo's cache carries a fetch error -> header plus one error
///   row (the same shape CHECKS uses for a provider error): a failure is
///   never silently empty. The first errored repo wins if more than one is
///   in scope — one band, one error line.
/// - Otherwise one row per PR whose head branch is not in `local_branches`,
///   sorted by PR number for stable render order (rule 8).
///
/// Collapse key is namespaced per project (`sec:prs:{slug}`).
fn push_pull_requests_section(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    slug: &str,
    local_branches: &HashSet<String>,
    force_expanded: bool,
) {
    let mut seen_repo_identities: HashSet<&str> = HashSet::new();
    let mut error: Option<&str> = None;
    // Each row carries a representative `ws_idx` for ITS OWN repo, resolved
    // once per member rather than once per PR: a project can hold members
    // from several repos, so one project-wide index would open the worktree
    // in the wrong one whenever the band mixes repos.
    let mut rows: Vec<(&crate::workspace::OpenPr, Option<usize>)> = Vec::new();
    for member in app.projects.resolved_members(slug) {
        if member.worktrees != WorktreesScope::All {
            continue;
        }
        if !seen_repo_identities.insert(member.repo_identity.as_str()) {
            continue;
        }
        let Some(cache) = app.repo_open_prs.get(&member.repo_identity) else {
            continue;
        };
        if let Some(err) = cache.error.as_deref() {
            if error.is_none() {
                error = Some(err);
            }
            continue;
        }
        let repo_ws_idx = ws_idx_for_identity(app, &member.repo_identity);
        rows.extend(
            cache
                .prs
                .iter()
                .filter(|pr| !local_branches.contains(&pr.head_ref_name))
                .map(|pr| (pr, repo_ws_idx)),
        );
    }
    let collapse_key = format!("sec:prs:{slug}");
    if let Some(error) = error {
        entries.push(WorkspaceListEntry::SectionHeader {
            kind: ProjectSection::PullRequests,
            collapse_key: collapse_key.clone(),
            done: 0,
            total: 0,
        });
        if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
            return;
        }
        entries.push(WorkspaceListEntry::SectionItem {
            kind: ProjectSection::PullRequests,
            label: error.to_string(),
            detail: None,
            running: false,
            ws_idx: None,
        });
        return;
    }
    if rows.is_empty() {
        return;
    }
    rows.sort_by_key(|(pr, _)| pr.number);
    entries.push(WorkspaceListEntry::SectionHeader {
        kind: ProjectSection::PullRequests,
        collapse_key: collapse_key.clone(),
        done: 0,
        total: rows.len(),
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for (pr, ws_idx) in rows {
        entries.push(WorkspaceListEntry::PrRow {
            number: pr.number,
            title: pr.title.clone(),
            url: pr.url.clone(),
            head_ref: pr.head_ref_name.clone(),
            is_draft: pr.is_draft,
            checks: pr.checks,
            ws_idx,
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
    use crate::persist::projects::{
        Member, Project, ProjectsFile, ProjectsStore, Sections, WorktreesScope,
    };

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

    /// Same as `project` but with the CHECKS band declared — the design's
    /// "toggle CHECKS" knob (`sections.checks` selects the providers).
    fn project_with_checks(members: Vec<Member>) -> Project {
        let mut project = project(members);
        project.sections = Some(Sections {
            checks: Some(vec!["gh".to_string()]),
            commands: None,
            order: None,
        });
        project
    }

    fn check_run(name: &str, status: &str, conclusion: Option<&str>) -> crate::workspace::CheckRun {
        crate::workspace::CheckRun {
            name: name.to_string(),
            status: status.to_string(),
            conclusion: conclusion.map(str::to_string),
        }
    }

    fn checks_status(
        pr_number: Option<u64>,
        checks: Vec<crate::workspace::CheckRun>,
        error: Option<&str>,
    ) -> crate::workspace::WorkspaceCheckStatus {
        crate::workspace::WorkspaceCheckStatus {
            pr: pr_number.map(|number| crate::workspace::PrSummary {
                number,
                title: "t".to_string(),
                state: "OPEN".to_string(),
                url: String::new(),
                mergeable: None,
            }),
            checks,
            error: error.map(str::to_string),
        }
    }

    /// The `(done, total)` of the worktree's CHECKS band header, if emitted.
    fn checks_band(entries: &[WorkspaceListEntry]) -> Option<(usize, usize)> {
        entries.iter().find_map(|e| match e {
            WorkspaceListEntry::SectionHeader {
                kind: ProjectSection::Checks,
                done,
                total,
                ..
            } => Some((*done, *total)),
            _ => None,
        })
    }

    /// The labels of the CHECKS band's item rows, in order.
    fn checks_items(entries: &[WorkspaceListEntry]) -> Vec<String> {
        entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionItem {
                    kind: ProjectSection::Checks,
                    label,
                    ..
                } => Some(label.clone()),
                _ => None,
            })
            .collect()
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
    fn checks_section_counts_passing_over_total_and_lists_failing_rows() {
        let repo = temp_test_dir("checks-mixed");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project_with_checks(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_check_status = Some(checks_status(
            Some(42),
            vec![
                check_run("build", "COMPLETED", Some("SUCCESS")),
                check_run("clippy", "COMPLETED", Some("FAILURE")),
                check_run("test", "IN_PROGRESS", None),
                check_run("docs", "COMPLETED", Some("SKIPPED")),
                check_run("lint", "COMPLETED", Some("ERROR")),
            ],
            None,
        ));
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        // n/m = passing/total: build, docs pass; clippy, lint fail; test runs.
        assert_eq!(
            checks_band(&entries),
            Some((2, 5)),
            "header must carry checks_counts: {entries:?}"
        );
        assert_eq!(
            checks_items(&entries),
            vec!["clippy".to_string(), "lint".to_string()],
            "only failing checks get rows, in provider order: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn checks_row_pr_number_lands_on_the_worktree_row() {
        let repo = temp_test_dir("checks-pr");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project_with_checks(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_check_status = Some(checks_status(
            Some(128),
            vec![check_run("build", "COMPLETED", Some("SUCCESS"))],
            None,
        ));
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        let pr = entries.iter().find_map(|e| match e {
            WorkspaceListEntry::WorktreeRow { pr, .. } => Some(*pr),
            _ => None,
        });
        assert_eq!(
            pr,
            Some(Some(128)),
            "the worktree row must carry the PR number: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn checks_section_renders_provider_error_as_a_row() {
        let repo = temp_test_dir("checks-error");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project_with_checks(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_check_status = Some(checks_status(None, Vec::new(), Some("gh: boom")));
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            checks_band(&entries),
            Some((0, 0)),
            "an errored band still renders its header: {entries:?}"
        );
        assert_eq!(
            checks_items(&entries),
            vec!["gh: boom".to_string()],
            "a provider error is a visible row, never silently empty: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn checks_section_not_applicable_renders_no_section() {
        let repo = temp_test_dir("checks-na");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project_with_checks(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        // NotApplicable arrives on the legacy shape as the sentinel error.
        ws.cached_check_status = Some(checks_status(
            None,
            Vec::new(),
            Some(crate::workspace::NOT_APPLICABLE_ERROR),
        ));
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        assert_eq!(checks_band(&entries), None, "{entries:?}");
        assert!(checks_items(&entries).is_empty(), "{entries:?}");

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn checks_section_absent_without_declaration_or_before_first_fetch() {
        let repo = temp_test_dir("checks-gating");
        init_fake_git_repo(&repo, None);

        // Each half is scoped: `IsolatedDirs` holds a non-reentrant lock, so
        // the first guard must drop before the second `store_with` runs.
        {
            // (a) Data present but the project declares no CHECKS band ->
            // nothing.
            let mut file = ProjectsFile::default();
            file.projects
                .insert("proj".to_string(), project(vec![member(&repo)]));
            let (_isolated, store) = store_with(file);
            let mut app = AppState::test_new();
            app.projects = store;

            let mut ws = ws_at(&repo);
            ws.cached_check_status = Some(checks_status(
                Some(1),
                vec![check_run("clippy", "COMPLETED", Some("FAILURE"))],
                None,
            ));
            app.workspaces = vec![ws];

            let entries = project_view_entries(&app, false);
            assert_eq!(
                checks_band(&entries),
                None,
                "an undeclared band must not render: {entries:?}"
            );
        }

        {
            // (b) Declared but the first fetch has not landed yet -> nothing
            // (rule 5: bands render only when non-empty).
            let mut file = ProjectsFile::default();
            file.projects
                .insert("proj".to_string(), project_with_checks(vec![member(&repo)]));
            let (_isolated, store) = store_with(file);
            let mut app = AppState::test_new();
            app.projects = store;
            app.workspaces = vec![ws_at(&repo)];

            let entries = project_view_entries(&app, false);
            assert_eq!(
                checks_band(&entries),
                None,
                "no cached outcome means no band: {entries:?}"
            );
        }

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn checks_section_absent_when_pr_has_zero_check_runs() {
        let repo = temp_test_dir("checks-empty");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project_with_checks(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_check_status = Some(checks_status(Some(9), Vec::new(), None));
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            checks_band(&entries),
            None,
            "a 0/0 band is empty and must not render (rule 5): {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn checks_section_collapse_hides_rows_keeps_header() {
        let repo = temp_test_dir("checks-collapse");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project_with_checks(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_check_status = Some(checks_status(
            None,
            vec![check_run("clippy", "COMPLETED", Some("FAILURE"))],
            None,
        ));
        app.workspaces = vec![ws];

        // The band's collapse key is namespaced per worktree.
        let checkout_key = crate::workspace::git_space_metadata(&repo)
            .expect("fixture must resolve")
            .checkout_key;
        app.collapsed_space_keys
            .insert(format!("sec:checks:{checkout_key}"));

        let entries = project_view_entries(&app, false);
        assert_eq!(
            checks_band(&entries),
            Some((0, 1)),
            "a collapsed band keeps its header: {entries:?}"
        );
        assert!(
            checks_items(&entries).is_empty(),
            "a collapsed band hides its rows: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    // ── bora-55c.3: live worktree COMMANDS band ──────────────────────────

    fn project_with_commands(members: Vec<Member>, names: &[&str]) -> Project {
        let mut project = project(members);
        project.sections = Some(Sections {
            checks: None,
            commands: Some(names.iter().map(ToString::to_string).collect()),
            order: None,
        });
        project
    }

    fn bora_cmd(
        label: &str,
        mode: crate::bora_config::BoraCommandMode,
    ) -> crate::bora_config::BoraCommand {
        crate::bora_config::BoraCommand {
            label: label.to_string(),
            command: format!("run {label}"),
            mode,
            branch: None,
        }
    }

    /// The `(label, running, ws_idx)` of every COMMANDS item row, in order.
    fn commands_items(entries: &[WorkspaceListEntry]) -> Vec<(String, bool, Option<usize>)> {
        entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionItem {
                    kind: ProjectSection::Commands,
                    label,
                    running,
                    ws_idx,
                    ..
                } => Some((label.clone(), *running, *ws_idx)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn commands_section_counts_running_over_selected_and_marks_running_rows() {
        let repo = temp_test_dir("commands-live");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects.insert(
            "proj".to_string(),
            project_with_commands(vec![member(&repo)], &["dev", "test", "deploy"]),
        );
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_commands = Some(vec![
            bora_cmd("dev", crate::bora_config::BoraCommandMode::Pane),
            bora_cmd("test", crate::bora_config::BoraCommandMode::Pane),
            // Shell-mode is fire-and-forget: declared but never counted.
            bora_cmd("deploy", crate::bora_config::BoraCommandMode::Shell),
            // Declared but NOT selected by the project: narrowed out.
            bora_cmd("extra", crate::bora_config::BoraCommandMode::Pane),
        ]);
        let pane = ws
            .focused_pane_id()
            .expect("test workspace has a root pane");
        ws.pane_state_mut(pane)
            .expect("root pane state")
            .command_label = Some("dev".to_string());
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, ProjectSection::Commands),
            Some((1, 2)),
            "n/m = selected commands with a live tagged pane / selected \
             pane-mode declared (shell-mode and unselected excluded): {entries:?}"
        );
        assert_eq!(
            commands_items(&entries),
            vec![
                ("dev".to_string(), true, Some(0)),
                ("test".to_string(), false, Some(0)),
            ],
            "running row marked, idle row unmarked, both launchable into the \
             representative workspace: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn commands_section_absent_without_selection_or_matching_declared() {
        let repo = temp_test_dir("commands-gating");
        init_fake_git_repo(&repo, None);

        // (a)+(b) share one projects file, hence one guard. The guard MUST
        // drop before (c)'s `store_with` — `IsolatedDirs` holds the
        // process-global, non-reentrant config-env lock and a second guard
        // deadlocks the test (AGENTS.md, learned 2026-08-22).
        {
            // (a) Selection exists but the tick has not refreshed any
            // declared commands -> no band (rule 5), no loader call from
            // render.
            let mut file = ProjectsFile::default();
            file.projects.insert(
                "proj".to_string(),
                project_with_commands(vec![member(&repo)], &["dev"]),
            );
            let (_isolated, store) = store_with(file);
            let mut app = AppState::test_new();
            app.projects = store;
            app.workspaces = vec![ws_at(&repo)];
            let entries = project_view_entries(&app, false);
            assert_eq!(
                section_band(&entries, ProjectSection::Commands),
                None,
                "no refreshed declarations -> no band: {entries:?}"
            );

            // (b) Declarations exist but none match the selection -> no
            // band.
            app.workspaces[0].cached_commands = Some(vec![bora_cmd(
                "other",
                crate::bora_config::BoraCommandMode::Pane,
            )]);
            let entries = project_view_entries(&app, false);
            assert_eq!(
                section_band(&entries, ProjectSection::Commands),
                None,
                "selection matches nothing declared -> no band: {entries:?}"
            );
        }

        // (c) Declarations match but the project never selects commands ->
        // no band.
        {
            let mut file = ProjectsFile::default();
            file.projects
                .insert("proj".to_string(), project(vec![member(&repo)]));
            let (_isolated, store) = store_with(file);
            let mut app = AppState::test_new();
            app.projects = store;
            let mut ws = ws_at(&repo);
            ws.cached_commands = Some(vec![bora_cmd(
                "dev",
                crate::bora_config::BoraCommandMode::Pane,
            )]);
            app.workspaces = vec![ws];
            let entries = project_view_entries(&app, false);
            assert_eq!(
                section_band(&entries, ProjectSection::Commands),
                None,
                "project without sections.commands -> no band: {entries:?}"
            );
        }

        std::fs::remove_dir_all(&repo).unwrap();
    }

    // ── bora-s3y.3: project-level TODOS/NOTES bands ──────────────────────

    fn todos_summary(
        done: usize,
        total: usize,
        actionable: &[&str],
    ) -> crate::persist::todos::TodosSummary {
        crate::persist::todos::TodosSummary {
            done,
            total,
            actionable: actionable.iter().map(ToString::to_string).collect(),
        }
    }

    fn section_band(
        entries: &[WorkspaceListEntry],
        want: ProjectSection,
    ) -> Option<(usize, usize)> {
        entries.iter().find_map(|e| match e {
            WorkspaceListEntry::SectionHeader {
                kind, done, total, ..
            } if *kind == want => Some((*done, *total)),
            _ => None,
        })
    }

    fn section_items(entries: &[WorkspaceListEntry], want: ProjectSection) -> Vec<String> {
        entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionItem { kind, label, .. } if *kind == want => {
                    Some(label.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn position_of(
        entries: &[WorkspaceListEntry],
        pred: impl Fn(&WorkspaceListEntry) -> bool,
    ) -> usize {
        entries
            .iter()
            .position(pred)
            .unwrap_or_else(|| panic!("entry not found: {entries:?}"))
    }

    #[test]
    fn todos_notes_render_between_project_row_and_worktrees_with_counts() {
        let repo = temp_test_dir("todos-notes");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];
        app.project_todos.insert(
            "proj".to_string(),
            todos_summary(1, 3, &["ship sidebar", "close epic"]),
        );
        app.project_notes.insert(
            "proj".to_string(),
            vec!["decisions".to_string(), "plan".to_string()],
        );

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, ProjectSection::Todos),
            Some((1, 3)),
            "TODOS header n/m = done/total: {entries:?}"
        );
        assert_eq!(
            section_items(&entries, ProjectSection::Todos),
            vec!["ship sidebar".to_string(), "close epic".to_string()],
            "one row per actionable open todo: {entries:?}"
        );
        assert_eq!(
            section_band(&entries, ProjectSection::Notes),
            Some((0, 2)),
            "NOTES header carries the doc count: {entries:?}"
        );
        assert_eq!(
            section_items(&entries, ProjectSection::Notes),
            vec!["decisions".to_string(), "plan".to_string()],
            "one row per scratchpad doc: {entries:?}"
        );

        let project_pos = position_of(&entries, |e| {
            matches!(e, WorkspaceListEntry::ProjectRow { declared: true, .. })
        });
        let todos_pos = position_of(&entries, |e| {
            matches!(
                e,
                WorkspaceListEntry::SectionHeader {
                    kind: ProjectSection::Todos,
                    ..
                }
            )
        });
        let notes_pos = position_of(&entries, |e| {
            matches!(
                e,
                WorkspaceListEntry::SectionHeader {
                    kind: ProjectSection::Notes,
                    ..
                }
            )
        });
        let worktree_pos = position_of(&entries, |e| {
            matches!(e, WorkspaceListEntry::WorktreeRow { .. })
        });
        assert!(
            project_pos < todos_pos && todos_pos < notes_pos && notes_pos < worktree_pos,
            "sections sit between the project row and its worktrees, TODOS then NOTES: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn todos_section_renders_only_actionable_rows() {
        let repo = temp_test_dir("todos-actionable");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];
        // Two open todos in the totals, but only one is actionable — the
        // blocked one has no row (blocking itself is pinned by
        // persist::todos' TodosSummary tests).
        app.project_todos
            .insert("proj".to_string(), todos_summary(0, 2, &["free task"]));

        let entries = project_view_entries(&app, false);
        assert_eq!(section_band(&entries, ProjectSection::Todos), Some((0, 2)));
        assert_eq!(
            section_items(&entries, ProjectSection::Todos),
            vec!["free task".to_string()],
            "blocked todos are excluded from the section's actionable rows: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn todos_notes_collapse_hides_rows_keeps_headers() {
        let repo = temp_test_dir("todos-collapse");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];
        app.project_todos
            .insert("proj".to_string(), todos_summary(0, 1, &["task"]));
        app.project_notes
            .insert("proj".to_string(), vec!["plan".to_string()]);
        app.collapsed_space_keys
            .insert("sec:todos:proj".to_string());
        app.collapsed_space_keys
            .insert("sec:notes:proj".to_string());

        let entries = project_view_entries(&app, false);
        assert_eq!(section_band(&entries, ProjectSection::Todos), Some((0, 1)));
        assert_eq!(section_band(&entries, ProjectSection::Notes), Some((0, 1)));
        assert!(section_items(&entries, ProjectSection::Todos).is_empty());
        assert!(section_items(&entries, ProjectSection::Notes).is_empty());

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn todos_notes_absent_without_data_and_for_orphans() {
        let repo = temp_test_dir("todos-empty");
        init_fake_git_repo(&repo, None);
        let orphan = temp_test_dir("todos-orphan");
        init_fake_git_repo(&orphan, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        // One declared member (no todos/notes seeded) and one orphan
        // workspace no project claims.
        app.workspaces = vec![ws_at(&repo), ws_at(&orphan)];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, ProjectSection::Todos),
            None,
            "no snapshot -> no band (rule 5): {entries:?}"
        );
        assert_eq!(section_band(&entries, ProjectSection::Notes), None);
        assert!(
            entries.iter().any(|e| matches!(
                e,
                WorkspaceListEntry::ProjectRow {
                    declared: false,
                    ..
                }
            )),
            "the orphan workspace still groups under the undeclared row, which \
             must NOT grow project-level sections: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
        std::fs::remove_dir_all(&orphan).unwrap();
    }

    #[test]
    fn checks_section_lockstep_rows_stay_height_one() {
        // G4: the new CHECKS rows are ordinary entries — every lockstep pass
        // derives their height from `entry_row_height`, which must return 1.
        let repo = temp_test_dir("checks-lockstep");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project_with_checks(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_check_status = Some(checks_status(
            Some(3),
            vec![
                check_run("build", "COMPLETED", Some("SUCCESS")),
                check_run("clippy", "COMPLETED", Some("FAILURE")),
            ],
            None,
        ));
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        assert!(
            checks_band(&entries).is_some() && !checks_items(&entries).is_empty(),
            "fixture must actually emit a CHECKS band: {entries:?}"
        );
        for (idx, entry) in entries.iter().enumerate() {
            assert_eq!(
                crate::ui::sidebar::entry_row_height(entry, &entries, idx),
                1,
                "entry {idx}: {entry:?}"
            );
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
        file.projects.insert(
            "proj".to_string(),
            project_with_checks(vec![member(&checkout_a)]),
        );
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut multi_pane = ws_at(&checkout_a);
        multi_pane.test_split(Direction::Horizontal);
        // A CHECKS band too, so SectionHeader/SectionItem are among the
        // variants this guard covers.
        multi_pane.cached_check_status = Some(checks_status(
            Some(1),
            vec![check_run("clippy", "COMPLETED", Some("FAILURE"))],
            None,
        ));
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

    #[test]
    fn worktrees_scope_all_matches_other_checkouts_this_requires_exact_checkout() {
        let member_checkout = temp_test_dir("scope-member");
        let other_checkout = temp_test_dir("scope-other");
        let origin = "git@github.com:owner/scope-repo.git";
        init_fake_git_repo(&member_checkout, Some(origin));
        init_fake_git_repo(&other_checkout, Some(origin));

        // `WorktreesScope::All`: a workspace on a DIFFERENT checkout of the
        // same `repo_identity` lands under the declared project.
        let mut file = ProjectsFile::default();
        file.projects.insert(
            "proj".to_string(),
            project(vec![Member {
                dir: member_checkout.display().to_string(),
                worktrees: WorktreesScope::All,
                template: None,
            }]),
        );
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&other_checkout)];

        let entries = project_view_entries(&app, false);
        let proj_total = entries.iter().find_map(|e| match e {
            WorkspaceListEntry::ProjectRow {
                name,
                total,
                declared: true,
                ..
            } if name == "proj" => Some(*total),
            _ => None,
        });
        assert_eq!(
            proj_total,
            Some(1),
            "All scope must match a workspace on a different checkout of the \
             same repo_identity: {entries:?}"
        );
        let has_orphans = entries.iter().any(|e| {
            matches!(
                e,
                WorkspaceListEntry::ProjectRow {
                    declared: false,
                    ..
                }
            )
        });
        assert!(
            !has_orphans,
            "the workspace must not ALSO fall into Ungrouped: {entries:?}"
        );

        // `WorktreesScope::This`: the same workspace now lands in
        // Ungrouped instead — the blocking defect this bead fixes.
        let mut file2 = ProjectsFile::default();
        file2.projects.insert(
            "proj".to_string(),
            project(vec![Member {
                dir: member_checkout.display().to_string(),
                worktrees: WorktreesScope::This,
                template: None,
            }]),
        );
        drop(_isolated);
        let (_isolated2, store2) = store_with(file2);
        let mut app2 = AppState::test_new();
        app2.projects = store2;
        app2.workspaces = vec![ws_at(&other_checkout)];

        let entries2 = project_view_entries(&app2, false);
        let proj_total2 = entries2.iter().find_map(|e| match e {
            WorkspaceListEntry::ProjectRow {
                name,
                total,
                declared: true,
                ..
            } if name == "proj" => Some(*total),
            _ => None,
        });
        assert_eq!(
            proj_total2,
            Some(0),
            "This scope must not match a different checkout: {entries2:?}"
        );
        let orphans_total2 = entries2.iter().find_map(|e| match e {
            WorkspaceListEntry::ProjectRow {
                total,
                declared: false,
                ..
            } => Some(*total),
            _ => None,
        });
        assert_eq!(
            orphans_total2,
            Some(1),
            "the workspace must land in Ungrouped instead: {entries2:?}"
        );

        std::fs::remove_dir_all(&member_checkout).unwrap();
        std::fs::remove_dir_all(&other_checkout).unwrap();
    }

    #[test]
    fn unopened_worktree_renders_dimmed_row_and_widens_total_open_one_does_not_duplicate() {
        let member_checkout = temp_test_dir("unopened-member");
        let sibling_worktree = temp_test_dir("unopened-sibling");
        let origin = "git@github.com:owner/unopened-repo.git";
        init_fake_git_repo(&member_checkout, Some(origin));
        init_fake_git_repo(&sibling_worktree, Some(origin));
        let sibling_key = crate::worktree::canonical_or_original(&sibling_worktree)
            .display()
            .to_string();

        let project_file = |dir: &std::path::Path| {
            let mut file = ProjectsFile::default();
            file.projects.insert(
                "proj".to_string(),
                project(vec![Member {
                    dir: dir.display().to_string(),
                    worktrees: WorktreesScope::All,
                    template: None,
                }]),
            );
            file
        };

        // Only the member's own checkout is open; the sibling worktree is
        // in the inventory with no open workspace.
        let (_isolated, store) = store_with(project_file(&member_checkout));
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&member_checkout)];
        let repo_identity = app.workspaces[0].git_space().unwrap().repo_identity.clone();
        app.worktree_inventory.insert(
            repo_identity.clone(),
            crate::app::state::RepoWorktreeInventory {
                worktrees: vec![crate::app::state::InventoryWorktree {
                    checkout_key: sibling_key.clone(),
                    branch: Some("feature/x".to_string()),
                    is_bare: false,
                    is_prunable: false,
                }],
                error: None,
            },
        );

        let entries = project_view_entries(&app, false);
        let unopened_rows: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::WorktreeRow {
                    checkout_key,
                    unopened: true,
                    ..
                } => Some(checkout_key.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            unopened_rows,
            vec![sibling_key.as_str()],
            "a worktree with no open workspace must render as unopened: true: {entries:?}"
        );
        let (live, total) = entries
            .iter()
            .find_map(|e| match e {
                WorkspaceListEntry::ProjectRow {
                    name, live, total, ..
                } if name == "proj" => Some((*live, *total)),
                _ => None,
            })
            .expect("proj row");
        assert_eq!(live, 1, "live stays the open workspace count: {entries:?}");
        assert_eq!(
            total, 2,
            "total widens by the unopened worktree, so total > live: {entries:?}"
        );

        // The sibling worktree now ALSO has an open workspace: the
        // inventory entry must not produce a second, unopened row.
        drop(_isolated);
        let (_isolated2, store2) = store_with(project_file(&member_checkout));
        let mut app2 = AppState::test_new();
        app2.projects = store2;
        app2.workspaces = vec![ws_at(&member_checkout), ws_at(&sibling_worktree)];
        app2.worktree_inventory.insert(
            repo_identity,
            crate::app::state::RepoWorktreeInventory {
                worktrees: vec![crate::app::state::InventoryWorktree {
                    checkout_key: sibling_key,
                    branch: Some("feature/x".to_string()),
                    is_bare: false,
                    is_prunable: false,
                }],
                error: None,
            },
        );
        let entries2 = project_view_entries(&app2, false);
        let unopened_rows2: Vec<&str> = entries2
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::WorktreeRow {
                    checkout_key,
                    unopened: true,
                    ..
                } => Some(checkout_key.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            unopened_rows2.is_empty(),
            "an inventory worktree that IS open must not also render as unopened: {entries2:?}"
        );
        assert_eq!(
            worktree_rows(&entries2).len(),
            2,
            "both checkouts render as open rows, no duplicate: {entries2:?}"
        );

        std::fs::remove_dir_all(&member_checkout).unwrap();
        std::fs::remove_dir_all(&sibling_worktree).unwrap();
    }

    #[test]
    fn unopened_worktree_skips_bare_and_prunable_entries() {
        let member_checkout = temp_test_dir("unopened-skip-member");
        let origin = "git@github.com:owner/skip-repo.git";
        init_fake_git_repo(&member_checkout, Some(origin));

        let mut file = ProjectsFile::default();
        file.projects.insert(
            "proj".to_string(),
            project(vec![Member {
                dir: member_checkout.display().to_string(),
                worktrees: WorktreesScope::All,
                template: None,
            }]),
        );
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&member_checkout)];
        let repo_identity = app.workspaces[0].git_space().unwrap().repo_identity.clone();
        app.worktree_inventory.insert(
            repo_identity,
            crate::app::state::RepoWorktreeInventory {
                worktrees: vec![
                    crate::app::state::InventoryWorktree {
                        checkout_key: "/tmp/bora-fixture-bare.git".to_string(),
                        branch: None,
                        is_bare: true,
                        is_prunable: false,
                    },
                    crate::app::state::InventoryWorktree {
                        checkout_key: "/tmp/bora-fixture-prunable".to_string(),
                        branch: Some("stale".to_string()),
                        is_bare: false,
                        is_prunable: true,
                    },
                ],
                error: None,
            },
        );

        let entries = project_view_entries(&app, false);
        let unopened_count = entries
            .iter()
            .filter(|e| matches!(e, WorkspaceListEntry::WorktreeRow { unopened: true, .. }))
            .count();
        assert_eq!(
            unopened_count, 0,
            "bare and prunable inventory entries must not render a row: {entries:?}"
        );

        std::fs::remove_dir_all(&member_checkout).unwrap();
    }

    // ── bora-5ia: declarable `sections.order:` ────────────────────────────

    /// The `SectionHeader` kinds an entries slice carries, in render order.
    fn section_header_kinds(entries: &[WorkspaceListEntry]) -> Vec<ProjectSection> {
        entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionHeader { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect()
    }

    /// A project with all four bands declared (COMMANDS `dev`, CHECKS `gh`)
    /// plus an optional `sections.order:`.
    fn project_with_all_sections(members: Vec<Member>, order: Option<&[&str]>) -> Project {
        let mut project = project(members);
        project.sections = Some(Sections {
            checks: Some(vec!["gh".to_string()]),
            commands: Some(vec!["dev".to_string()]),
            order: order.map(|names| names.iter().map(ToString::to_string).collect()),
        });
        project
    }

    /// One workspace with every band's eligibility satisfied: a live
    /// pane-mode `dev` command, a failing check, plus project-level todos
    /// and notes seeded on `app` — so all four `SectionHeader`s render and
    /// order is the only thing left to observe.
    fn app_with_full_bands(
        repo: &std::path::Path,
        order: Option<&[&str]>,
    ) -> (IsolatedDirs, AppState) {
        let mut file = ProjectsFile::default();
        file.projects.insert(
            "proj".to_string(),
            project_with_all_sections(vec![member(repo)], order),
        );
        let (isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(repo);
        ws.cached_commands = Some(vec![bora_cmd(
            "dev",
            crate::bora_config::BoraCommandMode::Pane,
        )]);
        ws.cached_check_status = Some(checks_status(
            Some(1),
            vec![check_run("build", "COMPLETED", Some("FAILURE"))],
            None,
        ));
        app.workspaces = vec![ws];
        app.project_todos
            .insert("proj".to_string(), todos_summary(0, 1, &["ship it"]));
        app.project_notes
            .insert("proj".to_string(), vec!["notes".to_string()]);

        (isolated, app)
    }

    #[test]
    fn section_order_resolve_absent_matches_fixed_order() {
        assert_eq!(
            resolve_section_order(None),
            ProjectSection::ALL,
            "no declared order must resolve to today's fixed sequence"
        );
        assert_eq!(
            resolve_section_order(Some(&[])),
            ProjectSection::ALL,
            "an empty order: must resolve the same as an absent one"
        );
    }

    #[test]
    fn section_order_resolve_full_declaration_matches_declared_sequence() {
        let names = [
            "notes".to_string(),
            "pull_requests".to_string(),
            "todos".to_string(),
            "checks".to_string(),
            "commands".to_string(),
        ];
        assert_eq!(
            resolve_section_order(Some(&names)),
            [
                ProjectSection::Notes,
                ProjectSection::PullRequests,
                ProjectSection::Todos,
                ProjectSection::Checks,
                ProjectSection::Commands,
            ],
            "a full order (including the pull_requests wire name) must be honored exactly"
        );
    }

    #[test]
    fn section_order_resolve_partial_declaration_appends_unlisted_in_fixed_order() {
        let names = ["checks".to_string()];
        assert_eq!(
            resolve_section_order(Some(&names)),
            [
                ProjectSection::Checks,
                ProjectSection::Commands,
                ProjectSection::Todos,
                ProjectSection::Notes,
                ProjectSection::PullRequests,
            ],
            "the listed section leads, the other four follow in \
             ProjectSection::ALL order — nothing declared-but-unlisted is lost"
        );
    }

    #[test]
    fn section_order_resolve_unknown_name_ignored() {
        let names = [
            "checks".to_string(),
            "banana".to_string(),
            "notes".to_string(),
        ];
        assert_eq!(
            resolve_section_order(Some(&names)),
            [
                ProjectSection::Checks,
                ProjectSection::Notes,
                ProjectSection::Commands,
                ProjectSection::Todos,
                ProjectSection::PullRequests,
            ],
            "an unrecognized name is ignored, never an error, and never \
             consumes a slot"
        );
    }

    #[test]
    fn section_order_resolve_duplicate_name_honored_once_at_first_position() {
        let names = [
            "checks".to_string(),
            "checks".to_string(),
            "commands".to_string(),
        ];
        assert_eq!(
            resolve_section_order(Some(&names)),
            [
                ProjectSection::Checks,
                ProjectSection::Commands,
                ProjectSection::Todos,
                ProjectSection::Notes,
                ProjectSection::PullRequests,
            ],
            "a repeated name counts once, at its first position"
        );
    }

    #[test]
    fn section_order_wiring_reorders_rendered_bands() {
        let repo = temp_test_dir("section-order-full");
        init_fake_git_repo(&repo, None);

        let order = ["notes", "todos", "checks", "commands"];
        let (_isolated, app) = app_with_full_bands(&repo, Some(&order));

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_header_kinds(&entries),
            vec![
                ProjectSection::Notes,
                ProjectSection::Todos,
                ProjectSection::Checks,
                ProjectSection::Commands,
            ],
            "declared order threads through both the project-level (TODOS/\
             NOTES) and worktree-level (COMMANDS/CHECKS) bands: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn section_order_absent_matches_todays_default_rendered_sequence() {
        let repo = temp_test_dir("section-order-default");
        init_fake_git_repo(&repo, None);

        let (_isolated, app) = app_with_full_bands(&repo, None);

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_header_kinds(&entries),
            vec![
                ProjectSection::Todos,
                ProjectSection::Notes,
                ProjectSection::Commands,
                ProjectSection::Checks,
            ],
            "an undeclared order must render exactly today's fixed \
             sequence: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn section_order_listing_first_does_not_render_an_undeclared_section() {
        let repo = temp_test_dir("section-order-visibility");
        init_fake_git_repo(&repo, None);

        // The project declares only COMMANDS — CHECKS is undeclared — yet
        // `order:` lists CHECKS first, and the workspace carries real check
        // data that WOULD render a CHECKS band if eligibility were ever
        // bypassed.
        let mut file = ProjectsFile::default();
        let mut proj = project(vec![member(&repo)]);
        proj.sections = Some(Sections {
            checks: None,
            commands: Some(vec!["dev".to_string()]),
            order: Some(vec![
                "checks".to_string(),
                "commands".to_string(),
                "todos".to_string(),
                "notes".to_string(),
            ]),
        });
        file.projects.insert("proj".to_string(), proj);
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_commands = Some(vec![bora_cmd(
            "dev",
            crate::bora_config::BoraCommandMode::Pane,
        )]);
        ws.cached_check_status = Some(checks_status(
            Some(1),
            vec![check_run("build", "COMPLETED", Some("FAILURE"))],
            None,
        ));
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_header_kinds(&entries),
            vec![ProjectSection::Commands],
            "CHECKS listed first must still render nothing — it was never \
             declared, and ordering decides sequence, never visibility: \
             {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn section_order_to_yaml_round_trip_omits_absent_order() {
        let mut file = ProjectsFile::default();
        file.projects.insert(
            "alpha".to_string(),
            project_with_checks(vec![member(std::path::Path::new("/tmp/alpha"))]),
        );
        file.projects.insert(
            "zeta".to_string(),
            project_with_all_sections(
                vec![member(std::path::Path::new("/tmp/zeta"))],
                Some(&["checks", "notes"]),
            ),
        );

        let yaml = crate::persist::projects::to_yaml(&file).expect("serializes");
        let alpha_doc = yaml
            .split("zeta:")
            .next()
            .expect("alpha project serializes before zeta");
        assert!(
            !alpha_doc.contains("order:"),
            "a project without a declared order must round-trip without an \
             `order:` key: {yaml}"
        );
        assert!(
            yaml.contains("order:"),
            "a project WITH a declared order must serialize the key: {yaml}"
        );

        let reparsed = crate::persist::projects::parse_projects_yaml(&yaml).expect("reparses");
        assert_eq!(
            reparsed
                .projects
                .get("zeta")
                .and_then(|p| p.sections.as_ref())
                .and_then(|s| s.order.as_deref()),
            Some(&["checks".to_string(), "notes".to_string()][..]),
            "the declared order round-trips through YAML unchanged"
        );
    }

    // ── bora-yw6.2: project-level PULL REQUESTS band ─────────────────────

    fn open_pr(
        number: u64,
        head_ref: &str,
        is_draft: bool,
        checks: Option<crate::workspace::ChecksRollup>,
    ) -> crate::workspace::OpenPr {
        crate::workspace::OpenPr {
            number,
            title: format!("pr {number}"),
            url: format!("https://github.com/owner/repo/pull/{number}"),
            head_ref_name: head_ref.to_string(),
            is_draft,
            mergeable: None,
            checks,
        }
    }

    fn repo_open_prs(prs: Vec<crate::workspace::OpenPr>) -> crate::workspace::RepoOpenPrs {
        crate::workspace::RepoOpenPrs { prs, error: None }
    }

    /// The `(number, head_ref)` of every `PrRow` entry, in order.
    fn pull_requests_rows(entries: &[WorkspaceListEntry]) -> Vec<(u64, String)> {
        entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::PrRow {
                    number, head_ref, ..
                } => Some((*number, head_ref.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn pull_requests_section_absent_without_any_cached_repo_data() {
        let repo = temp_test_dir("prs-absent");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, ProjectSection::PullRequests),
            None,
            "no cached repo_open_prs data at all -> no band (rule 5): {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn pull_requests_section_lists_only_prs_without_a_local_worktree() {
        // G5: a PR whose head branch already has a local worktree — open OR
        // on-disk-unopened — must be omitted, tested both ways.
        let repo = temp_test_dir("prs-local-worktree");
        init_fake_git_repo(&repo, None);
        let identity = crate::workspace::git_space_metadata(&repo)
            .expect("fixture must resolve")
            .repo_identity;

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_git_branch = Some("main".to_string());
        app.workspaces = vec![ws];
        app.worktree_inventory.insert(
            identity.clone(),
            crate::app::state::RepoWorktreeInventory {
                worktrees: vec![crate::app::state::InventoryWorktree {
                    checkout_key: "unopened-checkout".to_string(),
                    branch: Some("feat/unopened".to_string()),
                    is_bare: false,
                    is_prunable: false,
                }],
                error: None,
            },
        );
        app.repo_open_prs.insert(
            identity,
            repo_open_prs(vec![
                open_pr(1, "main", false, None),
                open_pr(
                    2,
                    "feat/other",
                    false,
                    Some(crate::workspace::ChecksRollup::Passing),
                ),
                open_pr(3, "feat/unopened", false, None),
            ]),
        );

        let entries = project_view_entries(&app, false);
        assert_eq!(
            pull_requests_rows(&entries),
            vec![(2, "feat/other".to_string())],
            "PR #1 (open worktree branch) and PR #3 (unopened-on-disk worktree \
             branch) must both be omitted; only PR #2 has no local worktree at \
             all: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn pull_requests_section_renders_provider_error_as_a_row() {
        let repo = temp_test_dir("prs-error");
        init_fake_git_repo(&repo, None);
        let identity = crate::workspace::git_space_metadata(&repo)
            .expect("fixture must resolve")
            .repo_identity;

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];
        app.repo_open_prs.insert(
            identity,
            crate::workspace::RepoOpenPrs {
                prs: Vec::new(),
                error: Some("gh: not logged in".to_string()),
            },
        );

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, ProjectSection::PullRequests),
            Some((0, 0)),
            "an errored band still renders its header: {entries:?}"
        );
        assert_eq!(
            section_items(&entries, ProjectSection::PullRequests),
            vec!["gh: not logged in".to_string()],
            "a provider error is a visible row, never silently empty: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn pull_requests_section_absent_when_every_pr_has_a_local_worktree() {
        let repo = temp_test_dir("prs-empty");
        init_fake_git_repo(&repo, None);
        let identity = crate::workspace::git_space_metadata(&repo)
            .expect("fixture must resolve")
            .repo_identity;

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&repo);
        ws.cached_git_branch = Some("main".to_string());
        app.workspaces = vec![ws];
        app.repo_open_prs.insert(
            identity,
            repo_open_prs(vec![open_pr(1, "main", false, None)]),
        );

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, ProjectSection::PullRequests),
            None,
            "every cached PR has a local worktree -> zero rows -> no band (rule 5): {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn section_order_wiring_reorders_pull_requests_band() {
        let repo = temp_test_dir("prs-order");
        init_fake_git_repo(&repo, None);
        let identity = crate::workspace::git_space_metadata(&repo)
            .expect("fixture must resolve")
            .repo_identity;

        let order = ["pull_requests", "notes", "todos", "checks", "commands"];
        let (_isolated, mut app) = app_with_full_bands(&repo, Some(&order));
        app.repo_open_prs.insert(
            identity,
            repo_open_prs(vec![open_pr(
                9,
                "feat/x",
                false,
                Some(crate::workspace::ChecksRollup::Failing),
            )]),
        );

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_header_kinds(&entries),
            vec![
                ProjectSection::PullRequests,
                ProjectSection::Notes,
                ProjectSection::Todos,
                ProjectSection::Checks,
                ProjectSection::Commands,
            ],
            "declared order threads PULL REQUESTS through the project-level group too: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn workspace_list_lockstep_pull_requests_agree_across_passes() {
        // G4: PrRow flows through the real pipeline exactly like every other
        // row — `workspace_list_entries` (view-mode dispatch) into all three
        // lockstep passes named at sidebar.rs's "Shared row-height" doc.
        let repo = temp_test_dir("prs-full-lockstep");
        init_fake_git_repo(&repo, None);
        let identity = crate::workspace::git_space_metadata(&repo)
            .expect("fixture must resolve")
            .repo_identity;

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.view_mode = crate::config::ViewMode::Project;

        let mut ws = ws_at(&repo);
        ws.cached_git_branch = Some("main".to_string());
        app.workspaces = vec![ws];
        app.repo_open_prs.insert(
            identity,
            repo_open_prs(vec![open_pr(
                7,
                "feat/lockstep",
                true,
                Some(crate::workspace::ChecksRollup::Failing),
            )]),
        );
        app.ensure_test_terminals();

        let entries = crate::ui::sidebar::workspace_list_entries(&app);
        let pr_row_idx = entries
            .iter()
            .position(|e| matches!(e, WorkspaceListEntry::PrRow { number: 7, .. }))
            .expect("fixture must reach the PrRow through the real view-mode dispatch");
        let worktree_idx = entries
            .iter()
            .position(|e| matches!(e, WorkspaceListEntry::WorktreeRow { .. }))
            .expect("fixture must still emit the worktree row after the PR band");
        assert_eq!(
            worktree_idx,
            pr_row_idx + 1,
            "the WorktreeRow must land immediately after the PrRow: {entries:?}"
        );

        // Pass 1: height.
        let total_height: u16 = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| crate::ui::sidebar::entry_row_height(entry, &entries, idx))
            .sum();
        assert_eq!(
            total_height as usize,
            entries.len(),
            "every row in this fixture is height 1"
        );

        // Pass 2: visible-count agrees with the height pass.
        let width = 60;
        let exact = ratatui::layout::Rect::new(
            0,
            0,
            width,
            total_height + crate::ui::sidebar::WORKSPACE_SECTION_HEADER_ROWS + 1,
        );
        assert_eq!(
            crate::ui::sidebar::workspace_list_visible_count(&app, exact, 0),
            entries.len(),
            "visible-count pass must agree with the height pass"
        );

        // Pass 3: geometry — the WorktreeRow's hit area sits exactly one row
        // below where the PrRow sits, which only holds if the geometry walk
        // advanced `row_y` by the PrRow's `entry_row_height` instead of
        // silently stalling on its row.
        let sidebar_area = ratatui::layout::Rect::new(
            0,
            0,
            width,
            total_height + crate::ui::sidebar::WORKSPACE_SECTION_HEADER_ROWS + 20,
        );
        let (_, _, project_rows) =
            crate::ui::sidebar::compute_workspace_list_areas_all(&app, sidebar_area);
        let ws_area =
            crate::ui::sidebar::workspace_list_rect(sidebar_area, app.sidebar_section_split);
        let body = crate::ui::sidebar::workspace_list_body_rect(&app, ws_area, false);
        // The PrRow is clickable: this fixture's repo has an open workspace, so
        // the band resolved a representative `ws_idx` and the row carries an
        // `OpenPr` target. This assertion replaces an earlier one that required
        // NO hit area, which was correct only while the click path was
        // deliberately unwired — asserting it still is would now pin the row to
        // rendering and doing nothing.
        let pr_area = project_rows
            .iter()
            .find(|area| area.rect.y == body.y + pr_row_idx as u16)
            .expect("the PrRow must get a hit area now that the click path exists");
        assert_eq!(
            pr_area.target,
            crate::app::state::ProjectRowTarget::OpenPr {
                ws_idx: 0,
                number: 7
            },
            "the PrRow's hit area must open PR #7 in a worktree, naming a \
             workspace of its own repo: {project_rows:?}"
        );
        let worktree_area = project_rows
            .iter()
            .find(|area| {
                matches!(
                    area.target,
                    crate::app::state::ProjectRowTarget::Worktree { .. }
                )
            })
            .expect("worktree row must still get a hit area");
        assert_eq!(
            worktree_area.rect.y,
            body.y + worktree_idx as u16,
            "the WorktreeRow's hit area must be pushed down by exactly the PrRow's row height"
        );

        // Pass 4: render — the PR number and title text land at the PrRow's
        // prefix-sum row.
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(
            exact.width,
            exact.height,
        ))
        .expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        terminal
            .draw(|frame| {
                crate::ui::sidebar::render_workspace_list(&app, &runtimes, frame, exact, false)
            })
            .expect("workspace list should render");
        let row_text = |row: u16| -> String {
            (0..exact.width)
                .map(|col| terminal.backend().buffer()[(col, row)].symbol().to_string())
                .collect()
        };
        let render_y = crate::ui::sidebar::WORKSPACE_SECTION_HEADER_ROWS + pr_row_idx as u16;
        let text = row_text(render_y);
        assert!(
            text.contains('7') && text.contains("pr 7"),
            "PR number and title must render at the row the height/geometry passes agree on: {text:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }
}
