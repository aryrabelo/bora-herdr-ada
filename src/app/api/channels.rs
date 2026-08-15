use crate::api::schema::{
    AgentPromptParams, AgentStatus, ChannelCreateParams, ChannelDelivery, ChannelDeliveryStatus,
    ChannelHistoryParams, ChannelMember, ChannelMembersParams, ChannelMessage, ChannelSendParams,
    ChannelSummary, ResponseResult,
};
use crate::app::App;
use crate::persist::channels;

use super::responses::{encode_error, encode_success};

const DEFAULT_CHANNEL_HISTORY_LINES: u32 = 50;
const MAX_CHANNEL_HISTORY_LINES: u32 = 1000;

impl App {
    pub(super) fn handle_channel_create(
        &mut self,
        id: String,
        params: ChannelCreateParams,
    ) -> String {
        let name = channels::normalize_channel_name(&params.name);
        if name.is_empty() {
            return encode_error(id, "invalid_channel_name", "channel name must not be empty");
        }
        if self.find_channel_workspace(&name).is_some() {
            return encode_error(
                id,
                "channel_exists",
                format!("channel #{name} already exists"),
            );
        }
        let follow_cwd = self.workspace_creation_source().and_then(|ws_idx| {
            self.focused_pane_cwd_in_workspace(ws_idx)
                .or_else(|| self.seed_cwd_from_workspace(ws_idx))
        });
        let cwd = self.resolve_new_terminal_cwd(follow_cwd);
        match self.create_workspace_with_launch_env(cwd, false, Vec::new()) {
            Ok(index) => {
                if let Some(workspace) = self.state.workspaces.get_mut(index) {
                    workspace.set_custom_name(format!("#{name}"));
                    crate::logging::workspace_renamed(&workspace.id);
                }
                self.state.mark_session_dirty();
                self.emit_workspace_open_events(index);
                let channel = self.channel_summary(index, &name);
                encode_success(id, ResponseResult::ChannelCreated { channel })
            }
            Err(err) => encode_error(id, "channel_create_failed", err.to_string()),
        }
    }

    pub(super) fn handle_channel_list(&mut self, id: String) -> String {
        let channels = self
            .state
            .workspaces
            .iter()
            .enumerate()
            .filter_map(|(idx, ws)| {
                workspace_channel_name(ws).map(|name| self.channel_summary(idx, name))
            })
            .collect();
        encode_success(id, ResponseResult::ChannelList { channels })
    }

