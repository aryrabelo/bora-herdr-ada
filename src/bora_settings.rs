//! Per-project workspace configuration parsed from `.bora/settings.toml`.
//!
//! Conductor-style workspace isolation: each worktree can seed files (copy or
//! symlink from the root checkout), get a stable per-workspace port, and run a
//! setup script right after creation. Distinct from the unrelated `.bora.toml`
//! (`crate::bora_config`), which drives the UI command menu.

use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct BoraSettings {
    pub scripts: BoraScripts,
    pub files: Option<BoraFiles>,
    pub ports: BoraPortsRange,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct BoraScripts {
    pub setup: Option<String>,
    pub run: Option<String>,
    pub run_mode: BoraRunMode,
}

#[derive(Debug, Clone, Copy, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum BoraRunMode {
    #[default]
    Concurrent,
    Exclusive,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct BoraFiles {
    pub copy: Vec<String>,
    pub symlink: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub(crate) struct BoraPortsRange {
    pub base: u16,
    pub max: u16,
}

impl Default for BoraPortsRange {
    fn default() -> Self {
        Self {
            base: 4100,
            max: 4199,
        }
    }
}

/// Parse `<repo_root>/.bora/settings.toml`. Missing file → `None`; a parse
/// error is logged and returns `None` (mirrors `bora_config::load_bora_config`).
pub(crate) fn load_bora_settings(repo_root: &Path) -> Option<BoraSettings> {
    let path = repo_root.join(".bora").join("settings.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<BoraSettings>(&content) {
        Ok(settings) => Some(settings),
        Err(err) => {
            tracing::warn!(path = %path.display(), "invalid .bora/settings.toml: {err}");
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SetupStatus {
    Ok,
    Failed(String),
    Skipped,
}

impl SetupStatus {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            SetupStatus::Ok => "ok",
            SetupStatus::Failed(_) => "failed",
            SetupStatus::Skipped => "skipped",
        }
    }
}

/// Absolute git common dir for `repo_root` (shared by all linked worktrees).
/// Falls back to `repo_root/.git` when git cannot be queried.
fn git_common_dir(repo_root: &Path) -> PathBuf {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["rev-parse", "--path-format=absolute", "--git-common-dir"])
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let dir = stdout.trim();
            if !dir.is_empty() {
                return PathBuf::from(dir);
            }
        }
    }
    repo_root.join(".git")
}

/// Stable per-workspace port persisted at `<git common dir>/info/bora-ports.json`.
/// An existing `key` keeps its port forever; a new key gets the lowest free port
/// in `[base, max]`; `None` when the range is exhausted.
pub(crate) fn allocate_port(repo_root: &Path, key: &str, range: &BoraPortsRange) -> Option<u16> {
    let info_dir = git_common_dir(repo_root).join("info");
    let map_path = info_dir.join("bora-ports.json");
    if let Err(err) = std::fs::create_dir_all(&info_dir) {
        tracing::warn!(path = %info_dir.display(), "bora-ports: failed to create info dir: {err}");
    }

    // Exclusive-create lockfile so N parallel `worktree create` / `workspace
    // run` calls against the same repo can't both claim the same free port.
    let _lock = PortsLock::acquire(map_path.with_extension("json.lock"));

    let mut map: BTreeMap<String, u16> = std::fs::read_to_string(&map_path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default();

    if let Some(port) = map.get(key) {
        return Some(*port);
    }

    let taken: HashSet<u16> = map.values().copied().collect();
    let port = (range.base..=range.max).find(|candidate| !taken.contains(candidate))?;

    map.insert(key.to_string(), port);
    match serde_json::to_string_pretty(&map) {
        Ok(json) => {
            if let Err(err) = std::fs::write(&map_path, json) {
                tracing::warn!(path = %map_path.display(), "bora-ports: failed to persist: {err}");
            }
        }
        Err(err) => tracing::warn!("bora-ports: failed to serialize map: {err}"),
    }
    Some(port)
}

/// Held for the duration of the ports-map read-modify-write. Acquired via
/// atomic exclusive create (same pattern as `remote/unix.rs`); a lock older
/// than [`PortsLock::STALE`] is treated as leaked by a dead process and
/// removed. Best-effort: on timeout the allocation proceeds unlocked (warned).
struct PortsLock {
    path: Option<PathBuf>,
}

impl PortsLock {
    const STALE: std::time::Duration = std::time::Duration::from_secs(5);

    fn acquire(path: PathBuf) -> Self {
        for _ in 0..40 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Self { path: Some(path) },
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|meta| meta.modified())
                        .ok()
                        .and_then(|modified| modified.elapsed().ok())
                        .is_some_and(|age| age > Self::STALE);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                Err(err) => {
                    tracing::warn!(path = %path.display(), "bora-ports: failed to create lock: {err}");
                    return Self { path: None };
                }
            }
        }
        tracing::warn!(path = %path.display(), "bora-ports: lock timed out; proceeding unlocked");
        Self { path: None }
    }
}

