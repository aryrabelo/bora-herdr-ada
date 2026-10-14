#[path = "worktree_bindings.rs"]
pub(crate) mod bindings;

use std::time::Duration;

use crate::api::schema::{Method, PaneTarget, Request, WorkspaceSetGroupParams, WorkspaceTarget};

use super::projection::ParseError;

pub(crate) use bindings::command;

/// How long to wait for a pane that launches its own agent to register it
/// before giving up on the rename. Measured on this fleet: registration lands
/// seconds after the pane exists, never instantly.
const AGENT_REGISTRATION_TIMEOUT: Duration = Duration::from_secs(30);

fn cli_error(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "cli:worktree:create",
        "error": { "code": code, "message": message }
    })
}

pub(super) fn run_worktree_command(args: &[String]) -> std::io::Result<i32> {
    if args.first().map(String::as_str) == Some("create") {
        return worktree_create(&args[1..]);
    }
    super::run_json_request(bindings::request(args))
}

/// `worktree create` is a dispatch chain, not one request: checkout, then the
/// sidebar folder, agent, and prompt the follow-up flags ask for.
fn worktree_create(args: &[String]) -> std::io::Result<i32> {
    let (mut params, follow_up) = match bindings::create_request(args) {
        Ok(parsed) => parsed,
        Err(ParseError::Usage(message)) => {
            eprintln!("{message}");
            return Ok(2);
        }
        Err(ParseError::Io(error)) => return Err(error),
        Err(ParseError::Clap(error)) => return super::print_clap_error(&error),
    };

    let prompt = match resolve_prompt_text(follow_up.prompt, follow_up.prompt_file) {
        Ok(prompt) => prompt,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    // The repo is the caller's shell, not whatever workspace the human last
    // clicked in the app: without this the server resolves the focused
    // workspace's repo and answers about a different repository entirely.
    if params.workspace_id.is_none() && params.cwd.is_none() {
        params.cwd = caller_repo_root(params.trust_repository);
    }

    // Label carries the branch so a later `workspace list` is searchable by it
    // and nobody opens a second workspace for the same branch.
    if params.label.is_none() {
        params.label = params
            .branch
            .as_deref()
            .map(crate::worktree::branch_to_path_slug)
            .filter(|slug| !slug.is_empty());
    }

    let agent = match resolve_agent_request(
        follow_up.agent_kind,
        follow_up.agent_name,
        prompt.is_some(),
        params.label.as_deref(),
    ) {
        Ok(agent) => agent,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    // The caller's own folder, resolved before anything is created so a
    // failure here never leaves a stray ungrouped workspace behind.
    let group = if follow_up.no_group {
        None
    } else {
        follow_up.group.or_else(caller_group)
    };

    let created = super::send_request(&Request {
        id: "cli:worktree:create".into(),
        method: Method::WorktreeCreate(params),
    })?;
    if created.get("error").is_some() {
        // Nothing was created: report the wire error unchanged.
        return super::print_response(&created);
    }

    finish_worktree_create(created, group, agent, prompt)
}

/// The agent half of `worktree create`, after defaults are applied.
struct AgentRequest {
    kind: String,
    name: String,
}

/// `--agent`/`--agent-name`, with the defaults that make the one-command form
/// work: the kind falls back to `[agents] default`, and the name to the
/// workspace label (the branch slug), so `bora agent prompt <slug>` keeps
/// addressing the same agent later.
fn resolve_agent_request(
    kind: Option<String>,
    name: Option<String>,
    wants_prompt: bool,
    label: Option<&str>,
) -> Result<Option<AgentRequest>, String> {
    if kind.is_none() && name.is_none() && !wants_prompt {
        return Ok(None);
    }
    let kind = kind.unwrap_or_else(|| {
        crate::config::Config::load()
            .config
            .agents
            .default_kind()
            .to_owned()
    });
    if crate::detect::parse_agent_label(&kind).is_none() {
        return Err(format!("unsupported interactive agent kind: {kind}"));
    }
    let Some(name) = name.or_else(|| label.map(str::to_owned)) else {
        return Err(
            "--agent needs a name: pass --agent-name NAME, or --branch/--label to derive one"
                .to_owned(),
        );
    };
    Ok(Some(AgentRequest { kind, name }))
}

/// Prompt text to deliver. `--prompt` is sent verbatim; `--prompt-file`
/// becomes a pointer to the absolute path instead of the file's contents, so
/// the spec survives the agent compacting its own context and can be re-read
/// at any point in the session.
fn resolve_prompt_text(
    prompt: Option<String>,
    prompt_file: Option<String>,
) -> Result<Option<String>, String> {
    if let Some(prompt) = prompt {
        return Ok(Some(prompt));
    }
    let Some(prompt_file) = prompt_file else {
        return Ok(None);
    };
    let path = crate::worktree::expand_tilde_absolute_path(&prompt_file);
    if !path.is_file() {
        return Err(format!("--prompt-file not found: {}", path.display()));
    }
    Ok(Some(format!(
        "Read and follow the spec at {}.",
        path.display()
    )))
}

/// Repo the caller is standing in. A linked worktree resolves to the parent
/// repo, because worktree creation starts there — dispatching from inside one
/// worktree to create another is the normal case, not an error.
fn caller_repo_root(trust_repository: bool) -> Option<String> {
    if super::target::is_remote() {
        return None;
    }
    let cwd = std::env::current_dir().ok()?;
    let root = crate::worktree::main_worktree_root(&cwd, trust_repository)?;
    Some(root.display().to_string())
}

/// Sidebar folder of the pane that ran this command, so dispatched work lands
/// next to the work that dispatched it. `None` outside a bora pane, or when
/// the caller's own workspace is ungrouped.
fn caller_group() -> Option<String> {
    // A remote target has its own panes; the local `$HERDR_PANE_ID` names
    // nothing there, exactly as remote commands never inherit local pane IDs.
    if super::target::is_remote() {
        return None;
    }
    let pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| super::normalize_pane_id(value.trim()))?;
    let pane = super::send_request_unchecked(&Request {
        id: "cli:worktree:create:pane".into(),
        method: Method::PaneGet(PaneTarget { pane_id }),
    })
    .ok()?;
    let workspace_id = pane["result"]["pane"]["workspace_id"].as_str()?.to_owned();
    let workspace = super::send_request_unchecked(&Request {
        id: "cli:worktree:create:workspace".into(),
        method: Method::WorkspaceGet(WorkspaceTarget { workspace_id }),
    })
    .ok()?;
    workspace["result"]["workspace"]["visual_group"]
        .as_str()
        .filter(|group| !group.trim().is_empty())
        .map(str::to_owned)
}

/// Everything after the checkout exists: folder, agent, prompt. Each link
/// failing stops the chain but still prints what already exists, because a
/// half-built dispatch the caller cannot see is worse than a visible failure.
fn finish_worktree_create(
    created: serde_json::Value,
    group: Option<String>,
    agent: Option<AgentRequest>,
    prompt: Option<String>,
) -> std::io::Result<i32> {
    let result = &created["result"];
    let Some(workspace_id) = result["workspace"]["workspace_id"]
        .as_str()
        .map(str::to_owned)
    else {
        return super::print_response(&created);
    };
    let pane_id = result["root_pane"]["pane_id"].as_str().map(str::to_owned);

    let mut summary = serde_json::json!({
        "type": "worktree_created",
        "ok": true,
        "workspace_id": workspace_id,
        "pane_id": pane_id,
        "path": result["worktree"]["path"],
        "branch": result["worktree"]["branch"],
        "branch_source": result["branch_source"],
        "head": result["head"],
        "label": result["workspace"]["label"],
        "group": result["workspace"]["visual_group"],
        "already_open": result["already_open"],
        "agent": serde_json::Value::Null,
        "prompt_sent": false,
        "workspace": result["workspace"],
        "tab": result["tab"],
        "root_pane": result["root_pane"],
        "worktree": result["worktree"],
    });

    if let Some(group) = group {
        let grouped = super::send_request(&Request {
            id: "cli:worktree:create:group".into(),
            method: Method::WorkspaceSetGroup(WorkspaceSetGroupParams {
                workspace_id,
                group: Some(group),
            }),
        })?;
        if grouped.get("error").is_some() {
            return print_partial_worktree_create(summary, "workspace_set_group", &grouped);
        }
        summary["group"] = grouped["result"]["workspace"]["visual_group"].clone();
        summary["workspace"] = grouped["result"]["workspace"].clone();
    }

    let Some(agent) = agent else {
        return print_worktree_create(&summary);
    };
    let Some(pane_id) = pane_id else {
        return print_partial_worktree_create(
            summary,
            "agent",
            &cli_error(
                "worktree_create_failed",
                "worktree workspace has no root pane to host an agent",
            ),
        );
    };
    let agent = match super::agent::ensure_named_agent_on_pane(
        &pane_id,
        &agent.kind,
        &agent.name,
        AGENT_REGISTRATION_TIMEOUT,
    )? {
        Ok(agent) => agent,
        Err(error) => return print_partial_worktree_create(summary, "agent", &error),
    };
    summary["agent"] = serde_json::json!({
        "name": agent.name,
        "status": agent.agent["agent_status"],
        "kind": agent.agent["agent"],
        "started": agent.started,
        "interactive_ready": agent.agent["interactive_ready"],
    });

    let Some(prompt) = prompt else {
        return print_worktree_create(&summary);
    };
    let prompted = super::agent::send_agent_prompt(&agent.name, &prompt)?;
    if prompted.get("error").is_some() {
        return print_partial_worktree_create(summary, "prompt", &prompted);
    }
    summary["prompt_sent"] = serde_json::Value::Bool(true);
    summary["prompt"] = prompted["result"].clone();
    print_worktree_create(&summary)
}

fn print_worktree_create(summary: &serde_json::Value) -> std::io::Result<i32> {
    super::print_response(&serde_json::json!({
        "id": "cli:worktree:create",
        "result": summary,
    }))
}

/// A link failed after the checkout was already created. Exit code 1, but on
/// stdout with `ok: false` and the failing step named: the caller has a real
/// worktree and workspace to clean up or reuse, and must be told about them.
fn print_partial_worktree_create(
    mut summary: serde_json::Value,
    failed_step: &str,
    error: &serde_json::Value,
) -> std::io::Result<i32> {
    summary["ok"] = serde_json::Value::Bool(false);
    summary["failed_step"] = serde_json::Value::String(failed_step.to_owned());
    summary["error"] = error["error"].clone();
    println!(
        "{}",
        serde_json::json!({ "id": "cli:worktree:create", "result": summary })
    );
    Ok(1)
}

fn normalize_path_arg(value: &str) -> std::io::Result<String> {
    if super::target::is_remote() {
        if super::target::remote_path_is_absolute(value) || value == "~" || value.starts_with("~/")
        {
            return Ok(value.to_owned());
        }
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "remote worktree paths must be absolute or start with ~/",
        ));
    }
    let path = crate::worktree::expand_tilde_path(value);
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    Ok(absolute.display().to_string())
}