    pub(super) fn handle_channel_send(&mut self, id: String, params: ChannelSendParams) -> String {
        if params.text.is_empty() {
            return encode_error(
                id,
                "empty_channel_message",
                "channel message must not be empty",
            );
        }
        let name = channels::normalize_channel_name(&params.name);
        let Some(ws_idx) = self.find_channel_workspace(&name) else {
            return encode_error(
                id,
                "channel_not_found",
                format!("channel #{name} not found"),
            );
        };

        // Structured addressing is the primary path: an explicit `to` that
        // does not resolve fails the send loudly, before anything is
        // appended or delivered. In-body `@nick` is convenience parsing for
        // human/TUI parity and never fails a send (orc aborts the whole
        // message on addressing it cannot parse — bora degrades instead).
        let mut to_pane: Option<String> = None;
        let raw_text = params.text.clone();
        let to = params
            .to
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        if let Some(to) = to {
            match self.resolve_channel_nick(ws_idx, to) {
                NickResolution::Unique(pane_id) => to_pane = Some(pane_id),
                NickResolution::Ambiguous(candidates) => {
                    return encode_error(
                        id,
                        "channel_nick_ambiguous",
                        format!(
                            "nick '{to}' matches {} channel members: {} — address one by pane id or a unique name",
                            candidates.len(),
                            candidates.join(", ")
                        ),
                    );
                }
                NickResolution::Unknown => {
                    return encode_error(
                        id,
                        "channel_nick_unknown",
                        format!(
                            "no channel member matches '{to}' — channel.members lists pane ids and names"
                        ),
                    );
                }
            }
        } else if let Some(nick) = leading_mention_nick(&raw_text) {
            // Leading `@nick ` token: targets only when it uniquely
            // resolves; unknown or ambiguous degrades to literal broadcast.
            match self.resolve_channel_nick(ws_idx, &nick) {
                NickResolution::Unique(pane_id) => to_pane = Some(pane_id),
                NickResolution::Ambiguous(candidates) => tracing::debug!(
                    channel = %name,
                    nick = %nick,
                    candidates = candidates.len(),
                    "channel.send: ambiguous leading mention, broadcasting as literal text"
                ),
                NickResolution::Unknown => tracing::debug!(
                    channel = %name,
                    nick = %nick,
                    "channel.send: unknown leading mention, broadcasting as literal text"
                ),
            }
        }
        // `\@` / `\#` escapes unescape to literal @ / # in the stored and
        // delivered text. This runs after addressing, which read the raw
        // text where the backslash keeps an escaped token from addressing.
        let text = unescape_channel_text(&raw_text);

        let sender_pane = params.from_pane.unwrap_or_default();
        let sender_name = self
            .pane_display_name(&sender_pane)
            .unwrap_or_else(|| "unknown".to_string());
        // Loop guard, mirroring the direct agent-prompt limit: a verified
        // sender pane may post to the same channel at most once per
        // rate-limit window, so two agents cannot ping-pong through a
        // channel faster than they could by prompting each other directly.
        // CLI/human sends without a from_pane stay exempt, like no-from
        // prompts. Checked after addressing validation so a rejected nick
        // never burns the sender's window, and before seq assignment.
        if !sender_pane.is_empty() {
            if let Err(remaining) = self.check_agent_prompt_rate_limit(
                &sender_pane,
                &format!("#{name}"),
                std::time::Instant::now(),
            ) {
                return encode_error(
                    id,
                    "channel_send_rate_limited",
                    format!(
                        "channel send from {sender_pane} to #{name} is rate-limited; retry in {}ms",
                        remaining.as_millis()
                    ),
                );
            }
        }
        let message = ChannelMessage {
            ts: now_rfc3339(),
            seq: channels::next_seq(&name),
            from_pane: sender_pane.clone(),
            from_name: sender_name.clone(),
            text: text.clone(),
            in_reply_to: params.in_reply_to,
            to_pane: to_pane.clone(),
        };
        if let Err(err) = channels::append_message(&name, &message) {
            return encode_error(id, "channel_send_failed", err.to_string());
        }
        self.state.push_chat_message(&name, message.clone());

        // Durable-record event, mirroring the QueuedPromptDelivered emission:
        // the message is on disk, so `channel.wait` followers (and events.wait
        // ChannelMessage filters) can wake on it. Emitted before fan-out —
        // delivery receipts are the deliveries list's job, not this event's.
        self.emit_event(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::ChannelMessage,
            data: crate::api::schema::EventData::ChannelMessage {
                channel: name.clone(),
                seq: message.seq,
                from_pane: (!sender_pane.is_empty()).then(|| sender_pane.clone()),
                from_name: sender_name.clone(),
                text: message.text.clone(),
                to_pane: message.to_pane,
            },
        });

        // The prefix is built here (not delegated to `handle_agent_prompt`'s
        // own from_pane attribution) so the delivered text carries the
        // channel name too; from_pane is passed as None below to avoid a
        // second `[from ...]` prefix being layered on top of this one.
        let prefixed = format!("[#{name} from {sender_pane} {sender_name}] {text}");

