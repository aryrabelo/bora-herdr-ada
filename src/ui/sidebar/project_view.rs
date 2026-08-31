//! The Project view's entry model: `ViewMode::Project`.
//!
//! Three levels and only three — project → worktree → workspace — plus the
//! section bands that hang off a worktree and, for each workspace, one
//! `PaneDotsRow` — a dot per pane, never a per-pane row.
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

use super::sections::{Section, SectionChild, SectionKind};
use super::{SectionBullet, SectionCounter, SectionDescriptor, SectionLevel, WorkspaceListEntry};
use crate::app::state::AppState;
use crate::persist::projects::{ResolvedMember, WorktreesScope};
use crate::workspace::Workspace;

/// Display name for the trailing implicit group holding workspaces that
/// match no declared project member (rule 4, `WorkspaceListEntry::ProjectRow`
/// doc). Never collides with a user's project `name:` — those come from
/// `projects.yml` and are rendered exactly as the user typed them.
const ORPHANS_NAME: &str = "Ungrouped";
/// Collapse key for the orphans group. Namespaced like every other key this
/// module emits (`proj:`, `wt:`) so it can never collide with a real project
/// slug's `proj:<slug>` key.
pub(crate) const ORPHANS_COLLAPSE_KEY: &str = "proj:__orphans__";

/// Build the Project-view entry list.
///
/// Contract with the row painters in `super`:
///
/// - Every variant's OWN content is height 1; `entry_row_height` may add a
///   trailing row_gap after the `PaneDotsRow` of a workspace block
///   (bora-c1h G7, T7 bora-79l divergence C: the gap separates BRANCH
///   GROUPS, never sibling workspaces of one branch) — that gap is a
///   property of ADJACENCY between two entries, computed identically by
///   all three lockstep passes (`sidebar.rs` "Shared row-height"), never
///   a per-variant constant, so it can never silently desync them.
/// - Order is depth-first and stable: `ProjectRow`, then ONE
///   `SectionRow` per BRANCH GROUP (6a, bora-79l.10 — repo identity +
///   branch, the same key T4's "+" uses), header at the TOP of the
///   group, followed by every member workspace's `PaneDotsRow`
///   contiguously (one dot per pane, always emitted even for a
///   single-pane workspace: the old per-pane `PaneRow` this replaced
///   was removed once Project view stopped emitting it, see
///   `push_pane_dots_row`'s doc) — and, after EVERY group's sessions,
///   the worktree's COMMANDS/CHECKS bands close the project group (T7
///   bora-79l divergence D: `push_worktree_bands`, anchored on the
///   first checkout only, one fetch covering every sibling workspace
///   on the branch).
/// - `apply_hidden_filter`'s depth ladder (0 project, 1 section, 2 band
///   header, 3 band item, 4 pane) mirrors that nesting — 6a keeps the
///   ladder but the section row is a CONTAINER now and the member block
///   is the workspace child. A new level means updating that closure
///   too.
/// - T3 (bora-79l): the branch line is a DECLARED header, not an
///   identity row — it carries no workspace/repo name at all (each
///   member's name lives on its own `PaneDotsRow` l1), and it obeys the
///   section model: `header_on` and `parts.diff` are read from the
///   project's `layout:` at emission (`section_model_flags`, matched on
///   the representative's checkout), defaulting ON when no layout
///   exists yet. The same-branch exception
///   (`hide_upper_duplicate_branch_headers`) survives 6a as the guard
///   for the rare STACKED runtime-sections case (6b's world); in this
///   emitter's one-section-per-branch shape the header simply sits at
///   the group's top.
/// - `PaneDotsRow.name` is set exactly once, at the end of emission,
///   by `disambiguate_pane_dots_names` — never recomputed
///   independently, so two blocks of one set can never disagree about
///   a member's name.
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

    // Pass 1 (C2, the actual P0 fix): a workspace explicitly bound
    // (`Workspace::project()`) to a slug still declared in `projects.yml`
    // claims that project outright, before any directory matching runs.
    // This is the only way the data model can express "workspace W
    // belongs to project B" when project A ALSO declares B's directory as
    // a member — directory alone can never answer that question once two
    // projects share it. A binding naming a slug that no longer exists is
    // never trusted blindly: it is looked up against `file.projects` here
    // and, on a miss, simply left unclaimed so pass 2 derives it like any
    // other workspace instead of orphaning it or panicking.
    let mut assigned: HashMap<&str, Vec<usize>> = HashMap::new();
    for idx in 0..app.workspaces.len() {
        if let Some(slug) = app.workspaces[idx].project() {
            if let Some((canonical_slug, _)) = file.projects.get_key_value(slug) {
                assigned
                    .entry(canonical_slug.as_str())
                    .or_default()
                    .push(idx);
                claimed.insert(idx);
            }
        }
    }

    // Pass 2: today's directory-derived membership, for every workspace
    // pass 1 left unclaimed. `best[idx]` tracks the single best-matching
    // project found so far for workspace `idx` as one
    // `((specificity), slug)` pair, in a `Vec` sized once up front (never
    // a per-(project, workspace) allocation). Projects are visited in
    // `file.projects`'s BTreeMap (slug-alphabetical) order, and a
    // candidate only overwrites the recorded best on a STRICTLY higher
    // `member_specificity` score, so the first slug encountered keeps any
    // tie — exactly the C2 tiebreak: most specific member first, slug
    // order last.
    let mut best: Vec<Option<((u8, usize), &str)>> = vec![None; app.workspaces.len()];
    for slug in file.projects.keys() {
        // Already resolved off the tick: no disk touched on this path.
        // Called exactly once per project, never per workspace.
        let members = app.projects.resolved_members(slug);
        for (idx, slot) in best.iter_mut().enumerate() {
            if claimed.contains(&idx) {
                continue;
            }
            let ws = &app.workspaces[idx];
            let candidate = members
                .iter()
                .filter(|member| workspace_matches_member(ws, member))
                .map(member_specificity)
                .max();
            let Some(score) = candidate else { continue };
            let better = match slot {
                Some((current, _)) => score > *current,
                None => true,
            };
            if better {
                *slot = Some((score, slug.as_str()));
            }
        }
    }
    for (idx, slot) in best.iter().enumerate() {
        if let Some((_, slug)) = slot {
            assigned.entry(*slug).or_default().push(idx);
            claimed.insert(idx);
        }
    }

    // `ProjectsFile::projects` is a `BTreeMap`, so iterating it already
    // yields a stable, deterministic (slug-alphabetical) order across
    // renders — exactly what rule 8 requires for sibling project rows.
    for (slug, project) in &file.projects {
        let ws_idxs = assigned.remove(slug.as_str()).unwrap_or_default();
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
            // T3: the declared layout is the section model's source —
            // header switches and parts toggles ride on it.
            project.layout.as_deref(),
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
            // sections, no layout. `resolve_section_order(None)` is the
            // default registry order.
            resolve_section_order(None),
            &[],
            &[],
            None,
            force_expanded,
        );
    }

    // bora-b2r parity for the merged row (bora-c1h), 6a group shape:
    // two member BLOCKS that would render IDENTICAL (same display name)
    // each get a hint so they never collapse into one indistinguishable
    // row on screen. The branch text used to be trusted to tell two
    // same-named rows apart on its own, but that text renders dim, so a
    // name collision stays ambiguous to the eye even when the branches
    // differ (owner report) — the branch is now the FIRST hint tried
    // (shortest, already-meaningful token), falling back to a parent-dir
    // hint only when the branch doesn't separate the set either. Scoped
    // per project group: two workspaces of the same name in two
    // DIFFERENT groups are not ambiguous, since their own project
    // headers already tell them apart. 6a: the hint lands on the
    // `PaneDotsRow`'s own name — every member renders a block, and the
    // section header carries no name slot to hint (T3).
    disambiguate_pane_dots_names(app, &mut entries);

    entries
}

/// The base display name every `PaneDotsRow` l1 carries and the key
/// `disambiguate_pane_dots_names` collides on — one function, so the
/// collision detector and the renderer can never derive a different
/// notion of "this workspace's name". Custom name first, else the
/// cached auto label, else the repo name (the repo-name fallback is a
/// fixture/cold-cache path: `cached_auto_label` is empty only before
/// git-identity discovery has run).
pub(super) fn workspace_group_name(ws: &Workspace) -> String {
    ws.custom_name.clone().unwrap_or_else(|| {
        if ws.cached_auto_label.is_empty() {
            ws.cached_git_space
                .as_ref()
                .map(|space| space.repo_name.clone())
                .unwrap_or_default()
        } else {
            ws.cached_auto_label.clone()
        }
    })
}

/// Sets every `PaneDotsRow`'s final display name (6a): the base name
/// (`workspace_group_name`) plus a disambiguating hint for every set of
/// member blocks, WITHIN one project group, that would RENDER identical —
/// same display name (as far as emission can tell without the runtime
/// registry: custom name, else the cached auto label, else the repo
/// name). Before 6a the hint lived on a `SectionRow.name_hint` and was
/// copied onto the paired block (`sync_pane_dots_names`); with ONE
/// section row per group there is no per-workspace row to hint anymore,
/// so the name is decided where it is rendered, in ONE pass — the
/// section header prints no name (T3), which makes the block l1 the
/// shape's only renderer.
///
/// A shared branch no longer exempts a set from needing a hint (owner
/// report: the branch text is dim enough on screen that an identical
/// name is still ambiguous) — the branch is instead the first hint
/// tried, since it's the shortest token that is both meaningful and
/// already computed: when every block in the colliding set has a
/// distinct branch, the name becomes `"name (main)"` /
/// `"name (feat/x)"` directly (branches only differ ACROSS groups —
/// one group is one branch). Only when branches don't fully separate
/// the set (the bora-b2r case: same repo, same branch, two
/// workspaces) does this fall back to the member dir's basename, or
/// its parent's when the basename IS the repo name (linked worktree
/// checkouts like `worktree-a/myrepo`), so identical rows become
/// "name (worktree-a)" / "name (worktree-b)". Two blocks on the same
/// checkout with the same display name cannot be separated by either
/// hint and stay identical — the same residual the SR-per-workspace
/// disambiguator had.
fn disambiguate_pane_dots_names(app: &AppState, entries: &mut [WorkspaceListEntry]) {
    // `group_id` increments at every `ProjectRow`, so the collision key
    // below (`(group_id, name)`) never merges two same-named workspaces
    // that live under two DIFFERENT project headers — those are already
    // told apart by the header itself, and hinting them would be noise
    // (module doc, "only disambiguate rows that actually collide").
    let mut group_id = 0usize;
    let mut groups: HashMap<(usize, String), Vec<usize>> = HashMap::new();
    for (pos, entry) in entries.iter().enumerate() {
        if matches!(entry, WorkspaceListEntry::ProjectRow { .. }) {
            group_id += 1;
        }
        let WorkspaceListEntry::PaneDotsRow { ws_idx, .. } = entry else {
            continue;
        };
        let Some(ws) = app.workspaces.get(*ws_idx) else {
            continue;
        };
        groups
            .entry((group_id, workspace_group_name(ws)))
            .or_default()
            .push(pos);
    }
    // `hints` maps a colliding block's position to the hint it renders.
    let mut hints: HashMap<usize, String> = HashMap::new();
    for positions in groups.values().filter(|positions| positions.len() > 1) {
        // First disambiguator: the branch itself, when it alone
        // separates every block in the set.
        let branches: Vec<Option<String>> = positions
            .iter()
            .map(|&pos| {
                let WorkspaceListEntry::PaneDotsRow { ws_idx, .. } = &entries[pos] else {
                    return None;
                };
                app.workspaces.get(*ws_idx).and_then(Workspace::branch)
            })
            .collect();
        let mut seen_branches = HashSet::new();
        let branches_distinct = branches.iter().all(Option::is_some)
            && branches.iter().all(|branch| seen_branches.insert(branch));
        if branches_distinct {
            for (&pos, branch) in positions.iter().zip(branches) {
                if let Some(branch) = branch {
                    hints.insert(pos, branch);
                }
            }
            continue;
        }

        let dirs: Vec<String> = positions
            .iter()
            .map(|&pos| {
                let WorkspaceListEntry::PaneDotsRow { ws_idx, .. } = &entries[pos] else {
                    return String::new();
                };
                app.workspaces
                    .get(*ws_idx)
                    .map(crate::workspace::Workspace::project_member_dir)
                    .unwrap_or_default()
            })
            .collect();
        // Walk up from the basename until every block in the set differs:
        // depth 0 handles `multi-repo-x` vs `multi-repo-y`, depth 1 handles
        // `worktree-a/myrepo` vs `worktree-b/myrepo`.
        for depth in 0..4 {
            let candidates: Vec<Option<String>> = dirs
                .iter()
                .map(|dir| {
                    std::path::Path::new(dir)
                        .components()
                        .rev()
                        .nth(depth)
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                })
                .collect();
            let mut seen = HashSet::new();
            let distinct =
                candidates.iter().all(Option::is_some) && candidates.iter().all(|c| seen.insert(c));
            if !distinct {
                continue;
            }
            for (&pos, hint) in positions.iter().zip(candidates) {
                if let Some(hint) = hint {
                    hints.insert(pos, hint);
                }
            }
            break;
        }
    }
    // The single write: every block's name is final here — base, or
    // base plus its hint. Never recomputed anywhere else.
    for (pos, entry) in entries.iter_mut().enumerate() {
        let WorkspaceListEntry::PaneDotsRow { ws_idx, name, .. } = entry else {
            continue;
        };
        let Some(ws) = app.workspaces.get(*ws_idx) else {
            continue;
        };
        let base = workspace_group_name(ws);
        *name = match hints.get(&pos) {
            Some(hint) => format!("{base} ({hint})"),
            None => base,
        };
    }
}

