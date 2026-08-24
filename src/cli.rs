use std::time::{SystemTime, UNIX_EPOCH};

use crate::api::client::{ApiClient, ApiClientError};
use crate::api::schema::{
    AgentStatus, ChannelAskParams, ChannelCreateParams, ChannelHistoryParams, ChannelJoinParams,
    ChannelLeaveParams, ChannelListParams, ChannelMembersParams, ChannelNoteParams,
    ChannelOpenParams, ChannelSendParams, ChannelWaitParams, ClientWindowTitleSetParams,
    EmptyParams, Method, PaneAgentState, ReadFormat, ReadSource, Request, SplitDirection,
};

macro_rules! print {
    ($($arg:tt)*) => {{
        crate::platform::begin_cli_output();
        std::print!($($arg)*);
    }};
}

macro_rules! println {
    ($($arg:tt)*) => {{
        crate::platform::begin_cli_output();
        std::println!($($arg)*);
    }};
}

mod agent;
mod api;
mod completion;
mod integration;
mod mcp;
mod notification;
mod pane;
mod plugin;
mod protocol_guard;
mod runtime;
mod server;
mod server_not_running;
mod spec;
mod status;
mod tab;
mod workspace;
mod worktree;

const TERMINAL_SESSION_OBSERVE_USAGE: &str =
    "usage: herdr terminal session observe <target> [--cols N] [--rows N]";
const TERMINAL_SESSION_CONTROL_USAGE: &str =
    "usage: herdr terminal session control <target> [--takeover] [--cols N] [--rows N]";
pub(crate) const AGENT_HELP_FOOTER: &str = concat!(
    "Are you an AI? Use these resources ONLY IF your task specifically asks you to:\n",
    "  Help a human understand or set up Herdr for the first time:\n",
    "    https://herdr.dev/agent-guide.md\n",
    "  Debug or investigate a problem with Herdr:\n",
    "    https://herdr.dev/llms.txt\n",
    "  Control Herdr panes, agents, or workspaces:\n",
    "    SKIP if a Herdr skill is already in your context. Otherwise run: herdr --skill",
);

pub(crate) fn parse_token_assignment(raw: &str) -> Result<(String, Option<String>), String> {
    let Some((key, value)) = raw.split_once('=') else {
        return Err("token must use NAME=VALUE".into());
    };
    if key.is_empty() {
        return Err("token name must not be empty".into());
    }
    Ok((key.to_string(), Some(value.to_string())))
}

pub(crate) fn parse_env_assignment(raw: &str) -> Result<(String, String), String> {
    let Some((key, value)) = raw.split_once('=') else {
        return Err("env must use KEY=VALUE".into());
    };
    if key.is_empty() {
        return Err("env key must not be empty".into());
    }
    if key.contains('\0') || value.contains('\0') {
        return Err("env must not contain NUL bytes".into());
    }
    Ok((key.to_string(), value.to_string()))
}

pub enum CommandOutcome {
    Handled(i32),
    NotCli,
}

pub(super) fn print_read_response(response: &serde_json::Value) -> std::io::Result<i32> {
    if response.get("error").is_some() {
        eprintln!("{response}");
        return Ok(1);
    }
    if let Some(text) = response["result"]["read"]["text"].as_str() {
        print!("{text}");
    }
    Ok(0)
}

pub fn maybe_run(args: &[String]) -> std::io::Result<CommandOutcome> {
    let Some(command) = args.get(1).map(std::string::String::as_str) else {
        return Ok(CommandOutcome::NotCli);
    };

    if spec::print_requested_help(args)? {
        return Ok(CommandOutcome::Handled(0));
    }

    let exit_code = match command {
        "server" => {
            let Some(exit_code) = server::run_server_command(&args[2..])? else {
                return Ok(CommandOutcome::NotCli);
            };
            exit_code
        }
        "api" => api::run_api_command(&args[2..])?,
        "status" => status::run_status_command(&args[2..])?,
        "completion" | "completions" => completion::run_completion_command(&args[2..])?,
        "config" => run_config_command(&args[2..])?,
        "channel" => run_channel_command(&args[2..])?,
        "workspace" => workspace::run_workspace_command(&args[2..])?,
        "worktree" => worktree::run_worktree_command(&args[2..])?,
        "tab" => tab::run_tab_command(&args[2..])?,
        "notification" => notification::run_notification_command(&args[2..])?,
        "agent" => agent::run_agent_command(&args[2..])?,
        "terminal" => run_terminal_command(&args[2..])?,
        "pane" => pane::run_pane_command(&args[2..])?,
        "plugin" => plugin::run_plugin_command(&args[2..])?,
        "integration" => integration::run_integration_command(&args[2..])?,
        "session" => run_session_command(&args[2..])?,
        "mcp" => mcp::run_mcp_command(&args[2..])?,
        _ => return Ok(CommandOutcome::NotCli),
    };

    Ok(CommandOutcome::Handled(exit_code))
}

fn run_channel_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(std::string::String::as_str) {
        Some("set") => channel_set(&args[1..]),
        Some("show") if args.len() == 1 => {
            let config = crate::config::Config::load().config;
            println!("{}", config.update.channel.as_str());
            Ok(0)
        }
        Some("create") => channel_create(&args[1..]),
        Some("open") => channel_open(&args[1..]),
        Some("list") if args.len() == 1 => channel_list(),
        Some("send") => channel_send(&args[1..]),
        Some("note") => channel_note(&args[1..]),
        Some("ask") => channel_ask(&args[1..]),
        Some("history") => channel_history(&args[1..]),
        Some("tail") => channel_tail(&args[1..]),
        Some("members") => channel_members(&args[1..]),
        Some("join") => channel_join(&args[1..]),
        Some("leave") => channel_leave(&args[1..]),
        Some("help" | "--help" | "-h") => {
            print_channel_help();
            Ok(0)
        }
        _ => {
            print_channel_help();
            Ok(2)
        }
    }
}

fn channel_create(args: &[String]) -> std::io::Result<i32> {
    let Some(name) = args.first() else {
        eprintln!("usage: bora channel create <name>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: bora channel create <name>");
        return Ok(2);
    }
    print_response(&send_request(&Request {
        id: "cli:channel:create".into(),
        method: Method::ChannelCreate(ChannelCreateParams { name: name.clone() }),
    })?)
}

fn channel_open(args: &[String]) -> std::io::Result<i32> {
    let Some(name) = args.first() else {
        eprintln!("usage: bora channel open <name>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: bora channel open <name>");
        return Ok(2);
    }
    print_response(&send_request(&Request {
        id: "cli:channel:open".into(),
        method: Method::ChannelOpen(ChannelOpenParams { name: name.clone() }),
    })?)
}

fn channel_list() -> std::io::Result<i32> {
    let env_pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_pane_id(&value));
    print_response(&send_request(&Request {
        id: "cli:channel:list".into(),
        method: Method::ChannelList(ChannelListParams {
            from_pane: env_pane_id,
        }),
    })?)
}

