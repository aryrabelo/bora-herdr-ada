//! The sidebar CHECKS provider contract (`.local/prd/sidebar-design.md`
//! §"The check contract"): a provider is a command template that, asked about
//! a workspace's `{repo, dir?, branch}`, prints JSON rows
//! `[{ "name", "status", "conclusion" }]` mapping exactly onto `CheckRun`.
//!
//! `gh` is the built-in provider, not the definition of checks: today's
//! `gh pr view` acquisition lives here behind the same machinery a configured
//! provider from `defaults.checks` / `sections.checks` (E2, bora-i1r.2) uses.

use std::path::Path;

use super::check_status::{parse_gh_pr_json, CheckRun, PrSummary};

// ── Outcome ──────────────────────────────────────────────────────────────────

/// The three explicit outcomes of asking a provider about a branch.
///
/// These MUST stay distinct: an error renders as an error (never collapsed
/// into "no checks"), and not-applicable renders nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProviderOutcome {
    /// The provider answered: check rows (possibly empty) + optional PR summary.
    Rows {
        pr: Option<PrSummary>,
        checks: Vec<CheckRun>,
    },
    /// The provider does not apply here (not configured for the section, or no
    /// PR for this branch): no rows, no error.
    NotApplicable,
    /// The provider ran and failed; the message is user-facing.
    Error(String),
}

/// PR summary + check rows parsed from one provider's stdout.
#[derive(Debug)]
pub(crate) struct ParsedChecks {
    pub pr: Option<PrSummary>,
    pub checks: Vec<CheckRun>,
}

// ── Command seam ─────────────────────────────────────────────────────────────

/// Result of executing a provider command. A dedicated struct (rather than
/// `std::process::Output`) so tests fabricate results without platform APIs —
/// this codebase's tests never spawn subprocesses.
pub(crate) struct CommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Executes a provider command. `Err` carries the user-facing spawn-failure
/// message (e.g. the provider binary is not installed).
type Exec = dyn Fn(&Path, &str, &[String]) -> Result<CommandResult, String>;

fn real_exec(cwd: &Path, program: &str, args: &[String]) -> Result<CommandResult, String> {
    match std::process::Command::new(program)
        .current_dir(cwd)
        .args(args)
        .output()
    {
        Ok(output) => Ok(CommandResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        }),
        Err(e) => Err(if e.kind() == std::io::ErrorKind::NotFound {
            format!("{program} CLI not found")
        } else {
            format!("failed to run {program}: {e}")
        }),
    }
}

// ── Provider ─────────────────────────────────────────────────────────────────

/// A checks provider: a named command template plus the parser for its stdout.
///
/// The argv template substitutes `{branch}` (workspace branch) and `{dir}`
/// (workspace directory); `{repo}` is reserved for configured providers
/// (bora-i1r.2) — the built-in providers derive the repo from the git remote.
pub(crate) struct CheckProvider {
    /// Provider id, as named in `defaults.checks` / `sections.checks` (E2).
    pub(crate) name: &'static str,
    /// Program to execute, resolved via PATH.
    program: &'static str,
    /// argv template; `{branch}` / `{dir}` / `{repo}` substituted per run.
    args: &'static [&'static str],
    /// stderr substring that, on a non-zero exit, means "not applicable"
    /// rather than an error (gh: "no pull requests found").
    not_applicable_stderr: Option<&'static str>,
    /// Shape a failed run's (trimmed) stderr into the user-facing message.
    error_message: fn(&str) -> String,
    /// Parse a successful run's stdout into PR summary + check rows.
    parse: fn(&str) -> Result<ParsedChecks, String>,
}

impl CheckProvider {
    /// The built-in `gh` provider: `gh pr view <branch> --json ...`.
    ///
    /// This is the pre-provider `fetch_check_status` acquisition moved behind
    /// the contract unchanged — same argv, same JSON fields, same parser, same
    /// error strings.
    pub(crate) fn gh() -> Self {
        CheckProvider {
            name: "gh",
            program: "gh",
            args: &[
                "pr",
                "view",
                "{branch}",
                "--json",
                "number,title,state,url,statusCheckRollup,mergeable",
            ],
            not_applicable_stderr: Some("no pull requests found"),
            error_message: gh_error_message,
            parse: parse_gh_output,
        }
    }

