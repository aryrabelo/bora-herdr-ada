//! Store for `~/.config/bora/projects.yml` — the "Composition model" declaration
//! from `.local/prd/sidebar-design.md`: a project groups member directories
//! (repos or subdirectories of repos, derived via git discovery) under a name,
//! a channel, an optional orchestrator, and per-project check/command sections.
//!
//! The sidebar is the only *unconditional* reader; the socket verbs in
//! `app::api::projects` (bora-e9i.2) are the only writer, and re-read this
//! module's `load_projects_file_fresh` immediately before every mutation
//! rather than trusting any cached `ProjectsStore` value. MCP tools and an
//! editor are later beads in epic bora-e9i.
//!
//! YAML: `serde_yaml_ng`, not `serde_yaml`. The repo had zero YAML
//! dependencies before this; the design mandates YAML for this file, and
//! upstream `serde_yaml` is archived/deprecated while `serde_yaml_ng` is a
//! maintained drop-in with the same API.
//!
//! "File watch" is mtime+len polling, not a filesystem-watch dependency —
//! this repo has no such crate and already has the idiom (see
//! `detect::manifest::reload_manifests`). `ProjectsStore::reload_if_changed`
//! is cheap enough to call every tick: one `stat`, no allocation, on the
//! unchanged path.
//!
//! A parse error never replaces the last good value — the caller (sidebar)
//! surfaces the error as a toast and keeps rendering whatever last parsed
//! cleanly. This module never panics and never `unwrap()`s.
//!
//! Writes go through [`write_projects_file`]: serialize, write a sibling
//! `.tmp`, then rename — same idiom as
//! `persist::pending_prompts::write_pending_prompts` — so
//! `ProjectsStore::reload_if_changed`'s mtime/len poll never observes a
//! half-written file. [`update_projects_file`] wraps that with a fresh
//! read-modify-write: it reads the CURRENT on-disk content immediately
//! before applying the mutation, never a stale in-memory copy, so a socket
//! verb handler can never clobber another handler's edit with a write that
//! started from data it read a while ago.

// bora-e9i.1 landed the store, resolver, and reload trigger. bora-e9i.2
// (this bead) adds the write path the socket verbs in `app::api::projects`
// call. MCP tools and sidebar rendering are later beads in epic bora-e9i.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// `~/.config/bora/projects.yml` (namespace-aware via `config_dir()`).
pub fn projects_file_path() -> PathBuf {
    crate::config::config_dir().join("projects.yml")
}

/// Top-level document: shared defaults plus the project map, keyed by slug
/// (the map key is the `cnb` / `bora` in `projects.<slug>:`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ProjectsFile {
    #[serde(default)]
    pub defaults: ProjectDefaults,
    #[serde(default)]
    pub projects: BTreeMap<String, Project>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectDefaults {
    #[serde(default = "default_open_with")]
    pub open_with: String,
    /// Provider list, in order (e.g. `[gh]`).
    #[serde(default)]
    pub checks: Vec<String>,
    /// `all`, or a list of command names that projects narrow.
    #[serde(default)]
    pub commands: CommandsScope,
}

fn default_open_with() -> String {
    "bora workspace open".to_string()
}

impl Default for ProjectDefaults {
    fn default() -> Self {
        Self {
            open_with: default_open_with(),
            checks: Vec::new(),
            commands: CommandsScope::All,
        }
    }
}

/// `commands: all` or `commands: [dev, test]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum CommandsScope {
    #[default]
    All,
    List(Vec<String>),
}

impl Serialize for CommandsScope {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            CommandsScope::All => serializer.serialize_str("all"),
            CommandsScope::List(items) => items.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for CommandsScope {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Named(String),
            List(Vec<String>),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Named(name) if name == "all" => Ok(CommandsScope::All),
            Raw::Named(other) => Err(serde::de::Error::custom(format!(
                "expected \"all\" or a list of command names, found string {other:?}"
            ))),
            Raw::List(items) => Ok(CommandsScope::List(items)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Explicit override; falls back to `"#" + slug` — see `effective_channel`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default)]
    pub members: Vec<Member>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orchestrator: Option<Orchestrator>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Sections>,
    /// bora-1le.1 decision (`.local/prd/sidebar-design.md`, "Project =
    /// channel — resolved"): `false` opts every member of this project out
    /// of auto-join. An agent started in a member workspace then never
    /// joins `effective_channel` on its own; `channel.join` still works by
    /// hand. Defaults to `true` so a bare `members:` project keeps working
    /// with zero setup, per decision #8.
    #[serde(
        default = "default_auto_join",
        skip_serializing_if = "is_default_auto_join"
    )]
    pub auto_join: bool,
}

