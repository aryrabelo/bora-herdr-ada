use std::time::Duration;

use crate::api::schema::{
    Method, PaneTarget, Request, WorkspaceSetGroupParams, WorkspaceTarget, WorktreeCreateParams,
    WorktreeListParams, WorktreeOpenParams, WorktreeRemoveParams,
};

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

// Worktree output is always JSON. The parsers retain `--json` as a hidden compatibility no-op.
pub(super) fn run_worktree_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(std::string::String::as_str) else {
        print_worktree_help();
        return Ok(2);
    };

    match subcommand {
        "list" => worktree_list(&args[1..]),
        "create" => worktree_create(&args[1..]),
        "open" => worktree_open(&args[1..]),
        "remove" => worktree_remove(&args[1..]),
        "help" | "--help" | "-h" => {
            print_worktree_help();
            Ok(0)
        }
        _ => {
            print_worktree_help();
            Ok(2)
        }
    }
}

fn worktree_list(args: &[String]) -> std::io::Result<i32> {
    let mut workspace_id = None;
    let mut cwd = None;
    let mut trust_repository = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--workspace" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --workspace");
                    return Ok(2);
                };
                workspace_id = Some(super::normalize_workspace_id(value));
                index += 2;
            }
            "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --cwd");
                    return Ok(2);
                };
                cwd = Some(normalize_path_arg(value)?);
                index += 2;
            }
            "--trust-repository" => {
                trust_repository = true;
                index += 1;
            }
            "--json" => index += 1,
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    if workspace_id.is_some() && cwd.is_some() {
        eprintln!(
            "usage: bora worktree list [--workspace ID | --cwd PATH] [--json] [--trust-repository]"
        );
        return Ok(2);
    }

    super::runtime::worktree_list(WorktreeListParams {
        workspace_id,
        cwd,
        trust_repository,
    })
}

fn worktree_create(args: &[String]) -> std::io::Result<i32> {
    let mut workspace_id = None;
    let mut cwd = None;
    let mut branch = None;
    let mut base = None;
    let mut pr = None;
    let mut path = None;
    let mut label = None;
    let mut group = None;
    let mut no_group = false;
    let mut focus = false;
    let mut trust_repository = false;
    let mut agent_kind = None;
    let mut agent_name = None;
    let mut prompt = None;
    let mut prompt_file = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--workspace" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --workspace");
                    return Ok(2);
                };
                workspace_id = Some(super::normalize_workspace_id(value));
                index += 2;
            }
            "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --cwd");
                    return Ok(2);
                };
                cwd = Some(normalize_path_arg(value)?);
                index += 2;
            }
            "--branch" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --branch");
                    return Ok(2);
                };
                branch = Some(value.clone());
                index += 2;
            }
            "--base" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --base");
                    return Ok(2);
                };
                base = Some(value.clone());
                index += 2;
            }
            "--pr" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --pr");
                    return Ok(2);
                };
                let Ok(number) = value.parse::<u64>() else {
                    eprintln!("invalid value for --pr: {value}");
                    return Ok(2);
                };
                pr = Some(number);
                index += 2;
            }
            "--path" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --path");
                    return Ok(2);
                };
                path = Some(normalize_path_arg(value)?);
                index += 2;
            }
            "--label" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --label");
                    return Ok(2);
                };
                label = Some(value.clone());
                index += 2;
            }
            "--group" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --group");
                    return Ok(2);
                };
                group = Some(value.clone());
                index += 2;
            }
            "--no-group" => {
                no_group = true;
                index += 1;
            }
            "--agent" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent");
                    return Ok(2);
                };
                agent_kind = Some(value.clone());
                index += 2;
            }
            "--agent-name" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --agent-name");
                    return Ok(2);
                };
                agent_name = Some(value.clone());
                index += 2;
            }
            "--prompt" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --prompt");
                    return Ok(2);
                };
                prompt = Some(value.clone());
                index += 2;
            }
            "--prompt-file" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --prompt-file");
                    return Ok(2);
                };
                prompt_file = Some(value.clone());
                index += 2;
            }
            "--focus" => {
                focus = true;
                index += 1;
            }
            "--no-focus" => {
                focus = false;
                index += 1;
            }
            "--trust-repository" => {
                trust_repository = true;
                index += 1;
            }
            "--json" => index += 1,
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    if workspace_id.is_some() && cwd.is_some()
        || pr.is_some() && (branch.is_some() || base.is_some())
        || prompt.is_some() && prompt_file.is_some()
        || group.is_some() && no_group
    {
        print_worktree_create_usage();
        return Ok(2);
    }

    let prompt = match resolve_prompt_text(prompt, prompt_file) {
        Ok(prompt) => prompt,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };

    // The repo is the caller's shell, not whatever workspace the human last
    // clicked in the app: without this the server resolves the focused
    // workspace's repo and answers about a different repository entirely.
    if workspace_id.is_none() && cwd.is_none() {
        cwd = caller_repo_root(trust_repository);
    }

    // Label carries the branch so a later `workspace list` is searchable by it
    // and nobody opens a second workspace for the same branch.
    let label = label.or_else(|| {
        branch
            .as_deref()
            .map(crate::worktree::branch_to_path_slug)
            .filter(|slug| !slug.is_empty())
    });

    let agent =
        match resolve_agent_request(agent_kind, agent_name, prompt.is_some(), label.as_deref()) {
            Ok(agent) => agent,
            Err(message) => {
                eprintln!("{message}");
                return Ok(2);
            }
        };
    if prompt.is_some() && agent.is_none() {
        eprintln!(
            "--prompt/--prompt-file requires an agent; pass --agent KIND or --agent-name NAME"
        );
        return Ok(2);
    }

    // The caller's own folder, resolved before anything is created so a
    // failure here never leaves a stray ungrouped workspace behind.
    let group = if no_group {
        None
    } else {
        group.or_else(caller_group)
    };

    let created = super::send_request(&Request {
        id: "cli:worktree:create".into(),
        method: Method::WorktreeCreate(WorktreeCreateParams {
            workspace_id,
            cwd,
            branch,
            base,
            pr,
            path,
            label,
            focus,
            trust_repository,
        }),
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

fn worktree_open(args: &[String]) -> std::io::Result<i32> {
    let mut workspace_id = None;
    let mut cwd = None;
    let mut path = None;
    let mut branch = None;
    let mut label = None;
    let mut focus = false;
    let mut trust_repository = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--workspace" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --workspace");
                    return Ok(2);
                };
                workspace_id = Some(super::normalize_workspace_id(value));
                index += 2;
            }
            "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --cwd");
                    return Ok(2);
                };
                cwd = Some(normalize_path_arg(value)?);
                index += 2;
            }
            "--path" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --path");
                    return Ok(2);
                };
                path = Some(normalize_path_arg(value)?);
                index += 2;
            }
            "--branch" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --branch");
                    return Ok(2);
                };
                branch = Some(value.clone());
                index += 2;
            }
            "--label" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --label");
                    return Ok(2);
                };
                label = Some(value.clone());
                index += 2;
            }
            "--focus" => {
                focus = true;
                index += 1;
            }
            "--no-focus" => {
                focus = false;
                index += 1;
            }
            "--trust-repository" => {
                trust_repository = true;
                index += 1;
            }
            "--json" => index += 1,
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    if workspace_id.is_some() && cwd.is_some() {
        eprintln!(
            "usage: bora worktree open [--workspace ID | --cwd PATH] (--path PATH | --branch NAME) [--label TEXT] [--focus] [--no-focus] [--json] [--trust-repository]"
        );
        return Ok(2);
    }
    if path.is_some() == branch.is_some() {
        eprintln!(
            "usage: bora worktree open [--workspace ID | --cwd PATH] (--path PATH | --branch NAME) [--label TEXT] [--focus] [--no-focus] [--json] [--trust-repository]"
        );
        return Ok(2);
    }

    super::runtime::worktree_open(WorktreeOpenParams {
        workspace_id,
        cwd,
        path,
        branch,
        label,
        focus,
        trust_repository,
    })
}

