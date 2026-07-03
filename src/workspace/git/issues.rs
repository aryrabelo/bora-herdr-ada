use std::path::Path;

// ── Types ────────────────────────────────────────────────────────────────────

/// An open GitHub issue relevant to the current user (assigned or authored).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIssue {
    pub number: u64,
    pub title: String,
    pub url: String,
}

/// Open issues relevant to the current user for one repo.
///
/// Always returned as a value — errors are captured in the `error` field
/// rather than propagated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoIssues {
    pub issues: Vec<RepoIssue>,
    pub error: Option<String>,
}

/// A parsed issue that keeps `updatedAt` internally so merged lists can be
/// ordered by recency before the timestamp is dropped from `RepoIssue`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FetchedIssue {
    number: u64,
    title: String,
    url: String,
    /// RFC 3339 UTC timestamp as emitted by gh (e.g. "2026-07-01T12:00:00Z").
    /// Lexicographic order matches chronological order for this fixed format.
    updated_at: String,
}

// ── JSON parsing ─────────────────────────────────────────────────────────────

/// Parse `gh issue list --json` output into a list of `FetchedIssue`.
///
/// Expected JSON shape (from `--json number,title,url,updatedAt`):
/// ```json
/// [
///   {
///     "number": 12,
///     "title": "bug: thing",
///     "url": "https://github.com/owner/repo/issues/12",
///     "updatedAt": "2026-07-01T12:00:00Z"
///   },
///   ...
/// ]
/// ```
fn parse_gh_issue_list_json(json_str: &str) -> Result<Vec<FetchedIssue>, String> {
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
            let updated_at = item
                .get("updatedAt")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(FetchedIssue {
                number,
                title,
                url,
                updated_at,
            })
        })
        .collect())
}

/// Merge assigned + authored issue lists: dedupe by issue number (first
/// occurrence wins) and sort by `updatedAt` descending.
fn merge_issue_lists(assigned: Vec<FetchedIssue>, authored: Vec<FetchedIssue>) -> Vec<RepoIssue> {
    let mut seen = std::collections::HashSet::new();
    let mut merged: Vec<FetchedIssue> = Vec::new();
    for issue in assigned.into_iter().chain(authored) {
        if seen.insert(issue.number) {
            merged.push(issue);
        }
    }
    merged.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    merged
        .into_iter()
        .map(|issue| RepoIssue {
            number: issue.number,
            title: issue.title,
            url: issue.url,
        })
        .collect()
}

// ── Acquisition ──────────────────────────────────────────────────────────────

