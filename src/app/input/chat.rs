//! Chat view input handling and data flow.
//!
//! The chat view is TUI presentation state (`ChatViewState`) fed through the
//! same channel JSON API external clients use (`channel.list`,
//! `channel.history`, `channel.members`, `channel.send`). Live updates ride
//! the append hook in `handle_channel_send` — no polling loop.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::api::schema::{
    ChannelHistoryParams, ChannelMembersParams, ChannelMessage, ChannelSendParams, EmptyParams,
    Method, ResponseResult, SuccessResponse,
};
use crate::app::state::{AppState, Mode};
use crate::app::App;

use super::modal::leave_modal;

impl App {
    /// Open the chat view and fetch initial data through the channel API.
    /// No-op unless the `ui.chat_view` config flag is on (fork-only surface).
    pub(crate) fn open_chat_view(&mut self) {
        if !self.state.chat_view {
            return;
        }
        self.state.open_chat_view();
        self.refresh_chat_channels();
        self.refresh_chat_channel_data();
    }

    fn refresh_chat_channels(&mut self) {
        let response = self.dispatch_api_request(
            "tui.chat.channel_list",
            Method::ChannelList(EmptyParams::default()),
        );
        let Ok(parsed) = serde_json::from_str::<SuccessResponse>(&response) else {
            self.state.chat.status = Some("channel.list failed".into());
            return;
        };
        if let ResponseResult::ChannelList { channels } = parsed.result {
            self.state.chat.selected = self.state.chat.selected.min(channels.len());
            self.state.chat.channels = channels;
        }
    }

    /// Fetch history + members for the selected channel.
    fn refresh_chat_channel_data(&mut self) {
        let Some(name) = self.state.selected_chat_channel_name().map(str::to_string) else {
            return;
        };
        let history = self.dispatch_api_request(
            "tui.chat.channel_history",
            Method::ChannelHistory(ChannelHistoryParams {
                name: name.clone(),
                lines: None,
            }),
        );
        if let Ok(parsed) = serde_json::from_str::<SuccessResponse>(&history) {
            if let ResponseResult::ChannelHistory { messages } = parsed.result {
                self.state.chat.messages = messages;
                self.state.reset_chat_scroll_to_bottom();
            }
        }
        let members = self.dispatch_api_request(
            "tui.chat.channel_members",
            Method::ChannelMembers(ChannelMembersParams { name }),
        );
        if let Ok(parsed) = serde_json::from_str::<SuccessResponse>(&members) {
            if let ResponseResult::ChannelMembers { members } = parsed.result {
                self.state.chat.members = members;
            }
        }
    }

    /// Select a channel row (mouse click / keyboard) and reload its data.
    pub(crate) fn select_chat_channel(&mut self, idx: usize) {
        if idx >= self.state.chat.channels.len() {
            return;
        }
        self.state.chat.selected = idx;
        self.refresh_chat_channel_data();
    }

    /// Send the compose buffer through `channel.send`.
    fn send_chat_input(&mut self) {
        let text = self.state.chat.input.trim().to_string();
        if text.is_empty() {
            return;
        }
        let Some(name) = self.state.selected_chat_channel_name().map(str::to_string) else {
            return;
        };
        let response = self.dispatch_api_request(
            "tui.chat.channel_send",
            Method::ChannelSend(ChannelSendParams {
                name,
                text,
                from_pane: None,
                to: None,
                in_reply_to: None,
                // Trust anchor: in-process only, never deserializable from a
                // socket body. This is the one place the human seat is claimed.
                from_human: true,
            }),
        );
        let Ok(parsed) = serde_json::from_str::<SuccessResponse>(&response) else {
            // Keep the input so the user can retry; surface why it failed.
            let message = serde_json::from_str::<serde_json::Value>(&response)
                .ok()
                .and_then(|value| value["error"]["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| "send failed".into());
            self.state.chat.status = Some(message);
            return;
        };
        let _ = parsed;
        self.state.chat.status = None;
        self.state.chat.input.clear();
    }

    pub(crate) fn handle_chat_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => leave_modal(&mut self.state),
            KeyCode::Enter => self.send_chat_input(),
            KeyCode::Backspace => {
                self.state.chat.input.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.chat.input.clear();
            }
            KeyCode::Up => self.state.scroll_chat(-3),
            KeyCode::Down => self.state.scroll_chat(3),
            KeyCode::PageUp => {
                let page = self.state.chat_messages_viewport() as isize;
                self.state.scroll_chat(-page.max(1));
            }
            KeyCode::PageDown => {
                let page = self.state.chat_messages_viewport() as isize;
                self.state.scroll_chat(page.max(1));
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if self.state.chat.channels.len() >= 2 {
                    let next = (self.state.chat.selected + 1) % self.state.chat.channels.len();
                    self.select_chat_channel(next);
                }
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.state.chat.input.push(c);
            }
            _ => {}
        }
    }

