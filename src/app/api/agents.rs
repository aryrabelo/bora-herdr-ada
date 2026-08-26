use std::time::{Duration, Instant};

use bytes::Bytes;

use crate::api::schema::{
    AgentPromptOutcome, AgentPromptParams, AgentRenameParams, AgentSendKeysParams,
    AgentStartParams, AgentTarget, EventData, EventEnvelope, EventKind, PaneReadResult,
    ResponseResult, AGENT_PROMPTED_TEXT_LIMIT,
};
use crate::app::App;

use super::responses::{encode_error, encode_error_body, encode_success};

const AGENT_PROMPT_SUBMIT_DELAY: Duration = Duration::from_millis(300);

impl App {
    pub(super) fn handle_agent_list(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::AgentList {
                agents: self.collect_agent_infos(),
            },
        )
    }

    pub(super) fn handle_agent_get(&mut self, id: String, target: AgentTarget) -> String {
        self.reconcile_managed_agent_target(&target.target);
        let agent = match self.agent_info_for_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_focus(&mut self, id: String, target: AgentTarget) -> String {
        let agent = match self.focus_agent_target(&target.target) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_rename(&mut self, id: String, params: AgentRenameParams) -> String {
        let agent = match self.rename_agent_target(&params.target, params.name) {
            Ok(agent) => agent,
            Err(err) => return encode_error_body(id, self.agent_rename_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentInfo { agent })
    }

    pub(super) fn handle_agent_start(&mut self, id: String, params: AgentStartParams) -> String {
        let (agent, argv) = match self.start_agent(params) {
            Ok(started) => started,
            Err(err) => return encode_error_body(id, self.agent_start_error_body(err)),
        };

        encode_success(id, ResponseResult::AgentStarted { agent, argv })
    }

    pub(in crate::app) fn handle_agent_prompt(
        &mut self,
        id: String,
        mut params: AgentPromptParams,
    ) -> String {
        if params.text.is_empty() {
            return encode_error(id, "empty_agent_prompt", "agent prompt must not be empty");
        }
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        if let Some(agent) = self.agent_info(resolved.ws_idx, resolved.pane_id) {
            if agent_prompt_should_reject_busy(params.when_idle, agent.agent_status) {
                let target_pane = self
                    .public_pane_id(resolved.ws_idx, resolved.pane_id)
                    .unwrap_or_else(|| params.target.clone());
                let (queue_position, queue_id) =
                    self.enqueue_pending_agent_prompt(target_pane, params.clone());
                return encode_success(
                    id,
                    ResponseResult::AgentPrompted {
                        agent,
                        outcome: AgentPromptOutcome::Deferred,
                        queue_position: Some(queue_position),
                        queue_id: Some(queue_id),
                    },
                );
            }
        }
        if let Some(from_pane) = params.from_pane.as_deref() {
            let target_pane = self
                .public_pane_id(resolved.ws_idx, resolved.pane_id)
                .unwrap_or_else(|| params.target.clone());
            if let Err(remaining) =
                self.check_agent_prompt_rate_limit(from_pane, &target_pane, Instant::now())
            {
                return encode_error(
                    id,
                    "agent_prompt_rate_limited",
                    format!(
                        "agent prompt from {from_pane} to {target_pane} is rate-limited; retry in {}ms",
                        remaining.as_millis()
                    ),
                );
            }
        }
        if let Some(from_pane) = params.from_pane.clone() {
            let prefix = self.agent_prompt_from_prefix(&from_pane, params.peer_pid);
            params.text = format!("{prefix}{}", params.text);
        }
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
            .cloned()
        else {
            return agent_not_found(id, &params.target);
        };
        let Some(terminal) = self.state.terminals.get(&terminal_id) else {
            return agent_not_found(id, &params.target);
        };
        if terminal.state == crate::detect::AgentState::Blocked {
            return encode_error(
                id,
                "agent_blocked",
                format!(
                    "agent {} is blocked and requires interactive input",
                    params.target
                ),
            );
        }
        let Some(expected_agent) = terminal.effective_known_agent() else {
            return agent_not_ready(id, &params.target);
        };
        if terminal.managed_agent_launch_pending() {
            return agent_not_ready(id, &params.target);
        }
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &params.target);
        };
        if !super::super::agents::runtime_hosts_agent(runtime, expected_agent) {
            return encode_error(
                id,
                "agent_not_ready",
                format!(
                    "agent {} is no longer the pane foreground process",
                    params.target
                ),
            );
        }
        if expected_agent == crate::detect::Agent::GithubCopilot {
            // Copilot ignores synthetic Enter after focus loss until it receives focus gained.
            let focus = match crate::ghostty::encode_focus(crate::ghostty::FocusEvent::Gained) {
                Ok(focus) => focus,
                Err(err) => return encode_error(id, "agent_prompt_failed", err.to_string()),
            };
            if let Err(err) = runtime.try_send_bytes(Bytes::from(focus)) {
                return encode_error(id, "agent_prompt_failed", err.to_string());
            }
        }
        // `params.text` already carries the `[from ...]`/`[from? claimed ...]` prefix
        // (applied above via `agent_prompt_from_prefix`) when `from_pane` was set, so
        // the announced text used for injection/event reporting is just the text as-is.
        let announced_text = params.text.clone();
        let (text, enter) =
            crate::app::api_helpers::encode_api_submission_parts(runtime, &announced_text);
        if let Err(err) = runtime.try_send_bytes(Bytes::from(text)) {
            return encode_error(id, "agent_prompt_failed", err.to_string());
        }
        runtime.send_bytes_after(Bytes::from(enter), AGENT_PROMPT_SUBMIT_DELAY);
        let to_pane_id = self.public_pane_id(resolved.ws_idx, resolved.pane_id);
        let to_workspace_id = self.public_workspace_id(resolved.ws_idx);
        if let Some(to_pane_id) = to_pane_id {
            let (text, text_truncated) = truncate_agent_prompt_text(&announced_text);
            self.emit_event(EventEnvelope {
                event: EventKind::AgentPrompted,
                data: EventData::AgentPrompted {
                    from_pane_id: params.from_pane.clone(),
                    to_pane_id,
                    to_workspace_id,
                    text,
                    text_truncated,
                    text_len: announced_text.len(),
                },
            });
        }
        let Some(agent) = self.agent_info(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &params.target);
        };
        encode_success(
            id,
            ResponseResult::AgentPrompted {
                agent,
                outcome: AgentPromptOutcome::Injected,
                queue_position: None,
                queue_id: None,
            },
        )
    }

    /// Resolves a `from_pane` identifier into a `[from <public_pane_id> <display_name>] `
    /// prefix when the caller's OS-level peer PID (`AgentPromptParams::peer_pid`, set by
    /// the connection accept loop, never by the client) is confirmed to descend from that
    /// pane's shell process. Otherwise returns `[from? claimed <raw_from>] `: the claim is
    /// still delivered, just flagged as unverified so the receiving agent can weigh it
    /// accordingly. Never fails the prompt.
    fn agent_prompt_from_prefix(&self, raw_from: &str, peer_pid: Option<u32>) -> String {
        let Some((ws_idx, pane_id)) = self.parse_pane_id(raw_from) else {
            tracing::debug!(
                from_pane = raw_from,
                "agent_prompt: could not resolve from_pane, using raw id"
            );
            return format!("[from? claimed {raw_from}] ");
        };
        if !self.sender_pane_identity_verified(ws_idx, pane_id, peer_pid) {
            tracing::debug!(
                from_pane = raw_from,
                ?peer_pid,
                "agent_prompt: from_pane identity unverified"
            );
            return format!("[from? claimed {raw_from}] ");
        }
        let public_id = self
            .public_pane_id(ws_idx, pane_id)
            .unwrap_or_else(|| raw_from.to_string());
        let display_name = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.custom_name.clone())
            .or_else(|| {
                self.state
                    .workspaces
                    .get(ws_idx)
                    .and_then(|ws| ws.terminal_id(pane_id))
                    .and_then(|terminal_id| self.state.terminals.get(terminal_id))
                    .and_then(|terminal| terminal.effective_agent_label())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "unknown".into());
        format!("[from {public_id} {display_name}] ")
    }

    /// True only when `peer_pid` is the claimed pane's shell process itself, or a
    /// descendant of it in the OS process tree. `None` (no OS peer credentials, e.g. an
    /// unsupported platform) or a pane with no live shell always fails closed to
    /// unverified — never treated as an implicit pass.
    fn sender_pane_identity_verified(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        peer_pid: Option<u32>,
    ) -> bool {
        let Some(peer_pid) = peer_pid else {
            return false;
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return false;
        };
        let Some(shell_pid) = runtime.child_pid() else {
            return false;
        };
        crate::platform::pid_is_descendant_of(shell_pid, peer_pid)
    }

    pub(super) fn handle_agent_read(
        &mut self,
        id: String,
        params: crate::api::schema::AgentReadParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some((pane, workspace_id)) = self.lookup_runtime(resolved.ws_idx, resolved.pane_id)
        else {
            return agent_not_found(id, &params.target);
        };
        let snapshot = crate::app::api_helpers::read_terminal_snapshot(
            pane,
            params.source,
            params.format,
            params.lines,
        );

        encode_success(
            id,
            ResponseResult::PaneRead {
                read: PaneReadResult {
                    pane_id: self
                        .public_pane_id(resolved.ws_idx, resolved.pane_id)
                        .unwrap_or_else(|| params.target.clone()),
                    workspace_id,
                    tab_id: self
                        .public_tab_id(resolved.ws_idx, resolved.tab_idx)
                        .expect(
                            "resolved.tab_idx came from resolve_agent_target's terminal_target_for_pane via find_tab_index_for_pane on the same workspace snapshot; lookup_runtime above confirms the pane is still live and nothing removes tabs in between",
                        ),
                    source: params.source,
                    format: params.format,
                    text: snapshot.text,
                    revision: 0,
                    truncated: snapshot.truncated,
                },
            },
        )
    }

    pub(super) fn handle_agent_explain(&mut self, id: String, target: AgentTarget) -> String {
        let resolved = match self.resolve_agent_target(&target.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some((pane, _workspace_id)) = self.lookup_runtime(resolved.ws_idx, resolved.pane_id)
        else {
            return agent_not_found(id, &target.target);
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
        else {
            return agent_not_found(id, &target.target);
        };
        let Some(terminal) = self.state.terminals.get(terminal_id) else {
            return agent_not_found(id, &target.target);
        };
        if terminal.full_lifecycle_hook_authority_active() {
            let explain = serde_json::json!({
                "agent": terminal.effective_agent_label().unwrap_or("unknown"),
                "state": crate::detect::manifest::agent_state_label(terminal.state),
                "manifest_source": null,
                "manifest_version": null,
                "cached_remote_version": null,
                "local_override_shadowing_remote": false,
                "remote_update_status": null,
                "remote_update_error": null,
                "matched_rule": null,
                "visible_idle": false,
                "visible_blocker": false,
                "visible_working": false,
                "screen_detection_skipped": true,
                "screen_detection_skip_reason": "full_lifecycle_hook_authority",
                "skip_state_update": false,
                "skipped_update_reason": null,
                "fallback_reason": null,
                "warning": null,
                "evaluated_rules": [],
            });
            return encode_success(id, ResponseResult::AgentExplain { explain });
        }
        let Some(agent) = terminal.effective_known_agent().or(terminal.detected_agent) else {
            return encode_error(
                id,
                "agent_explain_unavailable",
                format!(
                    "agent target {} does not have a detected agent label",
                    target.target
                ),
            );
        };

        let screen = pane.detection_text();
        let osc_title = pane.agent_osc_title();
        let osc_progress = pane.agent_osc_progress();
        let explain = crate::detect::manifest::explain_with_input(
            agent,
            crate::detect::manifest::DetectionInput {
                screen: &screen,
                osc_title: &osc_title,
                osc_progress: &osc_progress,
            },
        );
        let value = crate::detect::manifest::explain_to_json_value(&explain);

        encode_success(id, ResponseResult::AgentExplain { explain: value })
    }

    pub(super) fn handle_agent_send_keys(
        &mut self,
        id: String,
        params: AgentSendKeysParams,
    ) -> String {
        let resolved = match self.resolve_agent_target(&params.target) {
            Ok(resolved) => resolved,
            Err(err) => return encode_error_body(id, self.agent_target_error_body(err)),
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
        else {
            return agent_not_found(id, &params.target);
        };
        let Some(expected_agent) = self
            .state
            .terminals
            .get(terminal_id)
            .and_then(crate::terminal::state::TerminalState::effective_known_agent)
        else {
            return agent_not_ready(id, &params.target);
        };
        let Some(runtime) = self.lookup_runtime_sender(resolved.ws_idx, resolved.pane_id) else {
            return agent_not_found(id, &params.target);
        };
        if !super::super::agents::runtime_hosts_agent(runtime, expected_agent) {
            return agent_not_ready(id, &params.target);
        }
        let encoded = match super::super::api_helpers::encode_api_keys(runtime, &params.keys) {
            Ok(encoded) => encoded,
            Err(key) => {
                return encode_error(id, "invalid_key", format!("unsupported key {key}"));
            }
        };
        let bytes: Vec<u8> = encoded.into_iter().flatten().collect();
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            return encode_error(id, "agent_send_keys_failed", err.to_string());
        }

        encode_success(id, ResponseResult::Ok {})
    }
}

/// Pure status-gate decision for `AgentPromptParams.when_idle`: true only when the
/// caller opted in AND the target is currently `working`. Never blocks; the caller
/// (`handle_agent_prompt`) queues the prompt via `enqueue_pending_agent_prompt` for
/// later replay rather than dropping it. Callers that need to wait for idle instead
/// of deferring use the api/wait.rs pre-send poll.
fn agent_prompt_should_reject_busy(
    when_idle: Option<bool>,
    status: crate::api::schema::AgentStatus,
) -> bool {
    when_idle == Some(true) && status == crate::api::schema::AgentStatus::Working
}

/// Truncates `text` to `AGENT_PROMPTED_TEXT_LIMIT` bytes at a UTF-8
/// character boundary for `EventData::AgentPrompted`. Returns the
/// (possibly-unchanged) text and whether it was truncated.
fn truncate_agent_prompt_text(text: &str) -> (String, bool) {
    if text.len() <= AGENT_PROMPTED_TEXT_LIMIT {
        return (text.to_string(), false);
    }
    let mut end = AGENT_PROMPTED_TEXT_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

fn agent_not_ready(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_ready",
        format!("agent {target} is not an active named agent"),
    )
}

fn agent_not_found(id: String, target: &str) -> String {
    encode_error(
        id,
        "agent_not_found",
        format!("agent target {target} not found"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        api::schema::{AgentStatus, SuccessResponse},
        app::Mode,
        config::{Config, IsolatedDirs},
        detect::{Agent, AgentState},
        workspace::Workspace,
    };

    fn app_with_agent() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = vec![Workspace::test_new("agent")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;
        app
    }

    #[test]
    fn agent_prompt_should_reject_busy_gates_on_when_idle_and_working_status() {
        assert!(agent_prompt_should_reject_busy(
            Some(true),
            AgentStatus::Working
        ));
        for status in [
            AgentStatus::Idle,
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Unknown,
        ] {
            assert!(!agent_prompt_should_reject_busy(Some(true), status));
        }
        assert!(!agent_prompt_should_reject_busy(
            Some(false),
            AgentStatus::Working
        ));
        assert!(!agent_prompt_should_reject_busy(None, AgentStatus::Working));
    }

    #[tokio::test]
    async fn agent_prompt_sends_text_then_delays_enter() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 1,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);

        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();
        let bracketed_started = std::time::Instant::now();
        let response = app.handle_agent_prompt(
            "req".into(),
            AgentPromptParams {
                target: public_pane_id,
                text: "A != B".into(),
                wait: None,
                from_pane: None,
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::AgentPrompted { agent, .. } = success.result else {
            panic!("expected prompted response");
        };
        assert_eq!(agent.name.as_deref(), Some("reviewer"));
        let to_pane_id = app.public_pane_id(0, pane_id).unwrap();
        assert!(app.event_hub.events_after(0).iter().any(|(_, event)| {
            matches!(
                &event.data,
                crate::api::schema::EventData::AgentPrompted {
                    from_pane_id,
                    to_pane_id: actual_pane_id,
                    text,
                    text_truncated,
                    text_len,
                    ..
                } if from_pane_id.is_none()
                    && actual_pane_id == &to_pane_id
                    && text == "A != B"
                    && !text_truncated
                    && *text_len == "A != B".len()
            )
        }));
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~A != B\x1b[201~")
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap()
                .unwrap(),
            Bytes::from_static(b"\r")
        );
        assert!(bracketed_started.elapsed() >= AGENT_PROMPT_SUBMIT_DELAY);

        app.lookup_runtime_sender(0, pane_id)
            .unwrap()
            .test_process_pty_bytes(b"\x1b[?2004l");
        let raw_started = std::time::Instant::now();
        let raw = app.handle_agent_prompt(
            "req-raw".into(),
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
                from_pane: None,
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let raw: SuccessResponse = serde_json::from_str(&raw).unwrap();
        assert!(matches!(raw.result, ResponseResult::AgentPrompted { .. }));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"A != B"));
        assert!(rx.try_recv().is_err());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap()
                .unwrap(),
            Bytes::from_static(b"\r")
        );
        assert!(raw_started.elapsed() >= AGENT_PROMPT_SUBMIT_DELAY);

        let rejected = app.handle_agent_prompt(
            "req-label".into(),
            AgentPromptParams {
                target: "opencode".into(),
                text: "wrong target".into(),
                wait: None,
                from_pane: None,
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&rejected).unwrap();
        assert_eq!(error.error.code, "agent_not_found");
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn agent_prompt_rejects_blocked_agent_without_writing() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::GithubCopilot), AgentState::Blocked);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let response = app.handle_agent_prompt(
            "req".into(),
            AgentPromptParams {
                target: "reviewer".into(),
                text: "unrelated prompt".into(),
                wait: None,
                from_pane: None,
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );

        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_blocked");
        assert!(
            tokio::time::timeout(
                AGENT_PROMPT_SUBMIT_DELAY + Duration::from_millis(100),
                rx.recv()
            )
            .await
            .is_err(),
            "blocked prompt wrote or scheduled terminal input"
        );
    }

    #[tokio::test]
    async fn agent_prompt_focuses_copilot_before_submitting() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::GithubCopilot), AgentState::Idle);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 3,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);

        let response = app.handle_agent_prompt(
            "req".into(),
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
                from_pane: None,
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\x1b[I"));
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~A != B\x1b[201~")
        );
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap()
                .unwrap(),
            Bytes::from_static(b"\r")
        );
    }

    #[tokio::test]
    async fn agent_prompt_from_prefix_uses_workspace_custom_name_when_verified() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.state.workspaces[0].custom_name = Some("brandos".into());
        let (runtime, _rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        runtime.test_set_child_pid(4242);
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        let prefix = app.agent_prompt_from_prefix(&public_pane_id, Some(4242));
        assert_eq!(prefix, format!("[from {public_pane_id} brandos] "));
    }

    #[tokio::test]
    async fn agent_prompt_from_prefix_falls_back_to_detected_agent_name_when_verified() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        app.state.workspaces[0].custom_name = None;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Codex), AgentState::Idle);
        let (runtime, _rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        runtime.test_set_child_pid(4243);
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        let prefix = app.agent_prompt_from_prefix(&public_pane_id, Some(4243));
        assert_eq!(prefix, format!("[from {public_pane_id} codex] "));
    }

    #[test]
    fn agent_prompt_from_prefix_falls_back_to_raw_id_when_unresolvable() {
        let app = app_with_agent();
        let prefix = app.agent_prompt_from_prefix("p_bogus_99", None);
        assert_eq!(prefix, "[from? claimed p_bogus_99] ");
    }

    #[tokio::test]
    async fn agent_prompt_from_prefix_is_unverified_without_peer_pid() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let (runtime, _rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        runtime.test_set_child_pid(4244);
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        // No OS peer credentials for this call: a caller-supplied `from_pane` is a
        // hint, never trusted at face value.
        let prefix = app.agent_prompt_from_prefix(&public_pane_id, None);
        assert_eq!(prefix, format!("[from? claimed {public_pane_id}] "));
    }

    #[tokio::test]
    async fn agent_prompt_from_prefix_is_unverified_when_peer_pid_does_not_descend_from_shell() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let (runtime, _rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        runtime.test_set_child_pid(4245);
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        // A peer pid that is neither the claimed pane's shell nor a live descendant
        // of it: the ancestry walk fails closed instead of trusting the claim.
        let prefix = app.agent_prompt_from_prefix(&public_pane_id, Some(4246));
        assert_eq!(prefix, format!("[from? claimed {public_pane_id}] "));
    }

    #[tokio::test]
    async fn agent_prompt_marks_unverified_from_pane_claim_in_submitted_text() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 1,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);
        app.state.workspaces[0].custom_name = Some("brandos".into());
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        // The target pane has no real shell process backing it in this test harness
        // (matching `runtime_hosts_agent`'s test-mode bypass), so `peer_pid` can
        // never be confirmed to descend from it: the claim is delivered, but
        // flagged unverified rather than silently trusted the way the
        // caller-supplied string used to be. Verified-path prefix formatting is
        // covered directly by `agent_prompt_from_prefix_*_when_verified` above.
        let response = app.handle_agent_prompt(
            "req".into(),
            AgentPromptParams {
                target: public_pane_id.clone(),
                text: "A != B".into(),
                wait: None,
                from_pane: Some(public_pane_id.clone()),
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: Some(4248),
                origin_channel: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));
        let expected = format!("[from? claimed {public_pane_id}] A != B");
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from(format!("\x1b[200~{expected}\x1b[201~"))
        );
    }

    #[tokio::test]
    async fn agent_prompt_queues_when_target_busy_and_when_idle() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 1,
            );
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        let response = app.handle_agent_prompt(
            "req".into(),
            AgentPromptParams {
                target: public_pane_id.clone(),
                text: "queued message".into(),
                wait: None,
                from_pane: None,
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::AgentPrompted {
            outcome,
            queue_position,
            ..
        } = success.result
        else {
            panic!("expected AgentPrompted, got {:?}", success.result);
        };
        assert_eq!(outcome, crate::api::schema::AgentPromptOutcome::Deferred);
        assert_eq!(queue_position, Some(1));
        assert!(
            rx.try_recv().is_err(),
            "busy prompt must not dispatch bytes"
        );

        let queue = app
            .pending_agent_prompts
            .get(&public_pane_id)
            .expect("prompt queued, not dropped");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].params.text, "queued message");
    }

    #[test]
    fn enqueue_pending_agent_prompt_drops_oldest_past_cap() {
        let mut app = app_with_agent();
        for i in 0..(crate::app::PENDING_AGENT_PROMPT_CAP + 1) {
            app.enqueue_pending_agent_prompt(
                "p_target".into(),
                AgentPromptParams {
                    target: "p_target".into(),
                    text: format!("msg-{i}"),
                    wait: None,
                    from_pane: None,
                    when_idle: Some(true),
                    when_idle_timeout_ms: None,
                    peer_pid: None,
                    origin_channel: None,
                },
            );
        }
        let queue = app.pending_agent_prompts.get("p_target").unwrap();
        assert_eq!(queue.len(), crate::app::PENDING_AGENT_PROMPT_CAP);
        // The oldest (msg-0) was dropped to make room; msg-1 is now the head.
        assert_eq!(queue.front().unwrap().params.text, "msg-1");
        assert_eq!(
            queue.back().unwrap().params.text,
            format!("msg-{}", crate::app::PENDING_AGENT_PROMPT_CAP)
        );
    }

    #[test]
    fn enqueue_pending_agent_prompt_cap_eviction_emits_dropped_event() {
        let mut app = app_with_agent();
        let mut last_queue_id = 0;
        for i in 0..(crate::app::PENDING_AGENT_PROMPT_CAP + 1) {
            let (_, queue_id) = app.enqueue_pending_agent_prompt(
                "p_target".into(),
                AgentPromptParams {
                    target: "p_target".into(),
                    text: format!("msg-{i}"),
                    wait: None,
                    from_pane: Some("p_sender".into()),
                    when_idle: Some(true),
                    when_idle_timeout_ms: None,
                    peer_pid: None,
                    origin_channel: None,
                },
            );
            if i == 0 {
                last_queue_id = queue_id;
            }
        }
        let events = app.event_hub.events_after(0);
        let dropped = events
            .iter()
            .find_map(|(_, envelope)| match &envelope.data {
                crate::api::schema::EventData::QueuedPromptDropped {
                    queue_id,
                    target_pane,
                    from_pane,
                    reason,
                    ..
                } if target_pane == "p_target" => Some((*queue_id, from_pane.clone(), *reason)),
                _ => None,
            })
            .expect("cap eviction must emit a queued_prompt_dropped event");
        assert_eq!(dropped.0, last_queue_id, "evicted entry was msg-0");
        assert_eq!(dropped.1.as_deref(), Some("p_sender"));
        assert_eq!(
            dropped.2,
            crate::api::schema::QueuedAgentPromptDropReason::Capacity
        );
        assert_eq!(
            events
                .iter()
                .filter(|(_, e)| e.event == crate::api::schema::EventKind::QueuedPromptDropped)
                .count(),
            1,
            "only the single evicted entry should be reported"
        );
    }

    #[tokio::test]
    async fn drain_pending_agent_prompts_delivers_once_target_is_idle() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 1,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        let (_, queue_id) = app.enqueue_pending_agent_prompt(
            public_pane_id.clone(),
            AgentPromptParams {
                target: public_pane_id.clone(),
                text: "deferred message".into(),
                wait: None,
                from_pane: None,
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );

        // Target leaves `Working`: the drain hook (wired in `emit_pane_state_update`)
        // is exercised directly here.
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::OpenCode), AgentState::Idle);
        app.drain_pending_agent_prompts(&public_pane_id);

        assert!(
            !app.pending_agent_prompts.contains_key(&public_pane_id),
            "drained queue must be removed, not left empty-but-present"
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~deferred message\x1b[201~")
        );
        let events = app.event_hub.events_after(0);
        assert!(
            events.iter().any(|(_, envelope)| matches!(
                &envelope.data,
                crate::api::schema::EventData::QueuedPromptDelivered {
                    queue_id: id,
                    target_pane,
                    ..
                } if *id == queue_id && target_pane == &public_pane_id
            )),
            "successful drain must emit a queued_prompt_delivered event"
        );
        assert!(
            !events
                .iter()
                .any(|(_, e)| e.event == crate::api::schema::EventKind::QueuedPromptDropped),
            "a delivered prompt must never also be reported as dropped"
        );
    }

    /// The bead's acceptance path end to end: a prompt is deferred, the server
    /// goes away, a new one comes up, and the message still gets delivered.
    /// Before `persist::pending_prompts` existed this was the exact failure —
    /// the sender got a `deferred` receipt and the restart silently ate the
    /// message, so a receipt that promised eventual delivery was a lie across
    /// any restart.
    #[tokio::test]
    async fn deferred_prompt_survives_a_server_restart_and_delivers() {
        let _isolated = IsolatedDirs::new("pending-prompt-restart");

        let (queue_id, saved_workspace_id) = {
            let mut first = app_with_agent();
            let pane_id = first.state.workspaces[0].tabs[0].root_pane;
            let terminal_id = first.state.workspaces[0].tabs[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            let terminal = first.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.set_agent_name("reviewer".into());
            terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
            let public_pane_id = first.public_pane_id(0, pane_id).unwrap();

            let (_, queue_id) = first.enqueue_pending_agent_prompt(
                public_pane_id.clone(),
                AgentPromptParams {
                    target: public_pane_id,
                    text: "survives the restart".into(),
                    wait: None,
                    from_pane: None,
                    when_idle: Some(true),
                    when_idle_timeout_ms: None,
                    peer_pid: None,
                    origin_channel: None,
                },
            );
            let workspace_id = first.state.workspaces[0].id.clone();
            crate::app::api::test_support::shutdown_test_runtimes(&mut first);
            (queue_id, workspace_id)
        };

        // The restart. A brand new `App` over the same state directory: it must
        // rehydrate the queue in its constructor, with no help from the caller.
        let mut second = app_with_agent();
        // Restore also restores workspace identity: `reserve_workspace_ids`
        // exists precisely so a restored workspace keeps the public id its
        // panes were addressed by. `generate_workspace_id`'s counter is
        // process-global, so a second `App` in the same test binary would
        // otherwise mint `w2` and the restored queue — correctly keyed by the
        // id the sender was given — would have no pane to drain to.
        second.state.workspaces[0].id = saved_workspace_id;
        let pane_id = second.state.workspaces[0].tabs[0].root_pane;
        let public_pane_id = second.public_pane_id(0, pane_id).unwrap();
        let restored = second
            .pending_agent_prompts
            .get(&public_pane_id)
            .expect("the deferred prompt must come back from disk on boot");
        assert_eq!(restored.len(), 1);
        assert_eq!(
            restored.front().unwrap().params.text,
            "survives the restart"
        );
        assert_eq!(
            restored.front().unwrap().queue_id,
            queue_id,
            "the queue_id on the receipt the sender already holds must survive too"
        );
        assert!(
            second.next_pending_agent_prompt_queue_id > queue_id,
            "a fresh enqueue must not reuse a restored queue_id: two prompts \
             sharing one id make their terminal-fate events indistinguishable"
        );

        // ...and it actually delivers, which is the promise the receipt made.
        let terminal_id = second.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = second.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Idle);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 1,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        second.state.insert_test_runtime(pane_id, runtime);

        second.drain_pending_agent_prompts(&public_pane_id);

        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~survives the restart\x1b[201~")
        );
        assert!(
            crate::persist::pending_prompts::read_pending_prompts().is_empty(),
            "a delivered prompt must be gone from disk, or the next restart \
             would deliver it a second time"
        );
        crate::app::api::test_support::shutdown_test_runtimes(&mut second);
    }

    /// The furo M7 closes, at the queue. The test above has to pin the
    /// workspace id so the pane id stays the same, and its own comment says
    /// why: keyed by pane, "the restored queue would have no pane to drain
    /// to". Here the pane id deliberately *does* change, the way a real cold
    /// restore reallocates it, and the prompt still finds its agent —
    /// because the record carries the identity, not just the seat.
    #[tokio::test]
    async fn deferred_prompt_follows_its_agent_to_a_reallocated_pane() {
        let _isolated = IsolatedDirs::new("pending-prompt-reallocated");

        let (agent_id, first_public_pane_id) = {
            let mut first = app_with_agent();
            let pane_id = first.state.workspaces[0].tabs[0].root_pane;
            let terminal_id = first.state.workspaces[0].tabs[0].panes[&pane_id]
                .attached_terminal_id
                .clone();
            let terminal = first.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
            let agent_id = terminal.agent_id.as_str().to_string();
            let public_pane_id = first.public_pane_id(0, pane_id).unwrap();

            first.enqueue_pending_agent_prompt(
                public_pane_id.clone(),
                AgentPromptParams {
                    target: public_pane_id.clone(),
                    text: "follows the agent".into(),
                    wait: None,
                    from_pane: None,
                    when_idle: Some(true),
                    when_idle_timeout_ms: None,
                    peer_pid: None,
                    origin_channel: None,
                },
            );
            crate::app::api::test_support::shutdown_test_runtimes(&mut first);
            (agent_id, public_pane_id)
        };

        // The restart, with the workspace id deliberately NOT pinned, so the
        // pane id is reallocated. Restore hands the agent back its identity
        // (see `restore.rs`'s unconditional `restore_agent_id`), which is
        // what this reproduces.
        let mut second = app_with_agent();
        let pane_id = second.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = second.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        second
            .state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .restore_agent_id(crate::terminal::AgentId::from_persisted(agent_id));
        // `App::new` already loaded the queue against a freshly minted
        // identity. Production order is the other way round — restore builds
        // the terminals, identities included, and only then does `App::new`
        // load the queue — so drop that first pass and load once, the way the
        // real boot path does.
        second.pending_agent_prompts.clear();
        second.load_pending_agent_prompts();
        let public_pane_id = second.public_pane_id(0, pane_id).unwrap();
        assert_ne!(
            public_pane_id, first_public_pane_id,
            "the test only proves something if the pane id really was reallocated"
        );
        let restored = second
            .pending_agent_prompts
            .get(&public_pane_id)
            .expect("the prompt must be re-targeted onto the agent's new pane");
        assert_eq!(restored.len(), 1);
        assert_eq!(restored.front().unwrap().params.text, "follows the agent");
        assert!(
            !second
                .pending_agent_prompts
                .contains_key(&first_public_pane_id),
            "nothing may stay queued against the stale seat, or a pane that \
             inherits that id would be handed somebody else's prompt"
        );
        crate::app::api::test_support::shutdown_test_runtimes(&mut second);
    }

    #[tokio::test]
    async fn drain_pending_agent_prompts_requeues_when_still_busy() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        app.enqueue_pending_agent_prompt(
            public_pane_id.clone(),
            AgentPromptParams {
                target: public_pane_id.clone(),
                text: "still busy".into(),
                wait: None,
                from_pane: None,
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );

        // A spurious drain trigger while the target is still `Working`: the replay
        // hits the busy gate again and is re-queued, not lost.
        app.drain_pending_agent_prompts(&public_pane_id);

        assert!(
            rx.try_recv().is_err(),
            "still-busy replay must not dispatch bytes"
        );
        let queue = app
            .pending_agent_prompts
            .get(&public_pane_id)
            .expect("prompt re-queued, not dropped");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].params.text, "still busy");
    }

    #[tokio::test]
    async fn pending_agent_prompt_settle_cancels_on_flicker_back_to_working() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        app.enqueue_pending_agent_prompt(
            public_pane_id.clone(),
            AgentPromptParams {
                target: public_pane_id.clone(),
                text: "flicker".into(),
                wait: None,
                from_pane: None,
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );

        let t0 = Instant::now();
        // Target observed leaving `Working`: starts the settle window.
        app.sync_pending_agent_prompt_drain_deadline(&public_pane_id, AgentStatus::Idle, t0);
        assert!(
            app.pending_agent_prompt_drain_deadlines
                .contains_key(&public_pane_id),
            "leaving Working with a non-empty queue must start the settle window"
        );

        // Flicker back to `Working` well inside the window: must cancel the settle.
        app.sync_pending_agent_prompt_drain_deadline(
            &public_pane_id,
            AgentStatus::Working,
            t0 + Duration::from_millis(200),
        );
        assert!(
            !app.pending_agent_prompt_drain_deadlines
                .contains_key(&public_pane_id),
            "a return to Working must cancel the pending settle"
        );

        // Even once the original window would have elapsed, nothing is due:
        // the cancel must stick, not just delay the fire.
        let drained = app.drain_settled_pending_agent_prompts(
            t0 + crate::app::PENDING_AGENT_PROMPT_DRAIN_SETTLE + Duration::from_millis(50),
        );
        assert!(!drained, "a cancelled settle must never fire a drain");
        assert!(
            rx.try_recv().is_err(),
            "cancelled settle must not dispatch bytes"
        );
        let queue = app
            .pending_agent_prompts
            .get(&public_pane_id)
            .expect("prompt stays queued after a cancelled settle");
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].params.text, "flicker");
    }

    #[tokio::test]
    async fn pending_agent_prompt_settle_delivers_once_target_stays_settled() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) =
            crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
                80, 24, 0, b"", 1,
            );
        runtime.test_process_pty_bytes(b"\x1b[?2004h");
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        let (_, queue_id) = app.enqueue_pending_agent_prompt(
            public_pane_id.clone(),
            AgentPromptParams {
                target: public_pane_id.clone(),
                text: "settled message".into(),
                wait: None,
                from_pane: None,
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );

        // Target actually goes idle: the eventual replay re-checks live agent
        // status too, so this must hold for the whole window, not just the
        // deadline bookkeeping.
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::OpenCode), AgentState::Idle);

        let t0 = Instant::now();
        app.sync_pending_agent_prompt_drain_deadline(&public_pane_id, AgentStatus::Idle, t0);

        // Before the window elapses, nothing is due yet.
        let drained_early = app.drain_settled_pending_agent_prompts(
            t0 + crate::app::PENDING_AGENT_PROMPT_DRAIN_SETTLE - Duration::from_millis(1),
        );
        assert!(
            !drained_early,
            "must not drain before the settle window elapses"
        );
        assert!(rx.try_recv().is_err());
        assert!(app.pending_agent_prompts.contains_key(&public_pane_id));

        // Once settled, the queued prompt replays.
        let drained = app.drain_settled_pending_agent_prompts(
            t0 + crate::app::PENDING_AGENT_PROMPT_DRAIN_SETTLE,
        );
        assert!(drained, "must drain once the settle window has elapsed");
        assert!(
            !app.pending_agent_prompt_drain_deadlines
                .contains_key(&public_pane_id),
            "a fired settle deadline must be cleared"
        );
        assert!(!app.pending_agent_prompts.contains_key(&public_pane_id));
        assert_eq!(
            rx.try_recv().unwrap(),
            Bytes::from_static(b"\x1b[200~settled message\x1b[201~")
        );
        let events = app.event_hub.events_after(0);
        assert!(
            events.iter().any(|(_, envelope)| matches!(
                &envelope.data,
                crate::api::schema::EventData::QueuedPromptDelivered {
                    queue_id: id,
                    target_pane,
                    ..
                } if *id == queue_id && target_pane == &public_pane_id
            )),
            "successful settled drain must emit a queued_prompt_delivered event"
        );
    }

    #[tokio::test]
    async fn fail_pending_agent_prompts_drops_queue_without_replay() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);
        let public_pane_id = app.public_pane_id(0, pane_id).unwrap();

        app.enqueue_pending_agent_prompt(
            public_pane_id.clone(),
            AgentPromptParams {
                target: public_pane_id.clone(),
                text: "orphaned".into(),
                wait: None,
                from_pane: None,
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );

        app.fail_pending_agent_prompts(&public_pane_id);

        assert!(!app.pending_agent_prompts.contains_key(&public_pane_id));
        assert!(
            rx.try_recv().is_err(),
            "failed queue must not dispatch bytes"
        );
    }

    #[tokio::test]
    async fn fail_pending_agent_prompts_notifies_known_sender_without_recursion() {
        let mut app = app_with_agent();
        let target_pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let target_terminal_id = app.state.workspaces[0].tabs[0].panes[&target_pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&target_terminal_id)
            .unwrap()
            .set_agent_name("reviewer".into());
        let target_public_id = app.public_pane_id(0, target_pane_id).unwrap();

        app.state.workspaces.push(Workspace::test_new("sender"));
        app.state.ensure_test_terminals();
        let sender_pane_id = app.state.workspaces[1].tabs[0].root_pane;
        let sender_terminal_id = app.state.workspaces[1].tabs[0].panes[&sender_pane_id]
            .attached_terminal_id
            .clone();
        {
            let sender_terminal = app.state.terminals.get_mut(&sender_terminal_id).unwrap();
            sender_terminal.set_agent_name("sender-agent".into());
            sender_terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Idle);
        }
        let (sender_runtime, mut sender_rx) =
            crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state
            .insert_test_runtime(sender_pane_id, sender_runtime);
        let sender_public_id = app.public_pane_id(1, sender_pane_id).unwrap();

        app.enqueue_pending_agent_prompt(
            target_public_id.clone(),
            AgentPromptParams {
                target: target_public_id.clone(),
                text: "orphaned".into(),
                wait: None,
                from_pane: Some(sender_public_id.clone()),
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );

        app.fail_pending_agent_prompts(&target_public_id);

        // The durable event is the record of truth...
        let events = app.event_hub.events_after(0);
        let dropped_events: Vec<_> = events
            .iter()
            .filter(|(_, envelope)| {
                envelope.event == crate::api::schema::EventKind::QueuedPromptDropped
            })
            .collect();
        assert_eq!(dropped_events.len(), 1, "exactly one drop, no recursion");
        assert!(matches!(
            &dropped_events[0].1.data,
            crate::api::schema::EventData::QueuedPromptDropped {
                target_pane,
                from_pane,
                reason: crate::api::schema::QueuedAgentPromptDropReason::PaneClosed,
                ..
            } if target_pane == &target_public_id && from_pane.as_deref() == Some(sender_public_id.as_str())
        ));

        // ...but the known, idle sender also gets a direct courtesy notice.
        let notice_bytes = sender_rx
            .try_recv()
            .expect("notice injected into sender pty");
        let notice_text = String::from_utf8(notice_bytes.to_vec()).unwrap();
        assert!(notice_text.contains("[bora] prompt to"));
        assert!(notice_text.contains(&target_public_id));
        assert!(notice_text.contains("dropped"));

        // The notice itself was never queued: the sender's own pending-prompt
        // queue is untouched, so it can never trigger a second drop report.
        assert!(!app.pending_agent_prompts.contains_key(&sender_public_id));
        assert_eq!(
            app.event_hub
                .events_after(0)
                .iter()
                .filter(|(_, e)| e.event == crate::api::schema::EventKind::QueuedPromptDropped)
                .count(),
            1,
            "the notice injection must never itself generate a drop event"
        );
    }

    #[tokio::test]
    async fn fail_pending_agent_prompts_skips_notice_when_sender_is_busy() {
        let mut app = app_with_agent();
        let target_pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let target_terminal_id = app.state.workspaces[0].tabs[0].panes[&target_pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&target_terminal_id)
            .unwrap()
            .set_agent_name("reviewer".into());
        let target_public_id = app.public_pane_id(0, target_pane_id).unwrap();

        app.state.workspaces.push(Workspace::test_new("sender"));
        app.state.ensure_test_terminals();
        let sender_pane_id = app.state.workspaces[1].tabs[0].root_pane;
        let sender_terminal_id = app.state.workspaces[1].tabs[0].panes[&sender_pane_id]
            .attached_terminal_id
            .clone();
        {
            let sender_terminal = app.state.terminals.get_mut(&sender_terminal_id).unwrap();
            sender_terminal.set_agent_name("sender-agent".into());
            // Busy sender: the courtesy notice must never interrupt it mid-task.
            sender_terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        }
        let (sender_runtime, mut sender_rx) =
            crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state
            .insert_test_runtime(sender_pane_id, sender_runtime);
        let sender_public_id = app.public_pane_id(1, sender_pane_id).unwrap();

        app.enqueue_pending_agent_prompt(
            target_public_id.clone(),
            AgentPromptParams {
                target: target_public_id.clone(),
                text: "orphaned".into(),
                wait: None,
                from_pane: Some(sender_public_id),
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );

        app.fail_pending_agent_prompts(&target_public_id);

        assert!(
            sender_rx.try_recv().is_err(),
            "busy sender must not receive a courtesy notice"
        );
        // The event is still the durable record even though the notice was skipped.
        assert!(app.event_hub.events_after(0).iter().any(|(_, envelope)| {
            envelope.event == crate::api::schema::EventKind::QueuedPromptDropped
        }));
    }

    #[tokio::test]
    async fn agent_send_keys_validates_every_key_before_writing() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let rejected = app.handle_agent_send_keys(
            "req-invalid".into(),
            AgentSendKeysParams {
                target: "reviewer".into(),
                keys: vec!["enter".into(), "not-a-key".into()],
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&rejected).unwrap();
        assert_eq!(error.error.code, "invalid_key");
        assert!(rx.try_recv().is_err());

        let sent = app.handle_agent_send_keys(
            "req-valid".into(),
            AgentSendKeysParams {
                target: "reviewer".into(),
                keys: vec!["up".into(), "enter".into()],
            },
        );
        let success: SuccessResponse = serde_json::from_str(&sent).unwrap();
        assert!(matches!(success.result, ResponseResult::Ok {}));
        assert_eq!(rx.try_recv().unwrap(), Bytes::from_static(b"\x1b[A\r"));
        assert!(rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn agent_prompt_rejects_managed_agent_while_startup_is_pending() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        let now = std::time::Instant::now();
        terminal.begin_managed_agent(
            "reviewer".into(),
            Agent::OpenCode,
            now,
            std::time::Duration::from_secs(3),
            std::time::Duration::from_secs(10),
        );
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Idle);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);

        let response = app.handle_agent_prompt(
            "req-pending".into(),
            AgentPromptParams {
                target: "reviewer".into(),
                text: "A != B".into(),
                wait: None,
                from_pane: None,
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(error.error.code, "agent_not_ready");
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn agent_focus_marks_already_focused_done_agent_seen() {
        let mut app = app_with_agent();
        app.state.outer_terminal_focus = Some(false);

        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_detected_state(Some(Agent::Pi), AgentState::Idle);
        app.state.workspaces[0].tabs[0]
            .panes
            .get_mut(&pane_id)
            .unwrap()
            .seen = false;
        app.state.workspaces[0].tabs[0].layout.focus_pane(pane_id);

        let response = app.handle_agent_focus(
            "req".into(),
            AgentTarget {
                target: app.public_pane_id(0, pane_id).unwrap(),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::AgentInfo { agent } = success.result else {
            panic!("expected agent info response");
        };
        assert_eq!(agent.agent_status, AgentStatus::Idle);
    }

    #[test]
    fn agent_rename_does_not_replace_the_pane_label() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_manual_label("shell-pane".into());
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Idle);
        let target = app.public_pane_id(0, pane_id).unwrap();

        for name in [Some("reviewer".to_string()), None] {
            let response = app.handle_agent_rename(
                "req".into(),
                AgentRenameParams {
                    target: target.clone(),
                    name,
                },
            );
            let success: SuccessResponse = serde_json::from_str(&response).unwrap();
            assert!(matches!(success.result, ResponseResult::AgentInfo { .. }));
            assert_eq!(
                app.state.terminals[&terminal_id].manual_label.as_deref(),
                Some("shell-pane")
            );
        }
    }

    #[tokio::test]
    async fn agent_prompt_rate_limits_repeated_from_pane_pair_but_exempts_missing_from_pane() {
        let mut app = app_with_agent();
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&pane_id]
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name("reviewer".into());
        terminal.set_detected_state(Some(Agent::OpenCode), AgentState::Working);
        let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane_id, runtime);
        let target_pane = app.public_pane_id(0, pane_id).unwrap();

        let first = app.handle_agent_prompt(
            "req-1".into(),
            AgentPromptParams {
                target: "reviewer".into(),
                text: "first".into(),
                wait: None,
                from_pane: Some("w1:p9".into()),
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&first).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));

        // Immediate repeat from the same (from_pane, target) pair: rejected.
        let second = app.handle_agent_prompt(
            "req-2".into(),
            AgentPromptParams {
                target: "reviewer".into(),
                text: "second".into(),
                wait: None,
                from_pane: Some("w1:p9".into()),
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let error: crate::api::schema::ErrorResponse = serde_json::from_str(&second).unwrap();
        assert_eq!(error.error.code, "agent_prompt_rate_limited");
        assert!(error.error.message.contains("w1:p9"));
        assert!(error.error.message.contains(&target_pane));

        // No from_pane at all: exempt, always succeeds regardless of cooldown.
        let third = app.handle_agent_prompt(
            "req-3".into(),
            AgentPromptParams {
                target: "reviewer".into(),
                text: "third".into(),
                wait: None,
                from_pane: None,
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&third).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));

        // Different sender to the same target: not limited by the first pair.
        let fourth = app.handle_agent_prompt(
            "req-4".into(),
            AgentPromptParams {
                target: "reviewer".into(),
                text: "fourth".into(),
                wait: None,
                from_pane: Some("w1:p7".into()),
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&fourth).unwrap();
        assert!(matches!(
            success.result,
            ResponseResult::AgentPrompted { .. }
        ));

        assert!(rx.try_recv().is_ok());
    }
}
