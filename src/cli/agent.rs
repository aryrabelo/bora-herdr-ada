use std::time::{Duration, Instant};

use crate::api::schema::{
    AgentPromptParams, AgentReadParams, AgentRenameParams, AgentSendKeysParams, AgentStartParams,
    AgentTarget, AgentWaitParams, EmptyParams, ErrorBody, ErrorResponse, Method,
    PaneProcessInfoParams, PaneTarget, Request, WorkspaceCreateParams,
};

const AGENT_START_POLL_INTERVAL: Duration = Duration::from_millis(100);
const PANE_SHELL_READINESS_RETRY_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) mod bindings;
#[cfg(test)]
mod bindings_tests;

pub(crate) fn command() -> clap::Command {
    bindings::command()
}

pub(super) fn run_agent_command(args: &[String]) -> std::io::Result<i32> {
    use super::projection::ParseError;
    use bindings::Operation;

    match args.first().map(String::as_str) {
        Some("--new") => return agent_new(&args[1..]),
        // `bora agent <name> prompt <text>`: get-or-create, the name is the
        // idempotency key. Real subcommands keep their own grammar.
        Some(name)
            if args.get(1).map(String::as_str) == Some("prompt")
                && !command()
                    .get_subcommands()
                    .any(|leaf| leaf.get_name() == name) =>
        {
            return agent_named_prompt(name, &args[2..]);
        }
        _ => {}
    }
    // `--until exited` waits for the agent's pane to exit; it is not an agent
    // status, so it is taken out before the status grammar sees it.
    let without_exited;
    let (args, wait_exited) = match args.first().map(String::as_str) {
        Some("wait") if until_exited_requested(args) => {
            without_exited = strip_until_exited(args);
            (&without_exited[..], true)
        }
        _ => (args, false),
    };

    // `explain` and `wait` historically treat a bare `help` operand after the target as help.
    let help_args;
    let args = match args.first().map(String::as_str) {
        Some(leaf @ ("explain" | "wait")) => {
            let operands = super::projection::operands(&command(), args);
            let skip = usize::from(leaf == "wait");
            if operands.iter().skip(skip).any(|operand| *operand == "help") {
                help_args = [leaf.to_owned(), "--help".to_owned()];
                &help_args[..]
            } else {
                args
            }
        }
        _ => args,
    };
    let operation = match super::projection::parse(
        command().bin_name("bora agent"),
        args,
        bindings::from_matches,
    ) {
        Ok(operation) => operation,
        Err(ParseError::Clap(error)) => return super::print_clap_error(&error),
        Err(ParseError::Usage(message)) => {
            eprintln!("{message}");
            return Ok(2);
        }
        Err(ParseError::Io(error)) => return Err(error),
    };
    match operation {
        Operation::List => agent_list(),
        Operation::Get(params) => agent_get(params),
        Operation::Focus(params) => agent_focus(params),
        Operation::Read(params) => agent_read(params),
        Operation::SendKeys(params) => agent_send_keys(params),
        Operation::Rename(params) => agent_rename(params),
        Operation::Wait(params) if wait_exited => {
            agent_wait_exited(&params.target, params.timeout_ms)
        }
        Operation::Wait(params) => agent_wait(params),
        Operation::Prompt(params) => agent_prompt(params),
        Operation::Start(params) => agent_start(params),
        Operation::Attach(params) => agent_attach(params),
        Operation::Explain(params) => agent_explain(params),
    }
}

fn agent_explain(params: bindings::Explain) -> std::io::Result<i32> {
    let bindings::Explain {
        file,
        agent,
        json,
        verbose,
        target,
    } = params;

    let explain = if let Some(path) = file {
        if target.is_some() {
            eprintln!("usage: bora agent explain --file PATH --agent LABEL [--json]");
            return Ok(2);
        }
        let Some(agent_label) = agent else {
            eprintln!("bora agent explain --file requires --agent LABEL");
            return Ok(2);
        };
        let content = match std::fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) => {
                let response = ErrorResponse {
                    id: "cli:agent:explain".into(),
                    error: ErrorBody {
                        code: "agent_explain_file_read_failed".into(),
                        message: format!("failed to read agent explain file {path}: {err}"),
                    },
                };
                let response = serde_json::to_string(&response).map_err(std::io::Error::other)?;
                eprintln!("{response}");
                return Ok(1);
            }
        };
        crate::detect::manifest::explain_to_json_value(&crate::detect::manifest::explain_for_label(
            &agent_label,
            &content,
        ))
    } else {
        let Some(target) = target else {
            eprintln!("usage: bora agent explain <target> [--json]");
            eprintln!("usage: bora agent explain --file PATH --agent LABEL [--json]");
            return Ok(2);
        };
        if agent.is_some() {
            eprintln!("--agent is only valid with --file");
            return Ok(2);
        }

        let response = super::send_request(&Request {
            id: "cli:agent:explain".into(),
            method: Method::AgentExplain(AgentTarget { target }),
        })?;
        if response.get("error").is_some() {
            return super::print_response(&response);
        }
        response["result"]["explain"].clone()
    };

    if json {
        println!("{explain}");
    } else {
        print_agent_explain_text(&explain, verbose);
    }
    Ok(0)
}

