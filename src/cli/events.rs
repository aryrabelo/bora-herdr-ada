use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::api::client::{ApiClient, ApiClientError, ConnectionTarget};
use crate::api::schema::{EventsSubscribeParams, Method, Request, Subscription};

/// Every `Subscription` variant whose wire form carries no parameters; these
/// are what `bora events` subscribes to without `--subscribe`. The three
/// pane-scoped variants (`pane.output_matched`, `pane.agent_status_changed`,
/// `pane.scroll_changed`) are deliberately absent: they need `--pane`.
const DEFAULT_EVENT_NAMES: &[&str] = &[
    "workspace.created",
    "workspace.updated",
    "workspace.metadata_updated",
    "workspace.renamed",
    "workspace.moved",
    "workspace.reordered",
    "workspace.closed",
    "workspace.focused",
    "worktree.created",
    "worktree.opened",
    "worktree.removed",
    "tab.created",
    "tab.closed",
    "tab.focused",
    "tab.renamed",
    "tab.moved",
    "pane.created",
    "pane.closed",
    "pane.updated",
    "pane.focused",
    "pane.moved",
    "pane.exited",
    "pane.agent_detected",
    "pane.result_reported",
    "layout.updated",
    "github.prs_refreshed",
    "github.pr_opened",
    "github.issues_refreshed",
    "todo.changed",
    "scratchpad.changed",
];

const EVENTS_USAGE: &str =
    "usage: bora events [--follow] [--subscribe <name>]... [--pane <id>] [--limit <n>] [--session <name>]";
#[derive(Default, Debug)]
struct EventsArgs {
    subscriptions: Vec<String>,
    pane_id: Option<String>,
    limit: Option<u32>,
    session: Option<String>,
}

pub(super) fn run_events_command(args: &[String]) -> std::io::Result<i32> {
    if matches!(args.first().map(String::as_str), Some("help")) {
        print_events_help();
        return Ok(0);
    }

    let expanded =
        super::expand_equals_args(args, &["--subscribe", "--pane", "--limit", "--session"]);
    let parsed = match parse_events_args(&expanded) {
        Ok(parsed) => parsed,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{EVENTS_USAGE}");
            return Ok(2);
        }
    };

    if let Some(message) =
        pane_without_subscribe_error(parsed.pane_id.as_deref(), &parsed.subscriptions)
    {
        eprintln!("{message}");
        eprintln!("{EVENTS_USAGE}");
        return Ok(2);
    }

    let subscriptions = match build_subscriptions(&parsed.subscriptions, parsed.pane_id.as_deref())
    {
        Ok(subscriptions) => subscriptions,
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{EVENTS_USAGE}");
            return Ok(2);
        }
    };

    if parsed.limit == Some(0) {
        return Ok(0);
    }

    let client = match parsed.session.as_deref() {
        Some(name) => ApiClient::for_target(ConnectionTarget::LocalSession(Some(name.to_string()))),
        None => ApiClient::local(),
    };
    super::ensure_server_protocol_compatible(&client, "cli:events:subscribe")?;

    let request = Request {
        id: "cli:events:subscribe".into(),
        method: Method::EventsSubscribe(EventsSubscribeParams { subscriptions }),
    };
    let (_ack, mut stream) = match client.open_stream(&request) {
        Ok(opened) => opened,
        // A server-side rejection (e.g. a `--pane` id the server cannot
        // resolve) carries a structured ErrorResponse: print its JSON like
        // every other verb instead of leaking Rust Debug formatting.
        Err(ApiClientError::ErrorResponse(response)) => {
            eprintln!(
                "{}",
                serde_json::to_string(&response).map_err(std::io::Error::other)?
            );
            return Ok(1);
        }
        Err(err) => {
            return Err(super::map_server_not_running_or_io(
                err,
                "cli:events:subscribe",
                &client,
            ))
        }
    };

    install_sigint_exit0();

    crate::platform::begin_cli_output();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut delivered: u32 = 0;
    loop {
        if parsed.limit.is_some_and(|limit| delivered >= limit) {
            return Ok(0);
        }
        match stream.next_value() {
            Ok(event) => {
                write_event_line(&mut out, &event)?;
                delivered += 1;
            }
            Err(ApiClientError::EmptyResponse) => {
                eprintln!("bora events: server closed the event stream");
                return Ok(1);
            }
            Err(err) => return Err(std::io::Error::other(err)),
        }
    }
}

