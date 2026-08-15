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

        let sender_pane = params.from_pane.unwrap_or_default();
        let sender_name = self
            .pane_display_name(&sender_pane)
            .unwrap_or_else(|| "unknown".to_string());
        let message = ChannelMessage {
            ts: now_rfc3339(),
            from_pane: sender_pane.clone(),
            from_name: sender_name.clone(),
            text: params.text.clone(),
        };
        if let Err(err) = channels::append_message(&name, &message) {
            return encode_error(id, "channel_send_failed", err.to_string());
        }

        // The prefix is built here (not delegated to `handle_agent_prompt`'s
        // own from_pane attribution) so the delivered text carries the
        // channel name too; from_pane is passed as None below to avoid a
        // second `[from ...]` prefix being layered on top of this one.
        let prefixed = format!("[#{name} from {sender_pane} {sender_name}] {}", params.text);

        let member_pane_ids: Vec<crate::layout::PaneId> = self
            .state
            .workspaces
            .get(ws_idx)
            .map(|ws| {
                ws.tabs
                    .iter()
                    .flat_map(|tab| tab.layout.pane_ids())
                    .collect()
            })
            .unwrap_or_default();
        let targets: Vec<String> = member_pane_ids
            .into_iter()
            .filter(|&pane_id| self.agent_info(ws_idx, pane_id).is_some())
            .filter_map(|pane_id| self.public_pane_id(ws_idx, pane_id))
            .collect();
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

    /// The workspace custom_name of the pane's workspace, or its detected
    /// agent name, whichever resolves — used to attribute a sender.
    fn pane_display_name(&self, public_pane_id: &str) -> Option<String> {
        let (ws_idx, pane_id) = self.parse_pane_id(public_pane_id)?;
        let ws = self.state.workspaces.get(ws_idx)?;
        if let Some(custom_name) = ws.custom_name.clone() {
            return Some(custom_name);
        }
        self.agent_info(ws_idx, pane_id)?.name
    }
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
}