fn print_agent_explain_text(explain: &serde_json::Value, verbose: bool) {
    println!("agent: {}", explain["agent"].as_str().unwrap_or("unknown"));
    println!("state: {}", explain["state"].as_str().unwrap_or("unknown"));
    println!(
        "manifest: {} {}",
        explain["manifest_source"].as_str().unwrap_or("none"),
        explain["manifest_version"].as_str().unwrap_or("unknown")
    );
    if let Some(rule) = explain["matched_rule"].as_object() {
        let rule_id = rule
            .get("id")
            .and_then(|value| value.as_str())
            .unwrap_or("-");
        println!(
            "rule: {} (region={} priority={})",
            rule_id,
            rule.get("region")
                .and_then(|value| value.as_str())
                .unwrap_or("-"),
            rule.get("priority")
                .and_then(serde_json::Value::as_i64)
                .unwrap_or(0),
        );
        if let Some(preview) = matched_rule_region_preview(explain, rule_id) {
            println!("evidence: {preview:?}");
        }
    } else {
        println!("rule: none");
    }
    if let Some(reason) = explain["fallback_reason"].as_str() {
        println!("fallback_reason: {reason}");
    }
    if let Some(reason) = explain["screen_detection_skip_reason"].as_str() {
        println!("screen_detection_skip_reason: {reason}");
    }
    if let Some(reason) = explain["skipped_update_reason"].as_str() {
        println!("skipped_update_reason: {reason}");
    }
    if let Some(warning) = explain["warning"].as_str() {
        println!("warning: {warning}");
    }

    if !verbose {
        return;
    }

    println!(
        "visible: idle={} blocker={} working={}",
        explain["visible_idle"].as_bool().unwrap_or(false),
        explain["visible_blocker"].as_bool().unwrap_or(false),
        explain["visible_working"].as_bool().unwrap_or(false)
    );
    println!(
        "cached_remote_version: {}",
        explain["cached_remote_version"].as_str().unwrap_or("none")
    );
    println!(
        "local_override_shadowing_remote: {}",
        explain["local_override_shadowing_remote"]
            .as_bool()
            .unwrap_or(false)
    );
    if let Some(status) = explain["remote_update_status"].as_str() {
        println!("remote_update_status: {status}");
    }
    if let Some(error) = explain["remote_update_error"].as_str() {
        println!("remote_update_error: {error}");
    }
    if let Some(evaluated_rules) = explain["evaluated_rules"]
        .as_array()
        .filter(|rules| !rules.is_empty())
    {
        println!("evaluated_rules:");
        for rule in evaluated_rules {
            println!(
                "  {} {} priority={} region={} state={}",
                if rule["matched"].as_bool().unwrap_or(false) {
                    "✓"
                } else {
                    "✗"
                },
                rule["id"].as_str().unwrap_or("-"),
                rule["priority"].as_i64().unwrap_or(0),
                rule["region"].as_str().unwrap_or("-"),
                rule["state"].as_str().unwrap_or("unknown")
            );
            let evidence = &rule["evidence"];
            println!(
                "    matchers: contains={:?} regex={:?} line_regex={:?} all={} any={} not={}",
                evidence["contains"],
                evidence["regex"],
                evidence["line_regex"],
                evidence["all_count"].as_u64().unwrap_or(0),
                evidence["any_count"].as_u64().unwrap_or(0),
                evidence["not_count"].as_u64().unwrap_or(0)
            );
            println!(
                "    region: bytes={} preview={:?}",
                evidence["region_bytes"].as_u64().unwrap_or(0),
                evidence["region_preview"].as_str().unwrap_or("")
            );
        }
    }
}

fn matched_rule_region_preview<'a>(
    explain: &'a serde_json::Value,
    rule_id: &str,
) -> Option<&'a str> {
    explain["evaluated_rules"]
        .as_array()?
        .iter()
        .find(|rule| rule["id"].as_str() == Some(rule_id))?["evidence"]["region_preview"]
        .as_str()
        .filter(|preview| !preview.is_empty())
}

fn agent_start(params: AgentStartParams) -> std::io::Result<i32> {
    let AgentStartParams {
        name,
        kind,
        pane_id,
        args: agent_args,
        timeout_ms,
    } = params;
    let Some(_) = crate::detect::parse_agent_label(&kind) else {
        eprintln!("unsupported interactive agent kind: {kind}");
        return Ok(2);
    };
    match start_agent_core(&name, &kind, &pane_id, agent_args, timeout_ms) {
        Ok(response) => super::print_response(&response),
        Err(err) => {
            print_agent_transport_error(err, "cli:agent:start", "agent_start_transport_failed")
        }
    }
}

