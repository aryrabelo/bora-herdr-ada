use std::path::Path;

use super::check_provider::{CheckProvider, ProviderOutcome};

// ── Types ────────────────────────────────────────────────────────────────────

/// PR summary and CI check status for a workspace branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceCheckStatus {
    pub pr: Option<PrSummary>,
    pub checks: Vec<CheckRun>,
    pub error: Option<String>,
}

/// The `error` value `legacy_check_status` uses for
/// `ProviderOutcome::NotApplicable`, preserved verbatim from the pre-provider
/// `fetch_check_status` so existing consumers keep their behavior.
pub(crate) const NOT_APPLICABLE_ERROR: &str = "no PR for this branch";

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

/// Aggregate state of a PR's checks, mirroring the statusline rollup rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksRollup {
    Passing,
    Failing,
    Pending,
}

/// The single owner of "one check's `(status, conclusion)` means this state".
///
/// Every consumer derives from this: `checks_rollup` and `open_prs`'s
/// `statusCheckRollup` reduction. An earlier version of this module owned only
/// the *failing* set and let each caller infer the rest, with a doc comment
/// promising the consumers "can never drift apart"; they drifted the moment a
/// fourth consumer appeared, because inferring two states from one predicate
/// is not a shared rule, it is three separate guesses.
///
/// `NEUTRAL` and `SKIPPED` are `Passing`: neither blocks a merge.
///
/// Anything unrecognised is `Pending`, never `Passing`. This is the load-bearing
/// case and it is deliberate in two places at once:
///
/// - a `COMPLETED` run whose conclusion is `None`, which GitHub really does
///   emit, previously fell through to `Passing` — a check with no result at all
///   displayed as green;
/// - a `COMPLETED` run carrying a conclusion string GitHub adds after this code
///   was written did the same.
///
/// Green is a claim about someone else's CI; pending is an admission that we do
/// not know. Only one of those is safe to be wrong about.
fn run_state(status: &str, conclusion: Option<&str>) -> ChecksRollup {
    if status != "COMPLETED" {
        return ChecksRollup::Pending;
    }
    match conclusion {
        Some(
            "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED" | "STARTUP_FAILURE",
        ) => ChecksRollup::Failing,
        Some("SUCCESS" | "NEUTRAL" | "SKIPPED") => ChecksRollup::Passing,
        _ => ChecksRollup::Pending,
    }
}

/// Classify one check run. Public so `open_prs` reduces `statusCheckRollup`
/// nodes through the same rule rather than a parallel copy of it.
pub(crate) fn check_run_state(status: &str, conclusion: Option<&str>) -> ChecksRollup {
    run_state(status, conclusion)
}

/// Reduce per-run states with `Failing` > `Pending` > `Passing`: one failed run
/// fails the rollup even while others are still running.
pub(crate) fn reduce_run_states(
    states: impl IntoIterator<Item = ChecksRollup>,
) -> Option<ChecksRollup> {
    let mut seen = None;
    for state in states {
        seen = Some(match (seen, state) {
            (Some(ChecksRollup::Failing), _) | (_, ChecksRollup::Failing) => ChecksRollup::Failing,
            (Some(ChecksRollup::Pending), _) | (_, ChecksRollup::Pending) => ChecksRollup::Pending,
            _ => ChecksRollup::Passing,
        });
    }
    seen
}