fn channel_send(args: &[String]) -> std::io::Result<i32> {
    let usage =
        "usage: bora channel send <name> <text> [--pane ID|--current] [--to NICK] [--reply-to SEQ]";
    let Some(name) = args.first() else {
        eprintln!("{usage}");
        return Ok(2);
    };
    let Some(text) = args.get(1) else {
        eprintln!("{usage}");
        return Ok(2);
    };
    let env_pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_pane_id(&value));
    let (from_pane, to, in_reply_to) = match parse_channel_send_flags(&args[2..], env_pane_id) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };
    let response = send_request(&Request {
        id: "cli:channel:send".into(),
        method: Method::ChannelSend(ChannelSendParams {
            name: name.clone(),
            text: text.clone(),
            from_pane,
            to,
            in_reply_to,
            from_human: false,
        }),
    })?;
    // The bell (agent injection fan-out) was cut because the channel is
    // mid-burst; the message was still recorded. Note it on stderr so the
    // human at the CLI isn't left assuming silence meant delivery.
    if response["result"]["suppressed"].as_bool() == Some(true) {
        eprintln!("[bora] #{name} is in a burst: message recorded, agents not pinged");
    }
    print_response(&response)
}

fn channel_note(args: &[String]) -> std::io::Result<i32> {
    let Some(name) = args.first() else {
        eprintln!("usage: bora channel note <name> <text> [--pane ID|--current]");
        return Ok(2);
    };
    let Some(text) = args.get(1) else {
        eprintln!("usage: bora channel note <name> <text> [--pane ID|--current]");
        return Ok(2);
    };
    let env_pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_pane_id(&value));
    let from_pane = match parse_channel_note_flags(&args[2..], env_pane_id) {
        Ok(from_pane) => from_pane,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };
    print_response(&send_request(&Request {
        id: "cli:channel:note".into(),
        method: Method::ChannelNote(ChannelNoteParams {
            name: name.clone(),
            text: text.clone(),
            from_pane,
        }),
    })?)
}

/// Parses the flags accepted by `bora channel note` after `<name> <text>`:
/// just `--pane ID` / `--current`, the same pane-source pair `channel send`
/// accepts — `note` never addresses a recipient.
fn parse_channel_note_flags(
    args: &[String],
    mut from_pane: Option<String>,
) -> Result<Option<String>, String> {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pane" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --pane".into());
                };
                from_pane = Some(normalize_pane_id(value));
                index += 2;
            }
            "--current" => {
                index += 1;
            }
            option => return Err(format!("unknown option: {option}")),
        }
    }
    Ok(from_pane)
}

fn channel_ask(args: &[String]) -> std::io::Result<i32> {
    let usage = "usage: bora channel ask <name> <nick> <text> [--pane ID|--current] [--timeout MS]";
    let Some(name) = args.first() else {
        eprintln!("{usage}");
        return Ok(2);
    };
    let Some(to) = args.get(1) else {
        eprintln!("{usage}");
        return Ok(2);
    };
    let Some(text) = args.get(2) else {
        eprintln!("{usage}");
        return Ok(2);
    };
    let env_pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_pane_id(&value));
    let (from_pane, timeout_ms) = match parse_channel_ask_flags(&args[3..], env_pane_id) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            return Ok(2);
        }
    };
    let response = send_request(&Request {
        id: "cli:channel:ask".into(),
        method: Method::ChannelAsk(ChannelAskParams {
            name: name.clone(),
            to: to.clone(),
            text: text.clone(),
            from_pane,
            timeout_ms,
        }),
    })?;
    if response.get("error").is_some() {
        return print_response(&response);
    }
    if response["result"]["answered"].as_bool() == Some(true) {
        let reply_text = response["result"]["reply"]["text"].as_str().unwrap_or("");
        println!("{reply_text}");
        return Ok(0);
    }
    let question_seq = response["result"]["question_seq"].as_u64().unwrap_or(0);
    eprintln!(
        "[bora] #{name} no reply to seq {question_seq} within the timeout — reply with: bora channel send {name} <text> --reply-to {question_seq}"
    );
    Ok(1)
}

/// Parses the flags accepted by `bora channel ask` after
/// `<name> <nick> <text>`. Returns `(from_pane, timeout_ms)`; `timeout_ms`
/// stays `None` (server default) unless `--timeout` is given.
fn parse_channel_ask_flags(
    args: &[String],
    mut from_pane: Option<String>,
) -> Result<(Option<String>, Option<u64>), String> {
    let mut timeout_ms = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pane" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --pane".into());
                };
                from_pane = Some(normalize_pane_id(value));
                index += 2;
            }
            "--current" => {
                index += 1;
            }
            "--timeout" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --timeout".into());
                };
                timeout_ms = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --timeout value: {value}"))?,
                );
                index += 2;
            }
            option => return Err(format!("unknown option: {option}")),
        }
    }
    Ok((from_pane, timeout_ms))
}

/// Parses the flags accepted by `bora channel send` after `<name> <text>`.
/// Returns `(from_pane, to, in_reply_to)`; `from_pane` starts from
/// `env_pane_id` and can be overridden by `--pane`. `--current` is accepted
/// as a no-op flag for explicitness since `env_pane_id` already reflects
/// the current pane. `--reply-to SEQ` answers a `channel.ask` question:
/// threaded verbatim onto the sent message's `in_reply_to`, never validated
/// client-side — the server rejects a seq past the channel's current max.
/// `(from_pane, timeout_ms, reply_to)`-shaped flag bundle for `channel ask`.
type ChannelAskFlags = (Option<String>, Option<String>, Option<u64>);

fn parse_channel_send_flags(
    args: &[String],
    mut from_pane: Option<String>,
) -> Result<ChannelAskFlags, String> {
    let mut to = None;
    let mut in_reply_to = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pane" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --pane".into());
                };
                from_pane = Some(normalize_pane_id(value));
                index += 2;
            }
            "--current" => {
                index += 1;
            }
            "--to" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --to".into());
                };
                to = Some(value.clone());
                index += 2;
            }
            "--reply-to" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --reply-to".into());
                };
                in_reply_to = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid --reply-to value: {value}"))?,
                );
                index += 2;
            }
            option => {
                return Err(format!("unknown option: {option}"));
            }
        }
    }
    Ok((from_pane, to, in_reply_to))
}

fn channel_history(args: &[String]) -> std::io::Result<i32> {
    let Some(name) = args.first() else {
        eprintln!("usage: bora channel history <name> [--lines N] [--json]");
        return Ok(2);
    };
    let mut lines = None;
    let mut json = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--lines" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --lines");
                    return Ok(2);
                };
                lines = match value.parse::<u32>() {
                    Ok(lines) => Some(lines),
                    Err(_) => {
                        eprintln!("--lines must be a non-negative integer");
                        return Ok(2);
                    }
                };
                index += 2;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            option => {
                eprintln!("unknown option: {option}");
                return Ok(2);
            }
        }
    }
    let env_pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_pane_id(&value));
    let response = send_request(&Request {
        id: "cli:channel:history".into(),
        method: Method::ChannelHistory(ChannelHistoryParams {
            name: name.clone(),
            lines,
            from_pane: env_pane_id,
        }),
    })?;
    if json {
        return print_response(&response);
    }
    if response.get("error").is_some() {
        return print_response(&response);
    }
    let messages = response["result"]["messages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for message in messages {
        let ts = message["ts"].as_str().unwrap_or("");
        let hhmm = ts.get(11..16).unwrap_or(ts);
        let from_name = message["from_name"].as_str().unwrap_or("?");
        let text = message["text"].as_str().unwrap_or("");
        println!("{hhmm} {from_name}: {text}");
    }
    Ok(0)
}