/// The start-and-wait core of `agent start`, factored so `agent --new` /
/// `agent <name> prompt` can drive the same retry loop on a pane they just
/// created. Returns the final wire response verbatim — success or error
/// JSON — and lets each caller decide what an error code means (a race on
/// `agent_name_taken` is a hard error for `start`, a resolved idempotency
/// hit for the get-or-create path).
fn start_agent_core(
    name: &str,
    kind: &str,
    pane_id: &str,
    agent_args: Vec<String>,
    timeout_ms: Option<u64>,
) -> std::io::Result<serde_json::Value> {
    let expected_kind = crate::detect::parse_agent_label(kind)
        .map(|agent| crate::detect::agent_label(agent).to_string());
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(30_000));
    let retryable_timeout = timeout > crate::app::AGENT_START_SETTLE_DELAY
        && timeout <= crate::app::MAX_AGENT_START_TIMEOUT;
    let pinned_terminal_id = pane_terminal_id(pane_id)?;
    let mut retry_deadline = None;
    let mut previous_busy_response: Option<serde_json::Value> = None;
    let mut response = loop {
        if let Some(previous_busy_response) = previous_busy_response.as_ref() {
            let retry_expired = retry_deadline.is_some_and(|deadline| Instant::now() >= deadline);
            if retry_expired
                || pane_terminal_id(pane_id)? != pinned_terminal_id
                || !pane_shell_is_initializing(pane_id)?
            {
                return Ok(previous_busy_response.clone());
            }
        }

        let response = super::send_request(&Request {
            id: "cli:agent:start".into(),
            method: Method::AgentStart(AgentStartParams {
                name: name.to_owned(),
                kind: kind.to_owned(),
                pane_id: pane_id.to_owned(),
                args: agent_args.clone(),
                timeout_ms,
            }),
        })?;
        if response.get("error").is_none() {
            break response;
        }
        if response["error"]["code"].as_str() != Some("agent_pane_busy")
            || !retryable_timeout
            || pinned_terminal_id.is_none()
            || pane_terminal_id(pane_id)? != pinned_terminal_id
            || !pane_shell_is_initializing(pane_id)?
        {
            return Ok(response);
        }

        let deadline = *retry_deadline
            .get_or_insert_with(|| Instant::now() + PANE_SHELL_READINESS_RETRY_TIMEOUT);
        previous_busy_response = Some(response);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            if let Some(previous_busy_response) = previous_busy_response.as_ref() {
                return Ok(previous_busy_response.clone());
            }
        }
        std::thread::sleep(AGENT_START_POLL_INTERVAL.min(remaining));
    };

    let Some(expected_terminal_id) = response["result"]["agent"]["terminal_id"].as_str() else {
        return Ok(cli_agent_error(
            "cli:agent:start",
            "agent_start_failed",
            "agent start response did not include terminal_id",
        ));
    };
    if pinned_terminal_id
        .as_deref()
        .is_some_and(|pinned| pinned != expected_terminal_id)
    {
        return Ok(agent_name_lost_error("cli:agent:start", name));
    }
    let Some(expected_kind) = expected_kind else {
        return Ok(cli_agent_error(
            "cli:agent:start",
            "agent_kind_mismatch",
            format!("unsupported interactive agent kind: {kind}"),
        ));
    };
    let waited = wait_for_named_agent(name, pane_id, timeout, &expected_kind, expected_terminal_id);
    match waited {
        Ok(Ok(agent)) => {
            response["result"]["agent"] = agent;
            Ok(response)
        }
        Ok(Err(error)) => Ok(error),
        Err(err) => Err(err),
    }
}

fn agent_list() -> std::io::Result<i32> {
    super::print_response(&super::send_request(&Request {
        id: "cli:agent:list".into(),
        method: Method::AgentList(EmptyParams::default()),
    })?)
}

fn agent_get(params: AgentTarget) -> std::io::Result<i32> {
    super::print_response(&super::send_request(&Request {
        id: "cli:agent:get".into(),
        method: Method::AgentGet(params),
    })?)
}

fn agent_focus(params: AgentTarget) -> std::io::Result<i32> {
    super::print_response(&super::send_request(&Request {
        id: "cli:agent:focus".into(),
        method: Method::AgentFocus(params),
    })?)
}

fn agent_attach(params: bindings::Attach) -> std::io::Result<i32> {
    let bindings::Attach { target, takeover } = params;

    let response = resolve_agent_target(&target, "cli:agent:attach:resolve")?;
    if response.get("error").is_some() {
        return super::print_response(&response);
    }
    let Some(terminal_id) = response["result"]["agent"]["terminal_id"].as_str() else {
        eprintln!("agent attach failed: response did not include terminal_id");
        return Ok(1);
    };
    crate::client::run_terminal_attach(terminal_id.to_owned(), takeover)?;
    Ok(0)
}

