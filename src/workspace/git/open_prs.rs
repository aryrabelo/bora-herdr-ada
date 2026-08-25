use std::path::Path;

use super::check_status::{check_run_state, reduce_run_states};
use super::ChecksRollup;

// ── Types ────────────────────────────────────────────────────────────────────

/// An open PR authored by the current user in a repo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenPr {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub head_ref_name: String,
    pub is_draft: bool,
    pub mergeable: Option<String>,
    pub checks: Option<ChecksRollup>,
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

// ── CI check rollup ─────────────────────────────────────────────────────────
//
// Reuses `check_status::ChecksRollup` and its `check_run_state`/
// `reduce_run_states` classifier rather than a second copy of the
// conclusion-string mapping: that classifier is the single owner of "this
// (status, conclusion) means this state" so `checks_rollup` (sidebar CHECKS
// rows) and this `statusCheckRollup` reduction can never drift apart again.
// What's genuinely ours is the JSON layer below: `statusCheckRollup` mixes
// two GraphQL node shapes — CheckRun (`status`/`conclusion`) and
// StatusContext (`state`) — and only this call site needs to tell them apart.

/// Buckets one `statusCheckRollup` array element through the shared
/// classifier. A StatusContext node's `state` is treated as a completed run's
/// conclusion (`SUCCESS`/`ERROR`/`FAILURE` land the same as the equivalent
/// CheckRun conclusion; anything else — `PENDING`, `EXPECTED` — falls into
/// the classifier's `Pending` catch-all). A node with neither a `state` nor a
/// `status` key is not a shape this code understands and contributes nothing.
fn node_outcome(item: &serde_json::Value) -> Option<ChecksRollup> {
    if let Some(state) = item.get("state").and_then(|v| v.as_str()) {
        return Some(check_run_state("COMPLETED", Some(state)));
    }
    let status = item.get("status").and_then(|v| v.as_str())?;
    let conclusion = item.get("conclusion").and_then(|v| v.as_str());
    Some(check_run_state(status, conclusion))
}

/// Reduces a PR's `statusCheckRollup` array via the shared
/// `Failing` > `Pending` > `Passing` precedence, `None` when no element
/// contributes (absent, null, or empty `statusCheckRollup`).
fn reduce_checks(items: &[serde_json::Value]) -> Option<ChecksRollup> {
    reduce_run_states(items.iter().filter_map(node_outcome))
}

// ── JSON parsing ─────────────────────────────────────────────────────────────

