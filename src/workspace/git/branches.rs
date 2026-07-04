use std::path::Path;

// ── Types ────────────────────────────────────────────────────────────────────

/// A local git branch in a repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoBranch {
    pub name: String,
    pub is_current: bool,
}

/// Local branches for one repo.
///
/// Always returned as a value — errors are captured in the `error` field
/// rather than propagated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoBranches {
    pub branches: Vec<RepoBranch>,
    pub error: Option<String>,
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/// Parse `git for-each-ref --format=%(refname:short)%00%(HEAD) refs/heads`
/// output into a list of `RepoBranch`, sorted current-first then
/// lexicographically. Each line is `<name>\0<head-marker>` where the marker is
/// `*` for the currently checked-out branch and a space otherwise.
fn parse_for_each_ref(stdout: &str) -> Vec<RepoBranch> {
    let mut branches: Vec<RepoBranch> = stdout
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\0');
            let name = parts.next()?.trim();
            if name.is_empty() {
                return None;
            }
            let is_current = parts.next().is_some_and(|marker| marker.contains('*'));
            Some(RepoBranch {
                name: name.to_string(),
                is_current,
            })
        })
        .collect();
    branches.sort_by(|a, b| {
        b.is_current
            .cmp(&a.is_current)
            .then_with(|| a.name.cmp(&b.name))
    });
    branches
}

// ── Acquisition ──────────────────────────────────────────────────────────────

/// List local branches for the repo at `cwd` via `git for-each-ref`.
///
/// Returns a `RepoBranches` that always has a value — errors are captured in
/// the `error` field rather than propagated.
pub fn fetch_local_branches(cwd: &Path) -> RepoBranches {
    let output = match std::process::Command::new("git")
        .current_dir(cwd)
        .args([
            "for-each-ref",
            "--format=%(refname:short)%00%(HEAD)",
            "refs/heads",
        ])
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                "git not found".to_string()
            } else {
                format!("failed to run git: {e}")
            };
            tracing::debug!("branches: {msg}");
            return RepoBranches {
                branches: Vec::new(),
                error: Some(msg),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let msg = if stderr.is_empty() {
            "git for-each-ref failed".to_string()
        } else {
            stderr
        };
        tracing::debug!("branches: git failed: {msg}");
        return RepoBranches {
            branches: Vec::new(),
            error: Some(msg),
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    RepoBranches {
        branches: parse_for_each_ref(&stdout),
        error: None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_and_current_marker() {
        let stdout = "feature/b\0\nmain\0*\nfeature/a\0\n";
        let branches = parse_for_each_ref(stdout);
        assert_eq!(
            branches,
            vec![
                RepoBranch {
                    name: "main".into(),
                    is_current: true,
                },
                RepoBranch {
                    name: "feature/a".into(),
                    is_current: false,
                },
                RepoBranch {
                    name: "feature/b".into(),
                    is_current: false,
                },
            ]
        );
    }

    #[test]
    fn empty_output_is_empty_list() {
        assert!(parse_for_each_ref("").is_empty());
        assert!(parse_for_each_ref("\n").is_empty());
    }

    #[test]
    fn fetch_local_branches_lists_current_first() {
        let repo = std::env::temp_dir().join(format!(
            "herdr-branches-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let status = std::process::Command::new("git")
                .current_dir(&repo)
                .args(args)
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "--quiet", "-b", "main"]);
        git(&["config", "user.email", "herdr@example.invalid"]);
        git(&["config", "user.name", "Herdr Test"]);
        std::fs::write(repo.join("README.md"), "x\n").unwrap();
        git(&["add", "README.md"]);
        git(&["commit", "--quiet", "-m", "init"]);
        git(&["branch", "feature/z"]);
        git(&["branch", "feature/a"]);

        let result = fetch_local_branches(&repo);
        assert!(
            result.error.is_none(),
            "unexpected error: {:?}",
            result.error
        );
        let names: Vec<&str> = result.branches.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, vec!["main", "feature/a", "feature/z"]);
        assert!(result.branches[0].is_current);
        assert!(!result.branches[1].is_current);

        let _ = std::fs::remove_dir_all(repo);
    }
}