fn agent_wait(params: AgentWaitParams) -> std::io::Result<i32> {
    super::print_response(&super::send_request(&Request {
        id: "cli:agent:wait".into(),
        method: Method::AgentWait(params),
    })?)
}

fn wait_for_named_agent(
    name: &str,
    fallback_pane_id: &str,
    timeout: Duration,
    expected_kind: &str,
    expected_terminal_id: &str,
) -> std::io::Result<Result<serde_json::Value, serde_json::Value>> {
    let deadline = Instant::now().checked_add(timeout);
    let mut first_poll = true;
    loop {
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            // Let the server reconcile its matching startup deadline before
            // returning so the pending name is immediately reusable.
            let _ = resolve_agent_target_unchecked(name, "cli:agent:start:timeout");
            return Ok(Err(agent_wait_timeout()));
        }
        let poll_id = "cli:agent:start";
        let mut response = if first_poll {
            first_poll = false;
            resolve_agent_target(name, poll_id)?
        } else {
            resolve_agent_target_unchecked(name, poll_id)?
        };
        if response.get("error").is_some() {
            response = resolve_agent_target_unchecked(fallback_pane_id, poll_id)?;
            if response.get("error").is_some() {
                std::thread::sleep(AGENT_START_POLL_INTERVAL);
                continue;
            }
        }
        let agent = &response["result"]["agent"];
        let outcome = if agent["terminal_id"].as_str() != Some(expected_terminal_id) {
            Some(Err(agent_name_lost_error("cli:agent:start", name)))
        } else if let Some(actual) = agent["agent"]
            .as_str()
            .filter(|actual| *actual != expected_kind)
        {
            Some(Err(cli_agent_error(
                "cli:agent:start",
                "agent_kind_mismatch",
                format!("expected {expected_kind}, detected {actual}"),
            )))
        } else if agent["name"].as_str() != Some(name) {
            Some(Err(agent_name_lost_error("cli:agent:start", name)))
        } else {
            match agent["agent_status"].as_str() {
                Some("blocked") => Some(Err(cli_agent_error(
                    "cli:agent:start",
                    "agent_not_ready",
                    format!("agent {name} is blocked during startup and is not ready for prompts"),
                ))),
                Some("unknown")
                    if expected_kind == "codex"
                        && agent["interactive_ready"].as_bool() == Some(true) =>
                {
                    Some(Ok(agent.clone()))
                }
                Some("working" | "unknown") => None,
                Some("idle" | "done") if agent["interactive_ready"].as_bool() == Some(true) => {
                    Some(Ok(agent.clone()))
                }
                Some("idle" | "done") if !agent["launch_pending"].as_bool().unwrap_or(false) => {
                    Some(Err(cli_agent_error(
                        "cli:agent:start",
                        "agent_start_failed",
                        "agent process exited before becoming interactive",
                    )))
                }
                _ => None,
            }
        };
        if let Some(outcome) = outcome {
            return Ok(outcome);
        }
        std::thread::sleep(AGENT_START_POLL_INTERVAL);
    }
}

fn pane_terminal_id(pane_id: &str) -> std::io::Result<Option<String>> {
    let response = super::send_request(&Request {
        id: "cli:agent:start:pane".into(),
        method: Method::PaneGet(PaneTarget {
            pane_id: pane_id.to_owned(),
        }),
    })?;
    Ok(response["result"]["pane"]["terminal_id"]
        .as_str()
        .map(str::to_owned))
}

fn pane_shell_is_initializing(pane_id: &str) -> std::io::Result<bool> {
    let response = super::send_request(&Request {
        id: "cli:agent:start:process_info".into(),
        method: Method::PaneProcessInfo(PaneProcessInfoParams {
            pane_id: Some(pane_id.to_owned()),
        }),
    })?;
    Ok(process_info_shows_shell_initialization(
        &response["result"]["process_info"],
    ))
}

#[cfg(unix)]
fn process_info_shows_shell_initialization(process_info: &serde_json::Value) -> bool {
    let Some(shell_pid) = process_info["shell_pid"].as_u64() else {
        return false;
    };
    if process_info["foreground_process_group_id"].as_u64() != Some(shell_pid) {
        return false;
    }
    process_info["foreground_processes"]
        .as_array()
        .is_some_and(|processes| {
            processes.iter().any(|process| {
                process["pid"].as_u64() == Some(shell_pid)
                    && (process["name"]
                        .as_str()
                        .is_some_and(crate::platform::is_pane_shell_process_name)
                        || process["argv"]
                            .as_array()
                            .and_then(|argv| argv.first())
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(crate::platform::is_pane_shell_process_name))
            })
        })
}

// Windows exposes no foreground process group, so shell initialization is not
// observable and a busy `agent.start` is not retried there.
#[cfg(not(unix))]
fn process_info_shows_shell_initialization(_process_info: &serde_json::Value) -> bool {
    false
}

