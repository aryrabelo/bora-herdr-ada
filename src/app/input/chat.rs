//! Chat view input handling and data flow.
//!
//! The chat view is TUI presentation state (`ChatViewState`) fed through the
//! same channel JSON API external clients use (`channel.list`,
//! `channel.history`, `channel.members`, `channel.send`, `channel.join`,
//! `channel.leave`, `agent.list`). Live updates ride the append hook in
//! `handle_channel_send` — no polling loop.
//!
//! Membership is managed in-view: the members column carries an `+ add agent`
//! affordance and a per-row remove control, and `ChatPrompt` is the modal
//! sub-mode those affordances open. Everything here is presentation and API
//! plumbing — the server stays the authority on who is a member.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::api::schema::{
    ChannelCreateParams, ChannelHistoryParams, ChannelJoinParams, ChannelLeaveParams,
    ChannelMemberSource, ChannelMembersParams, ChannelMessage, ChannelSendParams, EmptyParams,
    Method, ResponseResult, SuccessResponse,
};
use crate::app::state::{AppState, ChatMemberCandidate, ChatPrompt, Mode};
use crate::app::App;

use super::modal::leave_modal;
use super::overlays::rect_contains;

/// Candidate rows the add-member prompt shows at once; longer lists scroll
/// the selection into view.
const CHAT_PROMPT_ROWS: u16 = 8;

/// How recently the human must have typed for the mention auto-open to
/// stand down: never steal focus from someone mid-keystroke.
const CHAT_AUTO_OPEN_TYPING_WINDOW: std::time::Duration = std::time::Duration::from_secs(3);

/// Modes the chat view may auto-open over. An explicit allowlist of the
/// quiet browsing modes, so any mode added later defaults to "do not
/// hijack" — onboarding, prompts, and modals are never interrupted.
fn chat_auto_open_allowed(mode: Mode) -> bool {
    matches!(mode, Mode::Terminal | Mode::Navigate)
}

/// The `error.message` an API rejection carries, for the chat status line.
/// Falls back to `fallback` when the response is not a recognizable error.
fn api_error_message(response: &str, fallback: &str) -> String {
    serde_json::from_str::<serde_json::Value>(response)
        .ok()
        .and_then(|value| value["error"]["message"].as_str().map(str::to_string))
        .unwrap_or_else(|| fallback.to_string())
}

/// The `result` of a successful API response, or `None` when the call was
/// rejected or unparsable.
fn chat_api_result(response: &str) -> Option<ResponseResult> {
    serde_json::from_str::<SuccessResponse>(response)
        .ok()
        .map(|parsed| parsed.result)
}

/// Running agents that are not already members of the channel. Panes are
/// matched by public pane id — the id `channel.join` and `channel.members`
/// both speak — so a member never shows up as someone to add.
fn candidates_from_agents(
    agents: &[crate::api::schema::AgentInfo],
    members: &[crate::api::schema::ChannelMember],
) -> Vec<ChatMemberCandidate> {
    let joined: std::collections::HashSet<&str> = members
        .iter()
        .map(|member| member.pane_id.as_str())
        .collect();
    agents
        .iter()
        .filter(|agent| !joined.contains(agent.pane_id.as_str()))
        .map(|agent| ChatMemberCandidate {
            pane_id: agent.pane_id.clone(),
            name: agent
                .name
                .clone()
                .or_else(|| agent.display_agent.clone())
                .unwrap_or_else(|| agent.pane_id.clone()),
            cwd: agent.cwd.as_deref().map(short_cwd),
            status: crate::ui::agent_status_label(agent.agent_status).to_string(),
        })
        .collect()
}

/// Substring match over name and cwd; an empty needle matches everything.
fn candidate_matches(candidate: &ChatMemberCandidate, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    candidate.name.to_lowercase().contains(needle)
        || candidate
            .cwd
            .as_deref()
            .is_some_and(|cwd| cwd.to_lowercase().contains(needle))
}

/// Enough of a working directory to tell same-named agents apart: the last
/// two components, since the leading path is identical across a fleet of
/// worktrees. `~/Sites/bora-worktrees/chat-view` -> `bora-worktrees/chat-view`.
fn short_cwd(cwd: &str) -> String {
    let mut parts = cwd
        .trim_end_matches('/')
        .rsplit('/')
        .filter(|p| !p.is_empty());
    let Some(leaf) = parts.next() else {
        return cwd.to_string();
    };
    match parts.next() {
        Some(parent) => format!("{parent}/{leaf}"),
        None => leaf.to_string(),
    }
}