/// Parse `gh pr list --json` output into a list of `OpenPr`.
///
/// Expected JSON shape (from
/// `--json number,title,url,headRefName,isDraft,mergeable,statusCheckRollup`):
/// ```json
/// [
///   {
///     "number": 42,
///     "title": "feat: thing",
///     "url": "https://github.com/owner/repo/pull/42",
///     "headRefName": "feat/thing",
///     "isDraft": false,
///     "mergeable": "MERGEABLE",
///     "statusCheckRollup": [
///       { "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS" }
///     ]
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
            let mergeable = item
                .get("mergeable")
                .and_then(|v| v.as_str())
                .map(String::from);
            let checks = item
                .get("statusCheckRollup")
                .and_then(|v| v.as_array())
                .and_then(|arr| reduce_checks(arr));
            Some(OpenPr {
                number,
                title,
                url,
                head_ref_name,
                is_draft,
                mergeable,
                checks,
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
            "number,title,url,headRefName,isDraft,mergeable,statusCheckRollup",
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
                "isDraft": false,
                "mergeable": "MERGEABLE"
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
        assert_eq!(prs[0].mergeable.as_deref(), Some("MERGEABLE"));
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
        assert_eq!(prs[0].mergeable, None);
    }

    #[test]
    fn parse_conflicting_mergeable() {
        let json = r#"[
            {"number": 8, "mergeable": "CONFLICTING"},
            {"number": 9, "mergeable": null}
        ]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].mergeable.as_deref(), Some("CONFLICTING"));
        assert_eq!(prs[1].mergeable, None);
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

    #[test]
    fn pr_checks_rollup_absent_field_is_none() {
        let json = r#"[{"number": 1}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, None);
    }

    #[test]
    fn pr_checks_rollup_null_field_is_none() {
        let json = r#"[{"number": 1, "statusCheckRollup": null}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, None);
    }

    #[test]
    fn pr_checks_rollup_empty_array_is_none() {
        let json = r#"[{"number": 1, "statusCheckRollup": []}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, None);
    }

    #[test]
    fn pr_checks_rollup_all_success_variants_are_passing() {
        let json = r#"[{"number": 1, "statusCheckRollup": [
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "NEUTRAL"},
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SKIPPED"}
        ]}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, Some(ChecksRollup::Passing));
    }

    #[test]
    fn pr_checks_rollup_each_failing_conclusion_is_failing() {
        for conclusion in [
            "FAILURE",
            "ERROR",
            "TIMED_OUT",
            "CANCELLED",
            "ACTION_REQUIRED",
            "STARTUP_FAILURE",
        ] {
            let json = format!(
                r#"[{{"number": 1, "statusCheckRollup": [
                    {{"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "{conclusion}"}}
                ]}}]"#
            );
            let prs = parse_gh_pr_list_json(&json).unwrap();
            assert_eq!(
                prs[0].checks,
                Some(ChecksRollup::Failing),
                "conclusion {conclusion} should be Failing"
            );
        }
    }

    #[test]
    fn pr_checks_rollup_in_progress_check_is_pending() {
        let json = r#"[{"number": 1, "statusCheckRollup": [
            {"__typename": "CheckRun", "status": "IN_PROGRESS", "conclusion": null}
        ]}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, Some(ChecksRollup::Pending));
    }

    #[test]
    fn pr_checks_rollup_completed_with_null_conclusion_is_pending() {
        let json = r#"[{"number": 1, "statusCheckRollup": [
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": null}
        ]}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, Some(ChecksRollup::Pending));
    }

    #[test]
    fn pr_checks_rollup_status_context_node_success_is_passing() {
        let json = r#"[{"number": 1, "statusCheckRollup": [
            {"__typename": "StatusContext", "context": "ci/circleci", "state": "SUCCESS"}
        ]}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, Some(ChecksRollup::Passing));
    }

    #[test]
    fn pr_checks_rollup_status_context_node_error_is_failing() {
        let json = r#"[{"number": 1, "statusCheckRollup": [
            {"__typename": "StatusContext", "context": "ci/circleci", "state": "ERROR"}
        ]}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, Some(ChecksRollup::Failing));
    }

    #[test]
    fn pr_checks_rollup_status_context_node_pending_states_are_pending() {
        for state in ["PENDING", "EXPECTED"] {
            let json = format!(
                r#"[{{"number": 1, "statusCheckRollup": [
                    {{"__typename": "StatusContext", "context": "ci/x", "state": "{state}"}}
                ]}}]"#
            );
            let prs = parse_gh_pr_list_json(&json).unwrap();
            assert_eq!(
                prs[0].checks,
                Some(ChecksRollup::Pending),
                "state {state} should be Pending"
            );
        }
    }

    #[test]
    fn pr_checks_rollup_unrecognised_conclusion_is_not_passing() {
        let json = r#"[{"number": 1, "statusCheckRollup": [
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SOME_NEW_CONCLUSION"}
        ]}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_ne!(prs[0].checks, Some(ChecksRollup::Passing));
        assert_eq!(prs[0].checks, Some(ChecksRollup::Pending));
    }

    #[test]
    fn pr_checks_rollup_precedence_failing_beats_pending_and_passing() {
        let json = r#"[{"number": 1, "statusCheckRollup": [
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "status": "IN_PROGRESS", "conclusion": null},
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "FAILURE"}
        ]}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, Some(ChecksRollup::Failing));
    }

    #[test]
    fn pr_checks_rollup_precedence_pending_beats_passing() {
        let json = r#"[{"number": 1, "statusCheckRollup": [
            {"__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "status": "QUEUED", "conclusion": null}
        ]}]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, Some(ChecksRollup::Pending));
    }

    #[test]
    fn pr_checks_rollup_field_added_to_existing_happy_path_prs() {
        // Existing gh pr list fixtures don't include statusCheckRollup at all —
        // parsing must still succeed and default the new field to None.
        let json = r#"[
            {
                "number": 42,
                "title": "feat: add widget",
                "url": "https://github.com/owner/repo/pull/42",
                "headRefName": "feat/widget",
                "isDraft": false,
                "mergeable": "MERGEABLE"
            }
        ]"#;
        let prs = parse_gh_pr_list_json(json).unwrap();
        assert_eq!(prs[0].checks, None);
    }
}