fn agent_name_lost_error(request_id: &str, expected_name: &str) -> serde_json::Value {
    cli_agent_error(
        request_id,
        "agent_name_not_found",
        format!("named agent {expected_name} no longer owns the target terminal"),
    )
}

fn print_agent_transport_error(
    err: std::io::Error,
    request_id: &str,
    code: &str,
) -> std::io::Result<i32> {
    if super::protocol_mismatch_was_reported(&err) {
        return Ok(1);
    }
    // A dead-server marker reaches here from `send_request` in the agent
    // startup path; surface its deferred response exactly once instead of
    // printing a second, generic transport-error line.
    if let Some(response) = super::server_not_running_reported_response(&err) {
        let value = serde_json::to_value(response).map_err(std::io::Error::other)?;
        return super::print_response(&value);
    }
    super::print_response(&cli_agent_error(request_id, code, err.to_string()))
}

fn agent_wait_timeout() -> serde_json::Value {
    cli_agent_error(
        "cli:agent:start",
        "timeout",
        "timed out waiting for agent startup",
    )
}

fn cli_agent_error(id: &str, code: &str, message: impl Into<String>) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "error": { "code": code, "message": message.into() }
    })
}

fn resolve_agent_target(target: &str, request_id: &str) -> std::io::Result<serde_json::Value> {
    super::send_request(&agent_get_request(target, request_id))
}

fn resolve_agent_target_unchecked(
    target: &str,
    request_id: &str,
) -> std::io::Result<serde_json::Value> {
    super::send_request_unchecked(&agent_get_request(target, request_id))
}

fn agent_get_request(target: &str, request_id: &str) -> Request {
    Request {
        id: request_id.into(),
        method: Method::AgentGet(AgentTarget {
            target: target.to_owned(),
        }),
    }
}

fn agent_rename(params: AgentRenameParams) -> std::io::Result<i32> {
    super::print_response(&super::send_request(&Request {
        id: "cli:agent:rename".into(),
        method: Method::AgentRename(params),
    })?)
}

fn agent_prompt(mut params: AgentPromptParams) -> std::io::Result<i32> {
    params.from_pane = match params.from_pane {
        None => caller_pane(),
        Some(pane_id) if pane_id.is_empty() => None,
        explicit => explicit,
    };
    let response = super::send_request(&Request {
        id: "cli:agent:prompt".into(),
        method: Method::AgentPrompt(params),
    })?;
    if response.get("error").is_none() && response["result"]["outcome"].as_str() == Some("deferred")
    {
        let queue_position = response["result"]["queue_position"].as_u64();
        let position_note = queue_position
            .map(|position| format!(" (pos {position})"))
            .unwrap_or_default();
        eprintln!("deferred: target working, queued{position_note}");
    }
    super::print_response(&response)
}

fn agent_send_keys(params: AgentSendKeysParams) -> std::io::Result<i32> {
    super::print_response(&super::send_request(&Request {
        id: "cli:agent:send-keys".into(),
        method: Method::AgentSendKeys(params),
    })?)
}

fn agent_read(params: AgentReadParams) -> std::io::Result<i32> {
    let response = super::send_request(&Request {
        id: "cli:agent:read".into(),
        method: Method::AgentRead(params),
    })?;
    super::print_read_response(&response)
}

/// `bora agent --new "<prompt>" [--kind KIND] [--name NAME]` — one command
/// from "I want an agent working here" to a live session: create a workspace
/// on the caller's cwd, start the resolved agent on its root pane, inject the
/// prompt, print one JSON. Always creates: two runs give two agents.
fn agent_new(args: &[String]) -> std::io::Result<i32> {
    let Some(prompt) = args.first() else {
        eprintln!("usage: bora agent --new <prompt> [--kind KIND] [--name NAME]");
        return Ok(2);
    };
    let mut kind = None;
    let mut name = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--kind" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --kind");
                    return Ok(2);
                };
                kind = Some(value.clone());
                index += 2;
            }
            "--name" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --name");
                    return Ok(2);
                };
                name = Some(value.clone());
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    agent_new_core(name, kind, prompt, false)
}

/// `bora agent <name> prompt "<text>" [--kind KIND]` — get-or-create: the
/// name is the idempotency key. An existing agent just gets the prompt; a
/// missing one is created (workspace on cwd + start + prompt) with exactly
/// that name. A creation race (`agent_name_taken`) resolves to the winner.
fn agent_named_prompt(name: &str, args: &[String]) -> std::io::Result<i32> {
    let Some(text) = args.first() else {
        eprintln!("usage: bora agent <name> prompt <text> [--kind KIND]");
        return Ok(2);
    };
    let mut kind = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--kind" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --kind");
                    return Ok(2);
                };
                kind = Some(value.clone());
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    let probe = resolve_agent_target_unchecked(name, "cli:agent:new:probe")?;
    if probe.get("error").is_none() {
        return prompt_existing_agent(name, text, probe["result"]["agent"].clone());
    }
    if probe["error"]["code"].as_str() != Some("agent_not_found") {
        return super::print_response(&probe);
    }
    agent_new_core(Some(name.to_owned()), kind, text, true)
}

