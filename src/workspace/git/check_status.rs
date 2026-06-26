use std::path::Path;

// ── Types ────────────────────────────────────────────────────────────────────

/// PR summary and CI check status for a workspace branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCheckStatus {
    pub pr: Option<PrSummary>,
    pub checks: Vec<CheckRun>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrSummary {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub url: String,
    pub mergeable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

// ── JSON parsing ─────────────────────────────────────────────────────────────

/// Parse `gh pr view --json` output into a `WorkspaceCheckStatus`.
///
/// Expected JSON shape (from `--json number,title,state,url,statusCheckRollup,mergeable`):
/// ```json
/// {
///   "number": 42,
///   "title": "feat: thing",
///   "state": "OPEN",
///   "url": "https://github.com/owner/repo/pull/42",
///   "mergeable": "MERGEABLE",
///   "statusCheckRollup": [
///     { "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS" },
///     ...
///   ]
/// }
/// ```
pub(super) fn parse_gh_pr_json(json_str: &str) -> Result<WorkspaceCheckStatus, String> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON from gh: {e}"))?;

    let obj = value
        .as_object()
        .ok_or_else(|| "gh output is not a JSON object".to_string())?;

    let number = obj
        .get("number")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| "missing or invalid 'number' field".to_string())?;

    let title = obj
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let state = obj
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("UNKNOWN")
        .to_string();

    let url = obj
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mergeable = obj
        .get("mergeable")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    let checks = obj
        .get("statusCheckRollup")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let name = item.get("name").and_then(|v| v.as_str())?.to_string();
                    let status = item
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("QUEUED")
                        .to_string();
                    let conclusion = item
                        .get("conclusion")
                        .and_then(|v| v.as_str())
                        .map(str::to_string);
                    Some(CheckRun {
                        name,
                        status,
                        conclusion,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(WorkspaceCheckStatus {
        pr: Some(PrSummary {
            number,
            title,
            state,
            url,
            mergeable,
        }),
        checks,
        error: None,
    })
}

// ── Acquisition ──────────────────────────────────────────────────────────────

/// Fetch PR + check status for the given branch by running `gh pr view`.
///
/// Returns a `WorkspaceCheckStatus` that always has a value — errors are
/// captured in the `error` field rather than propagated.
#[allow(dead_code)] // called by App::start_checks_fetch (slice 4 trigger)
pub fn fetch_check_status(cwd: &Path, branch: &str) -> WorkspaceCheckStatus {
    let output = match std::process::Command::new("gh")
        .current_dir(cwd)
        .args([
            "pr",
            "view",
            branch,
            "--json",
            "number,title,state,url,statusCheckRollup,mergeable",
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
            tracing::debug!("check_status: {msg}");
            return WorkspaceCheckStatus {
                pr: None,
                checks: Vec::new(),
                error: Some(msg),
            };
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // Common cases: no PR exists, not authenticated, not a GitHub remote
        let msg = if stderr.contains("no pull requests found") {
            "no PR for this branch".to_string()
        } else if stderr.contains("authentication") || stderr.contains("auth") {
            "gh not authenticated".to_string()
        } else if stderr.is_empty() {
            "gh pr view failed".to_string()
        } else {
            stderr
        };
        tracing::debug!("check_status: gh failed for branch {branch:?}: {msg}");
        return WorkspaceCheckStatus {
            pr: None,
            checks: Vec::new(),
            error: Some(msg),
        };
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    match parse_gh_pr_json(&stdout) {
        Ok(status) => status,
        Err(e) => {
            tracing::warn!("check_status: failed to parse gh output: {e}");
            WorkspaceCheckStatus {
                pr: None,
                checks: Vec::new(),
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
    fn parse_pr_with_passing_checks() {
        let json = r#"{
            "number": 42,
            "title": "feat: add widget",
            "state": "OPEN",
            "url": "https://github.com/owner/repo/pull/42",
            "mergeable": "MERGEABLE",
            "statusCheckRollup": [
                {"name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS"}
            ]
        }"#;
        let status = parse_gh_pr_json(json).unwrap();
        let pr = status.pr.unwrap();
        assert_eq!(pr.number, 42);
        assert_eq!(pr.title, "feat: add widget");
        assert_eq!(pr.state, "OPEN");
        assert_eq!(pr.url, "https://github.com/owner/repo/pull/42");
        assert_eq!(pr.mergeable.as_deref(), Some("MERGEABLE"));
        assert_eq!(status.checks.len(), 2);
        assert_eq!(status.checks[0].name, "build");
        assert_eq!(status.checks[0].status, "COMPLETED");
        assert_eq!(status.checks[0].conclusion.as_deref(), Some("SUCCESS"));
        assert!(status.error.is_none());
    }

    #[test]
    fn parse_pr_with_mixed_check_results() {
        let json = r#"{
            "number": 99,
            "title": "fix: thing",
            "state": "OPEN",
            "url": "https://github.com/o/r/pull/99",
            "mergeable": "CONFLICTING",
            "statusCheckRollup": [
                {"name": "ci", "status": "COMPLETED", "conclusion": "FAILURE"},
                {"name": "deploy", "status": "IN_PROGRESS"},
                {"name": "security", "status": "QUEUED", "conclusion": null}
            ]
        }"#;
        let status = parse_gh_pr_json(json).unwrap();
        let pr = status.pr.unwrap();
        assert_eq!(pr.number, 99);
        assert_eq!(pr.mergeable.as_deref(), Some("CONFLICTING"));
        assert_eq!(status.checks.len(), 3);

        assert_eq!(status.checks[0].conclusion.as_deref(), Some("FAILURE"));
        assert_eq!(status.checks[1].status, "IN_PROGRESS");
        assert!(status.checks[1].conclusion.is_none());
        assert!(status.checks[2].conclusion.is_none());
    }

    #[test]
    fn parse_pr_with_no_checks() {
        let json = r#"{
            "number": 1,
            "title": "docs: readme",
            "state": "MERGED",
            "url": "https://github.com/o/r/pull/1",
            "mergeable": "",
            "statusCheckRollup": []
        }"#;
        let status = parse_gh_pr_json(json).unwrap();
        assert!(status.pr.is_some());
        assert!(status.checks.is_empty());
    }

    #[test]
    fn parse_pr_with_null_status_check_rollup() {
        let json = r#"{
            "number": 5,
            "title": "chore: bump",
            "state": "OPEN",
            "url": "https://github.com/o/r/pull/5",
            "statusCheckRollup": null
        }"#;
        let status = parse_gh_pr_json(json).unwrap();
        assert!(status.pr.is_some());
        assert!(status.checks.is_empty());
        assert!(status.pr.unwrap().mergeable.is_none());
    }

    #[test]
    fn parse_pr_with_missing_optional_fields() {
        // Minimal valid: just number
        let json = r#"{"number": 7}"#;
        let status = parse_gh_pr_json(json).unwrap();
        let pr = status.pr.unwrap();
        assert_eq!(pr.number, 7);
        assert_eq!(pr.title, "");
        assert_eq!(pr.state, "UNKNOWN");
        assert_eq!(pr.url, "");
        assert!(pr.mergeable.is_none());
        assert!(status.checks.is_empty());
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let result = parse_gh_pr_json("not json at all");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid JSON"));
    }

    #[test]
    fn parse_missing_number_returns_error() {
        let json = r#"{"title": "no number", "state": "OPEN"}"#;
        let result = parse_gh_pr_json(json);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("number"));
    }

    #[test]
    fn parse_check_run_without_name_is_skipped() {
        let json = r#"{
            "number": 10,
            "title": "t",
            "state": "OPEN",
            "url": "",
            "statusCheckRollup": [
                {"status": "COMPLETED", "conclusion": "SUCCESS"},
                {"name": "real", "status": "COMPLETED", "conclusion": "SUCCESS"}
            ]
        }"#;
        let status = parse_gh_pr_json(json).unwrap();
        // The nameless check is skipped
        assert_eq!(status.checks.len(), 1);
        assert_eq!(status.checks[0].name, "real");
    }

    #[test]
    fn parse_closed_pr() {
        let json = r#"{
            "number": 3,
            "title": "old PR",
            "state": "CLOSED",
            "url": "https://github.com/o/r/pull/3",
            "mergeable": "UNKNOWN",
            "statusCheckRollup": [
                {"name": "ci", "status": "COMPLETED", "conclusion": "NEUTRAL"}
            ]
        }"#;
        let status = parse_gh_pr_json(json).unwrap();
        let pr = status.pr.unwrap();
        assert_eq!(pr.state, "CLOSED");
        assert_eq!(status.checks[0].conclusion.as_deref(), Some("NEUTRAL"));
    }
}