/// Collapse key of a branch group's `SectionRow` (6a). Session-stable
/// and namespaced (`wsec:`) so it can never collide with a project's
/// `proj:` key or a band's `sec:` key. Built from the REPRESENTATIVE
/// member's `ws_idx` (the first of the group), so one toggle folds the
/// whole group's blocks — collapse is per SECTION now, one per branch
/// group. Before 6a it was per workspace (`wsec:{ws_idx}` keyed every
/// workspace's own row); before bora-c1h it was checkout-scoped.
fn section_collapse_key(ws_idx: usize) -> String {
    format!("wsec:{ws_idx}")
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

/// Ranks how specific `member`'s match is, for `project_view_entries`'s
/// pass-2 same-directory tiebreak (C2): when two or more declared
/// projects' members resolve to the same checkout and neither claiming
/// workspace carries an explicit `Workspace::project()` binding, the most
/// specific matching member decides which project wins, not iteration
/// order. Compared via Rust's derived tuple ordering — the first
/// component decides, the second only breaks a tie within it:
///
/// - `WorktreesScope::This` (pinned to exactly one checkout) always
///   outranks `All` (every worktree of the repo): a member that named
///   this exact checkout is a stronger signal than one that only covers
///   it as a side effect of covering the whole repo.
/// - Within the same scope, a deeper `subdir` (more path components) beats
///   a shallower one: the member that named the more specific slice of
///   the checkout wins.
///
/// Kept separate from `workspace_matches_member`'s boolean rather than
/// folded into it: that predicate answers "does this match" and every
/// existing caller of it wants exactly that, not a secretly-ranked bool a
/// future reader has to reverse-engineer.
fn member_specificity(member: &ResolvedMember) -> (u8, usize) {
    let scope_rank = match member.worktrees {
        WorktreesScope::This => 1,
        WorktreesScope::All => 0,
    };
    (scope_rank, member.subdir.components().count())
}

/// Push one `ProjectRow` (declared project or the trailing orphans group)
/// and its `WorktreeRow`/`SectionRow`/`PaneDotsRow` descendants.
// T3 pushed this past clippy's arity cap: `layout` is the section model the
// emission consumes and every other slot is a distinct group input — folding
// them into a params struct would only move the list one level down.
#[allow(clippy::too_many_arguments)]
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
    section_order: [&'static SectionDescriptor; SECTION_COUNT],
    commands: &[String],
    checks: &[String],
    // T3: the project's declared `layout:` — the section model
    // (`Section.header_on` / `SectionParts.diff`) consumed at emission.
    // `None` (no layout yet, or the orphans group) means every default.
    layout: Option<&[Section]>,
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

    // T3: everything this group pushes sits in `entries[group_start..]`,
    // which is exactly the slice the same-branch exception pass below
    // needs — per project group, never crossing into the next one.
    let group_start = entries.len();
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
        for section in filter_by_level(&section_order, SectionLevel::Project) {
            let ctx = SectionPushCtx::Project {
                app,
                slug,
                local_branches: &local_branches,
                force_expanded,
            };
            (section.push)(entries, &ctx);
        }
    }

    // T6 6a (bora-79l.10): sections are keyed by BRANCH GROUP, not by
    // checkout — every workspace of one (repo, branch) lands in ONE
    // `SectionRow` (the container), header at the top, the members'
    // blocks contiguous below. First-seen order over `ws_idxs` keeps
    // this stable across renders without an extra sort. The checkout
    // map built above survives because the bands (`push_worktree_
    // bands`), the unopened rows and the PULL REQUESTS filter are all
    // still checkout-shaped concerns.
    let mut group_order: Vec<String> = Vec::new();
    let mut by_branch_group: HashMap<String, Vec<usize>> = HashMap::new();
    for &idx in &ws_idxs {
        let key = branch_group_key(&app.workspaces[idx]);
        if !by_branch_group.contains_key(&key) {
            group_order.push(key.clone());
        }
        by_branch_group.entry(key).or_default().push(idx);
    }
    for group_key in &group_order {
        push_worktree(
            entries,
            app,
            group_key,
            &by_branch_group[group_key],
            layout,
            force_expanded,
        );
    }

    // T7 (bora-79l, divergence D): the worktree bands close the group,
    // after every checkout's sessions — the alvo's `COMANDO`/`CHECKS`
    // rows sit below the last branch group, never above the first.
    push_worktree_bands(
        entries,
        app,
        &order,
        &by_checkout,
        section_order,
        commands,
        checks,
        force_expanded,
    );

    // T6 6b (bora-79l.10): every DECLARED non-Branch section from the
    // project's `layout:`, in declaration order — project-scoped, not
    // tied to any one checkout, so it closes the group exactly once,
    // right after the branch groups and their worktree bands (T7
    // divergence D's "after every group's sessions" extended to the
    // whole project group).
    if let Some(layout) = layout {
        push_declared_sections(entries, layout, app, force_expanded);
    }

    // T3 same-branch exception: with every section of this group on the
    // table, hide every upper header that a lower same-(repo, branch)
    // header would duplicate.
    hide_upper_duplicate_branch_headers(&mut entries[group_start..], app);

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
            repo: repo_name_for_identity(app, &entry.repo_identity),
            branch: entry.branch.clone(),
            ahead: 0,
            behind: 0,
            pr: None,
            collapse_key: format!("unopened:{}", entry.checkout_key),
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

/// The repo-level identity used by the same-branch exception's collision
/// key (T3) — `repo_identity` when the workspace has git identity (shared
/// by every clone and worktree of one repository, `GitSpaceMetadata`'s
/// doc), else a synthetic per-workspace fallback so two identity-less
/// workspaces never collide and wrongly hide each other's header. Reuses
/// `checkout_group_key`'s existing fallback rather than inventing a second
/// one.
fn repo_identity_key(ws: &Workspace) -> String {
    ws.git_space()
        .map(|space| space.repo_identity.clone())
        .unwrap_or_else(|| checkout_group_key(ws))
}

/// T3/T6 6b: the section model's flags for one workspace, consumed at
/// emission — `(header_on, parts.diff, parts.dots)` of the Branch
/// `Section` whose children name this workspace's checkout. Matching key
/// is `SectionChild::Workspace::checkout` against the workspace's
/// `checkout_key`, the same key `persist::restore::
/// reconcile_section_layout` matches saved sections by, so the runtime
/// render and the persisted layout cannot disagree about which section a
/// workspace belongs to. Unmatched — no `layout:` declared yet (every
/// project today), or the workspace sits in no saved section — yields
/// `(true, true, true)`: the pre-model behavior, byte for byte.
///
/// `parts.dots` (T6 6b) is the l2 pane-dots line toggle: OFF collapses a
/// `PaneDotsRow` block to its name line alone. `push_pane_dots_row`
/// carries the flag straight onto the entry, so `entry_row_height` (and
/// the two other lockstep passes) read the toggle off the row itself
/// rather than re-deriving it from the model a second time.
fn section_model_flags(layout: Option<&[Section]>, ws: &Workspace) -> (bool, bool, bool) {
    let Some(sections) = layout else {
        return (true, true, true);
    };
    let checkout = checkout_group_key(ws);
    sections
        .iter()
        .filter(|section| section.kind == SectionKind::Branch)
        .find(|section| {
            section.children.iter().any(|child| {
                matches!(
                    child,
                    SectionChild::Workspace { checkout: key, .. } if *key == checkout
                )
            })
        })
        .map(|section| (section.header_on, section.parts.diff, section.parts.dots))
        .unwrap_or((true, true, true))
}

/// T3 same-branch exception, scoped to ONE project group's entries: a
/// branch header hides when a LOWER header of the same (repo identity,
/// branch) exists in the group — the lower one keeps its line, so two
/// headers of one branch never coexist visible (the owner's rule: the
/// header below hides the one above). Keyed on repo identity AND branch
/// name: branch `main` of repo A and branch `main` of repo B in one group
/// are different sections and both keep their headers. Headers the model
/// already turned OFF (`header_on: false`) render nothing and therefore
/// cannot hide anyone else's. Backward scan: the LAST row of a key claims
/// it, every earlier duplicate is marked hidden.
///
/// 6a narrowed where this can fire: emission groups by `branch_group`,
/// so this emitter's one-section-per-branch shape can never stack two
/// sections of one branch — the pass stays as the guard for 6b's
/// runtime sections (a user CAN stack two sections naming the same
/// branch once sections are montáveis), and the normal case is simply
/// "header at the top of its group".
fn hide_upper_duplicate_branch_headers(entries: &mut [WorkspaceListEntry], app: &AppState) {
    let mut seen_below: HashSet<(String, String)> = HashSet::new();
    for entry in entries.iter_mut().rev() {
        let WorkspaceListEntry::SectionRow {
            ws_idx,
            header_on,
            header_hidden,
            ..
        } = entry
        else {
            continue;
        };
        if !*header_on {
            continue;
        }
        let Some(ws) = app.workspaces.get(*ws_idx) else {
            continue;
        };
        let Some(branch) = ws.branch() else {
            continue;
        };
        if !seen_below.insert((repo_identity_key(ws), branch)) {
            *header_hidden = true;
        }
    }
}

/// T7 (bora-79l, divergence D): the worktree bands render after EVERY
/// session of the project group, not interleaved under the first
/// workspace's section (ALVO_CAPTURE rows 31-34: `COMANDO`/`CHECKS` follow
/// the last branch group). Still anchored on the FIRST checkout only —
/// one fetch covers every sibling workspace on the branch, the same
/// representative-workspace rule `push_checks_section` always had — and
/// still gated on that checkout's first section being expanded, the exact
/// collapse behavior the interleaved emission had.
fn push_worktree_bands(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    order: &[String],
    by_checkout: &HashMap<String, Vec<usize>>,
    section_order: [&'static SectionDescriptor; SECTION_COUNT],
    commands: &[String],
    checks: &[String],
    force_expanded: bool,
) {
    let Some(first_checkout) = order.first() else {
        return;
    };
    let Some(ws_idxs) = by_checkout.get(first_checkout) else {
        return;
    };
    let Some(&first_ws) = ws_idxs.first() else {
        return;
    };
    if !force_expanded
        && app
            .collapsed_space_keys
            .contains(&section_collapse_key(first_ws))
    {
        return;
    }
    for section in filter_by_level(&section_order, SectionLevel::Worktree) {
        let ctx = SectionPushCtx::Worktree {
            app,
            checkout_key: first_checkout,
            ws_idxs,
            commands,
            checks,
            force_expanded,
        };
        (section.push)(entries, &ctx);
    }
}

/// T7 (bora-79l, divergence C): the branch-GROUP key — `(repo identity,
/// branch)` flattened into one comparable string. The same pairing the
/// same-branch exception keys on (`hide_upper_duplicate_branch_headers`);
/// extracted here so "these two sections belong to one branch group" can
/// never mean two different things in the two rules. `project_view_
/// trailing_gap` reads it off consecutive `SectionRow`s to decide where
/// the blank separator row goes (blank between GROUPS, never between
/// sibling workspaces of one branch).
///
/// 6a: a workspace with no cached branch yet cannot PROVE it shares a
/// branch with anything, so it never merges across checkouts — its
/// group degenerates to its own checkout (same checkout still merges:
/// one HEAD, one branch, whatever it is). The same conservative rule
/// the same-branch exception always had (`None` never participates).
fn branch_group_key(ws: &Workspace) -> String {
    let branch = ws.branch();
    format!(
        "{}\u{1f}{}",
        repo_identity_key(ws),
        branch.unwrap_or_else(|| checkout_group_key(ws))
    )
}

/// The group's `+N −M` cluster: the SUM of every member's uncommitted
/// change set (6a — the header speaks for the whole branch group, so a
/// single member's numbers would undercount it). `None` when no member
/// counted anything. Folded once here at emission, never per render:
/// membership is emission-time knowledge the renderer does not have.
fn branch_group_diff(app: &AppState, ws_idxs: &[usize]) -> Option<(u32, u32)> {
    let mut added = 0u32;
    let mut removed = 0u32;
    let mut any = false;
    for &idx in ws_idxs {
        if let Some((a, r)) = app
            .workspaces
            .get(idx)
            .and_then(super::workspace_diff_counts)
        {
            added = added.saturating_add(a);
            removed = removed.saturating_add(r);
            any = true;
        }
    }
    any.then_some((added, removed))
}

/// Push ONE `SectionRow` for this branch group (6a, bora-79l.10) — the
/// group's container row, header at the TOP — followed by every member
/// workspace's `PaneDotsRow` block, contiguous (ALVO_CAPTURE rows 03-07:
/// the `main` header, then `main`, then `main-review`). Before 6a this
/// pushed one `SectionRow` per workspace and the same-branch exception
/// pushed the lone visible header BETWEEN the blocks; the group shape
/// kills that "generic-row" problem without a new variant.
///
/// The row's per-workspace fields name the REPRESENTATIVE — the FIRST
/// member: `ws_idx` (git/PR/checks render state, `SectionRow`'s doc),
/// `checkout_key`, and the `wsec:{ws_idx}` collapse key that now folds
/// the WHOLE group (one toggle per section). `header_on`/`show_diff`
/// come from the project's declared `layout:` matched against the
/// representative's checkout (`section_model_flags`); `diff` carries
/// `branch_group_diff`'s fold. T7 (divergence D) already moved the
/// COMMANDS/CHECKS bands out of here — `push_worktree_bands` renders
/// them after every group.
fn push_worktree(
    entries: &mut Vec<WorkspaceListEntry>,
    app: &AppState,
    group_key: &str,
    ws_idxs: &[usize],
    layout: Option<&[Section]>,
    force_expanded: bool,
) {
    let Some(&rep) = ws_idxs.first() else {
        return;
    };
    let rep_ws = &app.workspaces[rep];
    let collapse_key = section_collapse_key(rep);
    let (header_on, show_diff, show_dots) = section_model_flags(layout, rep_ws);
    entries.push(WorkspaceListEntry::SectionRow {
        ws_idx: rep,
        checkout_key: checkout_group_key(rep_ws),
        collapse_key: collapse_key.clone(),
        header_on,
        header_hidden: false,
        show_diff,
        branch_group: group_key.to_string(),
        diff: branch_group_diff(app, ws_idxs),
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for &ws_idx in ws_idxs {
        push_pane_dots_row(entries, ws_idx, show_dots);
    }
}

/// T6 6b (bora-79l.10): the descriptor a non-`Branch` `SectionKind` reads
/// through — `Comando`→`COMMANDS`, `Checks`→`CHECKS`, `Livre`→`LIVRE`.
/// `Branch` returns `None`: it never reaches here, it renders through its
/// own `push_worktree`/`SectionRow` path.
fn descriptor_for_kind(kind: SectionKind) -> Option<&'static SectionDescriptor> {
    match kind {
        SectionKind::Branch => None,
        SectionKind::Comando => Some(&COMMANDS),
        SectionKind::Checks => Some(&CHECKS),
        SectionKind::Livre => Some(&LIVRE),
    }
}

/// Collapse key for a declared (non-`Branch`) section — namespaced
/// (`sec:layout:`) and keyed on the section's own stable `id` (never its
/// position in `layout:`), the same id-not-index discipline every other
/// collapse key in this module already follows: reordering sections in
/// the file must not silently reassign a user's collapse state to a
/// different section.
fn declared_section_collapse_key(section_id: &str) -> String {
    format!("sec:layout:{section_id}")
}

/// T6 6b (bora-79l.10): render every DECLARED non-`Branch` `Section` from
/// a project's `layout:`, in declaration order, through the exact same
/// generic `SectionHeader`/`SectionItem` rows every registry band uses —
/// gate (1)'s "generic-row ANTES" claim
/// (`a_sixth_descriptor_renders_through_the_generic_rows_without_touching_the_registry`)
/// made real for a runtime-declared kind, not just a hand-written
/// throwaway. Called once per project group (`push_project_group`),
/// after the branch groups and their worktree bands: these sections are
/// project-scoped, not tied to any one checkout.
///
/// `done`/`total` follow the same "passing/declared" reading every other
/// band's counter uses: `done` counts children with `failing: false`,
/// `total` is every child, regardless of kind — a declared section
/// carries no live backing to derive a different metric from. Every
/// child gets its own row (unlike the live CHECKS band, which hides
/// passing checks): a user who typed a row into `projects.yml` typed it
/// to be seen. `running` on the item is `!failing`, so a `Checks`-kind
/// section's `FlagIdleAsError` bullet renders a failing child red and a
/// passing one green, and a `Comando`/`Livre` section's `Standard`
/// bullet renders the same distinction as green/dim — the only two
/// states `SectionBullet` can express. Collapse is independent of
/// `header_on` (same discipline `push_worktree` already applies to its
/// own `SectionRow`/blocks pair): a hidden header can still be
/// collapsed/expanded via its own `collapse_key`.
fn push_declared_sections(
    entries: &mut Vec<WorkspaceListEntry>,
    layout: &[Section],
    app: &AppState,
    force_expanded: bool,
) {
    for section in layout {
        let Some(kind) = descriptor_for_kind(section.kind) else {
            continue;
        };
        let collapse_key = declared_section_collapse_key(&section.id);
        let total = section.children.len();
        let done = section
            .children
            .iter()
            .filter(|child| matches!(child, SectionChild::Item { failing: false, .. }))
            .count();
        if section.header_on {
            entries.push(WorkspaceListEntry::SectionHeader {
                kind,
                collapse_key: collapse_key.clone(),
                done,
                total,
                name: section.name.clone(),
            });
        }
        if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
            continue;
        }
        for child in &section.children {
            let SectionChild::Item { label, failing } = child else {
                continue;
            };
            entries.push(WorkspaceListEntry::SectionItem {
                kind,
                label: label.clone(),
                detail: None,
                running: !failing,
                ws_idx: None,
            });
        }
    }
}

/// Normalized borrowed context for a band's push function (bora-by6). The
/// seven pre-registry push functions had non-uniform signatures — a
/// `selection: &[String]` here, a `ws: &Workspace` there, a bare `slug`
/// elsewhere, `local_branches` tacked on for one — which is exactly what
/// made a function-pointer table impossible before this type existed. Two
/// variants, one per `SectionLevel`, rather than one struct of all-optional
/// fields: a worktree-level push function destructures `Worktree` and
/// never sees a `slug`-shaped hole it has to ignore, and vice versa. Each
/// variant borrows only — no `String`/`Vec` owned here, no allocation.
#[derive(Clone, Copy)]
pub(crate) enum SectionPushCtx<'a> {
    Worktree {
        app: &'a AppState,
        checkout_key: &'a str,
        ws_idxs: &'a [usize],
        /// The project's `sections.commands` declaration (COMMANDS'
        /// selection).
        commands: &'a [String],
        /// The project's `sections.checks` declaration (CHECKS' toggle).
        checks: &'a [String],
        force_expanded: bool,
    },
    Project {
        app: &'a AppState,
        slug: &'a str,
        local_branches: &'a HashSet<String>,
        force_expanded: bool,
    },
}

/// The registry (bora-by6): every attachment band bora knows about, in
/// default render order. Growing this by one entry, plus writing its push
/// function, is the entire cost of a sixth band that reuses the generic
/// `SectionHeader`/`SectionItem` rows (G2) — no enum variant, no match arm
/// anywhere else. `&[&SectionDescriptor]` carries no explicit length in its
/// type, so appending an entry to the array literal below is the only edit;
/// `SECTION_COUNT` and every `[&SectionDescriptor; SECTION_COUNT]` derived
/// from it recompute at compile time.
pub(super) const REGISTRY: &[&SectionDescriptor] =
    &[&COMMANDS, &CHECKS, &TODOS, &NOTES, &PULL_REQUESTS];

/// `REGISTRY.len()`, computed once at compile time — never hand-updated
/// when the registry grows (see `REGISTRY`'s doc).
pub(super) const SECTION_COUNT: usize = REGISTRY.len();

pub(super) static COMMANDS: SectionDescriptor = SectionDescriptor {
    wire_name: "commands",
    glyph: "≡",
    // T7 (bora-79l): display label only — `wire_name` ("commands") is the
    // persisted/`sections.order:` contract and stays untouched, so old
    // projects.yml files keep resolving (ALVO_CAPTURE row 31 pins COMANDO).
    label: "COMANDO",
    level: SectionLevel::Worktree,
    counter: SectionCounter::Progress,
    bullet: SectionBullet::Standard,
    push: push_commands_section,
};

pub(super) static CHECKS: SectionDescriptor = SectionDescriptor {
    wire_name: "checks",
    // T7 (bora-79l): ALVO_CAPTURE row 33 pins `≡ CHECKS` — the same band
    // glyph COMANDO uses; the ✓ was an old rollup echo (the counter and
    // the ✗ items already carry the state).
    glyph: "≡",
    label: "CHECKS",
    level: SectionLevel::Worktree,
    counter: SectionCounter::Progress,
    // CHECKS rows exist only to flag failures, so an idle row IS the
    // problem — the red `✗` instead of the dim `·` every other band's idle
    // rows get.
    bullet: SectionBullet::FlagIdleAsError,
    push: push_checks_section,
};

pub(super) static TODOS: SectionDescriptor = SectionDescriptor {
    wire_name: "todos",
    glyph: "☐",
    label: "TODOS",
    level: SectionLevel::Project,
    counter: SectionCounter::Progress,
    bullet: SectionBullet::Standard,
    push: push_todos_section,
};

pub(super) static NOTES: SectionDescriptor = SectionDescriptor {
    wire_name: "notes",
    glyph: "✎",
    label: "NOTES",
    level: SectionLevel::Project,
    counter: SectionCounter::Count,
    bullet: SectionBullet::Standard,
    push: push_notes_section,
};

pub(super) static PULL_REQUESTS: SectionDescriptor = SectionDescriptor {
    wire_name: "pull_requests",
    glyph: "⇄",
    label: "PULL REQUESTS",
    level: SectionLevel::Project,
    counter: SectionCounter::Count,
    bullet: SectionBullet::Standard,
    push: push_pull_requests_section,
};

/// T6 6b (bora-79l.10): the empty, mountable slot (`SectionKind::Livre`).
/// Deliberately outside `REGISTRY` — it is not one of the five
/// `sections.order:`-addressable attachment bands (no wire name to
/// defend, `SECTION_COUNT`/`resolve_section_order` stay untouched — the
/// gate (1) note in `gates/bora-79l.10.md`) — `descriptor_for_kind`
/// looks it up directly from a project's declared `layout:`, never
/// through `filter_by_level`/`REGISTRY`. `push` is still required by
/// `SectionDescriptor`'s shape; it is unreachable in production (nothing
/// ever calls `(LIVRE.push)(...)` — `push_declared_sections` pushes the
/// generic rows itself) and exists only so the struct literal compiles.
pub(super) static LIVRE: SectionDescriptor = SectionDescriptor {
    wire_name: "livre",
    glyph: "▢",
    label: "LIVRE",
    level: SectionLevel::Worktree,
    counter: SectionCounter::Count,
    bullet: SectionBullet::Standard,
    push: push_livre_unreachable,
};

fn push_livre_unreachable(_entries: &mut Vec<WorkspaceListEntry>, _ctx: &SectionPushCtx<'_>) {
    debug_assert!(
        false,
        "LIVRE is outside REGISTRY and never pushed through it"
    );
}

/// Filters a resolved order (`resolve_section_order`'s return) down to one
/// level, preserving relative sequence — an alloc-free iterator over the
/// caller's own fixed-size array, replacing `worktree_section_order` and
/// `project_section_order`. This is what makes placing a band outside its
/// declared level unrepresentable rather than merely unconventional: a
/// descriptor's `level` decides which loop ever sees it, so there is no
/// dispatch match left to grow an `unreachable!()` arm in (bora-by6 G4).
fn filter_by_level<'a>(
    resolved: &'a [&'static SectionDescriptor; SECTION_COUNT],
    level: SectionLevel,
) -> impl Iterator<Item = &'static SectionDescriptor> + 'a {
    resolved.iter().copied().filter(move |d| d.level == level)
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
fn push_commands_section(entries: &mut Vec<WorkspaceListEntry>, ctx: &SectionPushCtx) {
    let SectionPushCtx::Worktree {
        app,
        checkout_key,
        ws_idxs,
        commands: selection,
        force_expanded,
        ..
    } = *ctx
    else {
        debug_assert!(false, "COMMANDS is a worktree-level band");
        return;
    };
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
        name: None,
        kind: &COMMANDS,
        collapse_key: collapse_key.clone(),
        done: declared.iter().filter(|cmd| is_running(&cmd.label)).count(),
        total: declared.len(),
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for cmd in declared {
        entries.push(WorkspaceListEntry::SectionItem {
            kind: &COMMANDS,
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

/// Resolves a project's declared `sections.order:` names into every
/// registered `SectionDescriptor` in render priority (bora-5ia, bora-yw6.2,
/// bora-by6). Contract:
///
/// - Names are matched case-insensitively (`SectionDescriptor::from_wire_name`).
/// - An unknown name is ignored, never an error — a future bora writing an
///   unregistered section name into `projects.yml` must not break an older
///   binary's sidebar.
/// - A declared-but-unlisted section still renders: it is appended after
///   the listed ones, in registry order. Ordering decides sequence, never
///   visibility.
/// - A duplicate name is honored once, at its first position.
/// - Absent or empty `order` resolves to exactly registry order, so
///   behavior is unchanged for every project that does not opt in.
///
/// Always returns every registered descriptor (a permutation of
/// `REGISTRY`) as a fixed-size array sized by `SECTION_COUNT`, not a `Vec`
/// — no allocation on the per-render, per-pane, per-client path (AGENTS.md,
/// "Multiplicative performance paths"). `push_project_group`/`push_worktree`
/// then read the relevant group out of it via `filter_by_level` — project-
/// level and worktree-level bands never interleave with each other (module
/// doc), only reorder within their own group. `SECTION_COUNT` derives from
/// `REGISTRY.len()` (`REGISTRY`'s doc), so a REGISTRY entry added for a
/// sixth band needs no edit here.
fn resolve_section_order(order: Option<&[String]>) -> [&'static SectionDescriptor; SECTION_COUNT] {
    let mut resolved = [REGISTRY[0]; SECTION_COUNT];
    let mut len = 0usize;
    if let Some(names) = order {
        for name in names {
            let Some(section) = SectionDescriptor::from_wire_name(name) else {
                continue;
            };
            if resolved[..len].contains(&section) {
                continue;
            }
            resolved[len] = section;
            len += 1;
        }
    }
    for &section in REGISTRY {
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
///   never silently empty (G8).
/// - Rows render the header with `n/m` = `checks_counts` (passing/total) and
///   one item per failing check; passing/pending checks have no row. A PR
///   with zero check runs renders no band (rule 5).
///
/// The collapse key is namespaced per worktree (`sec:checks:{checkout_key}`)
/// so two worktrees' bands never share collapse state.
fn push_checks_section(entries: &mut Vec<WorkspaceListEntry>, ctx: &SectionPushCtx) {
    let SectionPushCtx::Worktree {
        app,
        checkout_key,
        ws_idxs,
        checks: declared,
        force_expanded,
        ..
    } = *ctx
    else {
        debug_assert!(false, "CHECKS is a worktree-level band");
        return;
    };
    if declared.is_empty() {
        return;
    }
    // The representative workspace CHECKS reads — the same one the
    // `WorktreeRow`'s own PR badge reads (doc above). Derived from
    // `ws_idxs` here rather than taking `&Workspace` directly, so every
    // band's push function shares the one `SectionPushCtx` shape (bora-by6).
    let Some(&first_idx) = ws_idxs.first() else {
        return;
    };
    let ws = &app.workspaces[first_idx];
    let Some(status) = ws.cached_check_status.as_ref() else {
        return;
    };
    if status.is_not_applicable() {
        return;
    }
    let collapse_key = format!("sec:checks:{checkout_key}");
    if let Some(error) = status.error.as_deref() {
        entries.push(WorkspaceListEntry::SectionHeader {
            name: None,
            kind: &CHECKS,
            collapse_key: collapse_key.clone(),
            done: 0,
            total: 0,
        });
        if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
            return;
        }
        entries.push(WorkspaceListEntry::SectionItem {
            kind: &CHECKS,
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
        name: None,
        kind: &CHECKS,
        collapse_key: collapse_key.clone(),
        done: passing,
        total,
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for run in status.checks.iter().filter(|run| run.is_failing()) {
        entries.push(WorkspaceListEntry::SectionItem {
            kind: &CHECKS,
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
fn push_todos_section(entries: &mut Vec<WorkspaceListEntry>, ctx: &SectionPushCtx) {
    let SectionPushCtx::Project {
        app,
        slug,
        force_expanded,
        ..
    } = *ctx
    else {
        debug_assert!(false, "TODOS is a project-level band");
        return;
    };
    let Some(summary) = app.project_todos.get(slug) else {
        return;
    };
    if summary.total == 0 {
        return;
    }
    let collapse_key = format!("sec:todos:{slug}");
    entries.push(WorkspaceListEntry::SectionHeader {
        name: None,
        kind: &TODOS,
        collapse_key: collapse_key.clone(),
        done: summary.done,
        total: summary.total,
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for title in &summary.actionable {
        entries.push(WorkspaceListEntry::SectionItem {
            kind: &TODOS,
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
fn push_notes_section(entries: &mut Vec<WorkspaceListEntry>, ctx: &SectionPushCtx) {
    let SectionPushCtx::Project {
        app,
        slug,
        force_expanded,
        ..
    } = *ctx
    else {
        debug_assert!(false, "NOTES is a project-level band");
        return;
    };
    let Some(names) = app.project_notes.get(slug) else {
        return;
    };
    if names.is_empty() {
        return;
    }
    let collapse_key = format!("sec:notes:{slug}");
    entries.push(WorkspaceListEntry::SectionHeader {
        name: None,
        kind: &NOTES,
        collapse_key: collapse_key.clone(),
        done: 0,
        total: names.len(),
    });
    if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
        return;
    }
    for name in names {
        entries.push(WorkspaceListEntry::SectionItem {
            kind: &NOTES,
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
///   never silently empty (G8). The first errored repo wins if more than
///   one is in scope — one band, one error line.
/// - Otherwise one row per PR whose head branch is not in `local_branches`,
///   sorted by PR number for stable render order (rule 8).
///
/// Collapse key is namespaced per project (`sec:prs:{slug}`).
fn push_pull_requests_section(entries: &mut Vec<WorkspaceListEntry>, ctx: &SectionPushCtx) {
    let SectionPushCtx::Project {
        app,
        slug,
        local_branches,
        force_expanded,
    } = *ctx
    else {
        debug_assert!(false, "PULL REQUESTS is a project-level band");
        return;
    };
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
            name: None,
            kind: &PULL_REQUESTS,
            collapse_key: collapse_key.clone(),
            done: 0,
            total: 0,
        });
        if !force_expanded && app.collapsed_space_keys.contains(&collapse_key) {
            return;
        }
        entries.push(WorkspaceListEntry::SectionItem {
            kind: &PULL_REQUESTS,
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
        name: None,
        kind: &PULL_REQUESTS,
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

/// Push the one `PaneDotsRow` for a workspace — a dot per pane, always
/// exactly one block regardless of pane count (bora-c1h G4 successor,
/// the pair-of-rows shape): the workspace's identity under its group's
/// `SectionRow` lives entirely HERE (6a — the block is the member's
/// representation), so this carries NO pane data at all, not even a
/// count — the renderer reads live pane state straight off
/// `AppState.workspaces[ws_idx]` at render time (module doc). `name`
/// starts empty and is filled in exactly once, at the end of emission,
/// by `disambiguate_pane_dots_names` — never computed here, so no two
/// passes can drift apart on a member's name. `dots` (T6 6b) is the
/// owning section's `parts.dots` flag, carried straight onto the entry
/// so `entry_row_height` (and the two other lockstep passes) read the
/// l2-line toggle off the row itself instead of re-deriving it from the
/// model a second time.
fn push_pane_dots_row(entries: &mut Vec<WorkspaceListEntry>, ws_idx: usize, dots: bool) {
    entries.push(WorkspaceListEntry::PaneDotsRow {
        ws_idx,
        name: String::new(),
        dots,
    });
}

/// The stable, addressable pane id — the same `wNpN` form
/// `bora agent prompt` / `orc channel send` accept (`workspace_agent_label`'s
/// doc comment).
pub(crate) fn pane_address(ws: &Workspace, number: usize) -> String {
    format!(
        "{}p{}",
        ws.id,
        crate::workspace::encode_public_number(number)
    )
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

    /// A one-file cached change set with the given numstat — the same
    /// shape `workspace_diff_counts` (and therefore `branch_group_diff`)
    /// folds over.
    fn change_set(added: u32, removed: u32) -> crate::workspace::WorkspaceChangeSet {
        crate::workspace::WorkspaceChangeSet {
            sections: vec![crate::workspace::ChangeSection {
                kind: crate::workspace::ChangeSectionKind::Unstaged,
                files: vec![crate::workspace::ChangedFile {
                    path: "src/lib.rs".to_string(),
                    added: Some(added),
                    removed: Some(removed),
                    status: crate::workspace::ChangeStatus::Modified,
                }],
            }],
            base_ref: None,
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
            layout: None,
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
                kind, done, total, ..
            } if kind.wire_name == "checks" => Some((*done, *total)),
            _ => None,
        })
    }

    /// The labels of the CHECKS band's item rows, in order.
    fn checks_items(entries: &[WorkspaceListEntry]) -> Vec<String> {
        entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionItem { kind, label, .. }
                    if kind.wire_name == "checks" =>
                {
                    Some(label.clone())
                }
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

    /// `checkout_key` of every `SectionRow` in emission order — one entry
    /// per branch GROUP now (6a, the representative's checkout).
    fn worktree_rows(entries: &[WorkspaceListEntry]) -> Vec<&str> {
        entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionRow { checkout_key, .. } => Some(checkout_key.as_str()),
                _ => None,
            })
            .collect()
    }

    /// Every `ws_idx` under the `ProjectRow` (declared or the orphans
    /// group) named by `collapse_key` — everything between it and the
    /// next `ProjectRow` (or the end of the list). 6a: membership is
    /// the member BLOCKS (`PaneDotsRow`, one per workspace); the
    /// `SectionRow`s only name group representatives, so this doubles
    /// as the P0 regression's assertion surface: no `ws_idx` may ever
    /// appear under two different projects.
    fn project_ws_idxs(entries: &[WorkspaceListEntry], collapse_key: &str) -> Vec<usize> {
        let start = entries.iter().position(|e| {
            matches!(e, WorkspaceListEntry::ProjectRow { collapse_key: k, .. } if k == collapse_key)
        });
        let Some(start) = start else {
            return Vec::new();
        };
        entries[start + 1..]
            .iter()
            .take_while(|e| !matches!(e, WorkspaceListEntry::ProjectRow { .. }))
            .filter_map(|e| match e {
                WorkspaceListEntry::PaneDotsRow { ws_idx, .. } => Some(*ws_idx),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn same_branch_group_gets_one_section_row_distinct_checkouts_split() {
        // 6a (bora-79l.10): attribution — was
        // `same_checkout_gets_one_section_row_per_workspace_distinct_
        // checkouts_split` (bora-c1h: one SectionRow per WORKSPACE).
        // The container model merges every workspace of one branch
        // group into ONE SectionRow (header on top, blocks contiguous):
        // ws0/ws1 share checkout_a — same HEAD, so one section holds
        // both — while ws2 on checkout_b (same repo, no cached branch
        // on either side to prove a shared one) keeps its own section.
        // Fica vermelho se o agrupamento regredir pra checkout-por-
        // workspace, ou se a exceção conservadora (branch None nunca
        // mergeia entre checkouts) sumir.
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
        // identity (shared `origin`), different checkout_key. Branches
        // are set EXPLICITLY: `ws_at` inherits the bora checkout's own
        // branch from `Workspace::test_new` (a fixture leak — the fake
        // dirs' HEADs are never read), which would merge all three
        // into one group and measure nothing about the split.
        let mut ws0 = ws_at(&checkout_a);
        ws0.cached_git_branch = Some("main".to_string());
        let mut ws1 = ws_at(&checkout_a);
        ws1.cached_git_branch = Some("main".to_string());
        let mut ws2 = ws_at(&checkout_b);
        ws2.cached_git_branch = Some("feature/x".to_string());
        app.workspaces = vec![ws0, ws1, ws2];

        let entries = project_view_entries(&app, false);
        let section_ws_idxs: Vec<usize> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionRow { ws_idx, .. } => Some(*ws_idx),
                _ => None,
            })
            .collect();
        assert_eq!(
            section_ws_idxs,
            vec![0, 2],
            "ONE SectionRow per branch group: the shared checkout merges \
             ws0+ws1 under rep 0; ws2's distinct checkout splits: {entries:?}"
        );
        // The group container: rep 0's row, then BOTH member blocks
        // contiguous under it — no second header between them.
        let shape: Vec<&str> = entries
            .iter()
            .map(|e| match e {
                WorkspaceListEntry::SectionRow { .. } => "SR",
                WorkspaceListEntry::PaneDotsRow { .. } => "PDR",
                _ => "?",
            })
            .collect();
        assert_eq!(
            shape,
            vec!["?", "SR", "PDR", "PDR", "SR", "PDR"],
            "header at the TOP of the group, member blocks contiguous: {entries:?}"
        );
        let blocks: Vec<usize> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::PaneDotsRow { ws_idx, .. } => Some(*ws_idx),
                _ => None,
            })
            .collect();
        assert_eq!(
            blocks,
            vec![0, 1, 2],
            "every workspace still renders its own block: {entries:?}"
        );

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_b).unwrap();
    }

    #[test]
    fn worktree_sections_stay_distinct_when_every_worktree_shares_one_repo() {
        // bora-b2r parity on the merged row (bora-c1h), 6a: two
        // worktrees of one repo whose workspaces share a display name
        // ("t" here) MUST NOT collapse into two indistinguishable
        // BLOCKS — each gets a dim parent-dir hint on its `PaneDotsRow`
        // name (the block is the member's representation now), the
        // analogue of the old `WorktreeRow.repo` column never
        // collapsing.
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
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::PaneDotsRow { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec!["t (worktree-a)", "t (worktree-b)"],
            "both worktrees render, each disambiguated by its parent dir: {entries:?}"
        );

        std::fs::remove_dir_all(&container).unwrap();
    }

    #[test]
    fn worktree_sections_stay_distinct_when_a_project_spans_two_repos() {
        // Same bora-b2r parity across repos: both fixture workspaces are
        // named "t", so the blocks would render identical without a hint —
        // each must carry one, and the two hints must differ.
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
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::PaneDotsRow { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names.len(), 2, "both repos render: {entries:?}");
        assert!(
            names
                .iter()
                .all(|n| n.starts_with("t (") && n.ends_with(')')),
            "identical display names force hints on the block names: {names:?}"
        );
        assert_ne!(names[0], names[1], "hints must disambiguate: {names:?}");
        // The hint is the checkout dir's own component (depth 0 here):
        // the fixture's tempdir basename carries the repo's name.
        assert!(
            names[0].contains("multi-repo-x"),
            "hint names repo x: {names:?}"
        );
        assert!(
            names[1].contains("multi-repo-y"),
            "hint names repo y: {names:?}"
        );

        std::fs::remove_dir_all(&repo_x).unwrap();
        std::fs::remove_dir_all(&repo_y).unwrap();
    }

    #[test]
    fn identical_names_with_different_branches_now_get_a_branch_hint() {
        // Before this change, two same-named rows were left un-hinted the
        // moment their branches differed — the branch text next to the
        // name was trusted to tell them apart on its own. It renders too
        // dim for that (owner report), so the NAME itself now has to be
        // unique; the branch is the shortest available token, so it is
        // used as the hint directly rather than falling back to a
        // (nonexistent, here — both share `identity_cwd`) parent-dir hint.
        let mut app = AppState::test_new();
        let mut ws_a = Workspace::test_new("a");
        ws_a.custom_name = Some("svc".to_string());
        ws_a.cached_git_branch = Some("main".to_string());
        let mut ws_b = Workspace::test_new("b");
        ws_b.custom_name = Some("svc".to_string());
        ws_b.cached_git_branch = Some("feat/x".to_string());
        app.workspaces = vec![ws_a, ws_b];
        let entries = project_view_entries(&app, false);
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::PaneDotsRow { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec!["svc (main)", "svc (feat/x)"],
            "identical names must be disambiguated by branch, not silently left \
             identical because the branch already differs: {entries:?}"
        );
    }

    #[test]
    fn same_name_across_different_project_groups_is_not_hinted() {
        // Two different project headers already tell same-named blocks apart
        // visually — hinting across group boundaries would be noise, not
        // signal (module doc, "only disambiguate rows that actually collide").
        let mut file = ProjectsFile::default();
        file.projects.insert("alpha".to_string(), project(vec![]));
        file.projects.insert("beta".to_string(), project(vec![]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws_a = Workspace::test_new("a");
        ws_a.custom_name = Some("svc".to_string());
        ws_a.set_project(Some("alpha".to_string()));
        let mut ws_b = Workspace::test_new("b");
        ws_b.custom_name = Some("svc".to_string());
        ws_b.set_project(Some("beta".to_string()));
        app.workspaces = vec![ws_a, ws_b];

        let entries = project_view_entries(&app, false);
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::PaneDotsRow { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            vec!["svc", "svc"],
            "two different project groups already disambiguate visually via their own header: {entries:?}"
        );
    }

    #[test]
    fn every_workspace_emits_exactly_one_pane_dots_row_regardless_of_pane_count() {
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
        let pane_dots_rows_for = |ws_idx: usize| -> usize {
            entries
                .iter()
                .filter(|e| matches!(e, WorkspaceListEntry::PaneDotsRow { ws_idx: idx, .. } if *idx == ws_idx))
                .count()
        };
        assert_eq!(
            pane_dots_rows_for(0),
            1,
            "a single-pane workspace emits exactly one PaneDotsRow: {entries:?}"
        );
        assert_eq!(
            pane_dots_rows_for(1),
            1,
            "a 2-pane workspace still emits exactly ONE PaneDotsRow — the dots \
             live inside that one row, module doc: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn pane_dots_row_names_come_from_the_single_disambiguation_pass() {
        // 6a: the name is decided ONCE, on the block itself
        // (`disambiguate_pane_dots_names`) — there is no second row to
        // disagree with anymore. Pick a fixture the disambiguator
        // actually has to act on (two worktrees of one repo sharing a
        // name) and assert each block's name is exactly the workspace's
        // base name plus its dir hint.
        let container = temp_test_dir("pane-dots-name-match");
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
        for (ws_idx, hint) in [(0usize, "worktree-a"), (1usize, "worktree-b")] {
            let expected = format!("{} ({hint})", workspace_group_name(&app.workspaces[ws_idx]));
            let pane_dots_name = entries.iter().find_map(|e| match e {
                WorkspaceListEntry::PaneDotsRow {
                    ws_idx: idx, name, ..
                } if *idx == ws_idx => Some(name.clone()),
                _ => None,
            });
            assert_eq!(
                pane_dots_name,
                Some(expected.clone()),
                "PaneDotsRow.name must be the base name plus the \
                 disambiguator's hint ({expected:?}): {entries:?}"
            );
        }

        std::fs::remove_dir_all(&container).unwrap();
    }

    #[test]
    fn same_branch_group_merges_into_one_section_header_on_top() {
        // 6a (bora-79l.10): attribution — was
        // `same_branch_exception_hides_the_upper_header_keeps_the_lower`
        // (T3: two same-branch headers, upper hides). The container
        // model DISSOLVES that case: workspaces of one (repo, branch)
        // share ONE section, so there is nothing to hide — the header
        // sits at the TOP of the group and every member block renders
        // contiguously below (ALVO_CAPTURE rows 03-07). Fica vermelho
        // se uma segunda header da mesma branch voltar a existir, se a
        // header visível cair ENTRE os blocos, ou se algum bloco
        // sumir.
        let checkout_a = temp_test_dir("same-branch-a");
        let checkout_c = temp_test_dir("same-branch-c");
        let shared_origin = "git@github.com:owner/shared-repo.git";
        init_fake_git_repo(&checkout_a, Some(shared_origin));
        init_fake_git_repo(&checkout_c, Some(shared_origin));

        let mut file = ProjectsFile::default();
        file.projects.insert(
            "proj".to_string(),
            project(vec![member(&checkout_a), member(&checkout_c)]),
        );
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        // ws0 and ws1 share checkout_a — same repo, same branch `main`.
        // ws2 is another checkout of the same repo on `feature/x`.
        let mut ws0 = ws_at(&checkout_a);
        ws0.cached_git_branch = Some("main".to_string());
        let mut ws1 = ws_at(&checkout_a);
        ws1.cached_git_branch = Some("main".to_string());
        let mut ws2 = ws_at(&checkout_c);
        ws2.cached_git_branch = Some("feature/x".to_string());
        app.workspaces = vec![ws0, ws1, ws2];

        let entries = project_view_entries(&app, false);
        let headers: Vec<(usize, bool)> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionRow {
                    ws_idx,
                    header_hidden,
                    ..
                } => Some((*ws_idx, *header_hidden)),
                _ => None,
            })
            .collect();
        assert_eq!(
            headers,
            vec![(0, false), (2, false)],
            "ONE visible header per branch group — rep of the merged `main` \
             group, then the `feature/x` group's own: {entries:?}"
        );
        let shape: Vec<&str> = entries
            .iter()
            .map(|e| match e {
                WorkspaceListEntry::SectionRow { .. } => "SR",
                WorkspaceListEntry::PaneDotsRow { .. } => "PDR",
                _ => "?",
            })
            .collect();
        assert_eq!(
            shape,
            vec!["?", "SR", "PDR", "PDR", "SR", "PDR"],
            "the merged group's header is at the TOP, its two blocks glued \
             under it, never a header between them: {entries:?}"
        );

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_c).unwrap();
    }

    #[test]
    fn branch_group_diff_is_the_sum_of_every_members_change_set() {
        // 6a pin (contract: "o cluster de diff do grupo = SOMA dos
        // change sets dos membros, pinado por teste com 2 membros"):
        // the header speaks for the WHOLE branch group, so its `+N −M`
        // must fold every member's cached change set — a single
        // member's numbers wearing the group's header would undercount
        // it. Fica vermelho se a soma regredir pro diff do
        // representante isolado (ou de qualquer membro só).
        let checkout_a = temp_test_dir("group-diff-a");
        let checkout_b = temp_test_dir("group-diff-b");
        let origin = "git@github.com:owner/group-diff-repo.git";
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
        // Two workspaces of ONE branch group (same repo, same `main`,
        // different checkouts) — the merge case — plus a lone
        // feature/x workspace whose group is itself alone.
        let mut main_one = ws_at(&checkout_a);
        main_one.cached_git_branch = Some("main".to_string());
        main_one.cached_change_set = Some(change_set(10, 2));
        let mut main_two = ws_at(&checkout_b);
        main_two.cached_git_branch = Some("main".to_string());
        main_two.cached_change_set = Some(change_set(5, 7));
        let mut feature = ws_at(&checkout_a);
        feature.cached_git_branch = Some("feature/x".to_string());
        feature.cached_change_set = Some(change_set(900, 1));
        app.workspaces = vec![main_one, main_two, feature];

        let entries = project_view_entries(&app, false);
        let diffs: Vec<(usize, Option<(u32, u32)>)> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionRow { ws_idx, diff, .. } => Some((*ws_idx, *diff)),
                _ => None,
            })
            .collect();
        assert_eq!(
            diffs,
            vec![(0, Some((15, 9))), (2, Some((900, 1)))],
            "the merged `main` group's cluster is 10+5 / 2+7 — the SUM of \
             both members — and the lone `feature/x` group carries its own: \
             {entries:?}"
        );

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_b).unwrap();
    }

    #[test]
    fn same_branch_in_a_different_group_keeps_its_header() {
        // Fica vermelho se a exceção mesma-branch atravessar a fronteira
        // do grupo — o mesmo repo+branch em OUTRO grupo tem header própria.
        // Attribution: replaces `repo_shown_true_again_in_a_different_
        // project_group` — same fixture shape, same per-group scoping
        // question, aimed at the new rule.
        let checkout_a = temp_test_dir("same-branch-group-a");
        let checkout_b = temp_test_dir("same-branch-group-b");
        let shared_origin = "git@github.com:owner/cross-group-repo.git";
        init_fake_git_repo(&checkout_a, Some(shared_origin));
        init_fake_git_repo(&checkout_b, Some(shared_origin));

        let mut file = ProjectsFile::default();
        file.projects
            .insert("alpha".to_string(), project(vec![member(&checkout_a)]));
        file.projects
            .insert("beta".to_string(), project(vec![member(&checkout_b)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        let mut ws0 = ws_at(&checkout_a);
        ws0.cached_git_branch = Some("main".to_string());
        let mut ws1 = ws_at(&checkout_b);
        ws1.cached_git_branch = Some("main".to_string());
        app.workspaces = vec![ws0, ws1];
        // Same reason as the old A3 cross-group test: both checkouts share
        // `shared_origin`, so without the explicit binding the
        // slug-alphabetically-first project (`alpha`) would claim both and
        // the fixture would measure nothing about group boundaries.
        app.workspaces[1].set_project(Some("beta".to_string()));

        let entries = project_view_entries(&app, false);
        let hidden: Vec<(usize, bool)> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionRow {
                    ws_idx,
                    header_hidden,
                    ..
                } => Some((*ws_idx, *header_hidden)),
                _ => None,
            })
            .collect();
        assert_eq!(
            hidden,
            vec![(0, false), (1, false)],
            "the same repo+branch in two DIFFERENT groups keeps both headers \
             — the exception never crosses a group boundary: {entries:?}"
        );

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_b).unwrap();
    }

    #[test]
    fn header_off_from_the_model_hides_the_header_keeps_the_blocks() {
        // Fica vermelho se o render ignorar o campo `header_on` do modelo
        // Section — header OFF esconde a header e mantém os blocos.
        let checkout_a = temp_test_dir("header-off-a");
        let checkout_c = temp_test_dir("header-off-c");
        let shared_origin = "git@github.com:owner/header-off-repo.git";
        init_fake_git_repo(&checkout_a, Some(shared_origin));
        init_fake_git_repo(&checkout_c, Some(shared_origin));
        let checkout_c_key = crate::workspace::git_space_metadata(&checkout_c)
            .expect("fake checkout has git space")
            .checkout_key;

        use crate::ui::sidebar::sections::SectionParts;
        let mut declared = project(vec![member(&checkout_a), member(&checkout_c)]);
        declared.layout = Some(vec![Section {
            name: None,
            id: "sec-feature-x".to_string(),
            kind: SectionKind::Branch,
            header_on: false,
            parts: SectionParts {
                dots: true,
                diff: false,
            },
            children: vec![SectionChild::Workspace {
                name: "gamma".to_string(),
                checkout: checkout_c_key,
            }],
        }]);
        let mut file = ProjectsFile::default();
        file.projects.insert("proj".to_string(), declared);
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        let mut ws0 = ws_at(&checkout_a);
        ws0.cached_git_branch = Some("main".to_string());
        let mut ws2 = ws_at(&checkout_c);
        ws2.cached_git_branch = Some("feature/x".to_string());
        app.workspaces = vec![ws0, ws2];

        let entries = project_view_entries(&app, false);
        let flags: Vec<(usize, bool, bool)> = entries
            .iter()
            .filter_map(|e| match e {
                WorkspaceListEntry::SectionRow {
                    ws_idx,
                    header_on,
                    show_diff,
                    ..
                } => Some((*ws_idx, *header_on, *show_diff)),
                _ => None,
            })
            .collect();
        assert_eq!(
            flags,
            vec![(0, true, true), (1, false, false)],
            "the declared section's header_on/parts.diff are consumed at \
             emission; an unmatched workspace keeps the defaults: {entries:?}"
        );
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, WorkspaceListEntry::PaneDotsRow { ws_idx: 1, .. })),
            "header OFF hides the header, never the section's blocks: {entries:?}"
        );

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_c).unwrap();
    }

    #[test]
    fn hidden_branch_headers_render_no_line_and_keep_every_block() {
        // Fica vermelho se uma header oculta pintar qualquer célula, ou se
        // algum bloco de baixo sumir junto — o buffer renderizado é a prova.
        let checkout_a = temp_test_dir("hidden-render-a");
        let checkout_c = temp_test_dir("hidden-render-c");
        let shared_origin = "git@github.com:owner/hidden-render-repo.git";
        init_fake_git_repo(&checkout_a, Some(shared_origin));
        init_fake_git_repo(&checkout_c, Some(shared_origin));
        let checkout_c_key = crate::workspace::git_space_metadata(&checkout_c)
            .expect("fake checkout has git space")
            .checkout_key;

        use crate::ui::sidebar::sections::SectionParts;
        let mut declared = project(vec![member(&checkout_a), member(&checkout_c)]);
        declared.layout = Some(vec![Section {
            name: None,
            id: "sec-feature-x".to_string(),
            kind: SectionKind::Branch,
            header_on: false,
            parts: SectionParts::default(),
            children: vec![SectionChild::Workspace {
                name: "gamma".to_string(),
                checkout: checkout_c_key,
            }],
        }]);
        let mut file = ProjectsFile::default();
        file.projects.insert("proj".to_string(), declared);
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        app.projects = store;
        // 6a: ws0+ws1 share repo+branch `main` — ONE merged section
        // whose header sits at the top (visible); ws2's section is
        // model OFF (header hidden, blocks stay).
        let mut ws0 = ws_at(&checkout_a);
        ws0.custom_name = Some("alpha".to_string());
        ws0.cached_git_branch = Some("main".to_string());
        let mut ws1 = ws_at(&checkout_a);
        ws1.custom_name = Some("beta".to_string());
        ws1.cached_git_branch = Some("main".to_string());
        let mut ws2 = ws_at(&checkout_c);
        ws2.custom_name = Some("gamma".to_string());
        ws2.cached_git_branch = Some("feature/x".to_string());
        app.workspaces = vec![ws0, ws1, ws2];

        let area = ratatui::layout::Rect::new(0, 0, 40, 16);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .expect("test terminal");
        terminal
            .draw(|frame| {
                crate::ui::sidebar::render_workspace_list(&app, &runtimes, frame, area, false)
            })
            .expect("workspace list should render");

        let buffer = terminal.backend().buffer();
        let row = |y: u16| -> String {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        };
        let rows: Vec<String> = (0..area.height).map(row).collect();
        // One and only one branch glyph on screen: the merged `main`
        // group's header. ws2's row (model OFF) must be fully blank.
        let header_rows: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.contains('\u{2387}'))
            .map(|(y, _)| y)
            .collect();
        assert_eq!(
            header_rows.len(),
            1,
            "exactly one visible branch header: {rows:?}"
        );
        assert!(
            rows[header_rows[0]].contains("main"),
            "the surviving header declares the branch: {:?}",
            rows[header_rows[0]]
        );
        for name in ["alpha", "beta", "gamma"] {
            assert!(
                rows.iter().any(|r| r.contains(name)),
                "the {name} block must render below its (possibly hidden) header: {rows:?}"
            );
        }
        // The hidden headers claim no hit area either — a blank row that
        // toggles collapse on click would be an invisible affordance.
        let (_cards, _headers, project_rows) =
            crate::ui::compute_workspace_list_areas_all(&app, area);
        let section_hits: Vec<usize> = project_rows
            .iter()
            .filter(|a| {
                matches!(
                    a.target,
                    crate::app::state::ProjectRowTarget::Section { .. }
                )
            })
            .map(|a| a.rect.y as usize)
            .collect();
        assert_eq!(
            section_hits,
            vec![header_rows[0]],
            "only the visible header carries a Section hit area: {project_rows:?}"
        );

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_c).unwrap();
    }

    #[test]
    fn collapsed_section_row_hides_its_pane_dots_row() {
        let repo = temp_test_dir("pane-dots-collapse");
        init_fake_git_repo(&repo, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("proj".to_string(), project(vec![member(&repo)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];
        app.collapsed_space_keys.insert("wsec:0".to_string());

        let entries = project_view_entries(&app, false);
        assert!(
            entries
                .iter()
                .any(|e| matches!(e, WorkspaceListEntry::SectionRow { .. })),
            "the SectionRow itself always renders, collapsed or not: {entries:?}"
        );
        assert!(
            !entries
                .iter()
                .any(|e| matches!(e, WorkspaceListEntry::PaneDotsRow { .. })),
            "a collapsed SectionRow must hide its PaneDotsRow: {entries:?}"
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
                .any(|e| matches!(e, WorkspaceListEntry::PaneDotsRow { .. })),
            "fixture must actually exercise a PaneDotsRow: {entries:?}"
        );
        for entry in &entries {
            if let WorkspaceListEntry::PaneDotsRow { name, .. } = entry {
                assert!(
                    !name.contains(repo_name.as_str()),
                    "PaneDotsRow must never repeat the repo name ({repo_name:?}): {entry:?}"
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
    fn checks_row_reaches_the_pr_workspace_section() {
        // RESIDUAL (bora-c1h): PR number/ahead/behind/checks no longer live
        // as fields on the entry (`SectionRow` carries only
        // `ws_idx`/`checkout_key`/`collapse_key`) — they're read straight
        // off `AppState.workspaces[ws_idx].cached_check_status` at RENDER
        // time (`sidebar::section_row_line`'s call site). This test now
        // only pins that the entry pipeline still reaches the right
        // workspace; the PR number actually rendering is
        // `sidebar.rs`'s `section_row_line_shows_pr_and_checks_cluster`.
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
        let ws_idx = entries.iter().find_map(|e| match e {
            WorkspaceListEntry::SectionRow { ws_idx, .. } => Some(*ws_idx),
            _ => None,
        });
        assert_eq!(
            ws_idx,
            Some(0),
            "the section row must point at the PR's workspace: {entries:?}"
        );
        assert_eq!(
            app.workspaces[0]
                .cached_check_status
                .as_ref()
                .and_then(|s| s.pr.as_ref())
                .map(|pr| pr.number),
            Some(128),
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
                    kind,
                    label,
                    running,
                    ws_idx,
                    ..
                } if kind.wire_name == "commands" => Some((label.clone(), *running, *ws_idx)),
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
            section_band(&entries, &COMMANDS),
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
                section_band(&entries, &COMMANDS),
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
                section_band(&entries, &COMMANDS),
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
                section_band(&entries, &COMMANDS),
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
        want: &'static SectionDescriptor,
    ) -> Option<(usize, usize)> {
        entries.iter().find_map(|e| match e {
            WorkspaceListEntry::SectionHeader {
                kind, done, total, ..
            } if *kind == want => Some((*done, *total)),
            _ => None,
        })
    }

    fn section_items(
        entries: &[WorkspaceListEntry],
        want: &'static SectionDescriptor,
    ) -> Vec<String> {
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
            section_band(&entries, &TODOS),
            Some((1, 3)),
            "TODOS header n/m = done/total: {entries:?}"
        );
        assert_eq!(
            section_items(&entries, &TODOS),
            vec!["ship sidebar".to_string(), "close epic".to_string()],
            "one row per actionable open todo: {entries:?}"
        );
        assert_eq!(
            section_band(&entries, &NOTES),
            Some((0, 2)),
            "NOTES header carries the doc count: {entries:?}"
        );
        assert_eq!(
            section_items(&entries, &NOTES),
            vec!["decisions".to_string(), "plan".to_string()],
            "one row per scratchpad doc: {entries:?}"
        );

        let project_pos = position_of(&entries, |e| {
            matches!(e, WorkspaceListEntry::ProjectRow { declared: true, .. })
        });
        let todos_pos = position_of(&entries, |e| {
            matches!(
                e,
                WorkspaceListEntry::SectionHeader { kind, .. } if kind.wire_name == "todos"
            )
        });
        let notes_pos = position_of(&entries, |e| {
            matches!(
                e,
                WorkspaceListEntry::SectionHeader { kind, .. } if kind.wire_name == "notes"
            )
        });
        let worktree_pos = position_of(&entries, |e| {
            matches!(e, WorkspaceListEntry::SectionRow { .. })
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
        assert_eq!(section_band(&entries, &TODOS), Some((0, 2)));
        assert_eq!(
            section_items(&entries, &TODOS),
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
        assert_eq!(section_band(&entries, &TODOS), Some((0, 1)));
        assert_eq!(section_band(&entries, &NOTES), Some((0, 1)));
        assert!(section_items(&entries, &TODOS).is_empty());
        assert!(section_items(&entries, &NOTES).is_empty());

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
            section_band(&entries, &TODOS),
            None,
            "no snapshot -> no band (rule 5): {entries:?}"
        );
        assert_eq!(section_band(&entries, &NOTES), None);
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
        // derives their height from `entry_row_height`, which must return 1
        // (bora-79l F2: except `PaneDotsRow`, whose own content is 2 rows
        // tall since it split into l1 name + l2 dots — `entry_row_height`'s
        // own doc).
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
                crate::ui::sidebar::entry_row_height(entry, &entries, idx, 0),
                crate::ui::sidebar::expected_entry_height(&entries, idx)
                    + crate::ui::sidebar::project_view_trailing_gap(entry, &entries, idx, 0),
                "entry {idx}: {entry:?}"
            );
        }

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn every_emitted_entry_has_row_height_one_except_pane_dots_row() {
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
                crate::ui::sidebar::entry_row_height(entry, &entries, idx, 0),
                crate::ui::sidebar::expected_entry_height(&entries, idx)
                    + crate::ui::sidebar::project_view_trailing_gap(entry, &entries, idx, 0),
                "every Project-view row must match the contracted height — \
                 1, or 2 for a PaneDotsRow (bora-79l F2), or a padded \
                 project band (R3) — plus its trailing gap row (R5: the \
                 plain separator after a band) — entry {idx}: {entry:?}"
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
        // Branches set EXPLICITLY (the sibling's matches its inventory
        // entry below): `ws_at` would otherwise leak the bora checkout's
        // own branch from `Workspace::test_new` and merge both open
        // workspaces into one branch group — this test is about the
        // bora-qdi unopened-dedup, not about 6a's grouping.
        let mut open_member = ws_at(&member_checkout);
        open_member.cached_git_branch = Some("main".to_string());
        let mut open_sibling = ws_at(&sibling_worktree);
        open_sibling.cached_git_branch = Some("feature/x".to_string());
        app2.workspaces = vec![open_member, open_sibling];
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
    fn section_header_kinds(entries: &[WorkspaceListEntry]) -> Vec<&'static SectionDescriptor> {
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
            [&COMMANDS, &CHECKS, &TODOS, &NOTES, &PULL_REQUESTS],
            "no declared order must resolve to today's fixed sequence"
        );
        assert_eq!(
            resolve_section_order(Some(&[])),
            [&COMMANDS, &CHECKS, &TODOS, &NOTES, &PULL_REQUESTS],
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
            [&NOTES, &PULL_REQUESTS, &TODOS, &CHECKS, &COMMANDS],
            "a full order (including the pull_requests wire name) must be honored exactly"
        );
    }

    #[test]
    fn section_order_resolve_partial_declaration_appends_unlisted_in_fixed_order() {
        let names = ["checks".to_string()];
        assert_eq!(
            resolve_section_order(Some(&names)),
            [&CHECKS, &COMMANDS, &TODOS, &NOTES, &PULL_REQUESTS],
            "the listed section leads, the other four follow in \
             registry order — nothing declared-but-unlisted is lost"
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
            [&CHECKS, &NOTES, &COMMANDS, &TODOS, &PULL_REQUESTS],
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
            [&CHECKS, &COMMANDS, &TODOS, &NOTES, &PULL_REQUESTS],
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
            vec![&NOTES, &TODOS, &CHECKS, &COMMANDS],
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
            vec![&TODOS, &NOTES, &COMMANDS, &CHECKS],
            "an undeclared order must render exactly today's fixed \
             sequence: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn worktree_bands_render_after_every_session_of_the_group() {
        // T7 (bora-79l, divergence D): COMANDO/CHECKS fecham o grupo, depois
        // de TODAS as sessions — nunca intercaladas sob a primeira workspace
        // (ALVO_CAPTURE rows 27-34: hotfix na row 28, bands nas rows 31-34).
        // Fica vermelho se uma band voltar a aparecer entre o SectionRow e
        // seu PaneDotsRow, ou antes do último bloco do grupo; e a adjacência
        // SR→PDR é a que o trailing-gap e o name-sync assumem.
        let repo = temp_test_dir("bands-after-sessions");
        init_fake_git_repo(&repo, None);

        let (_isolated, app) = app_with_full_bands(&repo, None);
        let entries = project_view_entries(&app, false);

        // Every SectionRow is immediately followed by its own PaneDotsRow —
        // the pair rule T7's gap logic and `sync_pane_dots_names` rely on.
        for (idx, entry) in entries.iter().enumerate() {
            if matches!(entry, WorkspaceListEntry::SectionRow { .. }) {
                let next = entries.get(idx + 1);
                assert!(
                    matches!(next, Some(WorkspaceListEntry::PaneDotsRow { .. })),
                    "a SectionRow must sit directly above its own \
                     PaneDotsRow, entry {idx}: {entries:?}"
                );
            }
        }

        let last_session = entries
            .iter()
            .rposition(|e| matches!(e, WorkspaceListEntry::PaneDotsRow { .. }))
            .expect("fixture must emit sessions");
        let first_worktree_band = entries
            .iter()
            .position(|e| {
                matches!(
                    e,
                    WorkspaceListEntry::SectionHeader { kind, .. }
                        if kind.level == SectionLevel::Worktree
                )
            })
            .expect("fixture must emit the worktree bands");
        assert!(
            first_worktree_band > last_session,
            "the worktree bands must come after EVERY session of the \
             group (band at {first_worktree_band}, last session at \
             {last_session}): {entries:?}"
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
            vec![&COMMANDS],
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
            section_band(&entries, &PULL_REQUESTS),
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
            section_band(&entries, &PULL_REQUESTS),
            Some((0, 0)),
            "an errored band still renders its header: {entries:?}"
        );
        assert_eq!(
            section_items(&entries, &PULL_REQUESTS),
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
            section_band(&entries, &PULL_REQUESTS),
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
            vec![&PULL_REQUESTS, &NOTES, &TODOS, &CHECKS, &COMMANDS,],
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
        // Pin the app's own row_gap to 0: pass 1 below hardcodes 0 in
        // its `entry_row_height` calls, and 6a made the app's gap also
        // apply after `ProjectRow` (the alvo's respiro) — a nonzero
        // app gap would make the visible-count pass need one more row
        // than the height pass budgeted, measuring drift that isn't
        // there.
        app.sidebar_project.row_gap = 0;

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
            .position(|e| matches!(e, WorkspaceListEntry::SectionRow { .. }))
            .expect("fixture must still emit the section row after the PR band");
        assert_eq!(
            worktree_idx,
            pr_row_idx + 1,
            "the WorktreeRow must land immediately after the PrRow: {entries:?}"
        );

        // Pass 1: height.
        let total_height: u16 = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| crate::ui::sidebar::entry_row_height(entry, &entries, idx, 0))
            .sum();
        let expected_total_height: u16 = (0..entries.len())
            .map(|idx| {
                crate::ui::sidebar::expected_entry_height(&entries, idx)
                    + crate::ui::sidebar::project_view_trailing_gap(&entries[idx], &entries, idx, 0)
            })
            .sum();
        assert_eq!(
            total_height, expected_total_height,
            "every row in this fixture must match its contracted height \
             (1, PaneDotsRow 2, padded project band R3) plus its trailing \
             gap row (R5): {entries:?}"
        );

        // Pass 2: visible-count agrees with the height pass.
        let width = 60;
        let exact = ratatui::layout::Rect::new(
            0,
            0,
            width,
            total_height + crate::ui::sidebar::WORKSPACE_LIST_TOP_MARGIN_ROWS + 1,
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
            total_height + crate::ui::sidebar::WORKSPACE_LIST_TOP_MARGIN_ROWS + 20,
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
        // R3: index is not row — the project band above is taller than one
        // row, so sum the spans of everything before it. R5: the spans
        // include the band's trailing gap row, so this walks
        // `entry_row_height` — the advance the geometry pass itself uses.
        let pr_row_y = body.y
            + (0..pr_row_idx)
                .map(|idx| crate::ui::sidebar::entry_row_height(&entries[idx], &entries, idx, 0))
                .sum::<u16>();
        let pr_area = project_rows
            .iter()
            .find(|area| area.rect.y == pr_row_y)
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
                    crate::app::state::ProjectRowTarget::Section { .. }
                )
            })
            .expect("worktree row must still get a hit area");
        assert_eq!(
            worktree_area.rect.y,
            body.y
                + (0..worktree_idx)
                    .map(|idx| {
                        crate::ui::sidebar::entry_row_height(&entries[idx], &entries, idx, 0)
                    })
                    .sum::<u16>(),
            "the WorktreeRow's hit area must be pushed down by exactly the \
             preceding rows' spans — heights plus R5's trailing gap rows"
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
        let render_y = crate::ui::sidebar::WORKSPACE_LIST_TOP_MARGIN_ROWS
            + (0..pr_row_idx)
                .map(|idx| crate::ui::sidebar::entry_row_height(&entries[idx], &entries, idx, 0))
                .sum::<u16>();
        let text = row_text(render_y);
        assert!(
            text.contains('7') && text.contains("pr 7"),
            "PR number and title must render at the row the height/geometry passes agree on: {text:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    // ── bora-by6 G2: the two-site cost of a sixth band ────────────────────

    /// Proves G2's claim by doing it: a descriptor and a push function that
    /// were never added to `REGISTRY`, exercised through the exact same
    /// `SectionPushCtx`/`(descriptor.push)(...)` mechanism
    /// `push_project_group`/`push_worktree` use for every real band. If
    /// this compiles and renders identically to a registered band while
    /// `REGISTRY` never grew, the two-edit-site claim is not a description
    /// of the design, it is a property of it.
    #[test]
    fn a_sixth_descriptor_renders_through_the_generic_rows_without_touching_the_registry() {
        fn push_throwaway_section(entries: &mut Vec<WorkspaceListEntry>, ctx: &SectionPushCtx) {
            let SectionPushCtx::Project { force_expanded, .. } = *ctx else {
                return;
            };
            entries.push(WorkspaceListEntry::SectionHeader {
                name: None,
                kind: &THROWAWAY,
                collapse_key: "sec:throwaway:proj".to_string(),
                done: 1,
                total: 2,
            });
            if !force_expanded {
                return;
            }
            entries.push(WorkspaceListEntry::SectionItem {
                kind: &THROWAWAY,
                label: "a throwaway row".to_string(),
                detail: None,
                running: false,
                ws_idx: None,
            });
        }

        static THROWAWAY: SectionDescriptor = SectionDescriptor {
            wire_name: "throwaway",
            glyph: "?",
            label: "THROWAWAY",
            level: SectionLevel::Project,
            counter: SectionCounter::Progress,
            bullet: SectionBullet::Standard,
            push: push_throwaway_section,
        };

        let before = REGISTRY.len();
        assert!(
            SectionDescriptor::from_wire_name("throwaway").is_none(),
            "the throwaway descriptor must be unreachable through the production \
             registry — this test's whole point is that it never joined it"
        );

        let app = AppState::test_new();
        let ctx = SectionPushCtx::Project {
            app: &app,
            slug: "proj",
            local_branches: &HashSet::new(),
            force_expanded: true,
        };
        let mut entries = Vec::new();
        (THROWAWAY.push)(&mut entries, &ctx);

        assert_eq!(
            entries.len(),
            2,
            "header + one item row through the exact generic \
             SectionHeader/SectionItem rows every real band uses: {entries:?}"
        );
        assert!(matches!(
            entries[0],
            WorkspaceListEntry::SectionHeader { kind, done: 1, total: 2, .. }
                if kind.wire_name == "throwaway"
        ));
        match &entries[1] {
            WorkspaceListEntry::SectionItem { kind, label, .. } => {
                assert_eq!(kind.wire_name, "throwaway");
                assert_eq!(label, "a throwaway row");
            }
            other => panic!("expected a SectionItem row: {other:?}"),
        }

        assert_eq!(
            REGISTRY.len(),
            before,
            "the production registry must not have gained an entry"
        );
    }

    #[test]
    fn two_projects_on_the_same_directory_keep_their_own_explicitly_bound_workspaces() {
        // The P0 this fixes: two declared projects whose members resolve
        // to the SAME checkout. Under the old single-pass `claimed`
        // logic, slug-alphabetical iteration hands BOTH workspaces to
        // whichever project sorts first ("alpha") and the other project
        // ("beta") renders empty — even though each workspace was created
        // under its own project. C2 pass 1 fixes this: a workspace's own
        // `project()` binding wins outright, regardless of directory or
        // slug order.
        let checkout = temp_test_dir("same-dir-two-projects");
        init_fake_git_repo(&checkout, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("alpha".to_string(), project(vec![member(&checkout)]));
        file.projects
            .insert("beta".to_string(), project(vec![member(&checkout)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws_bound_to_beta = ws_at(&checkout);
        ws_bound_to_beta.set_project(Some("beta".to_string()));
        let mut ws_bound_to_alpha = ws_at(&checkout);
        ws_bound_to_alpha.set_project(Some("alpha".to_string()));
        // ws0 is explicitly bound to "beta" even though "alpha" sorts
        // first alphabetically and both projects declare the same
        // directory.
        app.workspaces = vec![ws_bound_to_beta, ws_bound_to_alpha];

        let entries = project_view_entries(&app, false);

        assert_eq!(
            project_ws_idxs(&entries, "proj:beta"),
            vec![0],
            "ws0 (bound to beta) must land under beta, not alpha, even though alpha \
             sorts first and both declare the same directory: {entries:?}"
        );
        assert_eq!(
            project_ws_idxs(&entries, "proj:alpha"),
            vec![1],
            "ws1 (bound to alpha) must land under alpha, and only alpha — neither \
             group may double-count the other's workspace: {entries:?}"
        );

        std::fs::remove_dir_all(&checkout).unwrap();
    }

    #[test]
    fn workspace_bound_to_unknown_project_slug_falls_through_to_directory_derivation() {
        // A binding naming a slug no longer in `projects.yml` must never
        // orphan the workspace and must never panic (C2) — it falls
        // through to pass 2's directory derivation exactly like an
        // unbound workspace.
        let checkout = temp_test_dir("unknown-slug-fallback");
        init_fake_git_repo(&checkout, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("real".to_string(), project(vec![member(&checkout)]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws = ws_at(&checkout);
        ws.set_project(Some("ghost".to_string()));
        app.workspaces = vec![ws];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            project_ws_idxs(&entries, "proj:real"),
            vec![0],
            "a binding to a nonexistent slug must fall through to directory \
             derivation and land under the real project matching its dir: {entries:?}"
        );
        assert!(
            !entries.iter().any(|e| matches!(
                e,
                WorkspaceListEntry::ProjectRow {
                    declared: false,
                    ..
                }
            )),
            "the workspace must never be orphaned when directory derivation succeeds: {entries:?}"
        );

        std::fs::remove_dir_all(&checkout).unwrap();
    }

    #[test]
    fn unbound_workspace_on_a_directory_claimed_by_two_projects_picks_the_more_specific_member() {
        // No explicit binding on either side: pass 2's tiebreak must pick
        // the more specific matching member (`member_specificity`, C2),
        // not just whichever project sorts first. "aaa_broad" sorts
        // BEFORE "zzz_specific" alphabetically — if the tiebreak were
        // still "first slug wins" this would (wrongly) pick aaa_broad. It
        // must pick zzz_specific instead, because `WorktreesScope::This`
        // outranks `All`.
        let checkout = temp_test_dir("tiebreak-specificity");
        init_fake_git_repo(&checkout, None);

        let broad_member = member(&checkout); // WorktreesScope::All (default)
        let specific_member = Member {
            dir: checkout.display().to_string(),
            worktrees: WorktreesScope::This,
            template: None,
        };

        let mut file = ProjectsFile::default();
        file.projects
            .insert("aaa_broad".to_string(), project(vec![broad_member]));
        file.projects
            .insert("zzz_specific".to_string(), project(vec![specific_member]));
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&checkout)];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            project_ws_idxs(&entries, "proj:zzz_specific"),
            vec![0],
            "the more specific (This) member must win over the broader (All) one, \
             even though its project sorts LAST alphabetically: {entries:?}"
        );
        assert!(
            project_ws_idxs(&entries, "proj:aaa_broad").is_empty(),
            "the broader match must not also claim the same workspace: {entries:?}"
        );

        std::fs::remove_dir_all(&checkout).unwrap();
    }

    #[test]
    fn project_view_entries_is_deterministic_across_repeated_renders() {
        // Two-pass resolution must never leak `HashMap`/`HashSet`
        // iteration order into the emitted entries (C2): the same
        // `AppState`, rendered twice, must produce identical entries. Mixes
        // an explicit binding, an ambiguous (two-project) derived match,
        // an unambiguous derived match, and a fully unmatched workspace so
        // every branch of the resolution runs.
        let checkout_a = temp_test_dir("determinism-a");
        let checkout_b = temp_test_dir("determinism-b");
        init_fake_git_repo(&checkout_a, None);
        init_fake_git_repo(&checkout_b, None);

        let mut file = ProjectsFile::default();
        file.projects
            .insert("one".to_string(), project(vec![member(&checkout_a)]));
        file.projects.insert(
            "two".to_string(),
            project(vec![member(&checkout_a), member(&checkout_b)]),
        );
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;

        let mut ws_explicit = ws_at(&checkout_a);
        ws_explicit.set_project(Some("two".to_string()));
        app.workspaces = vec![
            ws_explicit,
            ws_at(&checkout_a),
            ws_at(&checkout_b),
            Workspace::test_new("unmatched"),
        ];

        let first = project_view_entries(&app, false);
        let second = project_view_entries(&app, false);
        assert_eq!(
            first, second,
            "rendering the same AppState twice must produce identical entries: \
             first={first:?} second={second:?}"
        );

        std::fs::remove_dir_all(&checkout_a).unwrap();
        std::fs::remove_dir_all(&checkout_b).unwrap();
    }

    /// Backs the manual `PartialEq`/`Eq` impl on `SectionDescriptor`
    /// (`sidebar.rs`): it compares `wire_name` alone, which is only sound
    /// while every registry entry's wire name is unique.
    #[test]
    fn registry_wire_names_are_unique() {
        let mut seen = HashSet::new();
        for section in REGISTRY {
            assert!(
                seen.insert(section.wire_name),
                "duplicate wire_name {:?} in REGISTRY: {:?}",
                section.wire_name,
                REGISTRY.iter().map(|d| d.wire_name).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn declared_comando_section_renders_header_and_its_items() {
        // Bead 6b: a declared `SectionKind::Comando` section (not the
        // live COMMANDS band from `sections.commands`) renders through
        // the exact same generic `SectionHeader`/`SectionItem` rows —
        // every child gets a row, failing or not (unlike the live CHECKS
        // band's failing-only filter), and `Section.name` overrides the
        // descriptor's static label.
        let repo = temp_test_dir("layout-comando");
        init_fake_git_repo(&repo, None);

        let mut declared = project(vec![member(&repo)]);
        declared.layout = Some(vec![Section {
            id: "sec-comando".to_string(),
            kind: SectionKind::Comando,
            name: Some("Deploy".to_string()),
            header_on: true,
            parts: crate::ui::sidebar::sections::SectionParts::default(),
            children: vec![
                SectionChild::Item {
                    label: "build".to_string(),
                    failing: false,
                },
                SectionChild::Item {
                    label: "lint".to_string(),
                    failing: true,
                },
            ],
        }]);
        let mut file = ProjectsFile::default();
        file.projects.insert("proj".to_string(), declared);
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, &COMMANDS),
            Some((1, 2)),
            "declared Comando section header: done counts non-failing \
             children, total counts every child: {entries:?}"
        );
        assert_eq!(
            section_items(&entries, &COMMANDS),
            vec!["build".to_string(), "lint".to_string()],
            "one SectionItem per declared child, failing or not — the \
             generic rows, never the live CHECKS band's failing-only \
             filter: {entries:?}"
        );
        let header_name = entries.iter().find_map(|e| match e {
            WorkspaceListEntry::SectionHeader { kind, name, .. }
                if kind.wire_name == "commands" =>
            {
                Some(name.clone())
            }
            _ => None,
        });
        assert_eq!(
            header_name,
            Some(Some("Deploy".to_string())),
            "Section.name must land on the emitted SectionHeader entry: {entries:?}"
        );

        // The rendered row must actually use the declared name, not the
        // descriptor's static "COMANDO" label.
        app.ensure_test_terminals();
        let area = ratatui::layout::Rect::new(0, 0, 40, 20);
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(area.width, area.height))
                .expect("test terminal");
        terminal
            .draw(|frame| {
                crate::ui::sidebar::render_workspace_list(&app, &runtimes, frame, area, false)
            })
            .expect("workspace list should render");
        // Scan every rendered row rather than deriving the header's row
        // from its entry index: heights are not uniform (a `PaneDotsRow`
        // is two rows tall) and the list also honours `row_gap`, so an
        // index-derived offset lands on a neighbouring line and asserts
        // nothing. Exactly one row must carry the declared name, and the
        // descriptor's static label must appear nowhere.
        let rendered: Vec<String> = (0..area.height)
            .map(|row| {
                (0..area.width)
                    .map(|col| terminal.backend().buffer()[(col, row)].symbol().to_string())
                    .collect()
            })
            .collect();
        assert_eq!(
            rendered
                .iter()
                .filter(|line| line.contains("Deploy"))
                .count(),
            1,
            "exactly one rendered row carries the declared section name: {rendered:?}"
        );
        assert!(
            !rendered.iter().any(|line| line.contains(COMMANDS.label)),
            "the declared name replaces the descriptor's static label {:?}, \
             it does not render beside it: {rendered:?}",
            COMMANDS.label
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn declared_livre_section_with_no_children_renders_header_alone() {
        // Bead 6b: LIVRE is "the empty, mountable slot" — a section with
        // zero children still renders its header (that is the point of
        // the kind) and pushes no item rows.
        let repo = temp_test_dir("layout-livre");
        init_fake_git_repo(&repo, None);

        let mut declared = project(vec![member(&repo)]);
        declared.layout = Some(vec![Section {
            id: "sec-livre".to_string(),
            kind: SectionKind::Livre,
            name: None,
            header_on: true,
            parts: crate::ui::sidebar::sections::SectionParts::default(),
            children: Vec::new(),
        }]);
        let mut file = ProjectsFile::default();
        file.projects.insert("proj".to_string(), declared);
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, &LIVRE),
            Some((0, 0)),
            "an empty Livre section still renders its header alone: {entries:?}"
        );
        assert!(
            section_items(&entries, &LIVRE).is_empty(),
            "no children -> no item rows: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn declared_section_header_off_suppresses_header_keeps_items() {
        // Bead 6b: `header_on: false` on a non-Branch section suppresses
        // only the SectionHeader row — same rule the Branch path already
        // obeys (`header_off_from_the_model_hides_the_header_keeps_the_blocks`).
        let repo = temp_test_dir("layout-header-off");
        init_fake_git_repo(&repo, None);

        let mut declared = project(vec![member(&repo)]);
        declared.layout = Some(vec![Section {
            id: "sec-checks".to_string(),
            kind: SectionKind::Checks,
            name: None,
            header_on: false,
            parts: crate::ui::sidebar::sections::SectionParts::default(),
            children: vec![SectionChild::Item {
                label: "clippy".to_string(),
                failing: true,
            }],
        }]);
        let mut file = ProjectsFile::default();
        file.projects.insert("proj".to_string(), declared);
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.projects = store;
        app.workspaces = vec![ws_at(&repo)];

        let entries = project_view_entries(&app, false);
        assert_eq!(
            section_band(&entries, &CHECKS),
            None,
            "header_on: false must suppress the SectionHeader row: {entries:?}"
        );
        assert_eq!(
            section_items(&entries, &CHECKS),
            vec!["clippy".to_string()],
            "header OFF hides only the header, never the section's items: {entries:?}"
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }

    #[test]
    fn declared_dots_off_collapses_pane_dots_row_to_one_line_across_every_pass() {
        // Bead 6b: `SectionParts.dots` is the l2 pane-dots line toggle.
        // OFF must shrink a `PaneDotsRow` block from 2 rows to 1
        // everywhere at once — height, geometry (card rect height AND no
        // per-dot hit area), and render (no l2 text) — or the three
        // lockstep passes desync (sidebar.rs's "Shared row-height"
        // contract).
        let repo = temp_test_dir("layout-dots-off");
        init_fake_git_repo(&repo, None);
        let checkout_key = crate::workspace::git_space_metadata(&repo)
            .expect("fake checkout has git space")
            .checkout_key;

        let mut declared = project(vec![member(&repo)]);
        declared.layout = Some(vec![Section {
            id: "sec-branch".to_string(),
            kind: SectionKind::Branch,
            name: None,
            header_on: true,
            parts: crate::ui::sidebar::sections::SectionParts {
                dots: false,
                diff: true,
            },
            children: vec![SectionChild::Workspace {
                name: "alpha".to_string(),
                checkout: checkout_key,
            }],
        }]);
        let mut file = ProjectsFile::default();
        file.projects.insert("proj".to_string(), declared);
        let (_isolated, store) = store_with(file);
        let mut app = AppState::test_new();
        app.view_mode = crate::config::ViewMode::Project;
        app.projects = store;
        app.sidebar_project.row_gap = 0;
        let mut ws = ws_at(&repo);
        ws.custom_name = Some("alpha".to_string());
        app.workspaces = vec![ws];
        app.ensure_test_terminals();

        let entries = crate::ui::sidebar::workspace_list_entries(&app);
        let pane_dots_idx = entries
            .iter()
            .position(|e| matches!(e, WorkspaceListEntry::PaneDotsRow { .. }))
            .expect("fixture must emit a PaneDotsRow");
        assert!(
            matches!(
                entries[pane_dots_idx],
                WorkspaceListEntry::PaneDotsRow { dots: false, .. }
            ),
            "parts.dots: false must land on the emitted entry itself: {entries:?}"
        );

        // Pass 1: height.
        let total_height: u16 = entries
            .iter()
            .enumerate()
            .map(|(idx, entry)| crate::ui::sidebar::entry_row_height(entry, &entries, idx, 0))
            .sum();
        assert_eq!(
            crate::ui::sidebar::entry_row_height(
                &entries[pane_dots_idx],
                &entries,
                pane_dots_idx,
                0
            ),
            1,
            "dots off collapses the block to its l1 name line alone: {entries:?}"
        );

        // Pass 2: visible-count agrees with the height pass.
        let width = 40;
        let exact = ratatui::layout::Rect::new(
            0,
            0,
            width,
            total_height + crate::ui::sidebar::WORKSPACE_LIST_TOP_MARGIN_ROWS + 1,
        );
        assert_eq!(
            crate::ui::sidebar::workspace_list_visible_count(&app, exact, 0),
            entries.len(),
            "visible-count pass must agree with the height pass"
        );

        // Pass 3: geometry — the card rect is 1 row tall and no per-dot
        // hit area is emitted (there is no l2 row to click a dot on).
        let sidebar_area = ratatui::layout::Rect::new(
            0,
            0,
            width,
            total_height + crate::ui::sidebar::WORKSPACE_LIST_TOP_MARGIN_ROWS + 20,
        );
        let (cards, _headers, project_rows) =
            crate::ui::sidebar::compute_workspace_list_areas_all(&app, sidebar_area);
        let card = cards
            .iter()
            .find(|c| c.ws_idx == 0)
            .expect("the PaneDotsRow block must still push a card");
        assert_eq!(
            card.rect.height, 1,
            "dots off shrinks the card rect to 1 row: {card:?}"
        );
        assert!(
            !project_rows
                .iter()
                .any(|a| matches!(a.target, crate::app::state::ProjectRowTarget::Pane { .. })),
            "no l2 row means no per-dot hit area: {project_rows:?}"
        );

        // Pass 4: render — l1 still draws the name, l2 draws nothing (this
        // fixture's PaneDotsRow is the last entry, so its l2 row is blank
        // terminal space, not another row's content).
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
        // R3: the project band above this block is taller than one row,
        // so the block's first row is the SUM of the preceding row spans,
        // never the entry index (AGENTS.md: "an entry's index is not its
        // buffer row"). R5: the spans include the trailing gap rows (the
        // plain separator after each band), which belong to no entry's
        // contracted height — so this walk uses `entry_row_height`, the
        // same advance the render and geometry passes use.
        let l1_y = crate::ui::sidebar::WORKSPACE_LIST_TOP_MARGIN_ROWS
            + (0..pane_dots_idx)
                .map(|idx| crate::ui::sidebar::entry_row_height(&entries[idx], &entries, idx, 0))
                .sum::<u16>();
        assert!(
            row_text(l1_y).contains("alpha"),
            "l1 must still render the block's name: {:?}",
            row_text(l1_y)
        );
        let l2_y = l1_y + 1;
        assert!(
            row_text(l2_y).trim().is_empty(),
            "l2 must draw nothing when dots are off: {:?}",
            row_text(l2_y)
        );

        std::fs::remove_dir_all(&repo).unwrap();
    }
}