/// The shared composition behind `--new` and `<name> prompt`:
/// `workspace.create` (caller's cwd, no focus steal) → `agent.start` →
/// `agent.prompt`, entirely client-side over existing verbs. `idempotent`
/// marks the get-or-create form: a creation race on the exact name prompts
/// the winner instead of failing. The always-create form treats
/// `agent_name_taken` on an explicit `--name` as a hard error and suffixes
/// derived names (`-2`, `-3`…) instead.
fn agent_new_core(
    name: Option<String>,
    kind_flag: Option<String>,
    prompt: &str,
    idempotent: bool,
) -> std::io::Result<i32> {
    let config = crate::config::Config::load().config;
    let kind = resolve_new_kind(kind_flag.as_deref(), &config.agents);
    let Some(parsed_kind) = crate::detect::parse_agent_label(&kind) else {
        eprintln!("unsupported interactive agent kind: {kind}");
        return Ok(2);
    };
    let kind = crate::detect::agent_label(parsed_kind).to_string();

    let cwd = std::env::current_dir()?;
    let workspace = super::send_request(&Request {
        id: "cli:agent:new:workspace".into(),
        method: Method::WorkspaceCreate(WorkspaceCreateParams {
            source_workspace_id: None,
            cwd: Some(cwd.to_string_lossy().into_owned()),
            focus: false,
            label: None,
            env: std::collections::HashMap::new(),
            group: None,
        }),
    })?;
    if workspace.get("error").is_some() {
        return super::print_response(&workspace);
    }
    let Some(pane_id) = workspace["result"]["root_pane"]["pane_id"]
        .as_str()
        .map(str::to_owned)
    else {
        return super::print_response(&cli_agent_error(
            "cli:agent:new",
            "agent_new_failed",
            "workspace create response did not include root_pane.pane_id",
        ));
    };

    let explicit_name = name.is_some();
    let base = name.unwrap_or_else(|| sanitize_agent_name(&cwd));
    for attempt in 0..10u32 {
        let candidate = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{}", attempt + 1)
        };
        let started = match start_agent_core(&candidate, &kind, &pane_id, Vec::new(), None) {
            Ok(response) => response,
            Err(err) => {
                return print_agent_transport_error(
                    err,
                    "cli:agent:new",
                    "agent_start_transport_failed",
                );
            }
        };
        if started.get("error").is_none() {
            let prompted = send_new_agent_prompt(&candidate, prompt)?;
            if prompted.get("error").is_some() {
                return super::print_response(&prompted);
            }
            return super::print_response(&serde_json::json!({
                "id": "cli:agent:new",
                "result": {
                    "type": "agent_new",
                    "created": true,
                    "prompt_delivered": true,
                    "workspace": workspace["result"]["workspace"],
                    "pane": workspace["result"]["root_pane"],
                    "agent": started["result"]["agent"],
                    "prompt": prompted["result"],
                }
            }));
        }
        if started["error"]["code"].as_str() == Some("agent_name_taken") {
            if idempotent {
                // A concurrent dispatcher created this key first: the name
                // uniqueness invariant makes THAT agent the right target —
                // prompt it. The creation, not the prompt text, is what the
                // key deduplicates.
                let winner = resolve_agent_target_unchecked(&candidate, "cli:agent:new:winner")?;
                if winner.get("error").is_none() {
                    return prompt_existing_agent(
                        &candidate,
                        prompt,
                        winner["result"]["agent"].clone(),
                    );
                }
                return super::print_response(&winner);
            }
            if explicit_name {
                return super::print_response(&started);
            }
            continue;
        }
        return super::print_response(&cli_agent_error(
            "cli:agent:new",
            "agent_new_failed",
            format!(
                "{} (orphan pane: {pane_id})",
                started["error"]["message"]
                    .as_str()
                    .unwrap_or("agent start failed")
            ),
        ));
    }
    super::print_response(&cli_agent_error(
        "cli:agent:new",
        "agent_new_failed",
        format!("no free agent name based on `{base}` (orphan pane: {pane_id})"),
    ))
}

/// Get-or-create hit (or race resolution): the agent already existed, so
/// only the prompt is new. `created: false` is what tells an idempotent
/// re-dispatch apart from a fresh one.
fn prompt_existing_agent(
    name: &str,
    prompt: &str,
    agent: serde_json::Value,
) -> std::io::Result<i32> {
    let prompted = send_new_agent_prompt(name, prompt)?;
    if prompted.get("error").is_some() {
        return super::print_response(&prompted);
    }
    super::print_response(&serde_json::json!({
        "id": "cli:agent:new",
        "result": {
            "type": "agent_new",
            "created": false,
            "prompt_delivered": true,
            "agent": agent,
            "prompt": prompted["result"],
        }
    }))
}