fn parse_events_args(args: &[String]) -> Result<EventsArgs, String> {
    let mut parsed = EventsArgs::default();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            // Streaming is the default; `--follow` is accepted so the verb
            // reads the same as the ticket that names it.
            "--follow" => index += 1,
            "--subscribe" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --subscribe".into());
                };
                parsed.subscriptions.push(value.clone());
                index += 2;
            }
            "--pane" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --pane".into());
                };
                parsed.pane_id = Some(super::normalize_pane_id(value));
                index += 2;
            }
            "--limit" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --limit".into());
                };
                parsed.limit = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid value for --limit: {value}"))?,
                );
                index += 2;
            }
            // A global `--session` never reaches this parser (session.rs
            // strips it while resolving the active session); the arm keeps
            // the parser total and the help honest if that ever changes.
            "--session" => {
                let Some(value) = args.get(index + 1) else {
                    return Err("missing value for --session".into());
                };
                parsed.session = Some(value.clone());
                index += 2;
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    Ok(parsed)
}

/// Rejects `--pane` given without `--subscribe`: the default subscriptions
/// are not pane-scoped, so the flag would be silently ignored. Returns the
/// named error message when the guard trips.
fn pane_without_subscribe_error(pane_id: Option<&str>, subscriptions: &[String]) -> Option<String> {
    if pane_id.is_some() && subscriptions.is_empty() {
        Some(
            "--pane requires --subscribe <name>: the default subscriptions are not pane-scoped"
                .to_string(),
        )
    } else {
        None
    }
}

/// Maps a wire event name to its `Subscription`. The 30 parameterless
/// variants map directly; the pane-scoped three require `--pane` (and
/// `pane.output_matched` additionally needs a match expression this verb
/// does not expose).
fn subscription_for_name(name: &str, pane_id: Option<&str>) -> Result<Subscription, String> {
    if matches!(
        name,
        "pane.output_matched" | "pane.agent_status_changed" | "pane.scroll_changed"
    ) {
        let Some(pane_id) = pane_id else {
            return Err(format!("{name} requires --pane <id>"));
        };
        return match name {
            "pane.agent_status_changed" => Ok(Subscription::PaneAgentStatusChanged {
                pane_id: pane_id.to_string(),
                agent_status: None,
            }),
            "pane.scroll_changed" => Ok(Subscription::PaneScrollChanged {
                pane_id: pane_id.to_string(),
            }),
            _ => Err(
                "pane.output_matched needs a match expression that `bora events` does not \
                 expose; subscribe through the events.subscribe API instead"
                    .into(),
            ),
        };
    }

    let parameterless = match name {
        "workspace.created" => Subscription::WorkspaceCreated {},
        "workspace.updated" => Subscription::WorkspaceUpdated {},
        "workspace.metadata_updated" => Subscription::WorkspaceMetadataUpdated {},
        "workspace.renamed" => Subscription::WorkspaceRenamed {},
        "workspace.moved" => Subscription::WorkspaceMoved {},
        "workspace.reordered" => Subscription::WorkspaceReordered {},
        "workspace.closed" => Subscription::WorkspaceClosed {},
        "workspace.focused" => Subscription::WorkspaceFocused {},
        "worktree.created" => Subscription::WorktreeCreated {},
        "worktree.opened" => Subscription::WorktreeOpened {},
        "worktree.removed" => Subscription::WorktreeRemoved {},
        "tab.created" => Subscription::TabCreated {},
        "tab.closed" => Subscription::TabClosed {},
        "tab.focused" => Subscription::TabFocused {},
        "tab.renamed" => Subscription::TabRenamed {},
        "tab.moved" => Subscription::TabMoved {},
        "pane.created" => Subscription::PaneCreated {},
        "pane.closed" => Subscription::PaneClosed {},
        "pane.updated" => Subscription::PaneUpdated {},
        "pane.focused" => Subscription::PaneFocused {},
        "pane.moved" => Subscription::PaneMoved {},
        "pane.exited" => Subscription::PaneExited {},
        "pane.agent_detected" => Subscription::PaneAgentDetected {},
        "pane.result_reported" => Subscription::PaneResultReported {},
        "layout.updated" => Subscription::LayoutUpdated {},
        "github.prs_refreshed" => Subscription::GithubPrsRefreshed {},
        "github.pr_opened" => Subscription::GithubPrOpened {},
        "github.issues_refreshed" => Subscription::GithubIssuesRefreshed {},
        "todo.changed" => Subscription::TodoChanged {},
        "scratchpad.changed" => Subscription::ScratchpadChanged {},
        other => return Err(format!("unknown event: {other}")),
    };
    Ok(parameterless)
}