#[cfg(test)]
mod machine_tests {
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn simple_families_worktree_rejects_invalid_grammar_before_transport() {
        for values in [
            vec!["list", "--workspace"],
            vec!["list", "--workspace", "2", "--cwd", "."],
            vec!["create", "--branch"],
            vec!["create", "--base"],
            vec!["create", "--path"],
            vec!["create", "--label"],
            vec!["create", "--focus=true"],
            vec!["create", "--workspace", "2", "--cwd", "."],
            vec!["open"],
            vec!["open", "--json"],
            vec!["open", "--path", ".", "--branch", "topic"],
            vec![
                "open",
                "--workspace",
                "2",
                "--cwd",
                ".",
                "--branch",
                "topic",
            ],
            vec!["remove"],
            vec!["remove", "--json", "--force", "--trust-repository"],
            vec!["remove", "--workspace"],
        ] {
            assert!(
                super::bindings::request(&args(&values)).is_err(),
                "argv: {values:?}"
            );
        }
    }

    #[test]
    fn simple_families_worktree_validates_every_path_occurrence() {
        crate::cli::target::with_test_client(crate::api::client::ApiClient::local(), || {
            for values in [
                vec!["list", "--cwd", "relative", "--cwd", "/valid"],
                vec!["create", "--path", "relative", "--path", "/valid"],
                vec!["open", "--path", "relative", "--path", "/valid"],
            ] {
                assert!(super::bindings::request(&args(&values)).is_err());
            }
            assert_eq!(super::normalize_path_arg("~").unwrap(), "~");
        });
    }