/// Default prompt attribution: the pane this CLI runs in, if any.
fn caller_pane() -> Option<String> {
    std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| super::normalize_pane_id(&value))
}

fn send_new_agent_prompt(target: &str, text: &str) -> std::io::Result<serde_json::Value> {
    let from_pane = caller_pane();
    super::send_request(&Request {
        id: "cli:agent:prompt".into(),
        method: Method::AgentPrompt(AgentPromptParams {
            target: target.to_owned(),
            text: text.to_owned(),
            wait: None,
            from_pane,
            when_idle: None,
            when_idle_timeout_ms: None,
            peer_pid: None,
            origin_channel: None,
        }),
    })
}

/// Prompt delivery for composed commands: same verb `agent prompt` uses,
/// attributed to the caller pane so the receiving agent sees who dispatched it.
pub(super) fn send_agent_prompt(target: &str, text: &str) -> std::io::Result<serde_json::Value> {
    send_new_agent_prompt(target, text)
}

/// A named agent living on a pane, and how it got there.
pub(super) struct PaneAgent {
    pub(super) name: String,
    /// The `AgentInfo` JSON as the server reports it.
    pub(super) agent: serde_json::Value,
    /// `true` when this call started the process; `false` when an agent was
    /// already living on the pane and was only renamed.
    pub(super) started: bool,
}

/// Puts an agent named `name` on `pane_id`, whether or not the pane already
/// runs one of its own.
///
/// Both outcomes are success. A pane whose shell launches an agent by itself
/// answers `agent.start` with `agent_pane_busy` — reuse, not failure (the same
/// reading `agent_name_taken` gets on the get-or-create path). On that answer
/// this waits for the server's own registration instead of sleeping a fixed
/// amount, then renames what registered. A genuinely bare pane never pays that
/// wait: `agent.start` succeeds on the first try.
///
/// `Err` is transport failure; `Ok(Err(json))` is a wire error to report.
pub(super) fn ensure_named_agent_on_pane(
    pane_id: &str,
    kind: &str,
    name: &str,
    registration_timeout: Duration,
) -> std::io::Result<Result<PaneAgent, serde_json::Value>> {
    let Some(parsed_kind) = crate::detect::parse_agent_label(kind) else {
        return Ok(Err(cli_agent_error(
            "cli:agent:ensure",
            "agent_kind_mismatch",
            format!("unsupported interactive agent kind: {kind}"),
        )));
    };
    let kind = crate::detect::agent_label(parsed_kind).to_string();

    // An agent already registered here (a re-dispatch onto a workspace that
    // was still open) is the agent to reuse: starting a second one on the same
    // pane is impossible anyway.
    let existing = resolve_agent_target_unchecked(pane_id, "cli:agent:ensure")?;
    if existing.get("error").is_none() && !existing["result"]["agent"].is_null() {
        return adopt_pane_agent(
            pane_id,
            name,
            existing["result"]["agent"].clone(),
            registration_timeout,
        );
    }

    let started = start_agent_core(name, &kind, pane_id, Vec::new(), None)?;
    if started.get("error").is_none() {
        return Ok(Ok(PaneAgent {
            name: name.to_owned(),
            agent: started["result"]["agent"].clone(),
            started: true,
        }));
    }
    if started["error"]["code"].as_str() != Some("agent_pane_busy") {
        return Ok(Err(started));
    }
    // The pane runs its own agent. Wait for the server to register it, then
    // rename rather than report the busy pane as a failure.
    let Some(agent) = wait_for_pane_agent(pane_id, registration_timeout)? else {
        return Ok(Err(started));
    };
    adopt_pane_agent(pane_id, name, agent, registration_timeout)
}

/// Renames the agent already on the pane to `name`, waits for it to become
/// interactive, and returns its refreshed info. A pane whose agent is already
/// named `name` is left alone. Not becoming ready inside the window is not an
/// error: the caller reports the status it actually observed.
fn adopt_pane_agent(
    pane_id: &str,
    name: &str,
    agent: serde_json::Value,
    ready_timeout: Duration,
) -> std::io::Result<Result<PaneAgent, serde_json::Value>> {
    if agent["name"].as_str() != Some(name) {
        let renamed = super::send_request(&Request {
            id: "cli:agent:rename".into(),
            method: Method::AgentRename(AgentRenameParams {
                target: pane_id.to_owned(),
                name: Some(name.to_owned()),
            }),
        })?;
        if renamed.get("error").is_some() {
            return Ok(Err(renamed));
        }
    }
    Ok(Ok(PaneAgent {
        name: name.to_owned(),
        agent: wait_for_interactive_agent(pane_id, ready_timeout)?.unwrap_or(agent),
        started: false,
    }))
}