    /// Toast for an agent message addressed to the human seat when the chat
    /// view is closed. While the view is open the highlighted timeline line
    /// is the surface — and toasts render below interactive overlays, so a
    /// toast would be hidden there anyway. Reuses the existing toast
    /// notification surface (`ui.toast`); no new subsystem.
    pub(crate) fn notify_chat_to_human(&mut self, channel: &str, message: &ChannelMessage) {
        if !message.to_human || self.state.mode == Mode::Chat {
            return;
        }
        let previous_toast = self.state.toast.clone();
        self.state.toast = Some(crate::app::state::ToastNotification {
            kind: crate::app::state::ToastKind::NeedsAttention,
            title: format!("{} › you", message.from_name),
            context: format!("#{channel}"),
            position: None,
            target: None,
        });
        self.sync_toast_deadline(previous_toast);
        self.render_dirty.request_generic();
        self.render_notify.notify_one();
    }
}

impl AppState {
    /// Pure state half of opening: reset view state and switch modes. The
    /// App-level `open_chat_view` performs the data fetch.
    pub(crate) fn open_chat_view(&mut self) {
        self.chat.selected = 0;
        self.chat.scroll = 0;
        self.chat.input.clear();
        self.chat.status = None;
        self.mode = Mode::Chat;
    }

    pub(crate) fn selected_chat_channel_name(&self) -> Option<&str> {
        self.chat
            .channels
            .get(self.chat.selected)
            .map(|channel| channel.name.as_str())
    }

    /// Wheel / key scrolling of the message area, in wrapped display lines.
    pub(crate) fn scroll_chat(&mut self, delta: isize) {
        let max = self.chat_max_scroll() as isize;
        let next = self.chat.scroll as isize + delta;
        self.chat.scroll = next.clamp(0, max).max(0) as usize;
    }

    pub(crate) fn chat_messages_viewport(&self) -> usize {
        self.chat_messages_rect().height.max(1) as usize
    }

    pub(crate) fn chat_max_scroll(&self) -> usize {
        let lines = crate::ui::chat_display_line_count(&self.chat, self.chat_messages_width());
        lines.saturating_sub(self.chat_messages_viewport())
    }

    pub(crate) fn reset_chat_scroll_to_bottom(&mut self) {
        self.chat.scroll = self.chat_max_scroll();
    }