/// Server-side poll window per `--follow` iteration; each round-trip is a
/// fresh `channel.wait` connection, so Ctrl-C simply drops the socket and
/// the server's disconnect check ends that wait.
const CHANNEL_TAIL_FOLLOW_POLL_MS: u64 = 2_000;

fn channel_tail(args: &[String]) -> std::io::Result<i32> {
    let Some(name) = args.first() else {
        eprintln!("usage: bora channel tail <name> [--after SEQ] [--follow] [--json]");
        return Ok(2);
    };
    let mut after_seq: u64 = 0;
    let mut follow = false;
    let mut json = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--after" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --after");
                    return Ok(2);
                };
                after_seq = match value.parse::<u64>() {
                    Ok(seq) => seq,
                    Err(_) => {
                        eprintln!("--after must be a non-negative integer");
                        return Ok(2);
                    }
                };
                index += 2;
            }
            "--follow" | "-f" => {
                follow = true;
                index += 1;
            }
            "--json" => {
                json = true;
                index += 1;
            }
            option => {
                eprintln!("unknown option: {option}");
                return Ok(2);
            }
        }
    }

    let env_pane_id = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_pane_id(&value));

    loop {
        // One-shot (`--follow` absent) uses timeout 0: backlog snapshot,
        // never blocks. Follow mode blocks server-side per poll window.
        let response = send_request(&Request {
            id: "cli:channel:tail".into(),
            method: Method::ChannelWait(ChannelWaitParams {
                name: name.clone(),
                after_seq,
                timeout_ms: Some(if follow {
                    CHANNEL_TAIL_FOLLOW_POLL_MS
                } else {
                    0
                }),
                from_pane: env_pane_id.clone(),
            }),
        })?;
        if response.get("error").is_some() {
            return print_response(&response);
        }

        let result = &response["result"];
        if json {
            println!("{}", encode_response_json(&response));
        } else {
            if result["gap"].as_bool() == Some(true) {
                match result["oldest_seq"].as_u64() {
                    Some(oldest) => eprintln!(
                        "#gap: messages between your cursor and seq {oldest} were rotated away; resuming from the oldest retained"
                    ),
                    None => eprintln!(
                        "#gap: no history retained; cursor {after_seq} predates it"
                    ),
                }
            }
            for message in result["messages"].as_array().cloned().unwrap_or_default() {
                let seq = message["seq"].as_u64().unwrap_or(0);
                let ts = message["ts"].as_str().unwrap_or("");
                let hhmm = ts.get(11..16).unwrap_or(ts);
                let from_name = message["from_name"].as_str().unwrap_or("?");
                let text = message["text"].as_str().unwrap_or("");
                println!("{seq} {hhmm} {from_name}: {text}");
            }
        }

        after_seq = result["messages"]
            .as_array()
            .and_then(|messages| messages.last())
            .and_then(|message| message["seq"].as_u64())
            .unwrap_or(after_seq);
        if !follow {
            return Ok(0);
        }
    }
}

fn channel_members(args: &[String]) -> std::io::Result<i32> {
    let Some(name) = args.first() else {
        eprintln!("usage: bora channel members <name> [--json]");
        return Ok(2);
    };
    let mut json = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => {
                json = true;
                index += 1;
            }
            option => {
                eprintln!("unknown option: {option}");
                return Ok(2);
            }
        }
    }
    let response = send_request(&Request {
        id: "cli:channel:members".into(),
        method: Method::ChannelMembers(ChannelMembersParams { name: name.clone() }),
    })?;
    if json {
        return print_response(&response);
    }
    if response.get("error").is_some() {
        return print_response(&response);
    }
    let members = response["result"]["members"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for member in members {
        let pane_id = member["pane_id"].as_str().unwrap_or("?");
        let status = member["agent_status"].as_str().unwrap_or("-");
        let name = member["name"].as_str().unwrap_or("-");
        let source = member["source"].as_str().unwrap_or("-");
        println!("{pane_id}  {status}  {name}  {source}");
    }
    Ok(0)
}

/// `bora channel join <name> [--pane ID] [--scope-write DIR]... [--scope-read DIR[,DIR]]...`.
/// The pane defaults to `$HERDR_PANE_ID`, so an agent can join the channel
/// it was told about without knowing its own pane id. `--scope-write` and
/// `--scope-read` are repeatable; `--scope-read` also accepts a
/// comma-separated list in one flag. See CANAL-ESCOPO.md Shape 2.
fn channel_join(args: &[String]) -> std::io::Result<i32> {
    let usage = "usage: bora channel join <name> [--pane ID] [--scope-write DIR]... [--scope-read DIR[,DIR]]...";
    let Some(name) = args.first() else {
        eprintln!("{usage}");
        return Ok(2);
    };
    let parsed = match parse_channel_join_flags(&args[1..]) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("{err}");
            eprintln!("{usage}");
            return Ok(2);
        }
    };
    let Some(pane) = parsed.pane else {
        eprintln!("no pane to join: pass --pane ID or run inside a bora pane (HERDR_PANE_ID)");
        return Ok(2);
    };
    print_response(&send_request(&Request {
        id: "cli:channel:join".into(),
        method: Method::ChannelJoin(ChannelJoinParams {
            name: name.clone(),
            pane,
            scope_write: (!parsed.scope_write.is_empty()).then_some(parsed.scope_write),
            scope_read: (!parsed.scope_read.is_empty()).then_some(parsed.scope_read),
        }),
    })?)
}

/// `bora channel leave <name> [--pane ID]`. Scope cleanup for the pane is
/// entirely server-side (`channel.leave` also drops its scope entry).
fn channel_leave(args: &[String]) -> std::io::Result<i32> {
    let usage = "usage: bora channel leave <name> [--pane ID]";
    let Some(name) = args.first() else {
        eprintln!("{usage}");
        return Ok(2);
    };
    let pane = match parse_membership_pane(&args[1..]) {
        Ok(Some(pane)) => pane,
        Ok(None) => {
            eprintln!("no pane to leave: pass --pane ID or run inside a bora pane (HERDR_PANE_ID)");
            return Ok(2);
        }
        Err(err) => {
            eprintln!("{err}");
            eprintln!("{usage}");
            return Ok(2);
        }
    };
    print_response(&send_request(&Request {
        id: "cli:channel:leave".into(),
        method: Method::ChannelLeave(ChannelLeaveParams {
            name: name.clone(),
            pane,
        }),
    })?)
}

/// Parsed `bora channel join` flags.
struct ChannelJoinFlags {
    pane: Option<String>,
    scope_write: Vec<String>,
    scope_read: Vec<String>,
}