/// Roll up check runs into one displayable state. `None` when there are no checks.
pub fn checks_rollup(checks: &[CheckRun]) -> Option<ChecksRollup> {
    reduce_run_states(
        checks
            .iter()
            .map(|run| run_state(&run.status, run.conclusion.as_deref())),
    )
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
        .and_then(serde_json::Value::as_u64)
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
                    // CheckRun items carry `name`; StatusContext items
                    // (external CI like CircleCI) carry `context` + `state`.
                    let name = item
                        .get("name")
                        .or_else(|| item.get("context"))
                        .and_then(|v| v.as_str())?
                        .to_string();
                    if let Some(state) = item.get("state").and_then(|v| v.as_str()) {
                        let (status, conclusion) = match state {
                            "SUCCESS" | "FAILURE" | "ERROR" => {
                                ("COMPLETED".to_string(), Some(state.to_string()))
                            }
                            _ => ("IN_PROGRESS".to_string(), None),
                        };
                        return Some(CheckRun {
                            name,
                            status,
                            conclusion,
                        });
                    }
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

/// Fetch PR + check status for the given branch via the built-in `gh` provider
/// (see `check_provider.rs` for the provider contract).
///
/// Returns a `WorkspaceCheckStatus` that always has a value — errors are
/// captured in the `error` field rather than propagated.
#[allow(dead_code)] // called by App::start_checks_fetch (slice 4 trigger)
pub fn fetch_check_status(cwd: &Path, branch: &str) -> WorkspaceCheckStatus {
    legacy_check_status(CheckProvider::gh().run(cwd, branch))
}

/// Map a provider outcome onto the legacy `WorkspaceCheckStatus` shape today's
/// callers consume. `NotApplicable` keeps the historical "no PR for this
/// branch" error string so existing behavior is unchanged; E2 consumers
/// (bora-i1r.2) use `ProviderOutcome` directly, where not-applicable carries
/// neither rows nor error.
fn legacy_check_status(outcome: ProviderOutcome) -> WorkspaceCheckStatus {
    match outcome {
        ProviderOutcome::Rows { pr, checks } => WorkspaceCheckStatus {
            pr,
            checks,
            error: None,
        },
        ProviderOutcome::NotApplicable => WorkspaceCheckStatus {
            pr: None,
            checks: Vec::new(),
            error: Some(NOT_APPLICABLE_ERROR.to_string()),
        },
        ProviderOutcome::Error(msg) => WorkspaceCheckStatus {
            pr: None,
            checks: Vec::new(),
            error: Some(msg),
        },
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn run(status: &str, conclusion: Option<&str>) -> CheckRun {
        CheckRun {
            name: "c".into(),
            status: status.into(),
            conclusion: conclusion.map(str::to_string),
        }
    }

    #[test]
    fn rollup_none_when_no_checks() {
        assert_eq!(checks_rollup(&[]), None);
    }

    #[test]
    fn rollup_failing_beats_pending() {
        let checks = [run("IN_PROGRESS", None), run("COMPLETED", Some("FAILURE"))];
        assert_eq!(checks_rollup(&checks), Some(ChecksRollup::Failing));
    }

    #[test]
    fn rollup_hard_fail_conclusions_are_failing() {
        for c in [
            "ERROR",
            "TIMED_OUT",
            "CANCELLED",
            "ACTION_REQUIRED",
            "STARTUP_FAILURE",
        ] {
            assert_eq!(
                checks_rollup(&[run("COMPLETED", Some(c))]),
                Some(ChecksRollup::Failing),
                "{c}"
            );
        }
    }

    #[test]
    fn rollup_pending_when_incomplete() {
        let checks = [run("COMPLETED", Some("SUCCESS")), run("QUEUED", None)];
        assert_eq!(checks_rollup(&checks), Some(ChecksRollup::Pending));
    }

    #[test]
    fn rollup_passing_when_all_completed_ok() {
        let checks = [
            run("COMPLETED", Some("SUCCESS")),
            run("COMPLETED", Some("NEUTRAL")),
            run("COMPLETED", Some("SKIPPED")),
        ];
        assert_eq!(checks_rollup(&checks), Some(ChecksRollup::Passing));
    }

    // The two fallthroughs that used to read as green. Both were reachable and
    // neither was covered: `run_state` now buckets them Pending, and these are
    // the tests that go red if anyone widens the Passing arm back out.

    #[test]
    fn rollup_completed_with_null_conclusion_is_pending_not_passing() {
        // GitHub really emits this. Before `run_state` it fell through to
        // Passing: a check with no result at all displayed as a green tick.
        assert_eq!(
            checks_rollup(&[run("COMPLETED", None)]),
            Some(ChecksRollup::Pending)
        );
    }

    #[test]
    fn rollup_unrecognised_conclusion_is_pending_not_passing() {
        // A conclusion string added to the GitHub API after this code was
        // written must not be silently optimistic.
        assert_eq!(
            checks_rollup(&[run("COMPLETED", Some("SOME_FUTURE_CONCLUSION"))]),
            Some(ChecksRollup::Pending)
        );
    }

    #[test]
    fn rollup_unrecognised_status_is_pending_not_passing() {
        assert_eq!(
            checks_rollup(&[run("SOME_FUTURE_STATUS", Some("SUCCESS"))]),
            Some(ChecksRollup::Pending)
        );
    }

    #[test]
    fn parse_status_context_items_from_external_ci() {
        let json = r#"{
            "number": 7,
            "title": "t",
            "state": "OPEN",
            "url": "https://github.com/o/r/pull/7",
            "statusCheckRollup": [
                {"__typename": "StatusContext", "context": "ci/circleci: build", "state": "SUCCESS"},
                {"__typename": "StatusContext", "context": "ci/circleci: deploy", "state": "PENDING"},
                {"__typename": "StatusContext", "context": "ci/circleci: lint", "state": "FAILURE"}
            ]
        }"#;
        let status = parse_gh_pr_json(json).unwrap();
        assert_eq!(status.checks.len(), 3);
        assert_eq!(status.checks[0].name, "ci/circleci: build");
        assert_eq!(status.checks[0].status, "COMPLETED");
        assert_eq!(status.checks[0].conclusion.as_deref(), Some("SUCCESS"));
        assert_eq!(status.checks[1].status, "IN_PROGRESS");
        assert_eq!(status.checks[1].conclusion, None);
        assert_eq!(checks_rollup(&status.checks), Some(ChecksRollup::Failing));
    }

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
    fn legacy_status_maps_rows_without_error() {
        let outcome = ProviderOutcome::Rows {
            pr: None,
            checks: vec![CheckRun {
                name: "ci".into(),
                status: "COMPLETED".into(),
                conclusion: Some("SUCCESS".into()),
            }],
        };
        let status = legacy_check_status(outcome);
        assert_eq!(status.checks.len(), 1);
        assert!(status.error.is_none());
    }

    #[test]
    fn legacy_status_maps_not_applicable_to_historical_no_pr_error() {
        // Pre-provider behavior surfaced "no PR" as an error string; the
        // legacy mapping preserves it so callers see no behavior change.
        let status = legacy_check_status(ProviderOutcome::NotApplicable);
        assert!(status.pr.is_none());
        assert!(status.checks.is_empty());
        assert_eq!(status.error.as_deref(), Some("no PR for this branch"));
    }

    #[test]
    fn legacy_status_maps_error_to_error_field_never_empty() {
        let status = legacy_check_status(ProviderOutcome::Error("boom".to_string()));
        assert!(status.pr.is_none());
        assert!(status.checks.is_empty());
        assert_eq!(status.error.as_deref(), Some("boom"));
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