fn build_subscriptions(
    requested: &[String],
    pane_id: Option<&str>,
) -> Result<Vec<Subscription>, String> {
    if requested.is_empty() {
        DEFAULT_EVENT_NAMES
            .iter()
            .map(|name| subscription_for_name(name, None))
            .collect()
    } else {
        requested
            .iter()
            .map(|name| subscription_for_name(name, pane_id))
            .collect()
    }
}

/// Writes one event as a single JSON line and flushes immediately: a
/// `bora events` consumer must see each event the moment it arrives, not
/// when an output buffer happens to fill.
fn write_event_line(out: &mut impl Write, event: &serde_json::Value) -> std::io::Result<()> {
    let line = serde_json::to_string(event).map_err(std::io::Error::other)?;
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()
}

/// SIGINT must exit 0, but the event loop blocks in a socket read the
/// handler flag alone cannot interrupt, so a watcher thread performs the
/// exit. ponytail: 100 ms side-thread poll; swap to a socket recv timeout
/// if exit latency ever matters.
fn install_sigint_exit0() {
    let interrupted = Arc::new(AtomicBool::new(false));
    let handler_flag = interrupted.clone();
    if let Err(err) = ctrlc::set_handler(move || {
        handler_flag.store(true, Ordering::Release);
    }) {
        tracing::warn!(
            %err,
            "failed to install SIGINT handler; Ctrl-C keeps the default disposition"
        );
        return;
    }
    let watcher_flag = interrupted;
    std::thread::spawn(move || {
        while !watcher_flag.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(100));
        }
        std::process::exit(0);
    });
}