fn parse_channel_join_flags(args: &[String]) -> Result<ChannelJoinFlags, String> {
    let mut pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_pane_id(&value));
    let mut scope_write = Vec::new();
    let mut scope_read = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pane" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --pane".into());
                };
                if value.trim().is_empty() {
                    return Err("--pane must not be empty".into());
                }
                pane = Some(normalize_pane_id(value));
                index += 2;
            }
            "--scope-write" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --scope-write".into());
                };
                let value = value.trim();
                if value.is_empty() {
                    return Err("--scope-write must not be empty".into());
                }
                scope_write.push(value.to_string());
                index += 2;
            }
            "--scope-read" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --scope-read".into());
                };
                let dirs: Vec<String> = value
                    .split(',')
                    .map(str::trim)
                    .filter(|dir| !dir.is_empty())
                    .map(str::to_string)
                    .collect();
                if dirs.is_empty() {
                    return Err("--scope-read must not be empty".into());
                }
                scope_read.extend(dirs);
                index += 2;
            }
            option => return Err(format!("unknown option: {option}")),
        }
    }
    Ok(ChannelJoinFlags {
        pane,
        scope_write,
        scope_read,
    })
}

/// Pane a membership verb acts on: `--pane ID` when given, else
/// `$HERDR_PANE_ID`. `None` means neither was available — the caller reports
/// that rather than guessing a pane.
fn parse_membership_pane(args: &[String]) -> Result<Option<String>, String> {
    let mut pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|value| normalize_pane_id(&value));
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--pane" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --pane".into());
                };
                if value.trim().is_empty() {
                    return Err("--pane must not be empty".into());
                }
                pane = Some(normalize_pane_id(value));
                index += 2;
            }
            option => return Err(format!("unknown option: {option}")),
        }
    }
    Ok(pane)
}

fn channel_set(args: &[String]) -> std::io::Result<i32> {
    let Some(channel) = parse_channel_set_arg(args) else {
        eprintln!("usage: bora channel set <stable|preview>");
        return Ok(2);
    };

    if let Some(reason) = channel_set_rejection(
        channel,
        crate::update::preview_channel_rejection_for_current_install(),
    ) {
        eprintln!("{reason}.");
        return Ok(1);
    }

    let path = crate::config::config_path();
    let content = if path.exists() {
        std::fs::read_to_string(&path)?
    } else {
        String::new()
    };
    if let Err(err) = content.parse::<toml::Value>() {
        eprintln!(
            "config file at {} is invalid TOML: {err}. Fix it before changing the update channel.",
            path.display()
        );
        return Ok(1);
    }

    let updated = crate::config::upsert_section_value(
        &content,
        "update",
        "channel",
        &format!("\"{channel}\""),
    );
    if let Err(err) = updated.parse::<toml::Value>() {
        eprintln!(
            "changing the update channel would make {} invalid TOML: {err}; leaving config unchanged",
            path.display()
        );
        return Ok(1);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, updated)?;
    println!(
        "Bora update channel set to {channel} in {}.",
        path.display()
    );

    match channel_set_install_action(
        crate::update::package_manager_channel_update_guidance_for_current_install(),
    ) {
        ChannelSetInstallAction::PrintGuidance(guidance) => {
            println!("{guidance}");
            return Ok(0);
        }
        ChannelSetInstallAction::RunSelfUpdate => {}
    }

    crate::platform::end_cli_output();
    if let Err(err) = crate::update::self_update(crate::update::SelfUpdateOptions::default()) {
        eprintln!("update failed: {err}");
        eprintln!("Run `bora update` to retry.");
        return Ok(1);
    }

    Ok(0)
}

fn parse_channel_set_arg(args: &[String]) -> Option<&str> {
    let channel = args.first().map(std::string::String::as_str)?;
    if args.len() == 1 && matches!(channel, "stable" | "preview") {
        Some(channel)
    } else {
        None
    }
}

fn channel_set_rejection(
    channel: &str,
    install_rejection: Option<&'static str>,
) -> Option<&'static str> {
    if channel == "preview" {
        return install_rejection;
    }

    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelSetInstallAction {
    RunSelfUpdate,
    PrintGuidance(&'static str),
}

fn channel_set_install_action(
    package_manager_guidance: Option<&'static str>,
) -> ChannelSetInstallAction {
    match package_manager_guidance {
        Some(guidance) => ChannelSetInstallAction::PrintGuidance(guidance),
        None => ChannelSetInstallAction::RunSelfUpdate,
    }
}

fn print_channel_help() {
    eprintln!("bora channel commands:");
    eprintln!("  bora channel show                            print the configured update channel");
    eprintln!("  bora channel set <stable|preview>            choose the update channel");
    eprintln!("  bora channel create <name>                   create a #channel workspace");
    eprintln!("  bora channel open <name>                     focus a #channel and repair its");
    eprintln!(
        "                                                two-pane shape (transcript + shell)"
    );
    eprintln!("  bora channel list                            list #channel workspaces");
    eprintln!(
        "  bora channel send <name> <text> [--pane ID|--current] [--to NICK] [--reply-to SEQ]"
    );
    eprintln!(
        "                                                post to a #channel and prompt its agents"
    );
    eprintln!(
        "                                                --to NICK addresses one member; fails"
    );
    eprintln!(
        "                                                loudly on an unknown or ambiguous nick"
    );
    eprintln!(
        "                                                --reply-to SEQ answers a channel.ask"
    );
    eprintln!("  bora channel note <name> <text> [--pane ID|--current]");
    eprintln!(
        "                                                append to a #channel with ZERO bells —"
    );
    eprintln!(
        "                                                no injection, never suppressed by burst"
    );
    eprintln!("  bora channel ask <name> <nick> <text> [--pane ID|--current] [--timeout MS]");
    eprintln!(
        "                                                ask one member and block for their reply"
    );
    eprintln!(
        "                                                (default timeout 300000ms, cap 600000ms)"
    );
    eprintln!("  bora channel history <name> [--lines N] [--json]");
    eprintln!("                                                print a #channel's message history");
    eprintln!("  bora channel tail <name> [--after SEQ] [--follow] [--json]");
    eprintln!(
        "                                                print messages after a seq cursor and"
    );
    eprintln!("                                                optionally follow new ones");
    eprintln!("  bora channel members <name> [--json]         list a #channel's member panes");
    eprintln!("  bora channel join <name> [--pane ID]         add a pane living outside the");
    eprintln!("                                                channel to its member set");
    eprintln!(
        "                                                [--scope-write DIR]... [--scope-read"
    );
    eprintln!("                                                DIR[,DIR]]... declare the pane's");
    eprintln!(
        "                                                write/read scope (write implies read)"
    );
    eprintln!("  bora channel leave <name> [--pane ID]        drop a joined pane from a #channel");
}

fn run_config_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(std::string::String::as_str) else {
        print_config_help();
        return Ok(2);
    };

    match subcommand {
        "check" => config_check(&args[1..]),
        "reset-keys" => config_reset_keys(&args[1..]),
        "help" | "--help" | "-h" => {
            print_config_help();
            Ok(0)
        }
        _ => {
            print_config_help();
            Ok(2)
        }
    }
}

