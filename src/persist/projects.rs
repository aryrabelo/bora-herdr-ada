//! Store for `~/.config/bora/projects.yml` — the "Composition model" declaration
//! from `.local/prd/sidebar-design.md`: a project groups member directories
//! (repos or subdirectories of repos, derived via git discovery) under a name,
//! a channel, an optional orchestrator, and per-project check/command sections.
//!
//! The sidebar is the only reader; right-click, an editor, and MCP tools are
//! all writers of the same file (later beads in epic bora-e9i). This module
//! owns three things only: the schema, parsing it, and a cheap reload trigger.
//! Socket verbs, MCP tools, and sidebar rendering are later beads.
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

// bora-e9i.1 lands the store, resolver, and reload trigger only. Socket
// verbs (bora-e9i.2), MCP tools, and sidebar rendering are later beads in
// epic bora-e9i and are what will call most of this module's public API;
// until then it is only exercised from this module's own tests.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
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
}

impl Member {
    pub fn resolve(&self) -> MemberResolution {
        resolve_member(&self.dir)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
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
}

pub fn parse_projects_yaml(raw: &str) -> Result<ProjectsFile, String> {
    serde_yaml_ng::from_str(raw).map_err(|err| err.to_string())
}

pub fn to_yaml(file: &ProjectsFile) -> Result<String, String> {
    serde_yaml_ng::to_string(file).map_err(|err| err.to_string())
}

// ── Resolver: member dir -> (repo_identity, checkout_key, subdir) ─────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMember {
    pub repo_identity: String,
    pub checkout_key: String,
    /// Empty when the member dir *is* the checkout root.
    pub subdir: PathBuf,
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

pub struct ProjectsStore {
    path: PathBuf,
    value: ProjectsFile,
    stamp: Option<FileStamp>,
}

impl ProjectsStore {
    /// Loads from `projects_file_path()`. A missing or malformed file yields
    /// an empty store rather than failing — there is no "last good" before
    /// the first successful parse.
    pub fn load() -> Self {
        Self::at(projects_file_path())
    }

    fn at(path: PathBuf) -> Self {
        let mut store = Self {
            path,
            value: ProjectsFile::default(),
            stamp: None,
        };
        let _ = store.reload_if_changed();
        store
    }

    pub fn current(&self) -> &ProjectsFile {
        &self.value
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
                Ok(true)
            }
            Err(err) => Err(format!("{}: {err}", self.path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // ponytail: local copy, matching the pattern already used in
    // `persist::pending_prompts` and elsewhere for `XDG_STATE_HOME`. Extract
    // a shared helper if a fourth XDG-isolation copy appears.
    struct IsolatedConfigDir {
        _guard: parking_lot::MutexGuard<'static, ()>,
        old: Option<std::ffi::OsString>,
        dir: PathBuf,
    }

    impl IsolatedConfigDir {
        fn new(name: &str) -> Self {
            let guard = crate::config::test_config_env_lock().lock();
            let old = std::env::var_os("XDG_CONFIG_HOME");
            let dir =
                std::env::temp_dir().join(format!("bora-projects-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            std::env::set_var("XDG_CONFIG_HOME", &dir);
            Self {
                _guard: guard,
                old,
                dir,
            }
        }
    }

    impl Drop for IsolatedConfigDir {
        fn drop(&mut self) {
            match self.old.take() {
                Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    fn write_projects_file(raw: &str) {
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
        let _isolated = IsolatedConfigDir::new("malformed");
        write_projects_file(EXAMPLE_YAML);
        let mut store = ProjectsStore::load();
        assert!(
            store.current().projects.contains_key("cnb"),
            "the good file must have loaded"
        );

        write_projects_file("{ not: [valid, yaml");
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
        let _isolated = IsolatedConfigDir::new("reload");
        write_projects_file(
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
        write_projects_file(
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
}