/// Run one `gh issue list` invocation filtered by `filter_flag @me`
/// (`--assignee` or `--author`).
fn run_gh_issue_list(cwd: &Path, filter_flag: &str) -> Result<Vec<FetchedIssue>, String> {
    let output = match std::process::Command::new("gh")
        .current_dir(cwd)
        .args([
            "issue",
            "list",
            "--state",
            "open",
            "--json",
            "number,title,url,updatedAt",
            filter_flag,
            "@me",
        ])
        .output()
    {
        Ok(output) => output,
        Err(e) => {
            // gh not installed or not executable
            return Err(if e.kind() == std::io::ErrorKind::NotFound {
                "gh CLI not found".to_string()
            } else {
                format!("failed to run gh: {e}")
            });
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // Common cases: not authenticated, not a GitHub remote
        return Err(
            if stderr.contains("authentication") || stderr.contains("auth") {
                "gh not authenticated".to_string()
            } else if stderr.is_empty() {
                "gh issue list failed".to_string()
            } else {
                stderr
            },
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_gh_issue_list_json(&stdout)
}

/// Fetch open issues assigned to or authored by the current user for the repo
/// at `cwd`, via two `gh issue list` calls merged into one deduplicated list.
///
/// Returns a `RepoIssues` that always has a value — the first failure is
/// captured in the `error` field rather than propagated.
pub fn fetch_my_issues(cwd: &Path) -> RepoIssues {
    let mut first_error: Option<String> = None;
    let mut assigned = Vec::new();
    let mut authored = Vec::new();

    match run_gh_issue_list(cwd, "--assignee") {
        Ok(issues) => assigned = issues,
        Err(e) => {
            tracing::debug!("issues: gh failed for assignee @me: {e}");
            first_error = Some(e);
        }
    }
    match run_gh_issue_list(cwd, "--author") {
        Ok(issues) => authored = issues,
        Err(e) => {
            tracing::debug!("issues: gh failed for author @me: {e}");
            if first_error.is_none() {
                first_error = Some(e);
            }
        }
    }

    RepoIssues {
        issues: merge_issue_lists(assigned, authored),
        error: first_error,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64, title: &str, updated_at: &str) -> FetchedIssue {
        FetchedIssue {
            number,
            title: title.to_string(),
            url: format!("https://github.com/o/r/issues/{number}"),
            updated_at: updated_at.to_string(),
        }
    }

    #[test]
    fn parse_issue_list_happy_path() {
        let json = r#"[
            {
                "number": 12,
                "title": "bug: crash on start",
                "url": "https://github.com/owner/repo/issues/12",
                "updatedAt": "2026-07-01T12:00:00Z"
            },
            {
                "number": 8,
                "title": "feat: dark mode",
                "url": "https://github.com/owner/repo/issues/8",
                "updatedAt": "2026-06-28T09:30:00Z"
            }
        ]"#;
        let issues = parse_gh_issue_list_json(json).unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].number, 12);
        assert_eq!(issues[0].title, "bug: crash on start");
        assert_eq!(issues[0].url, "https://github.com/owner/repo/issues/12");
        assert_eq!(issues[0].updated_at, "2026-07-01T12:00:00Z");
    }

    #[test]
    fn parse_empty_array_returns_no_issues() {
        let issues = parse_gh_issue_list_json("[]").unwrap();
        assert!(issues.is_empty());
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let result = parse_gh_issue_list_json("nope");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid JSON"));
    }

    #[test]
    fn parse_non_array_returns_error() {
        let result = parse_gh_issue_list_json(r#"{"number": 1}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a JSON array"));
    }

    #[test]
    fn parse_issue_with_missing_optional_fields_uses_defaults() {
        let json = r#"[{"number": 3}]"#;
        let issues = parse_gh_issue_list_json(json).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 3);
        assert_eq!(issues[0].title, "");
        assert_eq!(issues[0].url, "");
        assert_eq!(issues[0].updated_at, "");
    }

    #[test]
    fn parse_issue_without_number_is_skipped() {
        let json = r#"[
            {"title": "no number", "url": "", "updatedAt": "2026-07-01T00:00:00Z"},
            {"number": 4, "title": "real", "url": "", "updatedAt": "2026-07-01T00:00:00Z"}
        ]"#;
        let issues = parse_gh_issue_list_json(json).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 4);
    }

    #[test]
    fn merge_dedupes_overlapping_issues_by_number() {
        let assigned = vec![
            issue(1, "assigned+authored", "2026-07-01T00:00:00Z"),
            issue(2, "assigned only", "2026-06-30T00:00:00Z"),
        ];
        let authored = vec![
            issue(1, "assigned+authored (author copy)", "2026-07-01T00:00:00Z"),
            issue(3, "authored only", "2026-06-29T00:00:00Z"),
        ];
        let merged = merge_issue_lists(assigned, authored);
        assert_eq!(merged.len(), 3);
        let numbers: Vec<u64> = merged.iter().map(|i| i.number).collect();
        assert_eq!(numbers, vec![1, 2, 3]);
        // First occurrence (assigned list) wins for the duplicated number.
        assert_eq!(merged[0].title, "assigned+authored");
    }

    #[test]
    fn merge_sorts_by_updated_at_descending() {
        let assigned = vec![
            issue(10, "old", "2026-01-05T00:00:00Z"),
            issue(11, "newest", "2026-07-02T18:00:00Z"),
        ];
        let authored = vec![issue(12, "middle", "2026-03-15T00:00:00Z")];
        let merged = merge_issue_lists(assigned, authored);
        let numbers: Vec<u64> = merged.iter().map(|i| i.number).collect();
        assert_eq!(numbers, vec![11, 12, 10]);
    }

    #[test]
    fn merge_of_empty_lists_is_empty() {
        assert!(merge_issue_lists(Vec::new(), Vec::new()).is_empty());
    }
}