impl Drop for PortsLock {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Whether `.bora/settings.toml` explicitly defines a `[ports]` table (as
/// opposed to the deserialized default). Drives port-system precedence.
fn has_ports_section(repo_root: &Path) -> bool {
    let path = repo_root.join(".bora").join("settings.toml");
    let Ok(content) = std::fs::read_to_string(&path) else {
        return false;
    };
    toml::from_str::<toml::Value>(&content)
        .ok()
        .is_some_and(|value| value.get("ports").is_some())
}

/// Unified port resolution across the two config systems, in precedence order:
/// 1. `.bora/settings.toml` with an explicit `[ports]` → persisted allocator.
/// 2. legacy `.bora.toml [ports]` → index-based `bora_config::port_for_checkout`.
/// 3. `.bora/settings.toml` present (no `[ports]`) → allocator, default range.
/// 4. neither → `None`.
pub(crate) fn resolve_port(repo_root: &Path, checkout_path: &Path, key: &str) -> Option<u16> {
    let settings = load_bora_settings(repo_root);

    if let Some(settings) = &settings {
        if has_ports_section(repo_root) {
            return allocate_port(repo_root, key, &settings.ports);
        }
    }

    if let Some(config) = crate::bora_config::load_bora_config(repo_root) {
        if let Some(ports) = &config.ports {
            return crate::bora_config::port_for_checkout(ports, repo_root, checkout_path);
        }
    }

    if let Some(settings) = &settings {
        return allocate_port(repo_root, key, &settings.ports);
    }

    None
}

/// Environment injected into setup/run scripts. `BORA_PORT` is present only when
/// a port is resolvable (via [`resolve_port`]).
pub(crate) fn workspace_env(
    repo_root: &Path,
    checkout_path: &Path,
    branch: Option<&str>,
    settings: &BoraSettings,
) -> Vec<(String, String)> {
    // `settings` is retained for API stability; port resolution reloads config
    // through `resolve_port` so both config systems are honored uniformly.
    let _ = settings;

    let workspace_id = checkout_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();

    let mut env = vec![
        (
            "BORA_ROOT_PATH".to_string(),
            repo_root.display().to_string(),
        ),
        (
            "BORA_WORKSPACE_PATH".to_string(),
            checkout_path.display().to_string(),
        ),
        ("BORA_WORKSPACE_ID".to_string(), workspace_id.clone()),
        ("BORA_BRANCH".to_string(), branch.unwrap_or("").to_string()),
    ];

    let key = branch
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| workspace_id.clone());
    if let Some(port) = resolve_port(repo_root, checkout_path, &key) {
        env.push(("BORA_PORT".to_string(), port.to_string()));
    }

    env
}

/// Full provisioning for a freshly created worktree. Safe to call from a
/// background thread; never panics.
pub(crate) fn provision_worktree(
    repo_root: &Path,
    checkout_path: &Path,
    branch: Option<&str>,
    no_setup: bool,
) -> SetupStatus {
    let settings = load_bora_settings(repo_root);

    // 1. Seed files: settings `[files]` if present, else legacy `.worktreeinclude`.
    match settings
        .as_ref()
        .and_then(|settings| settings.files.as_ref())
    {
        Some(files) => {
            for rel in &files.copy {
                copy_entry(repo_root, checkout_path, rel);
            }
            for rel in &files.symlink {
                symlink_entry(repo_root, checkout_path, rel);
            }
        }
        None => crate::worktree::copy_worktree_includes(repo_root, checkout_path),
    }

    // 2. `.context/` scratch dir, git-excluded via the shared exclude file.
    ensure_context_dir(repo_root, checkout_path);

    // 3. Setup script.
    if no_setup {
        return SetupStatus::Skipped;
    }
    let Some(settings) = settings else {
        return SetupStatus::Skipped;
    };
    let Some(script) = settings.scripts.setup.as_deref() else {
        return SetupStatus::Skipped;
    };
    if script.trim().is_empty() {
        return SetupStatus::Skipped;
    }
    run_setup_script(repo_root, checkout_path, branch, &settings, script)
}

/// Snapshot-copy `rel` from root into the checkout. Never overwrites an existing
/// destination; missing source is skipped.
fn copy_entry(repo_root: &Path, checkout_path: &Path, rel: &str) {
    let src = repo_root.join(rel);
    let dst = checkout_path.join(rel);
    if dst.symlink_metadata().is_ok() {
        tracing::info!(dst = %dst.display(), "bora settings: dst exists, skip copy");
        return;
    }
    if !src.exists() {
        tracing::debug!(src = %src.display(), "bora settings: copy source missing, skip");
        return;
    }
    if let Some(parent) = dst.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::warn!(path = %parent.display(), "bora settings: create parent failed: {err}");
            return;
        }
    }
    let result = if src.is_dir() {
        copy_dir_recursive(&src, &dst)
    } else {
        std::fs::copy(&src, &dst).map(|_| ())
    };
    if let Err(err) = result {
        tracing::warn!(src = %src.display(), dst = %dst.display(), "bora settings: copy failed: {err}");
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Symlink `rel` in the checkout to the absolute path in the root checkout.
/// Never overwrites an existing destination; missing source is skipped.
fn symlink_entry(repo_root: &Path, checkout_path: &Path, rel: &str) {
    let src = repo_root.join(rel);
    let dst = checkout_path.join(rel);
    if dst.symlink_metadata().is_ok() {
        tracing::info!(dst = %dst.display(), "bora settings: dst exists, skip symlink");
        return;
    }
    if !src.exists() {
        tracing::debug!(src = %src.display(), "bora settings: symlink source missing, skip");
        return;
    }
    if let Some(parent) = dst.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::warn!(path = %parent.display(), "bora settings: create parent failed: {err}");
            return;
        }
    }
    if let Err(err) = crate::platform::symlink(&src, &dst) {
        tracing::warn!(src = %src.display(), dst = %dst.display(), "bora settings: symlink failed: {err}");
    }
}