fn config_check(args: &[String]) -> std::io::Result<i32> {
    match args {
        [] => {}
        [flag] if matches!(flag.as_str(), "help" | "--help" | "-h") => {
            eprintln!("usage: herdr config check");
            return Ok(0);
        }
        _ => {
            eprintln!("usage: herdr config check");
            return Ok(2);
        }
    }

    let diagnostics = crate::config::Config::load().diagnostics;
    if diagnostics.is_empty() {
        println!("config: ok");
    } else {
        println!("config: issues found");
        for diagnostic in &diagnostics {
            println!("{diagnostic}");
        }
    }

    Ok(i32::from(!diagnostics.is_empty()))
}

fn config_reset_keys(args: &[String]) -> std::io::Result<i32> {
    if !args.is_empty() {
        eprintln!("usage: bora config reset-keys");
        return Ok(2);
    }

    let path = crate::config::config_path();
    if !path.exists() {
        println!(
            "No config file found at {}. Built-in v2 keybindings already apply.",
            path.display()
        );
        return Ok(0);
    }

    let content = std::fs::read_to_string(&path)?;
    let parsed = match content.parse::<toml::Value>() {
        Ok(value) => value,
        Err(err) => {
            eprintln!(
                "config file at {} is invalid TOML: {err}. Fix it manually or move it aside to use defaults.",
                path.display()
            );
            return Ok(1);
        }
    };
    let Some(table) = parsed.as_table() else {
        eprintln!(
            "config file at {} is invalid TOML: top-level config must be a table.",
            path.display()
        );
        return Ok(1);
    };

    if !table.contains_key("keys") {
        println!(
            "No [keys] config found in {}. Built-in v2 keybindings already apply.",
            path.display()
        );
        return Ok(0);
    }

    let (updated, removed) = crate::config::remove_keybinding_config_sections(&content);
    if !removed {
        eprintln!(
            "could not safely remove keybinding config from {} without rewriting comments; edit the file manually or remove the top-level keys setting.",
            path.display()
        );
        return Ok(1);
    }
    if let Err(err) = updated.parse::<toml::Value>() {
        eprintln!(
            "removing keybinding config would make {} invalid TOML: {err}; leaving config unchanged",
            path.display()
        );
        return Ok(1);
    }

    let backup_path = key_config_backup_path(&path);
    std::fs::copy(&path, &backup_path)?;
    std::fs::write(&path, updated)?;

    println!("Created backup: {}", backup_path.display());
    println!(
        "Removed [keys], [keys.indexed], and [[keys.command]] from {}.",
        path.display()
    );
    println!("Built-in v2 keybindings will apply after Bora restarts or reloads config.");
    println!("If a Bora server is running, run `bora server reload-config` to apply this now.");
    println!(
        "To restore: cp {} {}",
        backup_path.display(),
        path.display()
    );
    Ok(0)
}

fn key_config_backup_path(path: &std::path::Path) -> std::path::PathBuf {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    path.with_file_name(format!("{file_name}.bak-keybind-v2-{timestamp}"))
}

fn run_terminal_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(std::string::String::as_str) else {
        print_terminal_help();
        return Ok(2);
    };

    match subcommand {
        "attach" => terminal_attach(&args[1..]),
        "session" => terminal_session(&args[1..]),
        "title" => terminal_title(&args[1..]),
        "help" | "--help" | "-h" => {
            print_terminal_help();
            Ok(0)
        }
        _ => {
            print_terminal_help();
            Ok(2)
        }
    }
}

fn run_session_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(std::string::String::as_str) else {
        print_session_help();
        return Ok(2);
    };

    match subcommand {
        "list" => session_list(&args[1..]),
        "attach" => session_attach_help(&args[1..]),
        "stop" => session_stop(&args[1..]),
        "delete" => session_delete(&args[1..]),
        "help" | "--help" | "-h" => {
            print_session_help();
            Ok(0)
        }
        _ => {
            print_session_help();
            Ok(2)
        }
    }
}

fn session_attach_help(args: &[String]) -> std::io::Result<i32> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("usage: bora session attach <name>");
        return Ok(0);
    }
    eprintln!("usage: bora session attach <name>");
    Ok(2)
}

fn session_list(args: &[String]) -> std::io::Result<i32> {
    let json = match parse_session_json_only(args, "usage: bora session list [--json]") {
        Ok(json) => json,
        Err(code) => return Ok(code),
    };

    let sessions = crate::session::list_sessions()?;
    if json {
        _print_json(&serde_json::json!({
            "sessions": sessions,
        }));
    } else {
        print_session_table(&sessions);
    }
    Ok(0)
}

fn session_stop(args: &[String]) -> std::io::Result<i32> {
    let (name, json) =
        match parse_session_name_and_json(args, "usage: bora session stop <name> [--json]") {
            Ok(parsed) => parsed,
            Err(code) => return Ok(code),
        };

    let target = match crate::session::parse_target_name(&name) {
        Ok(target) => target,
        Err(message) => {
            print_session_error("invalid_session_name", &message);
            return Ok(1);
        }
    };
    match crate::session::stop_session(target.as_deref()) {
        Ok(session) => {
            if json {
                _print_json(&serde_json::json!({
                    "stopped": true,
                    "session": session,
                }));
            } else {
                println!("stopped session {}", session.name);
            }
            Ok(0)
        }
        Err(message) => {
            print_session_error("session_stop_failed", &message);
            Ok(1)
        }
    }
}

fn session_delete(args: &[String]) -> std::io::Result<i32> {
    let (name, json) =
        match parse_session_name_and_json(args, "usage: bora session delete <name> [--json]") {
            Ok(parsed) => parsed,
            Err(code) => return Ok(code),
        };

    match crate::session::delete_session(&name) {
        Ok(session) => {
            if json {
                _print_json(&serde_json::json!({
                    "deleted": true,
                    "session": session,
                }));
            } else {
                println!("deleted session {}", session.name);
            }
            Ok(0)
        }
        Err(message) => {
            print_session_error("session_delete_failed", &message);
            Ok(1)
        }
    }
}

fn terminal_attach(args: &[String]) -> std::io::Result<i32> {
    let (terminal_id, takeover) = match parse_attach_target(
        args,
        "usage: bora terminal attach <terminal_id> [--takeover]",
    ) {
        Ok(parsed) => parsed,
        Err(code) => return Ok(code),
    };
    crate::client::run_terminal_attach(terminal_id, takeover)?;
    Ok(0)
}

fn terminal_session(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(String::as_str) {
        Some("control") => terminal_session_control(&args[1..]),
        Some("observe") => terminal_session_observe(&args[1..]),
        Some("help" | "--help" | "-h") => {
            eprintln!("{TERMINAL_SESSION_CONTROL_USAGE}");
            eprintln!("{TERMINAL_SESSION_OBSERVE_USAGE}");
            Ok(0)
        }
        _ => {
            eprintln!("{TERMINAL_SESSION_CONTROL_USAGE}");
            eprintln!("{TERMINAL_SESSION_OBSERVE_USAGE}");
            Ok(2)
        }
    }
}

fn terminal_session_control(args: &[String]) -> std::io::Result<i32> {
    let options = match parse_terminal_session_options(
        args,
        TERMINAL_SESSION_CONTROL_USAGE,
        "control",
        true,
    )? {
        Ok(options) => options,
        Err(code) => return Ok(code),
    };

    crate::client::run_terminal_session_control(
        options.target,
        options.takeover,
        options.cols,
        options.rows,
    )?;
    Ok(0)
}