    /// Run the provider against the workspace at `cwd` on `branch`.
    pub(crate) fn run(&self, cwd: &Path, branch: &str) -> ProviderOutcome {
        self.run_with(cwd, branch, &real_exec)
    }

    /// `run` with the command-execution seam injected (tests).
    fn run_with(&self, cwd: &Path, branch: &str, exec: &Exec) -> ProviderOutcome {
        let args: Vec<String> = self
            .args
            .iter()
            .map(|arg| substitute(arg, cwd, branch))
            .collect();
        let output = match exec(cwd, self.program, &args) {
            Ok(output) => output,
            Err(msg) => {
                // Provider binary not installed / not executable.
                tracing::debug!("check_status: {msg}");
                return ProviderOutcome::Error(msg);
            }
        };
        if !output.success {
            // Common cases for gh: no PR exists, not authenticated, not a
            // GitHub remote.
            if self
                .not_applicable_stderr
                .is_some_and(|marker| output.stderr.contains(marker))
            {
                tracing::debug!(
                    "check_status: {} not applicable for branch {branch:?}",
                    self.name
                );
                return ProviderOutcome::NotApplicable;
            }
            let msg = (self.error_message)(&output.stderr);
            tracing::debug!(
                "check_status: {} failed for branch {branch:?}: {msg}",
                self.name
            );
            return ProviderOutcome::Error(msg);
        }
        match (self.parse)(&output.stdout) {
            Ok(parsed) => ProviderOutcome::Rows {
                pr: parsed.pr,
                checks: parsed.checks,
            },
            Err(e) => {
                tracing::warn!("check_status: failed to parse {} output: {e}", self.name);
                ProviderOutcome::Error(e)
            }
        }
    }
}

/// Substitute the template placeholders in one argv element.
fn substitute(arg: &str, dir: &Path, branch: &str) -> String {
    arg.replace("{branch}", branch)
        .replace("{dir}", &dir.display().to_string())
        // ponytail: built-ins don't use {repo}; bora-i1r.2 substitutes the
        // configured repo identity here when it wires sections.checks.
        .replace("{repo}", "")
}

/// gh's error shaping, preserved verbatim from the pre-provider
/// `fetch_check_status`.
fn gh_error_message(stderr: &str) -> String {
    if stderr.contains("authentication") || stderr.contains("auth") {
        "gh not authenticated".to_string()
    } else if stderr.is_empty() {
        "gh pr view failed".to_string()
    } else {
        stderr.to_string()
    }
}

/// gh's stdout is the full `gh pr view` object, not bare contract rows; reuse
/// the existing parser so the gh path's output stays byte-identical.
fn parse_gh_output(stdout: &str) -> Result<ParsedChecks, String> {
    let status = parse_gh_pr_json(stdout)?;
    Ok(ParsedChecks {
        pr: status.pr,
        checks: status.checks,
    })
}

