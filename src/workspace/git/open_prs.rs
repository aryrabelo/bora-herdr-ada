use std::path::Path;

// ── Types ────────────────────────────────────────────────────────────────────

/// An open PR authored by the current user in a repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub head_ref_name: String,
    pub is_draft: bool,
}

/// Open PRs authored by the current user for one repo.
///
/// Always returned as a value — errors are captured in the `error` field
/// rather than propagated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoOpenPrs {
    pub prs: Vec<OpenPr>,
    pub error: Option<String>,
}

// ── JSON parsing ─────────────────────────────────────────────────────────────

/// Parse `gh pr list --json` output into a list of `OpenPr`.
///
/// Expected JSON shape (from `--json number,title,url,headRefName,isDraft`):
/// ```json
/// [
///   {
///     "number": 42,
///     "title": "feat: thing",
///     "url": "https://github.com/owner/repo/pull/42",
///     "headRefName": "feat/thing",
///     "isDraft": false
///   },
///   ...
/// ]
/// ```
pub(super) fn parse_gh_pr_list_json(json_str: &str) -> Result<Vec<OpenPr>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON from gh: {e}"))?;

    let items = value
        .as_array()
        .ok_or_else(|| "gh output is not a JSON array".to_string())?;

    Ok(items
        .iter()
        .filter_map(|item| {
            let number = item.get("number").and_then(serde_json::Value::as_u64)?;
            let title = item
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let url = item
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let head_ref_name = item
                .get("headRefName")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let is_draft = item
                .get("isDraft")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            Some(OpenPr {
                number,
                title,
                url,
                head_ref_name,
                is_draft,
            })
        })
        .collect())
}

// ── Acquisition ──────────────────────────────────────────────────────────────

/// Fetch the current user's open PRs for the repo at `cwd` via `gh pr list`.
///
/// Returns a `RepoOpenPrs` that always has a value — errors are captured in
/// the `error` field rather than propagated.
pub fn fetch_my_open_prs(cwd: &Path) -> RepoOpenPrs {
    let output = match std::process::Command::new("gh")
        .current_dir(cwd)
        .args([
            "pr",
            "list",
            "--author",
            "@me",
            "--state",
            "open",
            "--json",
            "number,title,url,headRefName,isDraft",
        ])
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            // gh not installed or not executable
            let msg = if e.kind() == std::io::ErrorKind::NotFound {
                "gh CLI not found".to_string()
            } else {
                format!("failed to run gh: {e}")
            };
            tracing::debug!("open_prs: {msg}");
            return RepoOpenPrs {
                prs: Vec::new(),
                error: Some(msg),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // Common cases: not authenticated, not a GitHub remote
        let msg = if stderr.contains("authentication") || stderr.contains("auth") {
            "gh not authenticated".to_string()
        } else if stderr.is_empty() {
            "gh pr list failed".to_string()
        } else {
            stderr
        };
        tracing::debug!("open_prs: gh failed: {msg}");
        return RepoOpenPrs {
            prs: Vec::new(),
            error: Some(msg),
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    match parse_gh_pr_list_json(&stdout) {
        Ok(prs) => RepoOpenPrs { prs, error: None },
        Err(e) => {
            tracing::warn!("open_prs: failed to parse gh output: {e}");
            RepoOpenPrs {
                prs: Vec::new(),
                error: Some(e),
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_open_pr_list_happy_path() {
        let json = r#"[
            {
                "number": 42,
                "title": "feat: add widget",
                "url": "https://github.com/owner/repo/pull/42",
                "headRefName": "feat/widget",
                "isDraft": false
            },
            {
                "number": 43,
                "title": "fix: crash",
                "url": "https://github.com/owner/repo/pull/43",
                "headRefName": "fix/crash",
                "isDraft": false
            }
        ]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].number, 42);
        assert_eq!(prs[0].title, "feat: add widget");
        assert_eq!(prs[0].url, "https://github.com/owner/repo/pull/42");
        assert_eq!(prs[0].head_ref_name, "feat/widget");
        assert!(!prs[0].is_draft);
        assert_eq!(prs[1].number, 43);
    }

    #[test]
    fn parse_empty_array_returns_no_prs() {
        let prs = parse_gh_pr_list_json("[]").unwrap();
        assert!(prs.is_empty());
    }

    #[test]
    fn parse_draft_pr() {
        let json = r#"[
            {
                "number": 7,
                "title": "wip: experiment",
                "url": "https://github.com/o/r/pull/7",
                "headRefName": "wip/experiment",
                "isDraft": true
            }
        ]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs.len(), 1);
        assert!(prs[0].is_draft);
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let result = parse_gh_pr_list_json("not json at all");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid JSON"));
    }

    #[test]
    fn parse_non_array_returns_error() {
        let result = parse_gh_pr_list_json(r#"{"number": 1}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a JSON array"));
    }

    #[test]
    fn parse_pr_with_missing_optional_fields_uses_defaults() {
        // Minimal valid: just number
        let json = r#"[{"number": 9}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 9);
        assert_eq!(prs[0].title, "");
        assert_eq!(prs[0].url, "");
        assert_eq!(prs[0].head_ref_name, "");
        assert!(!prs[0].is_draft);
    }

    #[test]
    fn parse_pr_without_number_is_skipped() {
        let json = r#"[
            {"title": "no number", "url": "", "headRefName": "x", "isDraft": false},
            {"number": 5, "title": "real", "url": "", "headRefName": "y", "isDraft": false}
        ]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        // The numberless PR entry is skipped
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].number, 5);
        assert_eq!(prs[0].title, "real");
    }
}