fn terminal_session_observe(args: &[String]) -> std::io::Result<i32> {
    let options = match parse_terminal_session_options(
        args,
        TERMINAL_SESSION_OBSERVE_USAGE,
        "observe",
        false,
    )? {
        Ok(options) => options,
        Err(code) => return Ok(code),
    };

    crate::client::run_terminal_session_observe(options.target, options.cols, options.rows)?;
    Ok(0)
}

struct TerminalSessionOptions {
    target: String,
    cols: u16,
    rows: u16,
    takeover: bool,
}

fn parse_terminal_session_options(
    args: &[String],
    usage: &str,
    command: &str,
    allow_takeover: bool,
) -> std::io::Result<Result<TerminalSessionOptions, i32>> {
    if matches!(
        args.first().map(String::as_str),
        Some("help" | "--help" | "-h")
    ) {
        eprintln!("{usage}");
        return Ok(Err(0));
    }
    let Some(target) = args.first() else {
        eprintln!("{usage}");
        return Ok(Err(2));
    };

    let mut cols = 120;
    let mut rows = 40;
    let mut takeover = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--takeover" if allow_takeover => {
                takeover = true;
                i += 1;
            }
            "--cols" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("{usage}");
                    return Ok(Err(2));
                };
                cols = parse_terminal_dimension(value, "--cols")?;
                i += 2;
            }
            "--rows" => {
                let Some(value) = args.get(i + 1) else {
                    eprintln!("{usage}");
                    return Ok(Err(2));
                };
                rows = parse_terminal_dimension(value, "--rows")?;
                i += 2;
            }
            "help" | "--help" | "-h" => {
                eprintln!("{usage}");
                return Ok(Err(0));
            }
            other => {
                eprintln!("unknown terminal session {command} option: {other}");
                eprintln!("{usage}");
                return Ok(Err(2));
            }
        }
    }

    Ok(Ok(TerminalSessionOptions {
        target: target.clone(),
        cols,
        rows,
        takeover,
    }))
}

fn parse_terminal_dimension(raw: &str, flag: &str) -> std::io::Result<u16> {
    let parsed = raw.parse::<u16>().map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{flag} must be an integer between 1 and {}", u16::MAX),
        )
    })?;
    if parsed == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{flag} must be greater than 0"),
        ));
    }
    Ok(parsed)
}

fn terminal_title(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(std::string::String::as_str) {
        Some("set") => {
            if args.len() != 2 {
                eprintln!("usage: bora terminal title set <title>");
                return Ok(2);
            }
            print_response(&send_request(&Request {
                id: "cli:terminal:title:set".into(),
                method: Method::ClientWindowTitleSet(ClientWindowTitleSetParams {
                    title: args[1].clone(),
                }),
            })?)
        }
        Some("clear") => {
            if args.len() != 1 {
                eprintln!("usage: bora terminal title clear");
                return Ok(2);
            }
            print_response(&send_request(&Request {
                id: "cli:terminal:title:clear".into(),
                method: Method::ClientWindowTitleClear(EmptyParams::default()),
            })?)
        }
        Some("help" | "--help" | "-h") => {
            eprintln!("usage: bora terminal title set <title>");
            eprintln!("       bora terminal title clear");
            Ok(0)
        }
        _ => {
            eprintln!("usage: bora terminal title set <title>");
            eprintln!("       bora terminal title clear");
            Ok(2)
        }
    }
}

pub(super) fn parse_attach_target(args: &[String], usage: &str) -> Result<(String, bool), i32> {
    let Some(target) = args.first() else {
        eprintln!("{usage}");
        return Err(2);
    };
    let mut takeover = false;
    for arg in &args[1..] {
        match arg.as_str() {
            "--takeover" => takeover = true,
            "help" | "--help" | "-h" => {
                eprintln!("{usage}");
                return Err(0);
            }
            other => {
                eprintln!("unknown option: {other}");
                return Err(2);
            }
        }
    }
    Ok((target.clone(), takeover))
}

/// Wait until the pane hosting an agent exits (process gone). Unlike agent
/// status waits, this is the reliable "done" signal for one-shot agents whose
/// screen looks idle while they wait on a model. `events.wait` observes
/// current state atomically, so unlike the old subscription-stream client
/// there is no separate resolve/subscribe race window to guard here.
pub(super) fn wait_for_pane_exited(pane_id: &str, timeout_ms: Option<u64>) -> std::io::Result<i32> {
    let response = send_request(&Request {
        id: "cli:agent:wait".into(),
        method: Method::EventsWait(crate::api::schema::EventsWaitParams {
            match_event: crate::api::schema::EventMatch::PaneExited {
                pane_id: pane_id.to_owned(),
            },
            timeout_ms,
        }),
    })?;
    if response.get("error").is_some() {
        if response["error"]["code"].as_str() == Some("timeout") {
            eprintln!("timed out waiting for pane exit");
        } else {
            eprintln!("{}", encode_response_json(&response));
        }
        return Ok(1);
    }
    println!(
        "{}",
        serde_json::json!({
            "id": "cli:agent:wait",
            "result": { "type": "agent_wait", "pane_id": pane_id, "status": "exited" }
        })
    );
    Ok(0)
}

/// Single owner of this file's JSON re-encode: every caller passes an
/// already-decoded `serde_json::Value` (or, for `_print_json`, an owned
/// `json!` tree of plain scalars) round-tripping back to text, so
/// `serde_json::to_string` cannot fail — its only failure modes are
/// non-finite floats and non-string map keys, neither representable in a
/// `Value` built this way.
fn encode_response_json(value: &serde_json::Value) -> String {
    serde_json::to_string(value).expect(
        "serde_json::Value here always came from decoding valid JSON or from json!() of plain scalars; it cannot hold non-finite floats or non-string map keys",
    )
}

pub(super) fn print_response(response: &serde_json::Value) -> std::io::Result<i32> {
    if response.get("error").is_some() {
        eprintln!("{}", encode_response_json(response));
        return Ok(1);
    }

    println!("{}", encode_response_json(response));
    Ok(0)
}

pub(super) fn send_ok_request(method: Method) -> std::io::Result<i32> {
    let response = send_request(&Request {
        id: "cli:request".into(),
        method,
    })?;

    if response.get("error").is_some() {
        return print_response(&response);
    }

    Ok(0)
}

pub(super) fn send_request(request: &Request) -> std::io::Result<serde_json::Value> {
    let client = ApiClient::local();
    ensure_server_protocol_compatible(&client, &request.id)?;
    client
        .request_value(request)
        .map_err(|err| map_server_not_running_or_io(err, &request.id, &client))
}

pub(super) fn send_request_unchecked(request: &Request) -> std::io::Result<serde_json::Value> {
    let client = ApiClient::local();
    client
        .request_value(request)
        .map_err(|err| map_server_not_running_or_io(err, &request.id, &client))
}

