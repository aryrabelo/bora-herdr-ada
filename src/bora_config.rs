use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct BoraConfig {
    pub ports: Option<BoraPortsConfig>,
    #[serde(default)]
    pub commands: Vec<BoraCommand>,
    pub flow: Option<BoraFlowConfig>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct BoraFlowConfig {
    /// Per-repo override for the global `[flow]` command template used to run
    /// a flow for a GitHub issue. Same placeholders as the global template.
    pub command: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BoraPortsConfig {
    pub base: u16,
    #[serde(default = "default_per_worktree")]
    pub per_worktree: u16,
    /// Upper bound (inclusive). If the computed port exceeds this, allocation fails.
    pub max: Option<u16>,
}

fn default_per_worktree() -> u16 {
    10
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BoraCommand {
    pub label: String,
    pub command: String,
    /// "shell" (background, default) or "pane" (opens in a split pane).
    #[serde(default)]
    pub mode: BoraCommandMode,
    /// If set, command only appears when the workspace branch matches.
    pub branch: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BoraCommandMode {
    #[default]
    Shell,
    Pane,
}

/// Repo commands scoped to `ws`'s branch: wt's `[scripts.run.*]` merged with
/// the deprecated `.bora.toml [[commands]]` (see `load_bora_config`). Single
/// source shared by the workspace context menu and the sidebar Programs
/// launcher so branch filtering never drifts between the two surfaces.
pub(crate) fn workspace_commands(ws: &crate::workspace::Workspace) -> Vec<BoraCommand> {
    let Some(root) = ws.bora_config_root() else {
        return Vec::new();
    };
    let Some(config) = load_bora_config(root) else {
        return Vec::new();
    };
    let branch = ws.cached_git_branch.as_deref();
    config
        .commands
        .into_iter()
        .filter(|c| c.branch.as_deref().is_none_or(|b| branch == Some(b)))
        .collect()
}

// ---------------------------------------------------------------------------
// wt `.wt/settings.toml [scripts.run.*]` — the surviving command schema
// (sidebar design decision 4; `.bora.toml [[commands]]` is deprecated).
// ---------------------------------------------------------------------------

/// One `[scripts.run.<id>]` entry. Every field is optional at parse time so a
/// higher-precedence file can overlay individual fields of an id declared in
/// a lower-precedence one; `command` is required before the entry becomes a
/// `BoraCommand`. wt-only fields (`default`, `options.cwd`) are deliberately
/// not parsed: they steer `wt run`, not bora's command list.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct WtRunScript {
    command: Option<String>,
    args: Option<Vec<String>>,
    hide: Option<bool>,
}

/// `scripts.run` accepts the named form `[scripts.run.<id>]` and the legacy
/// string form `run = "cmd"` (rewritten to a `default` id, mirroring wt's
/// normalizeLegacyRun).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum WtRunScripts {
    Legacy(String),
    Named(BTreeMap<String, WtRunScript>),
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WtSettings {
    scripts: WtScripts,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WtScripts {
    run: Option<WtRunScripts>,
}

/// Overlay one settings layer onto the accumulated scripts, per id and per
/// field — wt merges by flattened leaf key, so a higher-precedence file can
/// override one field of an id while its other fields survive from a
/// lower-precedence file.
fn merge_wt_run_layer(acc: &mut BTreeMap<String, WtRunScript>, layer: WtRunScripts) {
    let named = match layer {
        WtRunScripts::Legacy(command) => BTreeMap::from([(
            "default".to_string(),
            WtRunScript {
                command: Some(command),
                ..WtRunScript::default()
            },
        )]),
        WtRunScripts::Named(named) => named,
    };
    for (id, script) in named {
        let entry = acc.entry(id).or_default();
        if script.command.is_some() {
            entry.command = script.command;
        }
        if script.args.is_some() {
            entry.args = script.args;
        }
        if script.hide.is_some() {
            entry.hide = script.hide;
        }
    }
}

/// Map merged wt scripts into bora commands. wt scripts run interactively in
/// the checkout, so they map to pane mode — the sidebar Programs launcher
/// enumerates pane-mode commands only. Hidden scripts stay hidden (wt's own
/// `--list` semantics). Entries without a command are dropped with a warning.
fn wt_commands(
    scripts: BTreeMap<String, WtRunScript>,
    warnings: &mut Vec<String>,
) -> Vec<BoraCommand> {
    let mut commands = Vec::with_capacity(scripts.len());
    for (id, script) in scripts {
        if script.hide == Some(true) {
            continue;
        }
        let Some(mut command) = script.command.filter(|c| !c.trim().is_empty()) else {
            warnings.push(format!(
                "wt [scripts.run.{id}] declares no command; skipping"
            ));
            continue;
        };
        if let Some(args) = script.args {
            for arg in args {
                command.push(' ');
                command.push_str(&arg);
            }
        }
        commands.push(BoraCommand {
            label: id,
            command,
            mode: BoraCommandMode::Pane,
            branch: None,
        });
    }
    commands
}

// ---------------------------------------------------------------------------
// Loader + per-repo cache
// ---------------------------------------------------------------------------

const WT_LOCAL_SETTINGS: &str = ".wt/settings.local.toml";
const WT_SHARED_SETTINGS: &str = ".wt/settings.toml";
const CONDUCTOR_SETTINGS: &str = ".conductor/settings.toml";
const BORA_TOML: &str = ".bora.toml";

#[derive(Debug)]
struct CachedRepoConfig {
    /// (path, mtime) for every file that participates in the load, captured
    /// at load time. A hit requires every fingerprint to match the current
    /// filesystem exactly; any mtime bump, creation, or deletion invalidates.
    fingerprints: Vec<(PathBuf, Option<SystemTime>)>,
    /// When the mtime fingerprint probe last ran. Calls younger than
    /// `PROBE_THROTTLE` trust it and return with zero stat syscalls.
    last_probe: Instant,
    config: Option<BoraConfig>,
}

static BORA_CONFIG_CACHE: LazyLock<Mutex<HashMap<PathBuf, CachedRepoConfig>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn config_cache() -> MutexGuard<'static, HashMap<PathBuf, CachedRepoConfig>> {
    BORA_CONFIG_CACHE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Stat + read seams so tests can drive controlled mtimes and count reads.
type StatFn<'a> = &'a mut dyn FnMut(&Path) -> Option<SystemTime>;
type ReadFn<'a> = &'a mut dyn FnMut(&Path) -> std::io::Result<String>;
/// Clock seam so tests can drive a controlled probe time (same pattern as
/// `StatFn`/`ReadFn`).
type ClockFn<'a> = &'a mut dyn FnMut() -> Instant;

/// How long a fresh mtime probe stays trusted before the next load stats
/// again. `load_bora_config` is called from per-render sidebar layout code,
/// so even the cheap metadata-only fingerprint stats are forbidden on a
/// per-tick path: within this window a cached entry returns with zero stat
/// syscalls. One second bounds config-change staleness to a delay no human
/// editor round-trip can observe, while collapsing every render tick in the
/// window to a mutex + HashMap lookup.
const PROBE_THROTTLE: Duration = Duration::from_secs(1);

fn load_bora_config_impl(
    repo_root: &Path,
    stat: StatFn<'_>,
    read: ReadFn<'_>,
    clock: ClockFn<'_>,
) -> (Option<BoraConfig>, Vec<String>) {
    let now = clock();
    {
        let cache = config_cache();
        if let Some(entry) = cache.get(repo_root) {
            // Probe throttle: within the window the last fingerprint probe is
            // still trusted — return the cached config with zero stat
            // syscalls. A backwards clock (only possible with a fake clock)
            // is treated as past-window so the probe runs.
            let fresh = now
                .checked_duration_since(entry.last_probe)
                .is_some_and(|elapsed| elapsed < PROBE_THROTTLE);
            if fresh {
                return (entry.config.clone(), Vec::new());
            }
        }
    }
    let wt_local = repo_root.join(WT_LOCAL_SETTINGS);
    let wt_shared = repo_root.join(WT_SHARED_SETTINGS);
    let conductor = repo_root.join(CONDUCTOR_SETTINGS);
    let bora_toml = repo_root.join(BORA_TOML);
    // Past the throttle window: the mtime probe IS the invalidation check —
    // cheap metadata-only stats, never a file open. On a fingerprint match
    // no file contents are read.
    let fingerprints: Vec<(PathBuf, Option<SystemTime>)> =
        [&wt_local, &wt_shared, &conductor, &bora_toml]
            .into_iter()
            .map(|path| (path.clone(), stat(path)))
            .collect();

    {
        let mut cache = config_cache();
        if let Some(entry) = cache.get_mut(repo_root) {
            entry.last_probe = now;
            if entry.fingerprints == fingerprints {
                return (entry.config.clone(), Vec::new());
            }
        }
    }

    let (config, warnings) = read_repo_config(&wt_local, &wt_shared, &conductor, &bora_toml, read);

    config_cache().insert(
        repo_root.to_path_buf(),
        CachedRepoConfig {
            fingerprints,
            last_probe: now,
            config: config.clone(),
        },
    );
    (config, warnings)
}

fn read_repo_config(
    wt_local: &Path,
    wt_shared: &Path,
    conductor: &Path,
    bora_toml: &Path,
    read: ReadFn<'_>,
) -> (Option<BoraConfig>, Vec<String>) {
    let mut warnings = Vec::new();

    // Lowest precedence first so higher layers overlay per id, per field.
    let mut scripts: BTreeMap<String, WtRunScript> = BTreeMap::new();
    for path in [conductor, wt_shared, wt_local] {
        let Ok(content) = read(path) else {
            continue; // missing or unreadable: no layer at this level
        };
        match toml::from_str::<WtSettings>(&content) {
            Ok(settings) => {
                if let Some(run) = settings.scripts.run {
                    merge_wt_run_layer(&mut scripts, run);
                }
            }
            Err(err) => warnings.push(format!("invalid {}: {err}", path.display())),
        }
    }
    let wt_commands = wt_commands(scripts, &mut warnings);

    let config = match read(bora_toml) {
        Ok(content) => match toml::from_str::<BoraConfig>(&content) {
            Ok(config) => {
                if !config.commands.is_empty() {
                    warnings.push(format!(
                        "{}: [[commands]] is deprecated — migrate each entry to \
                         .wt/settings.toml as [scripts.run.<label>] with \
                         command = \"...\" (config reference: \"Repository commands\")",
                        bora_toml.display()
                    ));
                }
                Some(config)
            }
            Err(err) => {
                warnings.push(format!("invalid {}: {err}", bora_toml.display()));
                None
            }
        },
        Err(_) => None,
    };

    if config.is_none() && wt_commands.is_empty() {
        return (None, warnings);
    }
    let mut config = config.unwrap_or_default();
    if !wt_commands.is_empty() {
        // wt's schema is the survivor: on a label collision the wt entry
        // replaces the deprecated .bora.toml entry.
        config
            .commands
            .retain(|c| !wt_commands.iter().any(|w| w.label == c.label));
        config.commands.extend(wt_commands);
    }
    (Some(config), warnings)
}

/// Load the repo's command/config surface: `.bora.toml` (deprecated
/// `[[commands]]`, still honored with a warning) merged with wt's
/// `[scripts.run.*]` from `.wt/settings.local.toml` > `.wt/settings.toml` >
/// `.conductor/settings.toml`. Results are cached per repo root and
/// invalidated when any participating file's mtime changes; a `PROBE_THROTTLE`
/// window additionally lets repeat loads within one second of the last
/// probe return with zero stat syscalls (per-render sidebar callers), and
/// past the window a metadata-only mtime probe decides whether file
/// contents need re-reading.
pub(crate) fn load_bora_config(repo_root: &Path) -> Option<BoraConfig> {
    let mut stat = |path: &Path| std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
    let mut read = |path: &Path| std::fs::read_to_string(path);
    let (config, warnings) =
        load_bora_config_impl(repo_root, &mut stat, &mut read, &mut Instant::now);
    for warning in warnings {
        tracing::warn!("{warning}");
    }
    config
}

/// Given a repo root and a specific checkout path, return the allocated port.
///
/// Primary (non-linked) worktree gets index 0.
/// Linked worktrees sorted by branch name get indices 1, 2, ...
/// Port = base + index * per_worktree.
pub(crate) fn port_for_checkout(
    config: &BoraPortsConfig,
    repo_root: &Path,
    checkout_path: &Path,
) -> Option<u16> {
    let worktrees = crate::worktree::list_existing_worktrees(repo_root).ok()?;
    let canonical_checkout = crate::worktree::canonical_or_original(checkout_path);
    let canonical_repo = crate::worktree::canonical_or_original(repo_root);

    // Separate primary and linked worktrees.
    let mut primary_match = false;
    let mut linked: Vec<&crate::worktree::ExistingWorktree> = Vec::new();

    for wt in &worktrees {
        if wt.is_bare {
            continue;
        }
        let canon = crate::worktree::canonical_or_original(&wt.path);
        if canon == canonical_repo {
            // Primary worktree.
            if canon == canonical_checkout {
                primary_match = true;
            }
        } else {
            linked.push(wt);
        }
    }

    linked.sort_by(|a, b| a.branch.cmp(&b.branch));

    if primary_match {
        return Some(config.base);
    }

    for (i, wt) in linked.iter().enumerate() {
        if crate::worktree::canonical_or_original(&wt.path) == canonical_checkout {
            let port = config.base + ((i as u16) + 1) * config.per_worktree;
            if let Some(max) = config.max {
                if port > max {
                    tracing::warn!(
                        port,
                        max,
                        "bora port {port} exceeds max {max}, not allocating"
                    );
                    return None;
                }
            }
            return Some(port);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::{Duration, UNIX_EPOCH};

    fn test_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("herdr-bora-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".wt")).unwrap();
        std::fs::create_dir_all(dir.join(".conductor")).unwrap();
        dir
    }

    fn load_with_real_fs(repo_root: &Path) -> (Option<BoraConfig>, Vec<String>) {
        let mut stat = |path: &Path| std::fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let mut read = |path: &Path| std::fs::read_to_string(path);
        load_bora_config_impl(repo_root, &mut stat, &mut read, &mut Instant::now)
    }

    #[test]
    fn parse_full_config() {
        let toml_str = r#"
[ports]
base = 3110
per_worktree = 10

[[commands]]
label = "Deploy"
command = "echo deploying..."
branch = "main"

[[commands]]
label = "Run"
command = "bun run dev"
mode = "pane"
"#;
        let config: BoraConfig = toml::from_str(toml_str).unwrap();
        let ports = config.ports.unwrap();
        assert_eq!(ports.base, 3110);
        assert_eq!(ports.per_worktree, 10);
        assert_eq!(config.commands.len(), 2);
        assert_eq!(config.commands[0].label, "Deploy");
        assert_eq!(config.commands[0].branch.as_deref(), Some("main"));
        assert_eq!(config.commands[0].mode, BoraCommandMode::Shell);
        assert_eq!(config.commands[1].label, "Run");
        assert_eq!(config.commands[1].mode, BoraCommandMode::Pane);
        assert!(config.commands[1].branch.is_none());
    }

    #[test]
    fn parse_commands_only() {
        let toml_str = r#"
[[commands]]
label = "Test"
command = "cargo test"
"#;
        let config: BoraConfig = toml::from_str(toml_str).unwrap();
        assert!(config.ports.is_none());
        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].label, "Test");
        assert_eq!(config.commands[0].mode, BoraCommandMode::Shell);
    }

    #[test]
    fn parse_with_default_per_worktree() {
        let toml_str = r#"
[ports]
base = 5000
"#;
        let config: BoraConfig = toml::from_str(toml_str).unwrap();
        let ports = config.ports.unwrap();
        assert_eq!(ports.base, 5000);
        assert_eq!(ports.per_worktree, 10);
        assert!(ports.max.is_none());
    }

    #[test]
    fn load_invalid_toml_returns_none() {
        let dir =
            std::env::temp_dir().join(format!("herdr-bora-test-invalid-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".bora.toml"), "not valid { toml").unwrap();
        assert!(load_bora_config(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_missing_file_returns_none() {
        let dir =
            std::env::temp_dir().join(format!("herdr-bora-test-missing-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(load_bora_config(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn port_allocation_with_synthetic_worktrees() {
        use crate::worktree::parse_worktree_list_porcelain;

        let porcelain = "\
worktree /repo/main
branch refs/heads/main

worktree /repo/trees/feature-b
branch refs/heads/feature-b

worktree /repo/trees/feature-a
branch refs/heads/feature-a

";
        let worktrees = parse_worktree_list_porcelain(porcelain);
        // Primary = /repo/main (index 0)
        // Linked sorted by branch: feature-a (index 1), feature-b (index 2)
        assert_eq!(worktrees.len(), 3);

        let config = BoraPortsConfig {
            base: 3000,
            per_worktree: 10,
            max: None,
        };

        // We can't call port_for_checkout directly since it calls
        // list_existing_worktrees (which runs git). Instead, verify the
        // sorting logic matches our expectations by testing the parse output.
        let canonical_repo = std::path::PathBuf::from("/repo/main");
        let mut linked: Vec<_> = worktrees
            .iter()
            .filter(|w| !w.is_bare && w.path != canonical_repo)
            .collect();
        linked.sort_by(|a, b| a.branch.cmp(&b.branch));

        assert_eq!(linked[0].branch.as_deref(), Some("feature-a"));
        assert_eq!(linked[1].branch.as_deref(), Some("feature-b"));

        // Port math: primary=3000, feature-a=3010, feature-b=3020
        assert_eq!(config.base, 3000);
        assert_eq!(config.base + config.per_worktree, 3010);
        assert_eq!(config.base + 2 * config.per_worktree, 3020);
    }

    #[test]
    fn empty_config_is_valid() {
        let config: BoraConfig = toml::from_str("").unwrap();
        assert!(config.ports.is_none());
        assert!(config.commands.is_empty());
        assert!(config.flow.is_none());
    }

    #[test]
    fn parse_flow_section() {
        let toml_str = r#"
[flow]
command = "uv run flow.py --issue {issue}"
"#;
        let config: BoraConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.flow.unwrap().command.as_deref(),
            Some("uv run flow.py --issue {issue}")
        );
    }

    #[test]
    fn parse_empty_flow_section() {
        let config: BoraConfig = toml::from_str("[flow]\n").unwrap();
        assert!(config.flow.unwrap().command.is_none());
    }

    #[test]
    fn parse_ports_with_max() {
        let toml_str = r#"
[ports]
base = 3000
max = 3050
"#;
        let config: BoraConfig = toml::from_str(toml_str).unwrap();
        let ports = config.ports.unwrap();
        assert_eq!(ports.base, 3000);
        assert_eq!(ports.max, Some(3050));
    }

    #[test]
    fn port_max_blocks_allocation() {
        // With base=3000, per_worktree=10, max=3015:
        // primary=3000 (ok), index 1=3010 (ok), index 2=3020 (exceeds 3015)
        let config = BoraPortsConfig {
            base: 3000,
            per_worktree: 10,
            max: Some(3015),
        };
        // index 1 is within bounds
        let port1 = config.base + config.per_worktree;
        assert_eq!(port1, 3010);
        assert!(port1 <= 3015);
        // index 2 exceeds max
        let port2 = config.base + 2 * config.per_worktree;
        assert_eq!(port2, 3020);
        assert!(port2 > 3015);
    }

    #[test]
    fn wt_scripts_named_form_parses() {
        // Live-example shape (~/Sites/wt/.wt/settings.toml): `default` is a
        // wt-only field and must not break the parse.
        let settings: WtSettings = toml::from_str(
            r#"
[scripts]
setup = "go build ./..."

[scripts.run.test]
command = "go test ./..."
default = true

[scripts.run.dev]
command = "bun run dev"
args = ["--port", "$BORA_PORT"]
hide = true
"#,
        )
        .unwrap();
        let Some(WtRunScripts::Named(scripts)) = settings.scripts.run else {
            panic!("expected named run scripts");
        };
        assert_eq!(scripts["test"].command.as_deref(), Some("go test ./..."));
        assert_eq!(scripts["dev"].args.as_ref().map(Vec::len), Some(2));
        assert_eq!(scripts["dev"].hide, Some(true));
        assert!(scripts["test"].args.is_none());
    }

    #[test]
    fn wt_scripts_legacy_string_form_parses() {
        let settings: WtSettings = toml::from_str("[scripts]\nrun = \"make dev\"\n").unwrap();
        let WtRunScripts::Legacy(cmd) = settings.scripts.run.expect("run scripts present") else {
            panic!("expected legacy string form");
        };
        assert_eq!(cmd, "make dev");
    }

    #[test]
    fn wt_precedence_chain_merges_per_id_per_field() {
        let dir = test_dir("wt-precedence");
        // Conductor (lowest precedence): two ids, one with a command+args.
        std::fs::write(
            dir.join(".conductor/settings.toml"),
            r#"
[scripts.run.build]
command = "make build"

[scripts.run.web]
command = "bun dev"
args = ["--host"]
"#,
        )
        .unwrap();
        // Shared settings: overrides web.command only.
        std::fs::write(
            dir.join(".wt/settings.toml"),
            r#"
[scripts.run.web]
command = "bun run dev"
"#,
        )
        .unwrap();
        // Local settings (highest precedence): adds api, overrides web.args.
        std::fs::write(
            dir.join(".wt/settings.local.toml"),
            r#"
[scripts.run.api]
command = "go run ./cmd/api"

[scripts.run.web]
args = ["--port", "5173"]
"#,
        )
        .unwrap();

        let (config, warnings) = load_with_real_fs(&dir);
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        let commands = config.expect("wt commands enumerate").commands;
        assert_eq!(commands.len(), 3);
        // Per-id merge: build (conductor) and api (local) both survive.
        // Per-field merge: web takes command from settings.toml and args from
        // settings.local.toml, layered over the conductor id.
        let web = commands
            .iter()
            .find(|c| c.label == "web")
            .expect("web command");
        assert_eq!(web.command, "bun run dev --port 5173");
        assert_eq!(web.mode, BoraCommandMode::Pane);
        assert!(web.branch.is_none());
        assert!(commands
            .iter()
            .any(|c| c.label == "build" && c.command == "make build"));
        assert!(commands.iter().any(|c| c.label == "api"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn legacy_bora_toml_commands_parse_and_warn_deprecated() {
        let dir = test_dir("legacy-warn");
        std::fs::write(
            dir.join(".bora.toml"),
            r#"
[[commands]]
label = "Deploy"
command = "echo deploying..."
"#,
        )
        .unwrap();
        let (config, warnings) = load_with_real_fs(&dir);
        let config = config.expect("legacy .bora.toml must keep parsing");
        assert_eq!(config.commands.len(), 1);
        assert_eq!(config.commands[0].label, "Deploy");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("deprecated") && w.contains("[scripts.run.")),
            "expected a deprecation warning, got {warnings:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wt_commands_win_over_legacy_on_label_collision() {
        let dir = test_dir("wt-collision");
        std::fs::write(
            dir.join(".bora.toml"),
            r#"
[[commands]]
label = "test"
command = "old"

[[commands]]
label = "legacy-only"
command = "keep"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join(".wt/settings.toml"),
            "[scripts.run.test]\ncommand = \"new\"\n",
        )
        .unwrap();
        let (config, warnings) = load_with_real_fs(&dir);
        let commands = config.expect("config").commands;
        let test = commands
            .iter()
            .find(|c| c.label == "test")
            .expect("test command");
        assert_eq!(test.command, "new");
        assert_eq!(test.mode, BoraCommandMode::Pane);
        assert!(commands
            .iter()
            .any(|c| c.label == "legacy-only" && c.command == "keep"));
        assert!(warnings.iter().any(|w| w.contains("deprecated")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_hit_within_same_mtime_skips_file_reads() {
        let root = PathBuf::from("/virtual/bora-test-cache-hit");
        let mtime = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let stats = Cell::new(0usize);
        let reads = Cell::new(0usize);
        let now = Cell::new(Instant::now());
        let mut stat = |_: &Path| {
            stats.set(stats.get() + 1);
            Some(mtime)
        };
        let mut read = |path: &Path| -> std::io::Result<String> {
            reads.set(reads.get() + 1);
            if path.ends_with(".wt/settings.toml") {
                Ok("[scripts.run.test]\ncommand = \"cargo test\"\n".to_string())
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
            }
        };
        let mut clock = || now.get();
        let (first, _) = load_bora_config_impl(&root, &mut stat, &mut read, &mut clock);
        assert!(first.is_some());
        let reads_after_first = reads.get();
        assert!(reads_after_first > 0);
        let stats_after_first = stats.get();

        // Past the probe window: the fingerprint probe re-runs.
        now.set(now.get() + PROBE_THROTTLE + Duration::from_secs(1));
        let (second, warnings) = load_bora_config_impl(&root, &mut stat, &mut read, &mut clock);
        let second = second.expect("cache hit still returns the config");
        // Same mtime on every file → cache hit: not a single file read. The
        // metadata-only stat probe still runs — it IS the invalidation check.
        assert_eq!(reads.get(), reads_after_first);
        assert!(stats.get() > stats_after_first);
        assert!(warnings.is_empty(), "cache hits do not re-emit warnings");
        assert_eq!(second.commands.len(), 1);
        assert_eq!(second.commands[0].command, "cargo test");
    }

    #[test]
    fn cache_hit_within_probe_window_skips_stats() {
        let root = PathBuf::from("/virtual/bora-test-probe-throttle");
        let mtime = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let stats = Cell::new(0usize);
        let reads = Cell::new(0usize);
        let now = Cell::new(Instant::now());
        let mut stat = |_: &Path| {
            stats.set(stats.get() + 1);
            Some(mtime)
        };
        let mut read = |path: &Path| -> std::io::Result<String> {
            reads.set(reads.get() + 1);
            if path.ends_with(".wt/settings.toml") {
                Ok("[scripts.run.test]\ncommand = \"cargo test\"\n".to_string())
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
            }
        };
        let mut clock = || now.get();
        let (first, _) = load_bora_config_impl(&root, &mut stat, &mut read, &mut clock);
        assert!(first.is_some());
        let stats_after_first = stats.get();
        let reads_after_first = reads.get();
        assert!(stats_after_first > 0);
        assert!(reads_after_first > 0);

        // Second load within PROBE_THROTTLE of the first probe: the cached
        // entry is trusted outright — zero stat syscalls, zero reads. This is
        // the per-render sidebar path: no filesystem metadata per tick.
        now.set(now.get() + PROBE_THROTTLE / 2);
        let (second, warnings) = load_bora_config_impl(&root, &mut stat, &mut read, &mut clock);
        let second = second.expect("throttled cache hit still returns the config");
        assert_eq!(stats.get(), stats_after_first, "no stats within the window");
        assert_eq!(reads.get(), reads_after_first, "no reads within the window");
        assert!(warnings.is_empty(), "cache hits do not re-emit warnings");
        assert_eq!(second.commands.len(), 1);
        assert_eq!(second.commands[0].command, "cargo test");
    }

    #[test]
    fn mtime_bump_invalidates_cache() {
        let root = PathBuf::from("/virtual/bora-test-mtime-bump");
        let mtime = Cell::new(UNIX_EPOCH + Duration::from_secs(1_700_000_000));
        let version = Cell::new(1u32);
        let now = Cell::new(Instant::now());
        let mut stat = |_: &Path| Some(mtime.get());
        let mut read = |path: &Path| -> std::io::Result<String> {
            if path.ends_with(".wt/settings.toml") {
                Ok(format!(
                    "[scripts.run.test]\ncommand = \"echo v{}\"\n",
                    version.get()
                ))
            } else {
                Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
            }
        };
        let mut clock = || now.get();
        let (first, _) = load_bora_config_impl(&root, &mut stat, &mut read, &mut clock);
        assert_eq!(first.unwrap().commands[0].command, "echo v1");
        // Bump the mtime and the content, and step past the probe window:
        // the next load must stat, see the new fingerprint, and re-read.
        mtime.set(mtime.get() + Duration::from_secs(60));
        version.set(2);
        now.set(now.get() + PROBE_THROTTLE + Duration::from_secs(1));
        let (second, _) = load_bora_config_impl(&root, &mut stat, &mut read, &mut clock);
        assert_eq!(second.unwrap().commands[0].command, "echo v2");
    }
}
