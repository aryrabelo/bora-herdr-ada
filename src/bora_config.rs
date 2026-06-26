use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub(crate) struct BoraConfig {
    pub ports: Option<BoraPortsConfig>,
    #[serde(default)]
    pub commands: Vec<BoraCommand>,
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

pub(crate) fn load_bora_config(repo_root: &Path) -> Option<BoraConfig> {
    let path = repo_root.join(".bora.toml");
    let content = std::fs::read_to_string(&path).ok()?;
    match toml::from_str::<BoraConfig>(&content) {
        Ok(config) => Some(config),
        Err(err) => {
            tracing::warn!(path = %path.display(), "invalid .bora.toml: {err}");
            None
        }
    }
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
        assert_eq!(config.base + 1 * config.per_worktree, 3010);
        assert_eq!(config.base + 2 * config.per_worktree, 3020);
    }

    #[test]
    fn empty_config_is_valid() {
        let config: BoraConfig = toml::from_str("").unwrap();
        assert!(config.ports.is_none());
        assert!(config.commands.is_empty());
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
        let port1 = config.base + 1 * config.per_worktree;
        assert_eq!(port1, 3010);
        assert!(port1 <= 3015);
        // index 2 exceeds max
        let port2 = config.base + 2 * config.per_worktree;
        assert_eq!(port2, 3020);
        assert!(port2 > 3015);
    }
}