fn ensure_server_protocol_compatible(client: &ApiClient, request_id: &str) -> std::io::Result<()> {
    let status = client
        .status()
        .map_err(|err| map_server_not_running_or_io(err, request_id, client))?;
    let server_protocol = status
        .protocol
        .ok_or_else(|| std::io::Error::other("server ping did not include a protocol version"))?;
    let Some(response) = protocol_guard::mismatch_response(
        request_id,
        server_protocol,
        &crate::session::active_restart_after_update_guidance(),
    ) else {
        return Ok(());
    };

    eprintln!(
        "{}",
        serde_json::to_string(&response).map_err(std::io::Error::other)?
    );
    Err(protocol_guard::reported_error())
}

pub(crate) fn protocol_mismatch_was_reported(err: &std::io::Error) -> bool {
    protocol_guard::was_reported(err)
}

pub(crate) fn server_not_running_was_reported(err: &std::io::Error) -> bool {
    server_not_running::was_reported(err)
}

/// Returns the `ErrorResponse` carried by a `server_not_running` marker, if any,
/// so the edge that surfaces the error can print it exactly once (deferred
/// printing: recovering callers like plugin offline fallback print nothing).
pub(crate) fn server_not_running_reported_response(
    err: &std::io::Error,
) -> Option<&crate::api::schema::ErrorResponse> {
    server_not_running::reported_response(err)
}

/// True when an io::Error indicates nothing is listening on the API socket.
/// Classify by `ErrorKind` only: Windows named pipes surface different raw
/// errno values than Unix domain sockets but the same error kinds.
pub(super) fn server_not_running_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

/// Maps an `ApiClientError` from a socket command into the io::Error that
/// bubbles up to `main`. A dead-server connect failure is reported as a
/// friendly `server_not_running` JSON error plus a recognizable marker; all
/// other errors fall through unchanged so existing handling is preserved.
fn map_server_not_running_or_io(
    err: ApiClientError,
    request_id: &str,
    client: &ApiClient,
) -> std::io::Error {
    match err {
        ApiClientError::Io(io_err) if server_not_running_error(&io_err) => {
            server_not_running::reported_error(server_not_running::response(
                request_id,
                &client.socket_path(),
            ))
        }
        err => api_client_error_to_io(err),
    }
}

fn api_client_error_to_io(err: ApiClientError) -> std::io::Error {
    match err {
        ApiClientError::Io(err) => err,
        err => std::io::Error::other(err),
    }
}

pub(super) fn normalize_workspace_id(value: &str) -> String {
    value.to_string()
}

pub(super) fn normalize_tab_id(value: &str) -> String {
    value.to_string()
}

pub(super) fn normalize_pane_id(value: &str) -> String {
    value.to_string()
}

pub(super) fn parse_split_direction(value: &str) -> std::io::Result<SplitDirection> {
    match value {
        "right" => Ok(SplitDirection::Right),
        "down" => Ok(SplitDirection::Down),
        _ => Err(std::io::Error::other(format!(
            "invalid split direction: {value}"
        ))),
    }
}

pub(super) fn parse_read_source(value: &str) -> std::io::Result<ReadSource> {
    match value {
        "visible" => Ok(ReadSource::Visible),
        "recent" => Ok(ReadSource::Recent),
        "recent-unwrapped" | "recent_unwrapped" => Ok(ReadSource::RecentUnwrapped),
        "detection" => Ok(ReadSource::Detection),
        _ => Err(std::io::Error::other(format!(
            "invalid read source: {value}"
        ))),
    }
}

pub(super) fn parse_read_format(value: &str) -> std::io::Result<ReadFormat> {
    match value {
        "text" => Ok(ReadFormat::Text),
        "ansi" => Ok(ReadFormat::Ansi),
        _ => Err(std::io::Error::other(format!(
            "invalid read format: {value}"
        ))),
    }
}

fn parse_agent_status(value: &str) -> std::io::Result<AgentStatus> {
    match value {
        "idle" => Ok(AgentStatus::Idle),
        "working" => Ok(AgentStatus::Working),
        "blocked" => Ok(AgentStatus::Blocked),
        "done" => Ok(AgentStatus::Done),
        "unknown" => Ok(AgentStatus::Unknown),
        _ => Err(std::io::Error::other(format!(
            "invalid agent status: {value} (expected idle, working, blocked, done, or unknown)"
        ))),
    }
}

pub(super) fn parse_pane_agent_state(value: &str) -> std::io::Result<PaneAgentState> {
    match value {
        "idle" => Ok(PaneAgentState::Idle),
        "working" => Ok(PaneAgentState::Working),
        "blocked" => Ok(PaneAgentState::Blocked),
        "unknown" => Ok(PaneAgentState::Unknown),
        _ => Err(std::io::Error::other(format!(
            "invalid pane agent state: {value} (expected idle, working, blocked, or unknown)"
        ))),
    }
}

pub(super) fn parse_u32_flag(flag: &str, value: &str) -> std::io::Result<u32> {
    value
        .parse::<u32>()
        .map_err(|_| std::io::Error::other(format!("invalid value for {flag}: {value}")))
}

pub(super) fn parse_u64_flag(flag: &str, value: &str) -> std::io::Result<u64> {
    value
        .parse::<u64>()
        .map_err(|_| std::io::Error::other(format!("invalid value for {flag}: {value}")))
}

/// Expand `--flag=value` tokens into separate `--flag` and `value` tokens so
/// the hand-rolled subcommand parsers accept the same `--flag=value` form the
/// clap-generated help and completions imply. Only `value_options` are split:
/// boolean and unknown options keep their attached value so they still reach
/// the parser's unknown-option branch.
pub(super) fn expand_equals_args(args: &[String], value_options: &[&str]) -> Vec<String> {
    let mut expanded = Vec::with_capacity(args.len());
    for arg in args {
        match arg.split_once('=') {
            Some((flag, value)) if value_options.contains(&flag) => {
                expanded.push(flag.to_string());
                expanded.push(value.to_string());
            }
            _ => expanded.push(arg.clone()),
        }
    }
    expanded
}

fn parse_session_json_only(args: &[String], usage: &str) -> Result<bool, i32> {
    match args {
        [] => Ok(false),
        [flag] if flag == "--json" => Ok(true),
        _ => {
            eprintln!("{usage}");
            Err(2)
        }
    }
}

fn parse_session_name_and_json(args: &[String], usage: &str) -> Result<(String, bool), i32> {
    let mut name = None;
    let mut json = false;
    for arg in args {
        if arg == "--json" {
            json = true;
        } else if name.is_none() {
            name = Some(arg.clone());
        } else {
            eprintln!("{usage}");
            return Err(2);
        }
    }

    let Some(name) = name else {
        eprintln!("{usage}");
        return Err(2);
    };
    Ok((name, json))
}

fn print_session_table(sessions: &[crate::session::SessionInfo]) {
    println!("{:<20} {:<8} {:<48} socket", "name", "status", "directory");
    for session in sessions {
        println!(
            "{:<20} {:<8} {:<48} {}",
            session.name,
            if session.running {
                "running"
            } else {
                "stopped"
            },
            session.session_dir,
            session.socket_path
        );
    }
}

fn print_session_error(code: &str, message: &str) {
    eprintln!(
        "{}",
        encode_response_json(&serde_json::json!({
            "error": {
                "code": code,
                "message": message,
            }
        }))
    );
}