/// Parse the provider contract JSON — `[{name, status, conclusion}]` — into
/// check rows. This is the parser every configured (non-gh) provider uses;
/// `conclusion` may be null or absent while a check is still running.
#[allow(dead_code)] // exercised by tests; config wiring lands in bora-i1r.2
pub(crate) fn parse_contract_json(json_str: &str) -> Result<ParsedChecks, String> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|e| format!("invalid JSON from provider: {e}"))?;
    let arr = value
        .as_array()
        .ok_or_else(|| "provider output is not a JSON array".to_string())?;
    let checks = arr
        .iter()
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
        .collect();
    Ok(ParsedChecks { pr: None, checks })
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(stdout: &str) -> CommandResult {
        CommandResult {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        }
    }

    fn fail(stderr: &str) -> CommandResult {
        CommandResult {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        }
    }

    /// Recorded `gh pr view feat/x --json number,title,state,url,statusCheckRollup,mergeable`
    /// output, mixing CheckRun items, a pending CheckRun, and legacy
    /// StatusContext items (external CI).
    const RECORDED_GH_JSON: &str = r#"{
        "number": 128,
        "title": "feat: sidebar checks",
        "state": "OPEN",
        "url": "https://github.com/ary/bora/pull/128",
        "mergeable": "MERGEABLE",
        "statusCheckRollup": [
            {"__typename": "CheckRun", "name": "clippy", "status": "COMPLETED", "conclusion": "SUCCESS"},
            {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE"},
            {"__typename": "CheckRun", "name": "build", "status": "IN_PROGRESS", "conclusion": null},
            {"__typename": "StatusContext", "context": "ci/circleci: lint", "state": "SUCCESS"},
            {"__typename": "StatusContext", "context": "ci/circleci: docs", "state": "PENDING"}
        ]
    }"#;

    #[test]
    fn gh_provider_characterization_mixed_check_shapes() {
        let provider = CheckProvider::gh();
        let exec = |_: &Path, _: &str, _: &[String]| Ok(ok(RECORDED_GH_JSON));
        let outcome = provider.run_with(Path::new("/repo"), "feat/x", &exec);

        // The provider path must produce exactly what the current parser
        // produces for the same recorded gh output.
        let expected = parse_gh_pr_json(RECORDED_GH_JSON).unwrap();
        assert_eq!(
            outcome,
            ProviderOutcome::Rows {
                pr: expected.pr.clone(),
                checks: expected.checks,
            }
        );

        // Pin the exact values, not just parser-equality.
        let ProviderOutcome::Rows { pr, checks } = outcome else {
            panic!("expected rows");
        };
        let pr = pr.unwrap();
        assert_eq!(pr.number, 128);
        assert_eq!(pr.title, "feat: sidebar checks");
        assert_eq!(pr.state, "OPEN");
        assert_eq!(pr.url, "https://github.com/ary/bora/pull/128");
        assert_eq!(pr.mergeable.as_deref(), Some("MERGEABLE"));
        assert_eq!(
            checks,
            vec![
                CheckRun {
                    name: "clippy".into(),
                    status: "COMPLETED".into(),
                    conclusion: Some("SUCCESS".into()),
                },
                CheckRun {
                    name: "test".into(),
                    status: "COMPLETED".into(),
                    conclusion: Some("FAILURE".into()),
                },
                CheckRun {
                    name: "build".into(),
                    status: "IN_PROGRESS".into(),
                    conclusion: None,
                },
                CheckRun {
                    name: "ci/circleci: lint".into(),
                    status: "COMPLETED".into(),
                    conclusion: Some("SUCCESS".into()),
                },
                CheckRun {
                    name: "ci/circleci: docs".into(),
                    status: "IN_PROGRESS".into(),
                    conclusion: None,
                },
            ]
        );
    }

    #[test]
    fn gh_provider_characterization_uses_legacy_argv() {
        let provider = CheckProvider::gh();
        let exec = |_: &Path, program: &str, args: &[String]| {
            assert_eq!(program, "gh");
            assert_eq!(
                args,
                &[
                    "pr".to_string(),
                    "view".to_string(),
                    "feat/x".to_string(), // {branch} substituted
                    "--json".to_string(),
                    "number,title,state,url,statusCheckRollup,mergeable".to_string(),
                ]
            );
            Ok(ok(RECORDED_GH_JSON))
        };
        let outcome = provider.run_with(Path::new("/repo"), "feat/x", &exec);
        assert!(matches!(outcome, ProviderOutcome::Rows { .. }));
    }

    /// A fake script provider: emits the contract JSON
    /// `[{name, status, conclusion}]`, like `gia` will.
    fn fake_script_provider() -> CheckProvider {
        fn script_error(stderr: &str) -> String {
            if stderr.is_empty() {
                "provider failed".to_string()
            } else {
                stderr.to_string()
            }
        }
        CheckProvider {
            name: "fake-ci",
            program: "fake-ci",
            args: &["checks", "{branch}", "--dir", "{dir}"],
            not_applicable_stderr: Some("no checks configured"),
            error_message: script_error,
            parse: parse_contract_json,
        }
    }

    #[test]
    fn fake_script_provider_renders_contract_rows() {
        let provider = fake_script_provider();
        let exec = |_: &Path, _: &str, _: &[String]| {
            Ok(ok(r#"[
                    {"name": "clippy", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    {"name": "unit", "status": "IN_PROGRESS", "conclusion": null},
                    {"name": "e2e", "status": "QUEUED"}
                ]"#))
        };
        let outcome = provider.run_with(Path::new("/repo"), "main", &exec);
        assert_eq!(
            outcome,
            ProviderOutcome::Rows {
                pr: None,
                checks: vec![
                    CheckRun {
                        name: "clippy".into(),
                        status: "COMPLETED".into(),
                        conclusion: Some("SUCCESS".into()),
                    },
                    CheckRun {
                        name: "unit".into(),
                        status: "IN_PROGRESS".into(),
                        conclusion: None,
                    },
                    CheckRun {
                        name: "e2e".into(),
                        status: "QUEUED".into(),
                        conclusion: None,
                    },
                ],
            }
        );
    }

    #[test]
    fn fake_script_provider_substitutes_branch_and_dir() {
        let provider = fake_script_provider();
        let exec = |_: &Path, _: &str, args: &[String]| {
            assert_eq!(
                args,
                &[
                    "checks".to_string(),
                    "main".to_string(),
                    "--dir".to_string(),
                    "/repo".to_string(),
                ]
            );
            Ok(ok("[]"))
        };
        let outcome = provider.run_with(Path::new("/repo"), "main", &exec);
        assert_eq!(
            outcome,
            ProviderOutcome::Rows {
                pr: None,
                checks: Vec::new(),
            }
        );
    }

    #[test]
    fn script_provider_contract_json_requires_an_array() {
        let result = parse_contract_json(r#"{"name": "x"}"#);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a JSON array"));
    }

    #[test]
    fn gh_provider_no_pr_is_not_applicable() {
        let provider = CheckProvider::gh();
        let exec = |_: &Path, _: &str, _: &[String]| {
            Ok(fail("no pull requests found for branch \"feat/x\""))
        };
        let outcome = provider.run_with(Path::new("/repo"), "feat/x", &exec);
        // Not applicable: neither rows nor error.
        assert_eq!(outcome, ProviderOutcome::NotApplicable);
    }

    #[test]
    fn script_provider_not_applicable_marker_is_not_an_error() {
        let provider = fake_script_provider();
        let exec = |_: &Path, _: &str, _: &[String]| Ok(fail("no checks configured here"));
        let outcome = provider.run_with(Path::new("/repo"), "main", &exec);
        assert_eq!(outcome, ProviderOutcome::NotApplicable);
    }

    #[test]
    fn gh_provider_failure_is_error_never_silently_empty() {
        let provider = CheckProvider::gh();
        let exec = |_: &Path, _: &str, _: &[String]| Ok(fail("connection refused"));
        let outcome = provider.run_with(Path::new("/repo"), "feat/x", &exec);
        assert_eq!(
            outcome,
            ProviderOutcome::Error("connection refused".to_string())
        );
    }

    #[test]
    fn gh_provider_error_message_shaping_is_preserved() {
        let provider = CheckProvider::gh();

        let exec = |_: &Path, _: &str, _: &[String]| Ok(fail("error: authentication required"));
        assert_eq!(
            provider.run_with(Path::new("/repo"), "b", &exec),
            ProviderOutcome::Error("gh not authenticated".to_string())
        );

        let exec = |_: &Path, _: &str, _: &[String]| Ok(fail(""));
        assert_eq!(
            provider.run_with(Path::new("/repo"), "b", &exec),
            ProviderOutcome::Error("gh pr view failed".to_string())
        );
    }

    #[test]
    fn gh_provider_spawn_failure_is_error() {
        let provider = CheckProvider::gh();
        let exec = |_: &Path, _: &str, _: &[String]| Err("gh CLI not found".to_string());
        let outcome = provider.run_with(Path::new("/repo"), "feat/x", &exec);
        assert_eq!(
            outcome,
            ProviderOutcome::Error("gh CLI not found".to_string())
        );
    }

    #[test]
    fn gh_provider_unparseable_output_is_error() {
        let provider = CheckProvider::gh();
        let exec = |_: &Path, _: &str, _: &[String]| Ok(ok("not json at all"));
        let outcome = provider.run_with(Path::new("/repo"), "feat/x", &exec);
        let ProviderOutcome::Error(msg) = outcome else {
            panic!("expected error, got {outcome:?}");
        };
        assert!(msg.contains("invalid JSON"));
    }
}
