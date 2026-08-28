//! Wire types for the `project.*` socket verbs — CRUD over
//! `~/.config/bora/projects.yml` (see `persist::projects`) plus member
//! management. Handlers live in `app::api::projects`.

use serde::{Deserialize, Serialize};

/// `project.create`: adds a new, empty project (no members) at `slug`.
/// Errors `project_exists` when `slug` is already taken — this is never a
/// silent overwrite; use `project.update` to change an existing project's
/// `name`/`channel`, and `project.member_add` to populate its members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectCreateParams {
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Explicit channel override; omitted falls back to `"#" + slug` — see
    /// `persist::projects::Project::effective_channel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    /// bora-1le.1: opts every member of this project out of auto-join when
    /// `Some(false)`. Omitted (or `Some(true)`) keeps the default: an agent
    /// started in a member workspace auto-joins `channel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_join: Option<bool>,
}

/// `project.update`: replaces `name`, `channel`, and `auto_join` wholesale
/// with whatever this request carries — a full replacement of exactly
/// those fields, not a partial "only touch what's `Some`" patch. Omitting
/// `name`/`channel` clears them back to unset; omitting `auto_join` resets
/// it back to its default (`true`) — none of the three mean "leave
/// unchanged". Never touches `members` — that is `project.member_add`/
/// `project.member_remove`'s job — nor `orchestrator`/`sections`, which
/// have no verb yet in this bead. Errors `project_not_found` when `slug`
/// does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectUpdateParams {
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_join: Option<bool>,
}

/// `project.member_add`: idempotent on `dir` — an exact string match
/// against an existing member's `dir` (the same key `project.member_remove`
/// matches on) updates that member's `worktrees` scope in place rather than
/// appending a duplicate row. Errors `project_not_found` when `slug` does
/// not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectMemberAddParams {
    pub slug: String,
    pub dir: String,
    #[serde(default)]
    pub worktrees: crate::persist::projects::WorktreesScope,
}

/// `project.member_remove`: errors `project_member_not_found` (naming
/// `slug` and `dir`) when `dir` is not a member of `slug`, so a caller
/// never thinks it removed something it did not. Errors `project_not_found`
/// when `slug` does not exist at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectMemberRemoveParams {
    pub slug: String,
    pub dir: String,
}

/// `project.section_create`: appends a new
/// [`crate::ui::sidebar::sections::Section`] to `slug`'s mountable
/// `layout:` (epic bora-79l, T6 pass 6b — see `app::sections::
/// create_section`). `name` is used verbatim when given; omitted, the
/// section gets a random two-word display name rather than a bare `None`
/// header. Errors `project_not_found` when `slug` does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectSectionCreateParams {
    pub slug: String,
    pub kind: crate::ui::sidebar::sections::SectionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `project.section_update`: applies `header_on`/`dots`/`diff` (each
/// `None` leaves that field untouched) to one section of `slug`'s
/// `layout:`, addressed either by its pinned `section_id` or by
/// `checkout` — see `app::sections::update_section`. Addressing by
/// `checkout` MATERIALIZES a fresh Branch section carrying that checkout
/// when `slug` declares no layout yet (or none of its sections name that
/// checkout), which is what makes the toggle work against a real
/// `projects.yml` with no `layout:` today. Errors `project_not_found`
/// when `slug` does not exist, `project_section_target_invalid` when
/// neither `section_id` nor `checkout` is given, and
/// `project_section_not_found` when `section_id` names a section that
/// does not exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectSectionUpdateParams {
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_on: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dots: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<bool>,
}

/// `project.list`'s per-project entry, and the `project` payload every
/// other `project.*` verb returns: the parsed project plus each member's
/// RESOLVED identity (via `persist::projects::Member::resolve`), so a
/// caller never has to redo git discovery itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectSummary {
    pub slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Always present: `channel` override, or `"#" + slug` — see
    /// `persist::projects::Project::effective_channel`. A caller never has
    /// to re-derive the default.
    pub channel: String,
    /// Effective `auto_join` (bora-1le.1) — always present, resolved from
    /// `Project::auto_join`'s own default, so a caller never has to
    /// re-derive it either.
    pub auto_join: bool,
    pub members: Vec<ProjectMemberInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectMemberInfo {
    pub dir: String,
    pub worktrees: crate::persist::projects::WorktreesScope,
    pub resolution: ProjectMemberResolution,
}

/// Wire shape of `persist::projects::MemberResolution`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ProjectMemberResolution {
    Resolved {
        repo_identity: String,
        checkout_key: String,
        /// Empty when the member dir *is* the checkout root.
        subdir: String,
    },
    Unresolved {
        dir: String,
        reason: String,
    },
}