fn print_events_help() {
    eprintln!("{EVENTS_USAGE}");
    eprintln!();
    eprintln!("Streams session events as one JSON object per line on stdout, flushed per line.");
    eprintln!("Without --subscribe, every event that needs no --pane is subscribed.");
    eprintln!();
    eprintln!("  --follow              accepted for compatibility; streaming is the default");
    eprintln!("  --subscribe <name>    event name to subscribe to (repeatable, e.g. pane.created)");
    eprintln!("  --pane <id>           pane id for the pane-scoped subscriptions");
    eprintln!("  --limit <n>           exit successfully after n events");
    eprintln!("  --session <name>      stream from a named session instead of the default one");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_default_names_map_to_parameterless_wire_variants() {
        assert_eq!(DEFAULT_EVENT_NAMES.len(), 30);
        for name in DEFAULT_EVENT_NAMES {
            let subscription = subscription_for_name(name, None)
                .unwrap_or_else(|err| panic!("{name} must map without --pane: {err}"));
            let wire = serde_json::to_value(&subscription).unwrap();
            assert_eq!(
                wire,
                serde_json::json!({ "type": name }),
                "wire name drift for {name}"
            );
        }
        for pane_scoped in [
            "pane.output_matched",
            "pane.agent_status_changed",
            "pane.scroll_changed",
        ] {
            assert!(
                !DEFAULT_EVENT_NAMES.contains(&pane_scoped),
                "{pane_scoped} must stay out of the default set"
            );
        }
    }

    #[test]
    fn events_pane_scoped_names_require_the_pane_flag() {
        for name in [
            "pane.output_matched",
            "pane.agent_status_changed",
            "pane.scroll_changed",
        ] {
            let message =
                subscription_for_name(name, None).expect_err("pane-scoped names require --pane");
            assert!(
                message.contains("--pane"),
                "error must name the missing flag: {message}"
            );
        }
        let status = subscription_for_name("pane.agent_status_changed", Some("p1")).unwrap();
        assert_eq!(
            serde_json::to_value(&status).unwrap(),
            serde_json::json!({"type": "pane.agent_status_changed", "pane_id": "p1"})
        );
        let scroll = subscription_for_name("pane.scroll_changed", Some("p1")).unwrap();
        assert_eq!(
            serde_json::to_value(&scroll).unwrap(),
            serde_json::json!({"type": "pane.scroll_changed", "pane_id": "p1"})
        );
    }

    #[test]
    fn events_unknown_names_are_named_errors() {
        let message =
            subscription_for_name("pane.creat", None).expect_err("unknown name must be named");
        assert!(message.contains("unknown event"), "got: {message}");
    }

    #[test]
    fn events_line_writer_flushes_per_line() {
        let mut out = FlushCountingWriter::default();
        write_event_line(&mut out, &serde_json::json!({"type": "pane.created"})).unwrap();
        assert_eq!(out.flushes, 1, "each event line must flush exactly once");
        write_event_line(&mut out, &serde_json::json!({"type": "pane.closed"})).unwrap();
        assert_eq!(out.flushes, 2);
        let text = String::from_utf8(out.bytes).unwrap();
        assert_eq!(
            text,
            "{\"type\":\"pane.created\"}\n{\"type\":\"pane.closed\"}\n"
        );
    }

    #[test]
    fn events_args_parse_repeatable_subscribe_and_limit() {
        let parsed = parse_events_args(&[
            "--follow".into(),
            "--subscribe".into(),
            "pane.created".into(),
            "--subscribe".into(),
            "tab.created".into(),
            "--limit".into(),
            "5".into(),
            "--pane".into(),
            "p9".into(),
        ])
        .unwrap();
        assert_eq!(parsed.subscriptions, ["pane.created", "tab.created"]);
        assert_eq!(parsed.limit, Some(5));
        assert_eq!(parsed.pane_id.as_deref(), Some("p9"));
        let message = parse_events_args(&["--limit".into(), "lots".into()])
            .expect_err("non-numeric limit must be a named error");
        assert!(message.contains("--limit"), "got: {message}");
    }

    #[test]
    fn events_pane_without_subscribe_is_a_named_error() {
        let message = pane_without_subscribe_error(Some("p1"), &[]).expect("guard must trip");
        assert!(
            message.contains("--pane"),
            "must name the given flag: {message}"
        );
        assert!(
            message.contains("--subscribe"),
            "must name the missing flag: {message}"
        );
        assert!(
            pane_without_subscribe_error(None, &[]).is_none(),
            "no --pane given: the guard must stay quiet"
        );
        assert!(
            pane_without_subscribe_error(Some("p1"), &["pane.scroll_changed".into()]).is_none(),
            "--pane with --subscribe is legitimate"
        );
    }

    #[test]
    fn events_build_subscriptions_applies_the_pane_flag_to_requested_names() {
        let requested = build_subscriptions(&["pane.scroll_changed".into()], Some("p1")).unwrap();
        assert_eq!(
            serde_json::to_value(&requested).unwrap(),
            serde_json::json!([{ "type": "pane.scroll_changed", "pane_id": "p1" }])
        );
        let defaults = build_subscriptions(&[], Some("p1")).unwrap();
        assert_eq!(defaults.len(), 30);
    }

    #[derive(Default)]
    struct FlushCountingWriter {
        bytes: Vec<u8>,
        flushes: usize,
    }

    impl std::io::Write for FlushCountingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            Ok(())
        }
    }
}