fn worktree_remove(args: &[String]) -> std::io::Result<i32> {
    let mut workspace_id = None;
    let mut force = false;
    let mut trust_repository = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--workspace" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --workspace");
                    return Ok(2);
                };
                workspace_id = Some(super::normalize_workspace_id(value));
                index += 2;
            }
            "--force" => {
                force = true;
                index += 1;
            }
            "--trust-repository" => {
                trust_repository = true;
                index += 1;
            }
            "--json" => index += 1,
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let Some(workspace_id) = workspace_id else {
        eprintln!(
            "usage: bora worktree remove --workspace ID [--force] [--json] [--trust-repository]"
        );
        return Ok(2);
    };

    super::runtime::worktree_remove(WorktreeRemoveParams {
        workspace_id,
        force,
        trust_repository,
    })
}

const WORKTREE_CREATE_USAGE: &str = "usage: bora worktree create [--workspace ID | --cwd PATH] [--branch NAME] [--base REF] [--pr NUMBER] [--path PATH] [--label TEXT] [--group NAME] [--no-group] [--agent KIND] [--agent-name NAME] [--prompt TEXT | --prompt-file PATH] [--focus] [--no-focus] [--json] [--trust-repository]";

fn print_worktree_create_usage() {
    eprintln!("{WORKTREE_CREATE_USAGE}");
    eprintln!("  --workspace and --cwd are mutually exclusive; without either, the caller's own repo is used");
    eprintln!("  --pr is mutually exclusive with --branch/--base");
    eprintln!("  --prompt and --prompt-file are mutually exclusive, and need an agent");
    eprintln!("  --group and --no-group are mutually exclusive; without either, the caller pane's folder is inherited");
}

fn print_worktree_help() {
    eprintln!("bora worktree commands:");
    eprintln!("  bora worktree list [--workspace ID | --cwd PATH] [--json] [--trust-repository]");
    eprintln!("  {}", WORKTREE_CREATE_USAGE.trim_start_matches("usage: "));
    eprintln!(
        "  bora worktree open [--workspace ID | --cwd PATH] (--path PATH | --branch NAME) [--label TEXT] [--focus] [--no-focus] [--json] [--trust-repository]"
    );
    eprintln!("  bora worktree remove --workspace ID [--force] [--json] [--trust-repository]");
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