/// What a left-click in the members column landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatMembersHit {
    /// The member's name: insert `@<name> ` into the composer.
    Mention(usize),
    /// The explicit remove control at the right edge of the row.
    Remove(usize),
    /// The `+ add agent` footer row.
    AddAgent,
}

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

    /// Open the create-a-channel prompt (the `+` row in the channel column,
    /// or Ctrl+N). Clears any stale error so the box opens clean.
    pub(crate) fn open_new_channel_prompt(&mut self) {
        self.state.chat.prompt = Some(ChatPrompt::NewChannel {
            input: String::new(),
        });
        self.state.chat.status = None;
    }

    /// Confirm the create-a-channel prompt: `channel.create`, then reload the
    /// list and select the new room so the human lands where they just made.
    /// A rejected create (duplicate name, creation failure) keeps the prompt
    /// open with the typed text intact and reports why on the status line.
    pub(crate) fn submit_new_channel_prompt(&mut self) {
        let Some(ChatPrompt::NewChannel { input }) = self.state.chat.prompt.as_ref() else {
            return;
        };
        // `channel.create` normalizes too, but a typed `#eng` must not reach
        // the API as `##eng` once the summary prefix is re-applied.
        let name = crate::persist::channels::normalize_channel_name(input);
        if name.is_empty() {
            // Nothing typed yet: Enter is a no-op, the prompt stays open.
            return;
        }
        let response = self.dispatch_api_request(
            "tui.chat.channel_create",
            Method::ChannelCreate(ChannelCreateParams { name: name.clone() }),
        );
        if serde_json::from_str::<SuccessResponse>(&response).is_err() {
            self.state.chat.status = Some(api_error_message(&response, "create failed"));
            return;
        }
        self.state.chat.prompt = None;
        self.state.chat.status = None;
        self.refresh_chat_channels();
        if let Some(idx) = self.state.chat.channels.iter().position(|channel| {
            crate::persist::channels::normalize_channel_name(&channel.name) == name
        }) {
            self.select_chat_channel(idx);
        }
        // A channel is a workspace: the sidebar gains a row, so pane content
        // reflows without the outer terminal size changing.
        self.state.request_full_repaint();
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
            self.state.chat.status = Some(api_error_message(&response, "send failed"));
            return;
        };
        let _ = parsed;
        self.state.chat.status = None;
        self.state.chat.input.clear();
    }

    /// Open the add-member prompt for the selected channel, listing every
    /// running agent that is not already a member. Uses `agent.list` — the
    /// existing agent inventory — rather than a second discovery path.
    pub(crate) fn open_chat_add_member(&mut self) {
        if self.state.selected_chat_channel_name().is_none() {
            return;
        }
        // The cached members list can be stale (someone joined or left through
        // the API since the last fetch), and a stale list would either hide a
        // stranger or offer an existing member. Re-read it first.
        self.refresh_chat_channel_data();
        let candidates = self.chat_member_candidates();
        if candidates.is_empty() {
            self.state.chat.status = Some("no agents left to add".into());
            return;
        }
        self.state.chat.status = None;
        self.state.chat.prompt = Some(ChatPrompt::AddMember {
            query: String::new(),
            selected: 0,
            candidates,
        });
    }

    /// Running agents minus the ones already in the selected channel.
    fn chat_member_candidates(&mut self) -> Vec<ChatMemberCandidate> {
        let response = self.dispatch_api_request(
            "tui.chat.agent_list",
            Method::AgentList(EmptyParams::default()),
        );
        let Ok(parsed) = serde_json::from_str::<SuccessResponse>(&response) else {
            self.state.chat.status = Some("agent.list failed".into());
            return Vec::new();
        };
        let ResponseResult::AgentList { agents } = parsed.result else {
            return Vec::new();
        };
        candidates_from_agents(&agents, &self.state.chat.members)
    }

    /// Join the candidate at `idx` of the prompt's filtered view.
    fn join_chat_candidate_at(&mut self, idx: usize) {
        let Some(candidate) = self
            .state
            .chat_prompt_candidates()
            .get(idx)
            .map(|candidate| (*candidate).clone())
        else {
            return;
        };
        self.join_chat_candidate(candidate);
    }

    /// `channel.join` the candidate, then refresh the members column so the
    /// human sees the membership land instead of taking our word for it.
    fn join_chat_candidate(&mut self, candidate: ChatMemberCandidate) {
        let Some(name) = self.state.selected_chat_channel_name().map(str::to_string) else {
            return;
        };
        let response = self.dispatch_api_request(
            "tui.chat.channel_join",
            Method::ChannelJoin(ChannelJoinParams {
                name,
                pane: candidate.pane_id.clone(),
            }),
        );
        self.state.chat.prompt = None;
        match chat_api_result(&response) {
            Some(ResponseResult::ChannelJoined { source, .. }) => {
                // A workspace-resident pane was a member all along: the call
                // succeeded but changed nothing, so say that rather than
                // implying we added anyone.
                self.state.chat.status = match source {
                    ChannelMemberSource::Workspace => Some(format!(
                        "{} already belongs to this channel's workspace",
                        candidate.name
                    )),
                    ChannelMemberSource::Joined => None,
                };
                self.refresh_chat_channel_data();
            }
            _ => {
                self.state.chat.status = Some(api_error_message(&response, "channel.join failed"));
            }
        }
    }

    /// `channel.leave` the member at `idx`. Workspace-resident members are
    /// members by construction: the server answers `removed: false` and that
    /// is what the status line reports — no pretending the click worked.
    fn remove_chat_member(&mut self, idx: usize) {
        let Some(member) = self.state.chat.members.get(idx).cloned() else {
            return;
        };
        let Some(name) = self.state.selected_chat_channel_name().map(str::to_string) else {
            return;
        };
        let label = member
            .name
            .clone()
            .unwrap_or_else(|| member.pane_id.clone());
        let response = self.dispatch_api_request(
            "tui.chat.channel_leave",
            Method::ChannelLeave(ChannelLeaveParams {
                name: name.clone(),
                pane: member.pane_id,
            }),
        );
        match chat_api_result(&response) {
            Some(ResponseResult::ChannelLeft { removed: true, .. }) => {
                self.state.chat.status = None;
                self.refresh_chat_channel_data();
            }
            Some(ResponseResult::ChannelLeft { removed: false, .. }) => {
                self.state.chat.status =
                    Some(format!("{label} lives in {name} — cannot be removed"));
            }
            _ => {
                self.state.chat.status = Some(api_error_message(&response, "channel.leave failed"));
            }
        }
    }

    /// Click-to-mention: insert `@<name> ` at the composer cursor so
    /// addressing a member never means typing their nick from memory. The
    /// composer is append-only, so the cursor is the end of the buffer.
    fn mention_chat_member(&mut self, idx: usize) {
        let Some(member) = self.state.chat.members.get(idx) else {
            return;
        };
        let name = member
            .name
            .clone()
            .unwrap_or_else(|| member.pane_id.clone());
        let input = &mut self.state.chat.input;
        if !input.is_empty() && !input.ends_with(' ') {
            input.push(' ');
        }
        input.push('@');
        input.push_str(&name);
        input.push(' ');
    }

    /// Left-click inside the chat overlay. An open prompt is modal, so it
    /// consumes clicks first; then the members column affordances, then the
    /// channel list, then a click outside the popup closes the view.
    pub(crate) fn handle_chat_click(&mut self, col: u16, row: u16) {
        if self.state.chat.prompt.is_some() {
            if let Some(idx) = self.state.chat_prompt_candidate_at(col, row) {
                self.join_chat_candidate_at(idx);
            } else if !self.state.chat_prompt_contains(col, row) {
                self.state.chat.prompt = None;
            }
            return;
        }
        match self.state.chat_members_hit_at(col, row) {
            Some(ChatMembersHit::AddAgent) => return self.open_chat_add_member(),
            Some(ChatMembersHit::Remove(idx)) => return self.remove_chat_member(idx),
            Some(ChatMembersHit::Mention(idx)) => return self.mention_chat_member(idx),
            None => {}
        }
        if self.state.chat_new_channel_hit(col, row) {
            return self.open_new_channel_prompt();
        }
        if let Some(idx) = self.state.chat_channel_index_at(col, row) {
            self.select_chat_channel(idx);
        } else if !self.state.chat_popup_contains(col, row) {
            self.close_chat_view();
        }
    }

    /// Keys while a prompt is open. `Esc` cancels the prompt only; the chat
    /// view stays up.
    fn handle_chat_prompt_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.state.chat.prompt = None,
            KeyCode::Enter => self.submit_chat_prompt(),
            KeyCode::Backspace => self.state.chat_prompt_pop(),
            KeyCode::Up => self.state.move_chat_prompt_selection(-1),
            KeyCode::Down => self.state.move_chat_prompt_selection(1),
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.state.chat_prompt_push(c)
            }
            _ => {}
        }
    }

    /// Confirm the open prompt: join the highlighted candidate, or create the
    /// typed channel.
    fn submit_chat_prompt(&mut self) {
        match &self.state.chat.prompt {
            Some(ChatPrompt::AddMember { selected, .. }) => {
                let selected = *selected;
                self.join_chat_candidate_at(selected)
            }
            Some(ChatPrompt::NewChannel { .. }) => self.submit_new_channel_prompt(),
            None => {}
        }
    }

    pub(crate) fn handle_chat_key(&mut self, key: KeyEvent) {
        // An open prompt owns the keyboard; the composer sees nothing.
        if self.state.chat.prompt.is_some() {
            return self.handle_chat_prompt_key(key);
        }
        match key.code {
            KeyCode::Esc => self.close_chat_view(),
            KeyCode::Enter => self.send_chat_input(),
            // Enter/Esc/Tab/arrows/Ctrl+U are taken by the composer, so the
            // add-member prompt gets Ctrl+A ("add agent").
            KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_chat_add_member()
            }
            KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_new_channel_prompt()
            }
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

    /// Surface for an agent message addressed to the human seat while the
    /// chat view is closed: auto-open the view on the mentioning channel
    /// when policy allows, else raise the existing NeedsAttention toast.
    /// While the view is open the highlighted timeline line is the surface
    /// — and toasts render below interactive overlays, so a toast would be
    /// hidden there anyway. Reuses the existing surfaces; no new subsystem.
    pub(crate) fn notify_chat_to_human(&mut self, channel: &str, message: &ChannelMessage) {
        if !message.to_human || self.state.mode == Mode::Chat {
            return;
        }
        if self.try_auto_open_chat(channel) {
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

    /// Auto-open policy for a mention: open the chat view on the channel
    /// that mentioned the human, unless a suppression rule says the human
    /// is busy — a keystroke inside the typing window, or a mode outside
    /// the quiet allowlist. A failed open (channel vanished from the list)
    /// also returns false, so the caller falls back to the toast.
    fn try_auto_open_chat(&mut self, channel: &str) -> bool {
        if !self.state.chat_open_on_mention
            || !chat_auto_open_allowed(self.state.mode)
            || self.human_last_input_at.elapsed() < CHAT_AUTO_OPEN_TYPING_WINDOW
            || !self.open_chat_view_on(channel)
        {
            return false;
        }
        tracing::debug!(channel, "chat view auto-opened on a mention");
        self.render_dirty.request_generic();
        self.render_notify.notify_one();
        true
    }

    /// Open the chat view selecting the named channel, reusing the plain
    /// open path (state reset + list/history fetch). The list is refreshed
    /// before the mode switch, so a channel missing from it returns false
    /// with the mode untouched — no opening on an arbitrary channel. Only
    /// the auto-open path calls this, and it records where to return.
    fn open_chat_view_on(&mut self, channel: &str) -> bool {
        if !self.state.chat_view {
            return false;
        }
        self.refresh_chat_channels();
        let needle = crate::persist::channels::normalize_channel_name(channel);
        let Some(idx) = self.state.chat.channels.iter().position(|summary| {
            crate::persist::channels::normalize_channel_name(&summary.name) == needle
        }) else {
            return false;
        };
        let return_mode = self.state.mode;
        self.state.open_chat_view();
        self.state.chat.selected = idx;
        self.state.chat.return_mode = Some(return_mode);
        self.refresh_chat_channel_data();
        true
    }

    /// Close the chat view. An auto-open recorded the mode it interrupted;
    /// closing returns there. Manual opens keep today's `leave_modal`.
    fn close_chat_view(&mut self) {
        match self.state.chat.return_mode.take() {
            Some(mode) if mode != Mode::Chat => self.state.mode = mode,
            _ => leave_modal(&mut self.state),
        }
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
        self.chat.prompt = None;
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

    /// Candidate rows the add-member prompt currently shows: `candidates`
    /// narrowed by `query` (substring over name and cwd). The full list stays
    /// in state so backspacing widens the view again.
    pub(crate) fn chat_prompt_candidates(&self) -> Vec<&ChatMemberCandidate> {
        let Some(ChatPrompt::AddMember {
            query, candidates, ..
        }) = self.chat.prompt.as_ref()
        else {
            return Vec::new();
        };
        let needle = query.trim().to_lowercase();
        candidates
            .iter()
            .filter(|candidate| candidate_matches(candidate, &needle))
            .collect()
    }

    /// Editable text of the open prompt: the channel name being typed, or
    /// the candidate filter.
    fn chat_prompt_text_mut(&mut self) -> Option<&mut String> {
        match self.chat.prompt.as_mut()? {
            ChatPrompt::NewChannel { input } => Some(input),
            ChatPrompt::AddMember { query, .. } => Some(query),
        }
    }

    pub(crate) fn chat_prompt_push(&mut self, c: char) {
        if let Some(text) = self.chat_prompt_text_mut() {
            text.push(c);
        }
        self.reset_chat_prompt_selection();
    }

    pub(crate) fn chat_prompt_pop(&mut self) {
        if let Some(text) = self.chat_prompt_text_mut() {
            text.pop();
        }
        self.reset_chat_prompt_selection();
    }

    /// Paste target inside the chat view: the open prompt when there is one,
    /// otherwise the composer.
    pub(crate) fn paste_into_chat(&mut self, text: &str) {
        match self.chat_prompt_text_mut() {
            Some(prompt_text) => {
                prompt_text.push_str(text);
                self.reset_chat_prompt_selection();
            }
            None => self.chat.input.push_str(text),
        }
    }

    /// Filtering reshuffles the rows, so the highlight goes back to the top
    /// rather than pointing at whatever now sits at the old index.
    fn reset_chat_prompt_selection(&mut self) {
        if let Some(ChatPrompt::AddMember { selected, .. }) = self.chat.prompt.as_mut() {
            *selected = 0;
        }
    }

    pub(crate) fn move_chat_prompt_selection(&mut self, delta: isize) {
        let last = self.chat_prompt_candidates().len().saturating_sub(1) as isize;
        if let Some(ChatPrompt::AddMember { selected, .. }) = self.chat.prompt.as_mut() {
            *selected = (*selected as isize + delta).clamp(0, last.max(0)) as usize;
        }
    }

    /// The prompt box: one small centered panel over the chat overlay, in the
    /// overlay's own visual vocabulary. `None` when no prompt is open or the
    /// overlay is too small to host one.
    pub(crate) fn chat_prompt_rect(&self) -> Option<Rect> {
        let prompt = self.chat.prompt.as_ref()?;
        let inner = self.chat_inner_rect();
        if inner.width < 28 || inner.height < 8 {
            return None;
        }
        let rows = match prompt {
            ChatPrompt::NewChannel { .. } => 0,
            ChatPrompt::AddMember { .. } => (self.chat_prompt_candidates().len() as u16)
                .clamp(1, CHAT_PROMPT_ROWS)
                .min(inner.height.saturating_sub(8)),
        };
        // Borders + title row + text row + candidate rows.
        let height = rows + 4;
        let width = inner.width.saturating_sub(8).clamp(28, 60);
        Some(Rect::new(
            inner.x + (inner.width - width) / 2,
            inner.y + (inner.height - height) / 2,
            width,
            height,
        ))
    }

    /// Row 0 is the title, row 1 the typed text, rows 2.. the candidates.
    fn chat_prompt_inner_rect(&self) -> Option<Rect> {
        let rect = self.chat_prompt_rect()?;
        Some(Rect::new(
            rect.x + 1,
            rect.y + 1,
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        ))
    }

    pub(crate) fn chat_prompt_text_rect(&self) -> Option<Rect> {
        let inner = self.chat_prompt_inner_rect()?;
        Some(Rect::new(inner.x, inner.y + 1, inner.width, 1))
    }

    pub(crate) fn chat_prompt_rows_rect(&self) -> Option<Rect> {
        let inner = self.chat_prompt_inner_rect()?;
        Some(Rect::new(
            inner.x,
            inner.y + 2,
            inner.width,
            inner.height.saturating_sub(2),
        ))
    }

    /// First visible candidate index: the list scrolls only as far as needed
    /// to keep the highlighted row on screen.
    pub(crate) fn chat_prompt_window_start(&self) -> usize {
        let Some(ChatPrompt::AddMember { selected, .. }) = self.chat.prompt.as_ref() else {
            return 0;
        };
        let visible = self
            .chat_prompt_rows_rect()
            .map(|rect| rect.height as usize)
            .unwrap_or(0)
            .max(1);
        selected.saturating_sub(visible - 1)
    }

    pub(crate) fn chat_prompt_contains(&self, col: u16, row: u16) -> bool {
        self.chat_prompt_rect()
            .is_some_and(|rect| rect_contains(rect, col, row))
    }

    /// Index into the filtered candidate view for a click on a candidate row.
    pub(crate) fn chat_prompt_candidate_at(&self, col: u16, row: u16) -> Option<usize> {
        let rows = self.chat_prompt_rows_rect()?;
        if !rect_contains(rows, col, row) {
            return None;
        }
        let idx = self.chat_prompt_window_start() + (row - rows.y) as usize;
        (idx < self.chat_prompt_candidates().len()).then_some(idx)
    }

    /// The members column footer row: the `+ add agent` affordance.
    pub(crate) fn chat_add_member_rect(&self) -> Rect {
        let area = self.chat_members_rect();
        Rect::new(
            area.x,
            area.y + area.height.saturating_sub(1),
            area.width,
            area.height.min(1),
        )
    }

    /// Column of the per-row remove control (`×`) in the members column.
    pub(crate) fn chat_member_remove_x(&self) -> u16 {
        let area = self.chat_members_rect();
        area.x + area.width.saturating_sub(1)
    }

    /// What a left-click in the members column landed on. Removal needs the
    /// explicit `×` control at the right edge — a stray click anywhere on the
    /// row mentions the member instead of ejecting them.
    pub(crate) fn chat_members_hit_at(&self, col: u16, row: u16) -> Option<ChatMembersHit> {
        let area = self.chat_members_rect();
        if !rect_contains(area, col, row) {
            return None;
        }
        let footer = self.chat_add_member_rect();
        if rect_contains(footer, col, row) {
            return Some(ChatMembersHit::AddAgent);
        }
        // Row 0 is the "N members" header; member rows start below it.
        let idx = row.checked_sub(area.y + 1)? as usize;
        if idx >= self.chat.members.len() {
            return None;
        }
        Some(if col >= self.chat_member_remove_x() {
            ChatMembersHit::Remove(idx)
        } else {
            ChatMembersHit::Mention(idx)
        })
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

    // ---- mention auto-open (ui.chat_open_on_mention) ---------------------

    /// The auto-open posture: flag on, view enabled and closed, human idle
    /// (last keystroke outside the 3 s window).
    fn auto_open_app() -> App {
        let mut app = test_app();
        app.state.chat_view = true;
        app.state.chat_open_on_mention = true;
        app.state.mode = Mode::Terminal;
        app.human_last_input_at = std::time::Instant::now() - std::time::Duration::from_secs(4);
        app
    }

    #[test]
    fn mention_with_the_flag_off_toasts_exactly_as_before() {
        let mut app = auto_open_app();
        app.state.chat_open_on_mention = false;

        app.notify_chat_to_human("design", &to_human_message("builder", "status?"));

        assert_eq!(app.state.mode, Mode::Terminal, "flag off never opens");
        let toast = app.state.toast.as_ref().expect("the toast is the fallback");
        assert_eq!(toast.kind, crate::app::state::ToastKind::NeedsAttention);
        assert_eq!(toast.title, "builder › you");
        assert_eq!(toast.context, "#design");
        assert!(toast.target.is_none());
    }

    #[test]
    fn a_keystroke_0s_ago_suppresses_the_open_and_toasts() {
        let mut app = auto_open_app();
        app.human_last_input_at = std::time::Instant::now();

        app.notify_chat_to_human("design", &to_human_message("builder", "status?"));

        assert_eq!(
            app.state.mode,
            Mode::Terminal,
            "never eat keystrokes from someone working in a pane"
        );
        assert!(
            app.state.toast.is_some(),
            "suppression falls back to the toast"
        );
    }

    #[test]
    fn busy_modes_are_never_hijacked() {
        for mode in [
            Mode::Onboarding,
            Mode::ReleaseNotes,
            Mode::ProductAnnouncement,
            Mode::Prefix,
            Mode::Copy,
            Mode::RenameWorkspace,
            Mode::RenameTab,
            Mode::RenamePane,
            Mode::SetWorkspaceGroup,
            Mode::LaunchProgramPrompt,
            Mode::NewLinkedWorktree,
            Mode::OpenExistingWorktree,
            Mode::ConfirmRemoveWorktree,
            Mode::Resize,
            Mode::ConfirmClose,
            Mode::ContextMenu,
            Mode::Settings,
            Mode::GlobalMenu,
            Mode::KeybindHelp,
            Mode::Navigator,
        ] {
            let mut app = auto_open_app();
            app.state.mode = mode;

            app.notify_chat_to_human("design", &to_human_message("builder", "hi"));

            assert_eq!(app.state.mode, mode, "{mode:?} must not be hijacked");
            assert!(app.state.toast.is_some(), "{mode:?} still gets the toast");
        }
    }

    #[test]
    fn broadcast_chatter_with_the_view_armed_still_does_nothing() {
        let mut app = auto_open_app();

        app.notify_chat_to_human("design", &message("plain broadcast"));

        assert_eq!(app.state.mode, Mode::Terminal, "no open for chatter");
        assert!(app.state.toast.is_none(), "and no toast either");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_mention_opens_the_view_on_the_mentioning_channel() {
        let mut app = creating_app();
        // Two channels, so "the right one" is distinguishable from row 0.
        for name in ["eng", "ops"] {
            app.open_new_channel_prompt();
            for c in name.chars() {
                press(&mut app, KeyCode::Char(c));
            }
            press(&mut app, KeyCode::Enter);
        }
        app.state.mode = Mode::Terminal; // view closed
        app.human_last_input_at = std::time::Instant::now() - std::time::Duration::from_secs(4);

        app.notify_chat_to_human("ops", &to_human_message("builder", "@arya status?"));

        assert_eq!(app.state.mode, Mode::Chat, "idle human, flag on: it opens");
        assert_eq!(
            app.state.selected_chat_channel_name(),
            Some("#ops"),
            "on the channel that mentioned, not channel 0"
        );
        assert!(app.state.toast.is_none(), "no toast under the open view");
        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn closing_after_an_auto_open_returns_to_the_prior_mode() {
        let mut app = creating_app();
        app.open_new_channel_prompt();
        for c in "eng".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        app.state.mode = Mode::Navigate; // where the human was
        app.human_last_input_at = std::time::Instant::now() - std::time::Duration::from_secs(4);

        app.notify_chat_to_human("eng", &to_human_message("builder", "@arya ready"));
        assert_eq!(app.state.mode, Mode::Chat);

        press(&mut app, KeyCode::Esc);

        assert_eq!(app.state.mode, Mode::Navigate, "back where they were");

        // A manual open records nothing, so close keeps today's behaviour:
        // workspaces exist, so it lands on Terminal.
        app.state.open_chat_view();
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.state.mode, Mode::Terminal);
        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_mention_naming_a_channel_absent_from_the_list_toasts_instead() {
        let mut app = creating_app();
        app.open_new_channel_prompt();
        for c in "eng".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        app.state.mode = Mode::Terminal;
        app.human_last_input_at = std::time::Instant::now() - std::time::Duration::from_secs(4);

        app.notify_chat_to_human("ghost", &to_human_message("builder", "@arya hi"));

        assert_eq!(
            app.state.mode,
            Mode::Terminal,
            "no opening on an arbitrary channel"
        );
        let toast = app.state.toast.as_ref().expect("toast fallback");
        assert_eq!(toast.context, "#ghost");
        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
    }

    /// A chat app whose `channel.create` builds a workspace that survives the
    /// next API call. The pane process must outlive the create: every
    /// dispatch drains internal events first, and an already-exited shell
    /// closes its workspace there — taking the channel with it.
    #[cfg(unix)]
    fn creating_app() -> App {
        let mut app = test_app();
        app.state.default_shell = "/bin/cat".into();
        app.state.shell_mode = crate::config::ShellModeConfig::NonLogin;
        app.state.chat_view = true;
        app.state.open_chat_view();
        app
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_chat_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn prompt_text(app: &App) -> &str {
        match app.state.chat.prompt.as_ref() {
            Some(ChatPrompt::NewChannel { input }) => input,
            other => panic!("expected an open NewChannel prompt, got {other:?}"),
        }
    }

    #[test]
    fn new_channel_prompt_captures_keystrokes_instead_of_the_composer() {
        let mut app = test_app();
        app.state.open_chat_view();
        app.state.chat.input = "half typed".into();

        app.handle_chat_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));

        press(&mut app, KeyCode::Char('e'));
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Backspace);

        assert_eq!(prompt_text(&app), "en");
        assert_eq!(
            app.state.chat.input, "half typed",
            "the composer neither receives the keystrokes nor loses its draft"
        );
    }

    #[test]
    fn esc_cancels_the_prompt_without_closing_the_chat_view() {
        let mut app = test_app();
        app.state.open_chat_view();
        app.open_new_channel_prompt();
        press(&mut app, KeyCode::Char('e'));

        press(&mut app, KeyCode::Esc);

        assert!(app.state.chat.prompt.is_none(), "the prompt is gone");
        assert_eq!(app.state.mode, Mode::Chat, "the view stays open");
        // Focus is back on the composer.
        press(&mut app, KeyCode::Char('h'));
        assert_eq!(app.state.chat.input, "h");
    }

    #[test]
    fn enter_on_a_blank_name_is_a_no_op_and_keeps_the_prompt() {
        let mut app = test_app();
        app.state.open_chat_view();
        app.open_new_channel_prompt();

        press(&mut app, KeyCode::Enter);
        assert_eq!(prompt_text(&app), "");

        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);

        assert_eq!(prompt_text(&app), " ", "whitespace is not a channel name");
        assert!(app.state.workspaces.is_empty(), "nothing was created");
        assert!(app.state.chat.status.is_none(), "and nothing failed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn creating_strips_a_leading_hash_and_selects_the_new_channel() {
        let mut app = creating_app();
        app.open_new_channel_prompt();
        for c in "#eng".chars() {
            press(&mut app, KeyCode::Char(c));
        }

        press(&mut app, KeyCode::Enter);

        assert!(app.state.chat.prompt.is_none(), "the prompt closed");
        assert!(app.state.chat.status.is_none(), "no error to report");
        let names: Vec<&str> = app
            .state
            .chat
            .channels
            .iter()
            .map(|channel| channel.name.as_str())
            .collect();
        assert_eq!(names, vec!["#eng"], "one channel, single-hashed");
        assert_eq!(
            app.state.selected_chat_channel_name(),
            Some("#eng"),
            "the human lands in the room they just made"
        );
        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_rejected_create_keeps_the_prompt_and_the_typed_name() {
        let mut app = creating_app();
        app.open_new_channel_prompt();
        for c in "eng".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.state.chat.prompt.is_none());

        // Same name again: the server rejects it and the view must say so.
        app.open_new_channel_prompt();
        for c in "#eng".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(
            prompt_text(&app),
            "#eng",
            "the prompt stays open with the text intact so it can be fixed"
        );
        assert_eq!(
            app.state.chat.status.as_deref(),
            Some("channel #eng already exists"),
            "the rejection reaches the status line instead of vanishing"
        );
        assert_eq!(app.state.chat.channels.len(), 1, "no duplicate was created");
        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
    }

    #[test]
    fn the_plus_row_opens_the_prompt_and_is_not_a_channel_row() {
        let mut app = super::super::app_for_mouse_test();
        app.state.open_chat_view();
        app.state.chat.channels = vec![channel("#a"), channel("#b")];
        let list = app.state.chat_channel_list_rect();
        let plus = app.state.chat_new_channel_rect();
        assert_eq!(plus.height, 1, "the column has room for the affordance");
        assert_eq!(plus.y, list.y + list.height - 1, "it sits at the bottom");

        // The `+` row is inside the channel column but is not channel 0..n.
        assert_eq!(app.state.chat_channel_index_at(plus.x, plus.y), None);
        assert_eq!(app.state.chat_channel_index_at(list.x, list.y), Some(0));

        app.handle_chat_click(plus.x, plus.y);

        assert_eq!(prompt_text(&app), "", "clicking `+` opens an empty prompt");
        assert_eq!(app.state.chat.selected, 0, "and selects no channel");
    }

    // ---- membership management (add / remove / mention) ------------------

    fn agent(
        pane_id: &str,
        name: &str,
        cwd: Option<&str>,
        status: crate::api::schema::AgentStatus,
    ) -> crate::api::schema::AgentInfo {
        crate::api::schema::AgentInfo {
            terminal_id: format!("t-{pane_id}"),
            name: Some(name.into()),
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: None,
            agent_status: status,
            screen_detection_skipped: false,
            state_labels: Default::default(),
            tokens: Default::default(),
            agent_session: None,
            workspace_id: "w1".into(),
            tab_id: "w1t1".into(),
            pane_id: pane_id.into(),
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_seq: 0,
            cwd: cwd.map(str::to_string),
            foreground_cwd: None,
            revision: 1,
        }
    }

    fn member(
        pane_id: &str,
        name: &str,
        source: ChannelMemberSource,
    ) -> crate::api::schema::ChannelMember {
        crate::api::schema::ChannelMember {
            pane_id: pane_id.into(),
            name: Some(name.into()),
            agent_status: Some(crate::api::schema::AgentStatus::Idle),
            source,
        }
    }

    /// Names of the candidate rows the prompt currently shows.
    fn candidate_names(app: &App) -> Vec<String> {
        app.state
            .chat_prompt_candidates()
            .iter()
            .map(|candidate| candidate.name.clone())
            .collect()
    }

    /// Index of the highlighted candidate row.
    fn highlighted(app: &App) -> usize {
        match app.state.chat.prompt.as_ref() {
            Some(ChatPrompt::AddMember { selected, .. }) => *selected,
            other => panic!("expected an open AddMember prompt, got {other:?}"),
        }
    }

    fn add_member_prompt(candidates: Vec<ChatMemberCandidate>) -> ChatPrompt {
        ChatPrompt::AddMember {
            query: String::new(),
            selected: 0,
            candidates,
        }
    }

    #[test]
    fn candidates_exclude_agents_already_in_the_channel() {
        let agents = vec![
            agent(
                "w1:p1",
                "reviewer",
                Some("/Users/x/Sites/bora"),
                crate::api::schema::AgentStatus::Working,
            ),
            agent(
                "w2:p1",
                "scout",
                None,
                crate::api::schema::AgentStatus::Idle,
            ),
        ];
        let members = vec![member("w1:p1", "reviewer", ChannelMemberSource::Workspace)];

        let candidates = candidates_from_agents(&agents, &members);

        assert_eq!(
            candidates
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["scout"],
            "an existing member is not offered as someone to add"
        );
        assert_eq!(candidates[0].pane_id, "w2:p1");
        assert_eq!(candidates[0].status, "idle");

        // The cwd is shortened to the last two components, which is what
        // tells same-named agents in different worktrees apart.
        let shortened = candidates_from_agents(&agents[..1], &[]);
        assert_eq!(shortened[0].cwd.as_deref(), Some("Sites/bora"));
    }

    #[test]
    fn typing_narrows_the_candidate_list_and_backspace_widens_it() {
        let mut app = test_app();
        app.state.open_chat_view();
        app.state.chat.prompt = Some(add_member_prompt(candidates_from_agents(
            &[
                agent(
                    "w1:p1",
                    "reviewer",
                    Some("/src/bora"),
                    crate::api::schema::AgentStatus::Idle,
                ),
                agent(
                    "w2:p1",
                    "scout",
                    Some("/src/loop-worktrees/api"),
                    crate::api::schema::AgentStatus::Working,
                ),
            ],
            &[],
        )));
        assert_eq!(candidate_names(&app), vec!["reviewer", "scout"]);

        press(&mut app, KeyCode::Char('s'));
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(candidate_names(&app), vec!["scout"], "name substring");

        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('p'));
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(
            candidate_names(&app),
            vec!["scout"],
            "the cwd is searchable too, not just the nick"
        );

        press(&mut app, KeyCode::Char('x'));
        assert!(
            candidate_names(&app).is_empty(),
            "a query matching nothing shows nothing rather than everything"
        );
        assert_eq!(
            app.state.chat.input, "",
            "none of it leaked into the composer"
        );
    }

    #[test]
    fn arrows_move_the_highlight_within_the_filtered_list() {
        let mut app = test_app();
        app.state.open_chat_view();
        app.state.chat.prompt = Some(add_member_prompt(candidates_from_agents(
            &[
                agent("w1:p1", "a", None, crate::api::schema::AgentStatus::Idle),
                agent("w2:p1", "b", None, crate::api::schema::AgentStatus::Idle),
            ],
            &[],
        )));

        press(&mut app, KeyCode::Down);
        assert_eq!(highlighted(&app), 1);
        // Past the end clamps instead of wrapping into nothing.
        press(&mut app, KeyCode::Down);
        assert_eq!(highlighted(&app), 1);
        press(&mut app, KeyCode::Up);
        press(&mut app, KeyCode::Up);
        assert_eq!(highlighted(&app), 0);

        // Typing reshuffles the rows, so the highlight returns to the top
        // rather than pointing at whatever moved into the old index.
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(highlighted(&app), 0);
    }

    #[test]
    fn esc_cancels_the_add_member_prompt_without_closing_the_view() {
        let mut app = test_app();
        app.state.open_chat_view();
        app.state.chat.input = "half typed".into();
        app.state.chat.prompt = Some(add_member_prompt(candidates_from_agents(
            &[agent(
                "w2:p1",
                "scout",
                None,
                crate::api::schema::AgentStatus::Idle,
            )],
            &[],
        )));

        press(&mut app, KeyCode::Esc);

        assert!(app.state.chat.prompt.is_none(), "the prompt is gone");
        assert_eq!(app.state.mode, Mode::Chat, "the chat view stays open");
        press(&mut app, KeyCode::Char('!'));
        assert_eq!(
            app.state.chat.input, "half typed!",
            "the composer gets its keyboard back with its draft intact"
        );
    }

    #[test]
    fn clicking_a_member_name_mentions_them_and_only_the_x_removes() {
        let mut app = super::super::app_for_mouse_test();
        app.state.open_chat_view();
        app.state.chat.channels = vec![channel("#eng")];
        app.state.chat.members = vec![
            member("w1:p1", "reviewer", ChannelMemberSource::Workspace),
            member("w2:p1", "scout", ChannelMemberSource::Joined),
        ];
        let members = app.state.chat_members_rect();
        assert!(members.width > 0, "the members column is visible");
        let first_row = members.y + 1;

        // Anywhere on the name is a mention, not a removal.
        assert_eq!(
            app.state.chat_members_hit_at(members.x, first_row),
            Some(ChatMembersHit::Mention(0))
        );
        app.handle_chat_click(members.x + 2, first_row);
        assert_eq!(app.state.chat.input, "@reviewer ");
        assert_eq!(
            app.state.chat.members.len(),
            2,
            "clicking a name never ejects anyone"
        );

        // A second mention keeps the composer readable.
        app.handle_chat_click(members.x + 2, first_row + 1);
        assert_eq!(app.state.chat.input, "@reviewer @scout ");

        // Removal needs the explicit control at the right edge.
        assert_eq!(
            app.state
                .chat_members_hit_at(app.state.chat_member_remove_x(), first_row),
            Some(ChatMembersHit::Remove(0))
        );

        // The footer row is the add affordance, not a member row.
        let footer = app.state.chat_add_member_rect();
        assert_eq!(
            app.state.chat_members_hit_at(footer.x, footer.y),
            Some(ChatMembersHit::AddAgent)
        );
    }

    /// Isolates the channel roster/transcript files this test writes, so a
    /// join never touches the developer's real state directory.
    struct IsolatedStateDir {
        _guard: std::sync::MutexGuard<'static, ()>,
        previous: Option<std::ffi::OsString>,
        dir: std::path::PathBuf,
    }

    impl IsolatedStateDir {
        fn new(name: &str) -> Self {
            let guard = crate::config::test_config_env_lock().lock().unwrap();
            let previous = std::env::var_os("XDG_STATE_HOME");
            let dir = std::env::temp_dir()
                .join(format!("bora-chat-members-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::env::set_var("XDG_STATE_HOME", &dir);
            Self {
                _guard: guard,
                previous,
                dir,
            }
        }
    }

    impl Drop for IsolatedStateDir {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var("XDG_STATE_HOME", value),
                None => std::env::remove_var("XDG_STATE_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// `#eng` plus a named agent pane in a *different* workspace — the case
    /// add-member exists for. Returns the app with the chat view open on the
    /// channel, and the outside agent's public pane id.
    fn app_with_channel_and_outside_agent() -> (App, String) {
        let mut app = creating_app();
        app.state.chat_view = true;
        // Real geometry, so the members column exists and can be clicked.
        app.state.view.sidebar_rect = ratatui::layout::Rect::new(0, 0, 26, 20);
        app.state.view.terminal_area = ratatui::layout::Rect::new(26, 0, 80, 20);
        app.dispatch_api_request(
            "test.channel_create",
            Method::ChannelCreate(crate::api::schema::ChannelCreateParams { name: "eng".into() }),
        );
        app.dispatch_api_request(
            "test.workspace_create",
            Method::WorkspaceCreate(crate::api::schema::WorkspaceCreateParams {
                cwd: None,
                focus: false,
                label: None,
                env: Default::default(),
                group: None,
            }),
        );
        let outside_ws = app.state.workspaces.len() - 1;
        let pane = app.state.workspaces[outside_ws].tabs[0].root_pane;
        app.state.ensure_test_terminals();
        let terminal_id = app.state.workspaces[outside_ws]
            .pane_state(pane)
            .expect("pane exists")
            .attached_terminal_id
            .clone();
        let terminal = app
            .state
            .terminals
            .get_mut(&terminal_id)
            .expect("terminal exists");
        terminal.set_agent_name("scout".into());
        terminal.set_detected_state(
            Some(crate::detect::Agent::OpenCode),
            crate::detect::AgentState::Idle,
        );
        let public = app
            .public_pane_id(outside_ws, pane)
            .expect("public pane id");
        app.open_chat_view();
        (app, public)
    }

    #[tokio::test]
    async fn enter_joins_the_highlighted_agent_and_the_members_column_shows_it() {
        let _isolated = IsolatedStateDir::new("join");
        let (mut app, scout) = app_with_channel_and_outside_agent();
        assert_eq!(app.state.selected_chat_channel_name(), Some("#eng"));
        assert!(
            !app.state
                .chat
                .members
                .iter()
                .any(|member| member.pane_id == scout),
            "the outside agent starts out a stranger to the channel"
        );

        // Ctrl+A is the keyboard route to the same prompt the `+ add agent`
        // row opens.
        app.handle_chat_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert_eq!(
            candidate_names(&app),
            vec!["scout"],
            "the running agent outside the channel is offered"
        );

        press(&mut app, KeyCode::Enter);

        assert!(app.state.chat.prompt.is_none(), "the prompt closes on join");
        assert!(
            app.state
                .chat
                .members
                .iter()
                .any(|member| member.pane_id == scout
                    && member.source == ChannelMemberSource::Joined),
            "the members column refreshed and shows the joined agent: {:?}",
            app.state.chat.members
        );
        assert_eq!(app.state.chat.status, None, "nothing failed");

        // And now that it is a member, it is no longer a candidate.
        app.handle_chat_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
        assert!(
            app.state.chat.prompt.is_none(),
            "no agents left to add, so no prompt opens"
        );
        assert_eq!(
            app.state.chat.status.as_deref(),
            Some("no agents left to add")
        );
        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn removing_a_workspace_resident_member_reports_the_refusal() {
        let _isolated = IsolatedStateDir::new("remove");
        let (mut app, _scout) = app_with_channel_and_outside_agent();
        let resident = app
            .state
            .chat
            .members
            .iter()
            .find(|member| member.source == ChannelMemberSource::Workspace)
            .expect("the channel workspace's own pane is a member by construction")
            .clone();
        let before = app.state.chat.members.len();

        // Click the explicit remove control on the resident's row.
        let idx = app
            .state
            .chat
            .members
            .iter()
            .position(|member| member.pane_id == resident.pane_id)
            .expect("member is listed");
        let row = app.state.chat_members_rect().y + 1 + idx as u16;
        app.handle_chat_click(app.state.chat_member_remove_x(), row);

        let label = resident
            .name
            .clone()
            .unwrap_or_else(|| resident.pane_id.clone());
        assert_eq!(
            app.state.chat.status.as_deref(),
            Some(format!("{label} lives in #eng — cannot be removed").as_str()),
            "the refusal is reported instead of a fake success"
        );
        assert_eq!(
            app.state.chat.members.len(),
            before,
            "and the member is still there"
        );
        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
    }
}