        // Targeted delivery reaches only the resolved pane — and never the
        // sender's own. Broadcast reaches every agent member pane as before.
        let targets: Vec<String> = match &to_pane {
            Some(target) if target != &sender_pane => vec![target.clone()],
            Some(_) => Vec::new(),
            None => self.channel_agent_member_pane_ids(ws_idx),
        };
        let deliveries = targets
            .into_iter()
            .map(|target| {
                let response = self.handle_agent_prompt(
                    format!("{id}:channel:{target}"),
                    AgentPromptParams {
                        target: target.clone(),
                        text: prefixed.clone(),
                        wait: None,
                        from_pane: None,
                        when_idle: Some(true),
                        when_idle_timeout_ms: None,
                        peer_pid: None,
                        origin_channel: Some(name.clone()),
                    },
                );
                classify_delivery(target, &response)
            })
            .collect();
        encode_success(id, ResponseResult::ChannelSent { deliveries })
    }

    pub(super) fn handle_channel_history(
        &mut self,
        id: String,
        params: ChannelHistoryParams,
    ) -> String {
        let name = channels::normalize_channel_name(&params.name);
        let limit = params
            .lines
            .unwrap_or(DEFAULT_CHANNEL_HISTORY_LINES)
            .min(MAX_CHANNEL_HISTORY_LINES) as usize;
        match channels::read_tail(&name, limit) {
            Ok(messages) => encode_success(id, ResponseResult::ChannelHistory { messages }),
            Err(err) => encode_error(id, "channel_history_failed", err.to_string()),
        }
    }

    pub(super) fn handle_channel_members(
        &mut self,
        id: String,
        params: ChannelMembersParams,
    ) -> String {
        let name = channels::normalize_channel_name(&params.name);
        let Some(ws_idx) = self.find_channel_workspace(&name) else {
            return encode_error(
                id,
                "channel_not_found",
                format!("channel #{name} not found"),
            );
        };
        let members = self.channel_members(ws_idx);
        encode_success(id, ResponseResult::ChannelMembers { members })
    }

    fn find_channel_workspace(&self, name: &str) -> Option<usize> {
        self.state
            .workspaces
            .iter()
            .position(|ws| workspace_channel_name(ws) == Some(name))
    }

    fn channel_summary(&self, ws_idx: usize, name: &str) -> ChannelSummary {
        let ws = &self.state.workspaces[ws_idx];
        let pane_ids: Vec<crate::layout::PaneId> = ws
            .tabs
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .collect();
        let pane_count = pane_ids.len();
        let agent_count = pane_ids
            .iter()
            .filter(|&&pane_id| self.agent_info(ws_idx, pane_id).is_some())
            .count();
        ChannelSummary {
            name: format!("#{name}"),
            pane_count,
            agent_count,
            member_status_counts: self.channel_member_status_counts(ws_idx),
        }
    }

    /// Every pane in the channel's workspace, as a `channel.members`
    /// listing — who would receive a `channel.send`.
    fn channel_members(&self, ws_idx: usize) -> Vec<ChannelMember> {
        let ws = &self.state.workspaces[ws_idx];
        let pane_ids: Vec<crate::layout::PaneId> = ws
            .tabs
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .collect();
        pane_ids
            .into_iter()
            .filter_map(|pane_id| {
                let public_id = self.public_pane_id(ws_idx, pane_id)?;
                let agent = self.agent_info(ws_idx, pane_id);
                let name = agent
                    .as_ref()
                    .and_then(|info| info.display_agent.clone().or_else(|| info.name.clone()));
                Some(ChannelMember {
                    pane_id: public_id,
                    name,
                    agent_status: agent.map(|info| info.agent_status),
                })
            })
            .collect()
    }

    /// Counts of member panes by agent status, keyed by the same
    /// snake_case strings `AgentStatus` serializes as. Panes not hosting a
    /// detected agent are excluded.
    fn channel_member_status_counts(
        &self,
        ws_idx: usize,
    ) -> std::collections::HashMap<String, usize> {
        let ws = &self.state.workspaces[ws_idx];
        let mut counts = std::collections::HashMap::new();
        for pane_id in ws.tabs.iter().flat_map(|tab| tab.layout.pane_ids()) {
            if let Some(info) = self.agent_info(ws_idx, pane_id) {
                *counts
                    .entry(agent_status_key(info.agent_status).to_string())
                    .or_insert(0) += 1;
            }
        }
        counts
    }

    /// The workspace custom_name of the pane's workspace, the pane's assigned
    /// agent name, or its detected agent kind, whichever resolves first — used
    /// to attribute a sender.
    fn pane_display_name(&self, public_pane_id: &str) -> Option<String> {
        let (ws_idx, pane_id) = self.parse_pane_id(public_pane_id)?;
        let ws = self.state.workspaces.get(ws_idx)?;
        if let Some(custom_name) = ws.custom_name.clone() {
            return Some(custom_name);
        }
        let info = self.agent_info(ws_idx, pane_id)?;
        info.name.or(info.agent)
    }

    /// Public pane ids of the channel workspace's agent-hosting member
    /// panes — the broadcast delivery set.
    fn channel_agent_member_pane_ids(&self, ws_idx: usize) -> Vec<String> {
        self.state
            .workspaces
            .get(ws_idx)
            .map(|ws| {
                ws.tabs
                    .iter()
                    .flat_map(|tab| tab.layout.pane_ids())
                    .filter(|&pane_id| self.agent_info(ws_idx, pane_id).is_some())
                    .filter_map(|pane_id| self.public_pane_id(ws_idx, pane_id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Resolves a nick (`channel.send`'s `to`, or a leading in-body
    /// `@nick`) against the channel workspace's agent member panes: exact
    /// match on the raw public pane id or any of the pane's display names
    /// (agent display name -> assigned name -> detected kind — the
    /// `pane_display_name` rungs that are per-pane; the workspace
    /// custom_name rung is the channel's own `#name` for every member and
    /// carries no routing information). Exactly one match -> `Unique`;
    /// two or more -> `Ambiguous` with `pane (name)` candidate labels;
    /// none -> `Unknown`.
    fn resolve_channel_nick(&self, ws_idx: usize, nick: &str) -> NickResolution {
        let Some(ws) = self.state.workspaces.get(ws_idx) else {
            return NickResolution::Unknown;
        };
        let mut matches: Vec<(String, Option<String>)> = Vec::new();
        for pane_id in ws.tabs.iter().flat_map(|tab| tab.layout.pane_ids()) {
            let Some(info) = self.agent_info(ws_idx, pane_id) else {
                continue;
            };
            let Some(public_id) = self.public_pane_id(ws_idx, pane_id) else {
                continue;
            };
            let named = info
                .display_agent
                .as_deref()
                .or(info.name.as_deref())
                .or(info.agent.as_deref());
            if public_id == nick || named == Some(nick) {
                matches.push((public_id, named.map(str::to_string)));
            }
        }
        if matches.is_empty() {
            return NickResolution::Unknown;
        }
        if matches.len() == 1 {
            let (pane_id, _) = matches.swap_remove(0);
            return NickResolution::Unique(pane_id);
        }
        let candidates = matches
            .into_iter()
            .map(|(pane_id, name)| match name {
                Some(name) => format!("{pane_id} ({name})"),
                None => pane_id,
            })
            .collect();
        NickResolution::Ambiguous(candidates)
    }
}

/// Outcome of resolving a nick against a channel's member agents.
enum NickResolution {
    Unique(String),
    Ambiguous(Vec<String>),
    Unknown,
}

/// Extracts the leading `@nick` addressing token from raw (pre-unescape)
/// text: `@nick rest...`. The token stops at the first character outside
/// `[A-Za-z0-9._-]`, and trailing `._-` are prose punctuation and trimmed
/// (`@bora.` addresses `bora` — orc's rule). A leading `\@` never
/// addresses; the backslash is the escape.
fn leading_mention_nick(text: &str) -> Option<String> {
    let rest = text.strip_prefix('@')?;
    let mut token: String = rest
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    while token.ends_with(['.', '_', '-']) {
        token.pop();
    }
    (!token.is_empty()).then_some(token)
}

/// Unescapes `\@` and `\#` to literal `@` / `#` in stored and delivered
/// channel text, so a message can talk about the syntax without invoking
/// it. Runs after addressing, which read the raw text where the backslash
/// keeps an escaped token from addressing.
fn unescape_channel_text(text: &str) -> String {
    text.replace("\\@", "@").replace("\\#", "#")
}

fn agent_status_key(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Idle => "idle",
        AgentStatus::Working => "working",
        AgentStatus::Blocked => "blocked",
        AgentStatus::Done => "done",
        AgentStatus::Unknown => "unknown",
    }
}

fn workspace_channel_name(ws: &crate::workspace::Workspace) -> Option<&str> {
    if ws.visual_group.is_some() {
        return None;
    }
    ws.custom_name
        .as_deref()
        .and_then(|name| name.strip_prefix('#'))
}

/// Classifies a `handle_agent_prompt` response into a `ChannelDelivery`
/// status. Reads the receipt's `outcome` field directly on success (no
/// error-code sniffing): `injected` -> Delivered, `deferred` -> Deferred
/// with the queue position in `detail`. Any `error` response is a genuine
/// failure -> Failed, regardless of its code.
fn classify_delivery(pane_id: String, response: &str) -> ChannelDelivery {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(response) else {
        return ChannelDelivery {
            pane_id,
            status: ChannelDeliveryStatus::Failed,
            detail: Some("invalid agent.prompt response".to_string()),
        };
    };
    if let Some(error) = parsed.get("error") {
        let detail = error
            .get("message")
            .and_then(|m| m.as_str())
            .map(str::to_string);
        return ChannelDelivery {
            pane_id,
            status: ChannelDeliveryStatus::Failed,
            detail,
        };
    }
    let result = parsed.get("result");
    let outcome = result
        .and_then(|result| result.get("outcome"))
        .and_then(|outcome| outcome.as_str())
        .unwrap_or("injected");
    if outcome == "deferred" {
        let queue_position = result
            .and_then(|result| result.get("queue_position"))
            .and_then(serde_json::Value::as_u64);
        return ChannelDelivery {
            pane_id,
            status: ChannelDeliveryStatus::Deferred,
            detail: queue_position.map(|position| format!("queued (pos {position})")),
        };
    }
    ChannelDelivery {
        pane_id,
        status: ChannelDeliveryStatus::Delivered,
        detail: None,
    }
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::ChannelDeliveryStatus;
    use crate::config::ShellModeConfig;

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.default_shell = super::super::test_support::exiting_test_command().into();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        app
    }

    fn create_channel(app: &mut App, name: &str) -> serde_json::Value {
        let response =
            app.handle_channel_create("req".into(), ChannelCreateParams { name: name.into() });
        serde_json::from_str(&response).unwrap()
    }

    struct IsolatedStateDir {
        _guard: std::sync::MutexGuard<'static, ()>,
        old_state: Option<std::ffi::OsString>,
        dir: std::path::PathBuf,
    }

    impl IsolatedStateDir {
        fn new(name: &str) -> Self {
            let guard = crate::config::test_config_env_lock().lock().unwrap();
            let old_state = std::env::var_os("XDG_STATE_HOME");
            let dir = std::env::temp_dir().join(format!(
                "bora-channel-handler-test-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::env::set_var("XDG_STATE_HOME", &dir);
            Self {
                _guard: guard,
                old_state,
                dir,
            }
        }
    }

    impl Drop for IsolatedStateDir {
        fn drop(&mut self) {
            match self.old_state.take() {
                Some(value) => std::env::set_var("XDG_STATE_HOME", value),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn create_normalizes_name_and_rejects_duplicates() {
        let mut app = test_app();
        let created = create_channel(&mut app, "#eng");
        assert_eq!(
            created["result"]["channel"]["name"],
            serde_json::json!("#eng")
        );

        let duplicate =
            app.handle_channel_create("req2".into(), ChannelCreateParams { name: "eng".into() });
        let duplicate: serde_json::Value = serde_json::from_str(&duplicate).unwrap();
        assert_eq!(
            duplicate["error"]["code"],
            serde_json::json!("channel_exists")
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn list_reports_only_hash_named_ungrouped_workspaces() {
        let mut app = test_app();
        create_channel(&mut app, "eng");
        // A regular (non-channel) workspace should not show up.
        app.handle_workspace_create(
            "req".into(),
            crate::api::schema::WorkspaceCreateParams {
                cwd: None,
                focus: false,
                label: None,
                env: Default::default(),
                group: None,
            },
        );

        let list = app.handle_channel_list("req".into());
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        let channels = list["result"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["name"], serde_json::json!("#eng"));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn send_appends_transcript_and_reports_history() {
        let _isolated = IsolatedStateDir::new("send");
        let mut app = test_app();
        create_channel(&mut app, "eng");

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "hello".into(),
                from_pane: Some("w1A:p2".into()),
                to: None,
                in_reply_to: None,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        // No agent-hosting panes exist in the fresh channel workspace, so
        // there is nothing to classify as delivered/deferred/failed yet —
        // the append to disk is what this test actually protects.
        assert!(sent["result"]["deliveries"].as_array().unwrap().is_empty());

        let history = app.handle_channel_history(
            "req".into(),
            ChannelHistoryParams {
                name: "eng".into(),
                lines: None,
            },
        );
        let history: serde_json::Value = serde_json::from_str(&history).unwrap();
        let messages = history["result"]["messages"].as_array().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["text"], serde_json::json!("hello"));
        assert_eq!(messages[0]["from_pane"], serde_json::json!("w1A:p2"));
        assert_eq!(messages[0]["seq"], serde_json::json!(1));

        // The durable append is announced on the event hub so channel.wait
        // followers and events.wait ChannelMessage filters can wake on it.
        let events = app.event_hub.events_after(0);
        let channel_event = events
            .iter()
            .find(|(_, envelope)| {
                matches!(
                    &envelope.data,
                    crate::api::schema::EventData::ChannelMessage { channel, .. }
                        if channel == "eng"
                )
            })
            .expect("send must emit a ChannelMessage event");
        assert_eq!(
            channel_event.1.event,
            crate::api::schema::EventKind::ChannelMessage
        );
        match &channel_event.1.data {
            crate::api::schema::EventData::ChannelMessage {
                channel,
                seq,
                from_pane,
                from_name,
                text,
                to_pane,
            } => {
                assert_eq!(channel, "eng");
                assert_eq!(*seq, 1);
                assert_eq!(from_pane.as_deref(), Some("w1A:p2"));
                assert_eq!(from_name, "unknown");
                assert_eq!(text, "hello");
                assert_eq!(to_pane, &None);
            }
            other => panic!("expected ChannelMessage event data, got {other:?}"),
        }
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn history_on_missing_channel_is_empty_not_error() {
        let _isolated = IsolatedStateDir::new("missing");
        let mut app = test_app();
        let history = app.handle_channel_history(
            "req".into(),
            ChannelHistoryParams {
                name: "nope".into(),
                lines: Some(10),
            },
        );
        let history: serde_json::Value = serde_json::from_str(&history).unwrap();
        assert!(history["result"]["messages"].as_array().unwrap().is_empty());
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn queued_prompt_drop_appends_system_line_to_originating_channel_history() {
        let _isolated = IsolatedStateDir::new("drop-notice");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        app.state.ensure_test_terminals();
        let terminal_id = app.state.workspaces[ws_idx]
            .pane_state(pane_id)
            .unwrap()
            .attached_terminal_id
            .clone();
        {
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.detected_agent = Some(crate::detect::Agent::Claude);
            // Busy target: `channel.send`'s `when_idle: true` fast path defers
            // rather than injecting immediately, matching what actually happens
            // when a member agent is mid-task.
            terminal.state = crate::detect::AgentState::Working;
        }
        let public_pane_id = app.public_pane_id(ws_idx, pane_id).unwrap();

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "hello".into(),
                from_pane: Some("w1A:p2".into()),
                to: None,
                in_reply_to: None,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0]["status"], serde_json::json!("deferred"));
        assert!(app.pending_agent_prompts.contains_key(&public_pane_id));

        // The member pane disappears before the queue can ever drain to it.
        app.fail_pending_agent_prompts(&public_pane_id);

        let history = channels::read_tail("eng", 10).unwrap();
        let system_line = history
            .iter()
            .find(|message| message.from_pane == "system")
            .expect("drop of a channel-originated delivery must append a system line");
        assert_eq!(system_line.from_name, "bora");
        assert!(system_line.text.contains(&public_pane_id));
        assert!(system_line.text.contains("dropped"));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn members_on_missing_channel_is_error() {
        let _isolated = IsolatedStateDir::new("members-missing");
        let mut app = test_app();
        let response = app.handle_channel_members(
            "req".into(),
            ChannelMembersParams {
                name: "nope".into(),
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(
            response["error"]["code"],
            serde_json::json!("channel_not_found")
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn members_lists_agent_pane_with_status_and_name() {
        let _isolated = IsolatedStateDir::new("members-agent");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        app.state.ensure_test_terminals();
        let terminal_id = app.state.workspaces[ws_idx]
            .pane_state(pane_id)
            .unwrap()
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.detected_agent = Some(crate::detect::Agent::Claude);
        terminal.state = crate::detect::AgentState::Idle;

        let response =
            app.handle_channel_members("req".into(), ChannelMembersParams { name: "eng".into() });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        let members = response["result"]["members"].as_array().unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["agent_status"], serde_json::json!("idle"));
        assert!(members[0]["pane_id"].as_str().unwrap().contains(':'));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn list_reports_member_status_counts() {
        let _isolated = IsolatedStateDir::new("members-counts");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        app.state.ensure_test_terminals();
        let terminal_id = app.state.workspaces[ws_idx]
            .pane_state(pane_id)
            .unwrap()
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.detected_agent = Some(crate::detect::Agent::Claude);
        terminal.state = crate::detect::AgentState::Working;

        let list = app.handle_channel_list("req".into());
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        let channels = list["result"]["channels"].as_array().unwrap();
        assert_eq!(
            channels[0]["member_status_counts"]["working"],
            serde_json::json!(1)
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[test]
    fn classify_delivery_maps_outcomes() {
        let injected = super::classify_delivery(
            "p1".into(),
            &serde_json::json!({
                "id": "x",
                "result": {"type": "agent_prompted", "outcome": "injected"}
            })
            .to_string(),
        );
        assert_eq!(injected.status, ChannelDeliveryStatus::Delivered);

        let deferred = super::classify_delivery(
            "p2".into(),
            &serde_json::json!({
                "id": "x",
                "result": {"type": "agent_prompted", "outcome": "deferred", "queue_position": 2}
            })
            .to_string(),
        );
        assert_eq!(deferred.status, ChannelDeliveryStatus::Deferred);
        assert_eq!(deferred.detail.as_deref(), Some("queued (pos 2)"));

        let failed = super::classify_delivery(
            "p3".into(),
            &serde_json::json!({"id": "x", "error": {"code": "agent_not_ready", "message": "gone"}})
                .to_string(),
        );
        assert_eq!(failed.status, ChannelDeliveryStatus::Failed);
    }

    /// Channel workspace with two agent member panes carrying the given
    /// names. The first is idle with a test runtime (promptable ->
    /// `delivered`; its receiver is returned and must stay alive or the
    /// runtime's send channel closes); the second is working (no runtime
    /// needed -> `deferred`). Returns both public pane ids.
    fn channel_with_two_agents(
        app: &mut App,
        first_name: &str,
        second_name: &str,
    ) -> (String, String, tokio::sync::mpsc::Receiver<bytes::Bytes>) {
        create_channel(app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let first = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let second =
            app.state.workspaces[ws_idx].test_split(ratatui::layout::Direction::Horizontal);
        app.state.ensure_test_terminals();
        for (pane, name, state) in [
            (first, first_name, crate::detect::AgentState::Idle),
            (second, second_name, crate::detect::AgentState::Working),
        ] {
            let terminal_id = app.state.workspaces[ws_idx]
                .pane_state(pane)
                .unwrap()
                .attached_terminal_id
                .clone();
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.set_agent_name(name.into());
            terminal.set_detected_state(Some(crate::detect::Agent::OpenCode), state);
        }
        let (runtime, rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(first, runtime);
        (
            app.public_pane_id(ws_idx, first).unwrap(),
            app.public_pane_id(ws_idx, second).unwrap(),
            rx,
        )
    }

    #[tokio::test]
    async fn send_to_param_targets_unique_nick_and_threads_reply_by_pane_id() {
        let _isolated = IsolatedStateDir::new("send-to-unique");
        let mut app = test_app();
        let (reviewer, worker, mut rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some("reviewer".into()),
                in_reply_to: None,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(
            deliveries.len(),
            1,
            "targeted send reaches exactly one pane"
        );
        assert_eq!(deliveries[0]["pane_id"], serde_json::json!(reviewer));
        assert_eq!(
            deliveries[0]["status"],
            serde_json::json!("delivered"),
            "delivery detail: {:?}",
            deliveries[0]["detail"]
        );

        // The target's runtime actually received the prefixed message.
        let injected = rx
            .try_recv()
            .expect("targeted delivery must inject into the target pane");
        let injected = String::from_utf8_lossy(&injected);
        assert!(injected.contains("[#eng from "), "got: {injected}");
        assert!(injected.contains("ping"), "got: {injected}");

        // Reply addressed by raw pane id, threaded back to seq 1.
        let reply = app.handle_channel_send(
            "req2".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "pong".into(),
                from_pane: Some(reviewer.clone()),
                to: Some(worker.clone()),
                in_reply_to: Some(1),
            },
        );
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        let deliveries = reply["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0]["pane_id"], serde_json::json!(worker));
        assert_eq!(deliveries[0]["status"], serde_json::json!("deferred"));

        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].seq, 1);
        assert_eq!(history[0].to_pane.as_deref(), Some(reviewer.as_str()));
        assert_eq!(history[0].in_reply_to, None);
        assert_eq!(history[1].seq, 2, "seq stays monotonic across sends");
        assert_eq!(history[1].to_pane.as_deref(), Some(worker.as_str()));
        assert_eq!(history[1].in_reply_to, Some(1));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn send_to_param_ambiguous_nick_errors_with_candidates() {
        let _isolated = IsolatedStateDir::new("send-to-ambiguous");
        let mut app = test_app();
        let (first, second, _rx) = channel_with_two_agents(&mut app, "dup", "dup");

        let error = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "which one?".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some("dup".into()),
                in_reply_to: None,
            },
        );
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(
            error["error"]["code"],
            serde_json::json!("channel_nick_ambiguous")
        );
        let message = error["error"]["message"].as_str().unwrap();
        assert!(
            message.contains(&first),
            "candidates must list {first}: {message}"
        );
        assert!(
            message.contains(&second),
            "candidates must list {second}: {message}"
        );
        assert!(
            channels::read_tail("eng", 10).unwrap().is_empty(),
            "a failed structured addressing must append nothing"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn send_to_param_unknown_nick_errors() {
        let _isolated = IsolatedStateDir::new("send-to-unknown");
        let mut app = test_app();
        let (_, _, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        let error = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "anyone?".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some("ghost".into()),
                in_reply_to: None,
            },
        );
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(
            error["error"]["code"],
            serde_json::json!("channel_nick_unknown")
        );
        assert!(
            channels::read_tail("eng", 10).unwrap().is_empty(),
            "a failed structured addressing must append nothing"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_send_rate_limits_repeated_from_pane_but_exempts_missing_from_pane() {
        let _isolated = IsolatedStateDir::new("send-rate-limit");
        let mut app = test_app();
        let (_, _, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        let first = app.handle_channel_send(
            "req1".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "primeira".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
            },
        );
        assert!(
            first.contains("channel_sent"),
            "first send must pass: {first}"
        );

        let second = app.handle_channel_send(
            "req2".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "segunda".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
            },
        );
        let error: serde_json::Value = serde_json::from_str(&second).unwrap();
        assert_eq!(
            error["error"]["code"],
            serde_json::json!("channel_send_rate_limited")
        );
        assert_eq!(
            channels::read_tail("eng", 10).unwrap().len(),
            1,
            "a rate-limited send must append nothing"
        );

        // No-from sends (CLI/human) stay exempt, mirroring no-from prompts.
        let third = app.handle_channel_send(
            "req3".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "cli".into(),
                from_pane: None,
                to: None,
                in_reply_to: None,
            },
        );
        assert!(
            third.contains("channel_sent"),
            "no-from send must pass: {third}"
        );
        let fourth = app.handle_channel_send(
            "req4".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "cli de novo".into(),
                from_pane: None,
                to: None,
                in_reply_to: None,
            },
        );
        assert!(
            fourth.contains("channel_sent"),
            "no-from resend must pass: {fourth}"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn leading_mention_targets_unique_nick() {
        let _isolated = IsolatedStateDir::new("mention-unique");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@reviewer please look".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0]["pane_id"], serde_json::json!(reviewer));
        assert_ne!(deliveries[0]["pane_id"], serde_json::json!(worker));

        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history[0].to_pane.as_deref(), Some(reviewer.as_str()));
        assert_eq!(history[0].text, "@reviewer please look");
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn leading_mention_degrades_to_broadcast_when_not_uniquely_resolvable() {
        let _isolated = IsolatedStateDir::new("mention-degrade");
        let mut app = test_app();
        let (_, _, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        // Unknown nick: literal broadcast, message unchanged, never an error.
        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@ghost are you here".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert!(sent["error"].is_null(), "in-body tokens never error");
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(
            deliveries.len(),
            2,
            "unknown mention broadcasts to everyone"
        );
        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history[0].to_pane, None);
        assert_eq!(history[0].text, "@ghost are you here");

        // Ambiguous nick (two agents named "dup"): also literal broadcast.
        let mut app2 = test_app();
        let (_, _, _rx2) = channel_with_two_agents(&mut app2, "dup", "dup");
        let sent2 = app2.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@dup pick one".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
            },
        );
        let sent2: serde_json::Value = serde_json::from_str(&sent2).unwrap();
        assert!(sent2["error"].is_null(), "in-body tokens never error");
        assert_eq!(sent2["result"]["deliveries"].as_array().unwrap().len(), 2);
        // Both apps share the process-global isolated state dir, so the
        // ambiguous send's line is the newest one in the shared transcript.
        let last2 = channels::read_tail("eng", 10)
            .unwrap()
            .pop()
            .expect("ambiguous in-body send must append");
        assert_eq!(last2.to_pane, None);
        assert_eq!(last2.text, "@dup pick one");
        super::super::test_support::shutdown_test_runtimes(&mut app);
        super::super::test_support::shutdown_test_runtimes(&mut app2);
    }

    #[tokio::test]
    async fn escapes_unescape_to_literal_and_never_address() {
        let _isolated = IsolatedStateDir::new("escapes");
        let mut app = test_app();
        let (reviewer, _worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        // `\@` keeps the mention from addressing (raw text starts with the
        // backslash), and both escapes unescape in the stored text.
        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "\\@reviewer hi \\#eng".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert!(sent["error"].is_null());
        assert_eq!(
            sent["result"]["deliveries"].as_array().unwrap().len(),
            2,
            "escaped mention broadcasts, it does not target"
        );
        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history[0].to_pane, None);
        assert_eq!(history[0].text, "@reviewer hi #eng");

        // Escapes still unescape on a targeted send. Sent from a different
        // pane: the same sender would trip the per-(pane, channel) rate limit.
        let targeted = app.handle_channel_send(
            "req2".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "see \\#eng notes".into(),
                from_pane: Some("w1A:p8".into()),
                to: Some("reviewer".into()),
                in_reply_to: None,
            },
        );
        let targeted: serde_json::Value = serde_json::from_str(&targeted).unwrap();
        assert_eq!(
            targeted["result"]["deliveries"].as_array().unwrap().len(),
            1
        );
        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history[1].to_pane.as_deref(), Some(reviewer.as_str()));
        assert_eq!(history[1].text, "see #eng notes");
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn targeted_send_to_sender_pane_appends_without_delivery() {
        let _isolated = IsolatedStateDir::new("self-target");
        let mut app = test_app();
        let (reviewer, _worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "note to self".into(),
                from_pane: Some(reviewer.clone()),
                to: Some("reviewer".into()),
                in_reply_to: None,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert!(
            sent["result"]["deliveries"].as_array().unwrap().is_empty(),
            "the sender pane never receives its own message injection"
        );
        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(
            history.len(),
            1,
            "the message is still part of the transcript"
        );
        assert_eq!(history[0].to_pane.as_deref(), Some(reviewer.as_str()));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[test]
    fn leading_mention_nick_parses_token_boundaries() {
        assert_eq!(leading_mention_nick("@rev hi"), Some("rev".into()));
        // Prose punctuation ends the token and trailing `._-` is trimmed.
        assert_eq!(leading_mention_nick("@rev, hi"), Some("rev".into()));
        assert_eq!(leading_mention_nick("@rev.. hi"), Some("rev".into()));
        assert_eq!(leading_mention_nick("@a.b-_c hi"), Some("a.b-_c".into()));
        // Escaped, empty, mid-body, or absent tokens never address.
        assert_eq!(leading_mention_nick("\\@rev hi"), None);
        assert_eq!(leading_mention_nick("@ hi"), None);
        assert_eq!(leading_mention_nick("hi @rev"), None);
        assert_eq!(leading_mention_nick("plain text"), None);
    }
}