fn print_config_help() {
    eprintln!("bora config commands:");
    eprintln!("  bora config check  validate config.toml and print diagnostics");
    eprintln!("  bora config reset-keys  back up config.toml and remove custom keybindings");
}

fn print_terminal_help() {
    eprintln!("bora terminal commands:");
    eprintln!("  bora terminal attach <terminal_id> [--takeover]");
    eprintln!("  bora terminal session control <target> [--takeover] [--cols N] [--rows N]");
    eprintln!("  bora terminal session observe <target> [--cols N] [--rows N]");
    eprintln!("  bora terminal title set <title>");
    eprintln!("  bora terminal title clear");
    eprintln!("  detach from direct attach with ctrl+b q; send literal ctrl+b with ctrl+b ctrl+b");
}

fn print_session_help() {
    eprintln!("bora session commands:");
    eprintln!("  bora session list [--json]");
    eprintln!("  bora session attach <name>");
    eprintln!("  bora session stop <name> [--json]");
    eprintln!("  bora session delete <name> [--json]");
    eprintln!("  use 'default' as <name> to target the default session for stop");
}

fn _print_json(value: &serde_json::Value) {
    println!("{}", encode_response_json(value));
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_channel_send_to_flag() {
        let args = vec!["--to".to_string(), "worker".to_string()];
        let (from_pane, to, in_reply_to) = super::parse_channel_send_flags(&args, None).unwrap();
        assert_eq!(from_pane, None);
        assert_eq!(to, Some("worker".to_string()));
        assert_eq!(in_reply_to, None);
    }

    #[test]
    fn channel_send_flags_default_to_none_without_to() {
        let args = vec!["--current".to_string()];
        let (from_pane, to, in_reply_to) =
            super::parse_channel_send_flags(&args, Some("w1A:p2".to_string())).unwrap();
        assert_eq!(from_pane, Some("w1A:p2".to_string()));
        assert_eq!(to, None);
        assert_eq!(in_reply_to, None);
    }

    #[test]
    fn channel_send_to_flag_requires_value() {
        let args = vec!["--to".to_string()];
        assert_eq!(
            super::parse_channel_send_flags(&args, None).unwrap_err(),
            "missing value for --to"
        );
    }

    #[test]
    fn channel_send_flags_combine_pane_and_to() {
        let args = vec![
            "--pane".to_string(),
            "w1A:p2".to_string(),
            "--to".to_string(),
            "reviewer".to_string(),
        ];
        let (from_pane, to, in_reply_to) = super::parse_channel_send_flags(&args, None).unwrap();
        assert_eq!(from_pane, Some("w1A:p2".to_string()));
        assert_eq!(to, Some("reviewer".to_string()));
        assert_eq!(in_reply_to, None);
    }

    #[test]
    fn channel_send_flags_parse_reply_to() {
        let args = vec!["--reply-to".to_string(), "7".to_string()];
        let (from_pane, to, in_reply_to) = super::parse_channel_send_flags(&args, None).unwrap();
        assert_eq!(from_pane, None);
        assert_eq!(to, None);
        assert_eq!(in_reply_to, Some(7));
    }

    #[test]
    fn channel_send_flags_reply_to_rejects_non_numeric() {
        let args = vec!["--reply-to".to_string(), "nope".to_string()];
        assert_eq!(
            super::parse_channel_send_flags(&args, None).unwrap_err(),
            "invalid --reply-to value: nope"
        );
    }

    #[test]
    fn parses_channel_set_argument() {
        assert_eq!(
            super::parse_channel_set_arg(&["preview".to_string()]),
            Some("preview")
        );
        assert_eq!(
            super::parse_channel_set_arg(&["stable".to_string()]),
            Some("stable")
        );
        assert_eq!(super::parse_channel_set_arg(&["nightly".to_string()]), None);
        assert_eq!(
            super::parse_channel_set_arg(&["preview".to_string(), "stable".to_string()]),
            None
        );
    }

    #[test]
    fn channel_set_only_applies_package_rejection_to_preview() {
        assert_eq!(
            super::channel_set_rejection("preview", Some("no preview")),
            Some("no preview")
        );
        assert_eq!(
            super::channel_set_rejection("stable", Some("no preview")),
            None
        );
        assert_eq!(super::channel_set_rejection("preview", None), None);
    }

    #[test]
    fn channel_set_skips_self_update_for_package_manager_guidance() {
        assert_eq!(
            super::channel_set_install_action(Some("use package manager")),
            super::ChannelSetInstallAction::PrintGuidance("use package manager")
        );
        assert_eq!(
            super::channel_set_install_action(None),
            super::ChannelSetInstallAction::RunSelfUpdate
        );
    }

    #[test]
    fn parse_env_assignment_accepts_empty_values() {
        assert_eq!(
            super::parse_env_assignment("HERDR_ROLE=").unwrap(),
            ("HERDR_ROLE".to_string(), String::new())
        );
    }

    #[test]
    fn parse_env_assignment_requires_key_value_separator() {
        assert_eq!(
            super::parse_env_assignment("HERDR_ROLE").unwrap_err(),
            "env must use KEY=VALUE"
        );
    }

    #[test]
    fn maps_dead_server_connect_failure_to_friendly_error() {
        use crate::api::client::{ApiClient, ApiClientError};

        let client = ApiClient::local();
        let socket = client.socket_path().display().to_string();

        // The helper does NOT print; it returns a recognizable marker carrying
        // the ErrorResponse so the surfacing edge can print it exactly once.
        let mapped = super::map_server_not_running_or_io(
            ApiClientError::Io(std::io::Error::from(std::io::ErrorKind::NotFound)),
            "cli:workspace:create",
            &client,
        );

        let response = super::server_not_running::reported_response(&mapped)
            .expect("dead-server connect failure should carry a server_not_running response");
        assert_eq!(response.id, "cli:workspace:create");
        assert_eq!(response.error.code, "server_not_running");
        assert!(response.error.message.contains(&socket));

        // The mapping is recognizable without string matching.
        assert!(super::server_not_running::was_reported(&mapped));
    }

    #[test]
    fn classifier_ignores_unrelated_io_kinds() {
        use crate::api::client::{ApiClient, ApiClientError};

        let client = ApiClient::local();
        let mapped = super::map_server_not_running_or_io(
            ApiClientError::Io(std::io::Error::from(std::io::ErrorKind::TimedOut)),
            "cli:workspace:create",
            &client,
        );
        assert!(!super::server_not_running::was_reported(&mapped));
    }

    #[test]
    fn expand_equals_args_splits_value_options_only() {
        // Known value options split; values may contain `=`. Boolean and
        // unknown options keep the attached form so parsers still reject them.
        let args = vec![
            "--match=a=b".to_string(),
            "name=value".to_string(),
            "--raw=value".to_string(),
            "--bogus=value".to_string(),
            "--timeout=5000".to_string(),
        ];
        assert_eq!(
            super::expand_equals_args(&args, &["--match", "--timeout"]),
            vec![
                "--match",
                "a=b",
                "name=value",
                "--raw=value",
                "--bogus=value",
                "--timeout",
                "5000",
            ]
        );
    }
}