fn default_auto_join() -> bool {
    true
}

fn is_default_auto_join(value: &bool) -> bool {
    *value
}

impl Project {
    /// `channel`, or `"#" + slug` when unset. `slug` is the project's key in
    /// `ProjectsFile::projects`, not stored on `Project` itself.
    pub fn effective_channel(&self, slug: &str) -> String {
        self.channel.clone().unwrap_or_else(|| format!("#{slug}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    /// A directory, `~` expanded on resolve. The repo, checkout, and subdir
    /// are all derived from it — see `resolve_member`.
    pub dir: String,
    #[serde(default)]
    pub worktrees: WorktreesScope,
    /// bora-1le.3 (sidebar-design decision #10): a herdr-plus template
    /// name this member opens with — substituted for `<template>` in
    /// `defaults.open_with` by [`resolve_open_with`]. Absent member or
    /// absent opener falls back to bora's own open; never a hard
    /// dependency on herdr-plus.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
}

impl Member {
    pub fn resolve(&self) -> MemberResolution {
        resolve_member(&self.dir)
    }
}

/// Which command line opens a project member (bora-1le.3, sidebar-design
/// decision #10: herdr-plus is detect-and-integrate, never a hard
/// dependency).
///
/// The shape: take `defaults.open_with`, substitute a per-member
/// `template:` for the literal `<template>` placeholder, and use the result
/// only when the caller reports the named opener program as available.
/// Anything else — opener absent, empty command — falls back to bora's own
/// open ([`default_open_with`]), once, with no error to surface. Detection
/// is the caller's concern (`opener_available`); this function is the pure
/// decision so it stays testable without an installed herdr-plus.
pub fn resolve_open_with(
    defaults: &ProjectDefaults,
    member: Option<&Member>,
    opener_available: impl Fn(&str) -> bool,
) -> String {
    let mut command = defaults.open_with.clone();
    if let Some(template) = member.and_then(|member| member.template.as_deref()) {
        command = command.replace("<template>", template);
    }
    let Some(program) = command.split_whitespace().next() else {
        return default_open_with();
    };
    if opener_available(program) {
        command
    } else {
        default_open_with()
    }
}

/// Derives `schemars::JsonSchema` in addition to the file-schema traits
/// because `api::schema::projects::ProjectMemberInfo` (the `project.list`
/// wire shape) reuses this type directly rather than mirroring it — the
/// same pattern `ResponseResult::ConfigReload` already uses for
/// `config::ConfigReloadStatus`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, schemars::JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum WorktreesScope {
    #[default]
    All,
    This,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Orchestrator {
    pub agent: String,
    /// A member `dir:` value — the orchestrator's own working checkout.
    pub member: String,
    pub run_file: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sections {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checks: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commands: Option<Vec<String>>,
    /// Declares the render order of the four section bands
    /// (`commands`/`checks`/`todos`/`notes`, case-insensitive). Absent or
    /// empty resolves to today's fixed order
    /// (`ui::sidebar::ProjectSection::ALL`); see
    /// `ui::sidebar::project_view::resolve_section_order` for the full
    /// contract (bora-5ia).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<Vec<String>>,
}

pub fn parse_projects_yaml(raw: &str) -> Result<ProjectsFile, String> {
    serde_yaml_ng::from_str(raw).map_err(|err| err.to_string())
}

pub fn to_yaml(file: &ProjectsFile) -> Result<String, String> {
    serde_yaml_ng::to_string(file).map_err(|err| err.to_string())
}

/// Reads and parses `projects.yml` fresh from disk — never a cached
/// `ProjectsStore` value. Every `project.*` socket verb (read or write)
/// goes through this, so a handler's answer or write always starts from
/// the current on-disk content, not a copy that may already be stale. A
/// missing file reads as an empty `ProjectsFile`, the same convention
/// `ProjectsStore::load` uses for "nothing written yet".
pub fn load_projects_file_fresh() -> Result<ProjectsFile, String> {
    let path = projects_file_path();
    match fs::read_to_string(&path) {
        Ok(raw) => parse_projects_yaml(&raw),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(ProjectsFile::default()),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// Serializes `file` to YAML and writes it to `projects_file_path()` via a
/// sibling `.tmp` + rename — same idiom as
/// `persist::pending_prompts::write_pending_prompts` — so
/// `ProjectsStore::reload_if_changed`'s mtime/len poll never observes a
/// half-written file.
pub fn write_projects_file(file: &ProjectsFile) -> io::Result<()> {
    let path = projects_file_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = to_yaml(file).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let tmp_path = path.with_extension("yml.tmp");
    {
        let mut tmp = fs::File::create(&tmp_path)?;
        tmp.write_all(body.as_bytes())?;
        tmp.flush()?;
    }
    fs::rename(&tmp_path, &path)
}

/// Which stage of [`update_projects_file`] failed: reading/parsing the
/// current file before the mutation, the mutation closure's own business
/// rule (`E` — a verb-specific validation failure), or writing the result
/// back to disk.
#[derive(Debug)]
pub enum ProjectsUpdateError<E> {
    Load(String),
    Mutate(E),
    Save(String),
}

/// Read-modify-write `projects.yml`: reads the CURRENT file fresh from disk
/// via [`load_projects_file_fresh`] (never a stale in-memory copy), applies
/// `mutate`, then writes the result back atomically via
/// [`write_projects_file`]. `mutate` returning `Err` aborts before any
/// write, so a rejected mutation (duplicate slug, unknown project, ...)
/// never touches the file on disk.
///
/// This is not a cross-process lock: two truly concurrent writers can still
/// each read before either writes, and the second write to land wins,
/// silently overwriting the first's edit. What it does guarantee is the two
/// things the acceptance criteria for the socket verbs ask for: a reader
/// (the sidebar's poll) never observes a half-written file, because the
/// write is a rename over a fully-written temp file; and a write never
/// starts from a handler-local cache that could already be behind the
/// file another handler just wrote, because every call re-reads from disk
/// first. Within this process, `app::api` dispatches one request at a
/// time, so that races-within-the-app-process case does not arise in
/// practice — this still holds against an external writer (a second `bora`
/// process, an editor) touching the same file concurrently.
pub fn update_projects_file<E>(
    mutate: impl FnOnce(&mut ProjectsFile) -> Result<(), E>,
) -> Result<ProjectsFile, ProjectsUpdateError<E>> {
    let mut file = load_projects_file_fresh().map_err(ProjectsUpdateError::Load)?;
    mutate(&mut file).map_err(ProjectsUpdateError::Mutate)?;
    write_projects_file(&file).map_err(|err| ProjectsUpdateError::Save(err.to_string()))?;
    Ok(file)
}

// ── Resolver: member dir -> (repo_identity, checkout_key, subdir) ─────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMember {
    pub repo_identity: String,
    pub checkout_key: String,
    /// Empty when the member dir *is* the checkout root.
    pub subdir: PathBuf,
    /// The declaring `Member.worktrees` (bora-qdi). `resolve_member` has no
    /// `Member` context — only a bare `dir: &str` — so it always sets this
    /// to `WorktreesScope::default()`; `ProjectsStore::resolve_all` is the
    /// one place that has the real `Member` and overwrites it there. Every
    /// other caller of `resolve_member`/`Member::resolve()`
    /// (`app::agents::member_covers`, `app::api::projects`) only ever reads
    /// `repo_identity`/`checkout_key`/`subdir`, so leaving their call sites
    /// pointed at the default is harmless.
    pub worktrees: WorktreesScope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberResolution {
    Resolved(ResolvedMember),
    /// A member `dir:` that does not exist or is not inside a git checkout.
    /// Never a panic and never a silent drop — `reason` is shown to the user
    /// wherever the member would otherwise have rendered.
    Unresolved {
        dir: PathBuf,
        reason: String,
    },
}

pub fn resolve_member(dir: &str) -> MemberResolution {
    let expanded = crate::worktree::expand_tilde_path(dir);
    if !expanded.exists() {
        return MemberResolution::Unresolved {
            dir: expanded,
            reason: "path does not exist".to_string(),
        };
    }
    let Some(meta) = crate::workspace::git_space_metadata(&expanded) else {
        return MemberResolution::Unresolved {
            dir: expanded,
            reason: "not a git checkout".to_string(),
        };
    };
    let repo_root = canonicalize_best_effort(&meta.repo_root);
    let member_canonical = canonicalize_best_effort(&expanded);
    let subdir = member_canonical
        .strip_prefix(&repo_root)
        .map(Path::to_path_buf)
        .unwrap_or_default();
    MemberResolution::Resolved(ResolvedMember {
        repo_identity: meta.repo_identity,
        checkout_key: meta.checkout_key,
        subdir,
        worktrees: WorktreesScope::default(),
    })
}

fn canonicalize_best_effort(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

// ── Store: mtime-polled reload, last-good-on-error ─────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    fn read(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        Some(Self {
            modified: meta.modified().ok(),
            len: meta.len(),
        })
    }
}

#[derive(Debug)]
pub struct ProjectsStore {
    path: PathBuf,
    value: ProjectsFile,
    stamp: Option<FileStamp>,
    /// `value`'s members, resolved to checkout identity. Recomputed only when
    /// the file changes; see `resolved_members`.
    resolved: std::collections::HashMap<String, Vec<ResolvedMember>>,
}

impl ProjectsStore {
    /// Loads from `projects_file_path()`. A missing or malformed file yields
    /// an empty store rather than failing — there is no "last good" before
    /// the first successful parse.
    pub fn load() -> Self {
        Self::at(projects_file_path())
    }

    /// A store bound to no file: `reload_if_changed` stays `Ok(false)` forever
    /// and nothing is ever read from disk. Unit tests use this so
    /// `AppState::test_new()` never sees the operator's real `projects.yml`.
    pub fn empty() -> Self {
        Self {
            path: PathBuf::new(),
            value: ProjectsFile::default(),
            stamp: None,
            resolved: std::collections::HashMap::new(),
        }
    }

    fn at(path: PathBuf) -> Self {
        let mut store = Self {
            path,
            value: ProjectsFile::default(),
            stamp: None,
            resolved: std::collections::HashMap::new(),
        };
        let _ = store.reload_if_changed();
        store
    }

    pub fn current(&self) -> &ProjectsFile {
        &self.value
    }

    /// A project's declared members, already resolved to checkout identity.
    ///
    /// Resolution walks the filesystem (`Member::resolve` -> git discovery), so
    /// it happens HERE — once per `projects.yml` change, off the tick — and
    /// never on the render path. The sidebar's entry builder runs per render,
    /// per pane, per attached client; resolving there would mean dozens of git
    /// discoveries per frame (AGENTS.md, "Multiplicative performance paths":
    /// the cost is the frequency, not the cardinality).
    pub fn resolved_members(&self, slug: &str) -> &[ResolvedMember] {
        self.resolved.get(slug).map_or(&[], Vec::as_slice)
    }

    fn resolve_all(&mut self) {
        self.resolved = self
            .value
            .projects
            .iter()
            .map(|(slug, project)| {
                let members = project
                    .members
                    .iter()
                    .filter_map(|member| match member.resolve() {
                        MemberResolution::Resolved(mut resolved) => {
                            // `resolve_member` only sees `dir: &str`, so it
                            // cannot know the declaring `Member`'s scope
                            // (bora-qdi) — attach it here, the one place that
                            // has both the resolution and the `Member`.
                            resolved.worktrees = member.worktrees;
                            Some(resolved)
                        }
                        // An unresolved member (bad path, not a checkout)
                        // contributes no matches rather than failing the whole
                        // project. Surfacing `reason` to the user is a later bead.
                        MemberResolution::Unresolved { .. } => None,
                    })
                    .collect();
                (slug.clone(), members)
            })
            .collect();
    }

    /// Cheap on the unchanged path: one `stat`, no allocation, no read. Safe
    /// to call from the tick loop every frame.
    ///
    /// `Ok(true)`: the file's mtime/len changed and (if present) parsed
    /// successfully — `current()` reflects the new value.
    /// `Ok(false)`: nothing changed; `current()` is unchanged.
    /// `Err`: the file changed but failed to parse. `current()` still
    /// returns the last good value; the caller surfaces `Err` as a toast.
    pub fn reload_if_changed(&mut self) -> Result<bool, String> {
        let stamp = FileStamp::read(&self.path);
        if stamp == self.stamp {
            return Ok(false);
        }
        self.stamp = stamp;
        if stamp.is_none() {
            // Removed (or never existed): nothing new to read. Keep the last
            // good value rather than reverting to empty defaults.
            return Ok(true);
        }
        let raw = fs::read_to_string(&self.path)
            .map_err(|err| format!("{}: {err}", self.path.display()))?;
        match parse_projects_yaml(&raw) {
            Ok(parsed) => {
                self.value = parsed;
                self.resolve_all();
                Ok(true)
            }
            Err(err) => Err(format!("{}: {err}", self.path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IsolatedDirs;

    // `r##` because the YAML contains `"#cnb"`, and `"#` would close an `r#"` literal.
    const EXAMPLE_YAML: &str = r##"
defaults:
  open_with: "bora workspace open"     # or "herdr-plus open <template>" when installed
  checks: [gh]                         # provider list, in order
  commands: all                        # or a list; projects narrow it

projects:
  cnb:
    name: CNB
    channel: "#cnb"                    # default: "#" + slug
    members:
      - dir: ~/Sites/cnb_landing_page  # repo derived; worktrees: all (default)
      - dir: ~/Sites/cnb_hono
      - dir: ~/Sites/cnb_hono/packages/landing   # subdir member, own row
    orchestrator:
      agent: claude
      member: ~/Sites/cnb_hono
      run_file: .bora/run.json         # dagr, when installed
    sections:
      checks: [gh, gia]                # providers for THIS project
      commands: [dev, test]            # subset of the repos' declared commands
  bora:
    members:
      - dir: ~/Sites/bora
      - dir: ~/Sites/wt
"##;

    fn write_raw_projects_file(raw: &str) {
        let path = projects_file_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, raw).unwrap();
    }

    /// A directory with a minimal `.git/HEAD`, enough for `git_space_metadata`
    /// (no real `git` binary needed — mirrors the fixture style already used
    /// in `workspace::git::discovery`'s own tests).
    fn init_fake_git_repo(root: &Path) {
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bora-projects-fixture-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_the_design_example() {
        let parsed = parse_projects_yaml(EXAMPLE_YAML).expect("example yaml must parse");

        assert_eq!(parsed.defaults.open_with, "bora workspace open");
        assert_eq!(parsed.defaults.checks, vec!["gh".to_string()]);
        assert_eq!(parsed.defaults.commands, CommandsScope::All);

        let cnb = parsed.projects.get("cnb").expect("cnb project present");
        assert!(
            cnb.auto_join,
            "auto_join must default to true when the field is absent from yaml"
        );
        assert_eq!(cnb.name.as_deref(), Some("CNB"));
        assert_eq!(cnb.channel.as_deref(), Some("#cnb"));
        assert_eq!(cnb.effective_channel("cnb"), "#cnb");
        assert_eq!(cnb.members.len(), 3);
        assert_eq!(cnb.members[0].dir, "~/Sites/cnb_landing_page");
        assert_eq!(
            cnb.members[0].worktrees,
            WorktreesScope::All,
            "worktrees default"
        );
        assert_eq!(cnb.members[2].dir, "~/Sites/cnb_hono/packages/landing");
        let orchestrator = cnb.orchestrator.as_ref().expect("orchestrator present");
        assert_eq!(orchestrator.agent, "claude");
        assert_eq!(orchestrator.member, "~/Sites/cnb_hono");
        assert_eq!(orchestrator.run_file, ".bora/run.json");
        let sections = cnb.sections.as_ref().expect("sections present");
        assert_eq!(
            sections.checks,
            Some(vec!["gh".to_string(), "gia".to_string()])
        );
        assert_eq!(
            sections.commands,
            Some(vec!["dev".to_string(), "test".to_string()])
        );

        let bora = parsed.projects.get("bora").expect("bora project present");
        assert!(
            bora.auto_join,
            "auto_join must default to true when the field is absent from yaml"
        );
        assert_eq!(bora.name, None);
        assert_eq!(bora.channel, None, "channel is derived, not stored");
        assert_eq!(
            bora.effective_channel("bora"),
            "#bora",
            "derived default: \"#\" + slug"
        );
        assert!(bora.orchestrator.is_none());
        assert!(bora.sections.is_none());
        assert_eq!(bora.members.len(), 2);

        let serialized = to_yaml(&parsed).expect("serializes back to yaml");
        let reparsed = parse_projects_yaml(&serialized).expect("serialized yaml re-parses");
        assert_eq!(
            parsed, reparsed,
            "serialize -> reparse must round-trip to the same value"
        );
    }

    #[test]
    fn auto_join_false_round_trips_and_true_is_omitted_from_yaml() {
        let yaml = r#"
projects:
  quiet:
    members: []
    auto_join: false
  loud:
    members: []
"#;
        let parsed = parse_projects_yaml(yaml).expect("yaml must parse");
        assert!(
            !parsed.projects.get("quiet").unwrap().auto_join,
            "explicit auto_join: false must be honored, not defaulted away"
        );
        assert!(
            parsed.projects.get("loud").unwrap().auto_join,
            "an omitted auto_join must default to true"
        );

        let serialized = to_yaml(&parsed).expect("serializes back to yaml");
        assert!(
            serialized.contains("auto_join: false"),
            "an explicit false must survive serialization, got:\n{serialized}"
        );
        assert_eq!(
            serialized.matches("auto_join").count(),
            1,
            "auto_join must appear exactly once (quiet's explicit false); \
             loud's default true must be omitted (skip_serializing_if), got:\n{serialized}"
        );

        let reparsed = parse_projects_yaml(&serialized).expect("serialized yaml re-parses");
        assert_eq!(parsed, reparsed, "serialize -> reparse must round-trip");
    }

    #[test]
    fn resolver_maps_repo_root_member_to_identity_checkout_and_empty_subdir() {
        let root = temp_test_dir("resolver-root");
        init_fake_git_repo(&root);

        match resolve_member(&root.display().to_string()) {
            MemberResolution::Resolved(resolved) => {
                assert!(!resolved.repo_identity.is_empty());
                assert!(!resolved.checkout_key.is_empty());
                assert_eq!(
                    resolved.subdir,
                    PathBuf::new(),
                    "the checkout root itself has no subdir"
                );
            }
            other => panic!("expected a resolved member, got {other:?}"),
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolver_maps_subdir_member_to_the_same_repo_with_a_non_empty_subdir() {
        let root = temp_test_dir("resolver-subdir");
        init_fake_git_repo(&root);
        let sub = root.join("packages").join("landing");
        fs::create_dir_all(&sub).unwrap();

        let root_resolved = match resolve_member(&root.display().to_string()) {
            MemberResolution::Resolved(r) => r,
            other => panic!("expected root to resolve, got {other:?}"),
        };
        let sub_resolved = match resolve_member(&sub.display().to_string()) {
            MemberResolution::Resolved(r) => r,
            other => panic!("expected subdir to resolve, got {other:?}"),
        };

        assert_eq!(sub_resolved.repo_identity, root_resolved.repo_identity);
        assert_eq!(sub_resolved.checkout_key, root_resolved.checkout_key);
        assert_eq!(sub_resolved.subdir, PathBuf::from("packages/landing"));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn resolver_reports_non_git_path_as_unresolved_never_panics() {
        let root = temp_test_dir("resolver-non-git");
        // Deliberately no `.git`: a plain directory.

        match resolve_member(&root.display().to_string()) {
            MemberResolution::Unresolved { reason, .. } => {
                assert!(!reason.is_empty());
            }
            other => panic!("expected unresolved for a non-git dir, got {other:?}"),
        }

        // A path that does not exist at all must also resolve cleanly, not panic.
        let missing = root.join("does-not-exist");
        match resolve_member(&missing.display().to_string()) {
            MemberResolution::Unresolved { .. } => {}
            other => panic!("expected unresolved for a missing dir, got {other:?}"),
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_file_returns_error_and_keeps_last_good_value() {
        let _isolated = IsolatedDirs::new("malformed");
        write_raw_projects_file(EXAMPLE_YAML);
        let mut store = ProjectsStore::load();
        assert!(
            store.current().projects.contains_key("cnb"),
            "the good file must have loaded"
        );

        write_raw_projects_file("{ not: [valid, yaml");
        let result = store.reload_if_changed();
        assert!(
            result.is_err(),
            "malformed yaml must be reported as an error"
        );
        assert!(
            store.current().projects.contains_key("cnb"),
            "a parse error must never discard the last good value"
        );
    }

    #[test]
    fn reload_if_changed_detects_a_change_and_is_a_noop_when_unchanged() {
        let _isolated = IsolatedDirs::new("reload");
        write_raw_projects_file(
            r#"
projects:
  a:
    members:
      - dir: ~/Sites/a
"#,
        );
        let mut store = ProjectsStore::load();
        assert!(store.current().projects.contains_key("a"));

        // Content length changes, so the (mtime, len) stamp differs even on
        // filesystems with coarse mtime resolution — no sleep-and-hope timing.
        write_raw_projects_file(
            r#"
projects:
  a:
    members:
      - dir: ~/Sites/a
  b:
    members:
      - dir: ~/Sites/b
"#,
        );
        assert_eq!(
            store.reload_if_changed(),
            Ok(true),
            "a real change must be picked up"
        );
        assert!(store.current().projects.contains_key("b"));

        assert_eq!(
            store.reload_if_changed(),
            Ok(false),
            "calling again with no change must not re-parse"
        );
    }

    #[test]
    fn member_template_field_parses_and_defaults_to_absent() {
        // deny_unknown_fields means the `template:` key only parses once the
        // field exists — this test is a compile+parse lock on the schema.
        let parsed = parse_projects_yaml(
            r#"
defaults:
  open_with: "herdr-plus open <template>"
projects:
  cnb:
    members:
      - dir: ~/Sites/cnb_hono
        template: web
      - dir: ~/Sites/wt
"#,
        )
        .expect("template member must parse");
        let cnb = parsed.projects.get("cnb").expect("cnb present");
        assert_eq!(cnb.members[0].template.as_deref(), Some("web"));
        assert_eq!(
            cnb.members[1].template, None,
            "template is opt-in per member"
        );
    }

    #[test]
    fn open_with_template_path_is_exercised_when_opener_available() {
        let defaults = ProjectDefaults {
            open_with: "herdr-plus open <template>".to_string(),
            ..ProjectDefaults::default()
        };
        let member = Member {
            dir: "~/Sites/cnb_hono".to_string(),
            worktrees: WorktreesScope::All,
            template: Some("web".to_string()),
        };

        assert_eq!(
            resolve_open_with(&defaults, Some(&member), |program| program == "herdr-plus"),
            "herdr-plus open web",
            "member template substitutes into the open_with placeholder"
        );
        // A member without a template leaves the placeholder for the opener
        // to interpret — bora does not invent a default template.
        assert_eq!(
            resolve_open_with(&defaults, None, |program| program == "herdr-plus"),
            "herdr-plus open <template>"
        );
    }

    #[test]
    fn open_with_missing_opener_falls_back_to_bora_open() {
        let defaults = ProjectDefaults {
            open_with: "herdr-plus open <template>".to_string(),
            ..ProjectDefaults::default()
        };
        let member = Member {
            dir: "~/Sites/cnb_hono".to_string(),
            worktrees: WorktreesScope::All,
            template: Some("web".to_string()),
        };

        assert_eq!(
            resolve_open_with(&defaults, Some(&member), |_| false),
            "bora workspace open",
            "absent opener degrades to bora's own open, returning a command — never an error"
        );
        // Degradation is total, not partial: the missing opener's template
        // argument must not leak into the fallback command line.
        assert!(
            !resolve_open_with(&defaults, Some(&member), |_| false).contains("herdr-plus"),
            "fallback must not reference the missing opener"
        );
        // An empty open_with degrades the same way instead of yielding an
        // empty command line.
        let empty = ProjectDefaults {
            open_with: String::new(),
            ..ProjectDefaults::default()
        };
        assert_eq!(
            resolve_open_with(&empty, None, |_| true),
            "bora workspace open"
        );
    }
}