    #[test]
    fn simple_families_worktree_dto_literal_defaults() {
        use crate::api::schema::{
            WorktreeCreateParams, WorktreeListParams, WorktreeOpenParams, WorktreeRemoveParams,
        };
        assert_eq!(
            serde_json::to_value(WorktreeListParams::default()).unwrap(),
            serde_json::json!({})
        );
        assert_eq!(
            serde_json::to_value(WorktreeCreateParams::default()).unwrap(),
            serde_json::json!({"focus": false})
        );
        assert_eq!(
            serde_json::to_value(WorktreeOpenParams::default()).unwrap(),
            serde_json::json!({"focus": false})
        );
        let remove: WorktreeRemoveParams =
            serde_json::from_value(serde_json::json!({"workspace_id": "ws-2"})).unwrap();
        assert!(!remove.force);
        assert!(!remove.trust_repository);
        assert_eq!(
            serde_json::to_value(remove).unwrap(),
            serde_json::json!({"workspace_id": "ws-2", "force": false})
        );
    }

    #[test]
    fn remote_worktree_paths_are_not_expanded_on_the_caller_machine() {
        crate::cli::target::with_test_client(crate::api::client::ApiClient::local(), || {
            for path in [
                "~/Projects/herdr",
                "/Users/can/Projects/herdr",
                r"C:\work\repo",
                "C:/work/repo",
                r"\\host\share\repo",
            ] {
                assert_eq!(super::normalize_path_arg(path).unwrap(), path);
            }
            assert!(super::normalize_path_arg("../other").is_err());
            assert!(super::normalize_path_arg("C:relative").is_err());
        });
    }
}