/// Polls until the pane's agent reports `interactive_ready`, so a prompt is
/// not typed into a process that is still drawing its first screen. Returns
/// the last snapshot it saw either way.
fn wait_for_interactive_agent(
    pane_id: &str,
    timeout: Duration,
) -> std::io::Result<Option<serde_json::Value>> {
    let deadline = Instant::now().checked_add(timeout);
    let mut last = None;
    loop {
        let response = resolve_agent_target_unchecked(pane_id, "cli:agent:ensure")?;
        if response.get("error").is_none() && !response["result"]["agent"].is_null() {
            let agent = response["result"]["agent"].clone();
            let ready = agent["interactive_ready"].as_bool() == Some(true);
            last = Some(agent);
            if ready {
                return Ok(last);
            }
        }
        if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
            return Ok(last);
        }
        std::thread::sleep(AGENT_START_POLL_INTERVAL);
    }
}

/// Polls the server's own agent registry until the pane reports an agent, or
/// the deadline passes. `agent_not_found` inside the window is "not yet", not
/// "never": concluding "bare pane" from a single early snapshot is what leads
/// straight into `agent_pane_busy`.
fn wait_for_pane_agent(
    pane_id: &str,
    timeout: Duration,
) -> std::io::Result<Option<serde_json::Value>> {
    let deadline = Instant::now().checked_add(timeout);
    loop {
        let response = resolve_agent_target_unchecked(pane_id, "cli:agent:ensure")?;
        if response.get("error").is_none() && !response["result"]["agent"].is_null() {
            return Ok(Some(response["result"]["agent"].clone()));
        }
        if deadline.is_none_or(|deadline| Instant::now() >= deadline) {
            return Ok(None);
        }
        std::thread::sleep(AGENT_START_POLL_INTERVAL);
    }
}

/// Default agent name for `agent --new`: the cwd's basename reduced to a
/// pane-friendly token (`My Proj!` → `my-proj`), `agent` when nothing
/// survives. Collision suffixes are the caller's loop.
fn sanitize_agent_name(cwd: &std::path::Path) -> String {
    let base = cwd
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = String::new();
    let mut last_dash = false;
    for ch in base.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() {
        "agent".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Kind for `agent --new`: `--kind` wins, then `[agents] default`, then
/// the hardcoded `"omp"` fallback inside `default_kind`.
fn resolve_new_kind(flag: Option<&str>, agents: &crate::config::AgentsConfig) -> String {
    flag.map(str::to_owned)
        .unwrap_or_else(|| agents.default_kind().to_owned())
}

/// `agent wait <target> --status exited` — resolve the target's pane and block
/// until that pane exits. A target that no longer resolves has already exited
/// (that is the success case, not an error): one-shot agents are reaped on exit.
fn agent_wait_exited(target: &str, timeout_ms: Option<u64>) -> std::io::Result<i32> {
    let response = resolve_agent_target(target, "cli:agent:wait:resolve")?;
    if response.get("error").is_some() {
        println!(
            "{}",
            serde_json::json!({
                "id": "cli:agent:wait",
                "result": { "type": "agent_wait", "target": target, "status": "exited" }
            })
        );
        return Ok(0);
    }
    let Some(pane_id) = response["result"]["agent"]["pane_id"].as_str() else {
        eprintln!("agent wait failed: response did not include pane_id");
        return Ok(1);
    };
    let pane_id = pane_id.to_owned();
    super::wait_for_pane_exited(&pane_id, timeout_ms)
}

fn until_exited_requested(args: &[String]) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == "--until" && pair[1] == "exited")
        || args.iter().any(|arg| arg == "--until=exited")
}

fn strip_until_exited(args: &[String]) -> Vec<String> {
    let mut kept = Vec::with_capacity(args.len());
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--until" && args.get(index + 1).map(String::as_str) == Some("exited") {
            index += 2;
            continue;
        }
        if args[index] != "--until=exited" {
            kept.push(args[index].clone());
        }
        index += 1;
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_agent_name_reduces_basenames() {
        assert_eq!(
            sanitize_agent_name(std::path::Path::new("/home/ary/My Proj!")),
            "my-proj"
        );
        assert_eq!(
            sanitize_agent_name(std::path::Path::new("/home/ary/dotfiles-2026")),
            "dotfiles-2026"
        );
        assert_eq!(
            sanitize_agent_name(std::path::Path::new("/home/ary/...")),
            "agent"
        );
        assert_eq!(sanitize_agent_name(std::path::Path::new("/")), "agent");
    }

    #[test]
    fn resolve_new_kind_prefers_flag_then_config_then_omp() {
        let mut agents = crate::config::AgentsConfig::default();
        assert_eq!(resolve_new_kind(Some("pi"), &agents), "pi");
        assert_eq!(resolve_new_kind(None, &agents), "omp");
        agents.default = Some("claude".to_owned());
        assert_eq!(resolve_new_kind(None, &agents), "claude");
        assert_eq!(resolve_new_kind(Some("pi"), &agents), "pi");
    }
}