    /// Live-update hook called by the channel send path after a successful
    /// append. Only the open, matching channel consumes it.
    pub(crate) fn push_chat_message(&mut self, channel: &str, message: ChannelMessage) {
        if self.mode != Mode::Chat {
            return;
        }
        // Summaries carry the `#` prefix; the append path passes the
        // normalized (hashless) name.
        let viewing = self
            .selected_chat_channel_name()
            .map(crate::persist::channels::normalize_channel_name);
        if viewing.as_deref() != Some(channel) {
            return;
        }
        let at_bottom = self.chat.scroll >= self.chat_max_scroll();
        self.chat.messages.push(message);
        if at_bottom {
            self.chat.scroll = self.chat_max_scroll();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{ChannelMessage, ChannelSenderKind, ChannelSummary};

    fn channel(name: &str) -> ChannelSummary {
        ChannelSummary {
            name: name.into(),
            pane_count: 2,
            agent_count: 1,
            member_status_counts: Default::default(),
        }
    }

    fn message(text: &str) -> ChannelMessage {
        ChannelMessage {
            ts: "2026-08-15T15:31:02Z".into(),
            seq: 1,
            from_pane: "w1:p1".into(),
            from_name: "builder".into(),
            from_kind: ChannelSenderKind::Agent,
            text: text.into(),
            in_reply_to: None,
            to_pane: None,
            to_human: false,
        }
    }

    fn to_human_message(from_name: &str, text: &str) -> ChannelMessage {
        ChannelMessage {
            from_pane: String::new(),
            from_name: from_name.into(),
            from_kind: ChannelSenderKind::Agent,
            to_human: true,
            ..message(text)
        }
    }

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(
            &crate::config::Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    #[test]
    fn open_chat_view_switches_mode_and_resets_state() {
        let mut state = AppState::test_new();
        state.chat.input = "leftover".into();
        state.chat.selected = 3;

        state.open_chat_view();

        assert_eq!(state.mode, Mode::Chat);
        assert_eq!(state.chat.selected, 0);
        assert_eq!(state.chat.scroll, 0);
        assert!(state.chat.input.is_empty());
        assert!(state.chat.status.is_none());
    }

    #[test]
    fn chat_input_buffer_appends_and_backspaces() {
        let mut state = AppState::test_new();
        state.open_chat_view();

        state.chat.input.push('h');
        state.chat.input.push('i');
        assert_eq!(state.chat.input, "hi");
        state.chat.input.pop();
        assert_eq!(state.chat.input, "h");
        state.chat.input.clear();
        assert!(state.chat.input.is_empty());
    }

    #[test]
    fn selected_channel_name_follows_selection() {
        let mut state = AppState::test_new();
        state.open_chat_view();
        state.chat.channels = vec![channel("#a"), channel("#b")];

        assert_eq!(state.selected_chat_channel_name(), Some("#a"));
        state.chat.selected = 1;
        assert_eq!(state.selected_chat_channel_name(), Some("#b"));
        // Out-of-range selection yields nothing instead of panicking.
        state.chat.selected = 9;
        assert_eq!(state.selected_chat_channel_name(), None);
    }

    #[test]
    fn push_chat_message_only_when_viewing_that_channel() {
        let mut state = AppState::test_new();
        state.push_chat_message("a", message("not open yet"));
        assert!(
            state.chat.messages.is_empty(),
            "closed view ignores appends"
        );

        state.open_chat_view();
        state.chat.channels = vec![channel("#a"), channel("#b")];

        state.push_chat_message("a", message("visible"));
        assert_eq!(state.chat.messages.len(), 1);

        state.push_chat_message("b", message("other channel"));
        assert_eq!(
            state.chat.messages.len(),
            1,
            "non-viewed channel messages are not shown inline"
        );
    }

    #[test]
    fn chat_scroll_clamps_to_bounds() {
        let mut state = AppState::test_new();
        state.open_chat_view();
        state.chat.scroll = 5;

        state.scroll_chat(-99);
        assert_eq!(state.chat.scroll, 0);
        state.scroll_chat(3);
        // No view geometry in tests -> max scroll is 0, so scrolling up
        // saturates back to the top.
        assert_eq!(state.chat.scroll, 0);
    }

    #[test]
    fn to_human_message_toasts_only_when_chat_view_is_closed() {
        let mut app = test_app();
        app.state.mode = Mode::Terminal; // any non-Chat mode = view closed

        app.notify_chat_to_human("design", &to_human_message("builder", "status?"));

        let toast = app.state.toast.as_ref().expect("toast fires while closed");
        assert_eq!(toast.kind, crate::app::state::ToastKind::NeedsAttention);
        assert_eq!(toast.title, "builder › you");
        assert_eq!(toast.context, "#design");
        assert!(
            toast.target.is_none(),
            "no pane to focus for the human seat"
        );

        // The trigger is view state, not the message: the same message with
        // the view open must not stack a toast under the overlay.
        app.state.open_chat_view();
        app.state.toast = None; // the earlier toast stays until its deadline
        app.notify_chat_to_human("design", &to_human_message("builder", "again"));
        assert!(app.state.toast.is_none(), "open view is its own surface");
    }

    #[test]
    fn broadcast_message_never_toasts_the_human() {
        let mut app = test_app();

        app.notify_chat_to_human("design", &message("plain broadcast"));

        assert!(
            app.state.toast.is_none(),
            "to_human=false is ordinary channel chatter"
        );
    }
}