/// Create `.context/` in the checkout and ensure `.context/` is git-excluded via
/// the shared `<git common dir>/info/exclude`.
fn ensure_context_dir(repo_root: &Path, checkout_path: &Path) {
    let context_dir = checkout_path.join(".context");
    if let Err(err) = std::fs::create_dir_all(&context_dir) {
        tracing::warn!(path = %context_dir.display(), "bora settings: create .context failed: {err}");
    }
    let exclude_path = git_common_dir(repo_root).join("info").join("exclude");
    ensure_exclude_line(&exclude_path, ".context/");
}

/// Append `line` to the git exclude file if not already present (idempotent).
fn ensure_exclude_line(exclude_path: &Path, line: &str) {
    let existing = std::fs::read_to_string(exclude_path).unwrap_or_default();
    if existing
        .lines()
        .any(|existing_line| existing_line.trim() == line)
    {
        return;
    }
    if let Some(parent) = exclude_path.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            tracing::warn!(path = %parent.display(), "bora settings: create exclude dir failed: {err}");
            return;
        }
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(line);
    content.push('\n');
    if let Err(err) = std::fs::write(exclude_path, content) {
        tracing::warn!(path = %exclude_path.display(), "bora settings: write exclude failed: {err}");
    }
}

/// Run the setup script via `/bin/sh -lc` in the checkout with workspace env.
fn run_setup_script(
    repo_root: &Path,
    checkout_path: &Path,
    branch: Option<&str>,
    settings: &BoraSettings,
    script: &str,
) -> SetupStatus {
    let env = workspace_env(repo_root, checkout_path, branch, settings);
    let mut proc = std::process::Command::new("/bin/sh");
    proc.arg("-lc")
        .arg(script)
        .current_dir(checkout_path)
        .envs(env);
    match proc.output() {
        Ok(output) if output.status.success() => SetupStatus::Ok,
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            let message = if stderr.is_empty() {
                format!("setup script exited with {}", output.status)
            } else {
                stderr.to_string()
            };
            SetupStatus::Failed(message)
        }
        Err(err) => SetupStatus::Failed(format!("failed to run setup script: {err}")),
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
            .expect("git command spawns");
        assert!(status.success(), "git failed: {}", args.join(" "));
    }

    fn init_repo(name: &str) -> PathBuf {
        let repo = unique_temp_path(name);
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "--quiet"]);
        repo
    }

    #[test]
    fn parses_settings_with_run_mode_and_defaults() {
        let toml = r#"
[scripts]
setup = "echo hi"
run = "pnpm dev"
run_mode = "exclusive"

[files]
copy = ["CLAUDE.md"]
symlink = [".claude"]

[ports]
base = 5000
"#;
        let settings: BoraSettings = toml::from_str(toml).unwrap();
        assert_eq!(settings.scripts.run_mode, BoraRunMode::Exclusive);
        assert_eq!(settings.scripts.setup.as_deref(), Some("echo hi"));
        assert_eq!(settings.scripts.run.as_deref(), Some("pnpm dev"));
        let files = settings.files.expect("files section");
        assert_eq!(files.copy, vec!["CLAUDE.md".to_string()]);
        assert_eq!(files.symlink, vec![".claude".to_string()]);
        // Explicit base, defaulted max.
        assert_eq!(settings.ports.base, 5000);
        assert_eq!(settings.ports.max, 4199);

        // Empty document falls back to all defaults.
        let empty: BoraSettings = toml::from_str("").unwrap();
        assert_eq!(empty.scripts.run_mode, BoraRunMode::Concurrent);
        assert!(empty.files.is_none());
        assert_eq!(empty.ports.base, 4100);
        assert_eq!(empty.ports.max, 4199);
    }

    #[test]
    fn setup_status_as_str() {
        assert_eq!(SetupStatus::Ok.as_str(), "ok");
        assert_eq!(SetupStatus::Failed("x".into()).as_str(), "failed");
        assert_eq!(SetupStatus::Skipped.as_str(), "skipped");
    }

    #[test]
    fn provision_symlinks_and_copies_snapshot_skips_existing() {
        let repo = init_repo("provision-files");
        std::fs::write(repo.join("CLAUDE.md"), "root claude\n").unwrap();
        std::fs::write(repo.join(".env.example"), "KEY=val\n").unwrap();
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(repo.join(".claude").join("config"), "cfg\n").unwrap();
        std::fs::create_dir_all(repo.join(".bora")).unwrap();
        std::fs::write(
            repo.join(".bora").join("settings.toml"),
            "[files]\ncopy = [\"CLAUDE.md\", \".env.example\"]\nsymlink = [\".claude\"]\n",
        )
        .unwrap();

        let checkout = unique_temp_path("provision-files-wt");
        std::fs::create_dir_all(&checkout).unwrap();
        // Pre-existing dst must not be overwritten.
        std::fs::write(checkout.join("CLAUDE.md"), "pre-existing\n").unwrap();

        let status = provision_worktree(&repo, &checkout, Some("feature/x"), true);
        assert_eq!(status, SetupStatus::Skipped);

        // Copy skipped for pre-existing file.
        assert_eq!(
            std::fs::read_to_string(checkout.join("CLAUDE.md")).unwrap(),
            "pre-existing\n"
        );
        // Copy applied for absent dst (real file, not symlink).
        let env_dst = checkout.join(".env.example");
        assert!(!std::fs::symlink_metadata(&env_dst)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_to_string(&env_dst).unwrap(), "KEY=val\n");
        // Symlink created, pointing at the absolute root path.
        let link = checkout.join(".claude");
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), repo.join(".claude"));
        assert_eq!(
            std::fs::read_to_string(link.join("config")).unwrap(),
            "cfg\n"
        );
    }

    #[test]
    fn provision_legacy_worktreeinclude_fallback() {
        let repo = init_repo("legacy");
        std::fs::write(repo.join("meta.txt"), "meta\n").unwrap();
        std::fs::write(repo.join(".worktreeinclude"), "meta.txt\n").unwrap();
        // No .bora/settings.toml at all → legacy path.
        let checkout = unique_temp_path("legacy-wt");
        std::fs::create_dir_all(&checkout).unwrap();

        let status = provision_worktree(&repo, &checkout, None, true);
        assert_eq!(status, SetupStatus::Skipped);
        assert_eq!(
            std::fs::read_to_string(checkout.join("meta.txt")).unwrap(),
            "meta\n"
        );

        // Settings present but no [files] section → still legacy fallback.
        std::fs::create_dir_all(repo.join(".bora")).unwrap();
        std::fs::write(
            repo.join(".bora").join("settings.toml"),
            "[scripts]\nsetup = \"true\"\n",
        )
        .unwrap();
        let checkout2 = unique_temp_path("legacy-wt2");
        std::fs::create_dir_all(&checkout2).unwrap();
        provision_worktree(&repo, &checkout2, None, true);
        assert_eq!(
            std::fs::read_to_string(checkout2.join("meta.txt")).unwrap(),
            "meta\n"
        );
    }

    #[test]
    fn setup_script_receives_workspace_env() {
        let repo = init_repo("setup-env");
        std::fs::create_dir_all(repo.join(".bora")).unwrap();
        std::fs::write(
            repo.join(".bora").join("settings.toml"),
            "[scripts]\nsetup = \"printf '%s\\n%s\\n%s\\n%s\\n' \\\"$BORA_ROOT_PATH\\\" \\\"$BORA_WORKSPACE_PATH\\\" \\\"$BORA_WORKSPACE_ID\\\" \\\"$BORA_BRANCH\\\" > envdump.txt\"\n",
        )
        .unwrap();

        let checkout = unique_temp_path("setup-env-wt");
        std::fs::create_dir_all(&checkout).unwrap();

        let status = provision_worktree(&repo, &checkout, Some("feature/y"), false);
        assert_eq!(status, SetupStatus::Ok);

        let dump = std::fs::read_to_string(checkout.join("envdump.txt")).unwrap();
        let lines: Vec<&str> = dump.lines().collect();
        assert_eq!(lines[0], repo.display().to_string());
        assert_eq!(lines[1], checkout.display().to_string());
        assert_eq!(
            lines[2],
            checkout.file_name().unwrap().to_string_lossy().as_ref()
        );
        assert_eq!(lines[3], "feature/y");

        // Failing script surfaces Failed.
        std::fs::write(
            repo.join(".bora").join("settings.toml"),
            "[scripts]\nsetup = \"exit 3\"\n",
        )
        .unwrap();
        let checkout_fail = unique_temp_path("setup-env-fail");
        std::fs::create_dir_all(&checkout_fail).unwrap();
        let status = provision_worktree(&repo, &checkout_fail, None, false);
        assert!(matches!(status, SetupStatus::Failed(_)), "got {status:?}");

        // no_setup skips execution entirely.
        let checkout_skip = unique_temp_path("setup-env-skip");
        std::fs::create_dir_all(&checkout_skip).unwrap();
        assert_eq!(
            provision_worktree(&repo, &checkout_skip, None, true),
            SetupStatus::Skipped
        );
        assert!(!checkout_skip.join("envdump.txt").exists());
    }

    #[test]
    fn port_allocation_is_stable_and_exhausts() {
        let repo = init_repo("ports");
        let range = BoraPortsRange {
            base: 4100,
            max: 4101,
        };
        let first = allocate_port(&repo, "branch-a", &range).unwrap();
        let first_again = allocate_port(&repo, "branch-a", &range).unwrap();
        assert_eq!(first, first_again);
        assert_eq!(first, 4100);

        let second = allocate_port(&repo, "branch-b", &range).unwrap();
        assert_ne!(first, second);
        assert_eq!(second, 4101);

        // Range exhausted for a new key.
        assert_eq!(allocate_port(&repo, "branch-c", &range), None);
    }

    #[test]
    fn settings_ports_win_over_legacy_bora_toml() {
        let repo = init_repo("resolve-ports");
        // Legacy config with index-based ports.
        std::fs::write(
            repo.join(".bora.toml"),
            "[ports]\nbase = 9000\nper_worktree = 10\n",
        )
        .unwrap();
        // Settings with explicit [ports] must take precedence.
        std::fs::create_dir_all(repo.join(".bora")).unwrap();
        std::fs::write(
            repo.join(".bora").join("settings.toml"),
            "[ports]\nbase = 4100\nmax = 4199\n",
        )
        .unwrap();

        let checkout = unique_temp_path("resolve-ports-wt");
        std::fs::create_dir_all(&checkout).unwrap();
        let port = resolve_port(&repo, &checkout, "feature/z").unwrap();
        // Allocator hands out the base of its range, not the legacy 9000.
        assert_eq!(port, 4100);
    }

    #[test]
    fn provision_creates_context_dir_and_excludes_once() {
        let repo = init_repo("context");
        let checkout = unique_temp_path("context-wt");
        std::fs::create_dir_all(&checkout).unwrap();

        provision_worktree(&repo, &checkout, None, true);
        assert!(checkout.join(".context").is_dir());

        let exclude = git_common_dir(&repo).join("info").join("exclude");
        let count = |path: &Path| {
            std::fs::read_to_string(path)
                .unwrap()
                .lines()
                .filter(|line| line.trim() == ".context/")
                .count()
        };
        assert_eq!(count(&exclude), 1);

        // Idempotent on a second provision.
        provision_worktree(&repo, &checkout, None, true);
        assert_eq!(count(&exclude), 1);
    }

    #[test]
    fn port_allocation_is_collision_free_under_concurrency() {
        let repo = init_repo("ports-concurrent");
        let range = BoraPortsRange {
            base: 6100,
            max: 6199,
        };
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let repo = repo.clone();
                let range = range.clone();
                std::thread::spawn(move || allocate_port(&repo, &format!("branch-{i}"), &range))
            })
            .collect();
        let ports: Vec<u16> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap().expect("port allocated"))
            .collect();
        let unique: HashSet<u16> = ports.iter().copied().collect();
        assert_eq!(
            unique.len(),
            8,
            "concurrent allocations collided: {ports:?}"
        );
        // Lockfile is released.
        assert!(!git_common_dir(&repo)
            .join("info")
            .join("bora-ports.json.lock")
            .exists());
    }
}
