use crate::api::schema::{
    AgentPromptParams, AgentStatus, ChannelCreateParams, ChannelDelivery, ChannelDeliveryStatus,
    ChannelHistoryParams, ChannelJoinParams, ChannelLeaveParams, ChannelMember,
    ChannelMemberSource, ChannelMembersParams, ChannelMessage, ChannelNoteParams,
    ChannelSendParams, ChannelSenderKind, ChannelSummary, ResponseResult,
};
use crate::app::App;
use crate::persist::channels;
use std::time::{Duration, Instant};

use super::responses::{encode_error, encode_success};

const DEFAULT_CHANNEL_HISTORY_LINES: u32 = 50;
const MAX_CHANNEL_HISTORY_LINES: u32 = 1000;

/// Bumped whenever [`CHANNEL_PROTOCOL`]'s content changes in a way an
/// already-briefed pane needs to see again. `App::send_channel_protocol`
/// re-sends only when a pane's recorded version (see
/// `channels::read_protocol_sent`) is behind this. v2: a pane with a
/// recorded scope entry (`channels::ChannelScopeEntry`) now gets a
/// formatted suffix naming its write/read directories, built by
/// `channel_scope_briefing` — CANAL-ESCOPO.md Shape 3's T1 layer.
const CHANNEL_PROTOCOL_VERSION: u32 = 2;

/// Injected once per pane into every channel it joins or is already a
/// member of — see `App::send_channel_protocol`. Teaches an LLM agent, in
/// its own terminal, how to use `#channel` messaging.
const CHANNEL_PROTOCOL: &str = concat!(
    "You are now on a bora #channel. Messages you receive look like:\n",
    "  [#channel from <pane> <nick>] <text>   (channel)\n",
    "  [from <pane> <nick>] <text>            (direct)\n",
    "\n",
    "Reply in-channel:\n",
    "  bora channel send <name> \"<text>\" --current\n",
    "  Your own pane id resolves automatically; you never need to pass it.\n",
    "\n",
    "Address one member by a leading @nick (e.g. \"@rev please check this\").\n",
    "An unknown or ambiguous @nick silently degrades to a broadcast — never\n",
    "resend because of that; the message already went out to everyone.\n",
    "\n",
    "Prefer explicit addressing when you know exactly who should act:\n",
    "  bora channel send <name> \"<text>\" --to <nick>\n",
    "This fails loudly on an unknown or ambiguous nick instead of degrading.\n",
    "\n",
    "Answering a channel.ask question: reply with\n",
    "  bora channel send <name> \"<text>\" --reply-to <seq>\n",
    "using the seq of the question you were sent — that is how the asker's\n",
    "wait resolves.\n",
    "\n",
    "Catch up on history you missed:\n",
    "  bora channel tail <name> --after <seq>\n",
    "\n",
    "Sends are rate-limited (2s per sender->target) and deferred while the\n",
    "target is busy. A `deferred` receipt means QUEUED for delivery — that\n",
    "is not a failure, do not resend it.\n",
    "\n",
    "Discipline: when you finish delegated work, @mention the delegator in\n",
    "your report. Never send a bare acknowledgement. Stay silent on a\n",
    "message if you have nothing to add.\n",
    "\n",
    "Escape a literal @ or # in your own text with \\@ and \\#.",
);

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

    /// Thin wrapper over [`Self::handle_channel_send_inner`] with
    /// `force_bell: false` — `channel.send`'s own path. `channel.ask`
    /// (`handle_channel_ask_question`) is the other caller of
    /// `handle_channel_send_inner`, with `force_bell: true` so its question
    /// always pierces an active burst; `force_bell` is never part of
    /// `ChannelSendParams` itself.
    pub(super) fn handle_channel_send(&mut self, id: String, params: ChannelSendParams) -> String {
        self.handle_channel_send_inner(id, params, false)
    }

    fn handle_channel_send_inner(
        &mut self,
        id: String,
        params: ChannelSendParams,
        force_bell: bool,
    ) -> String {
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

        // A reply must point at a seq the channel could plausibly have
        // produced: past seqs lost to rotation are accepted (history is
        // allowed to be gone), a seq past the channel's current max is not
        // (the future is not) — that can only be a typo or a stale/foreign
        // seq. Checked before addressing so a doomed reply never burns the
        // sender's rate-limit window either.
        if let Some(in_reply_to) = params.in_reply_to {
            let max_seq = channels::next_seq(&name).saturating_sub(1);
            if in_reply_to > max_seq {
                return encode_error(
                    id,
                    "channel_reply_unknown_seq",
                    format!(
                        "#{name} has no message with seq {in_reply_to} yet (current max is {max_seq})"
                    ),
                );
            }
        }

        // Structured addressing is the primary path: an explicit `to` that
        // does not resolve fails the send loudly, before anything is
        // appended or delivered. In-body `@nick` is convenience parsing for
        // human/TUI parity and never fails a send (orc aborts the whole
        // message on addressing it cannot parse — bora degrades instead).
        // Either path can land on the human seat, which has no pane: the
        // message is recorded as `to_human` and delivered to nobody.
        let mut to_pane: Option<String> = None;
        let mut to_human = false;
        let raw_text = params.text.clone();
        let to = params
            .to
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        if let Some(to) = to {
            match self.resolve_channel_nick(ws_idx, to) {
                NickResolution::Unique(pane_id) => to_pane = Some(pane_id),
                NickResolution::Human => to_human = true,
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
                NickResolution::Human => to_human = true,
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

        let sender_pane = if params.from_human {
            String::new()
        } else {
            params.from_pane.unwrap_or_default()
        };
        let sender_name = if params.from_human {
            self.state.chat_name.clone()
        } else {
            self.pane_display_name(&sender_pane)
                .unwrap_or_else(|| "unknown".to_string())
        };
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
            from_kind: if params.from_human {
                ChannelSenderKind::Human
            } else {
                ChannelSenderKind::Agent
            },
            text: text.clone(),
            in_reply_to: params.in_reply_to,
            to_pane: to_pane.clone(),
            to_human,
        };
        if let Err(err) = channels::append_message(&name, &message) {
            return encode_error(id, "channel_send_failed", err.to_string());
        }
        self.state.push_chat_message(&name, message.clone());
        self.notify_chat_to_human(&name, &message);

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

        // Burst damper: a per-channel sliding window over `channel.send`
        // timestamps (`ui.channel_burst_messages` within
        // `ui.channel_burst_window_secs`), mirroring orc's
        // `ORC_BURST_N`/`ORC_BURST_MIN`. The message is always recorded and
        // eventable above, regardless — this only decides whether the
        // fan-out below bells member panes. `force_bell` pierces it; today
        // only ever `false` from `channel.send` itself (see
        // `handle_channel_send`'s doc comment).
        let burst = self.record_channel_burst_send(&name, Instant::now());
        let suppressed = burst && !force_bell;
        if burst {
            if self.channels_in_burst.insert(name.clone()) {
                // Edge-triggered: only the transition into burst gets a
                // system line, so a storm doesn't double the transcript
                // with one line per suppressed message.
                self.append_channel_burst_notice(&name);
            }
        } else {
            self.channels_in_burst.remove(&name);
        }

        // The prefix is built here (not delegated to `handle_agent_prompt`'s
        // own from_pane attribution) so the delivered text carries the
        // channel name too; from_pane is passed as None below to avoid a
        // second `[from ...]` prefix being layered on top of this one.
        let prefixed = format!("[#{name} from {sender_pane} {sender_name}] {text}");

        // Targeted delivery reaches only the resolved pane — and never the
        // sender's own. A message addressed to the human seat reaches no
        // pane at all: the human reads it in the chat view transcript, and
        // injecting it into agents would put words in the human's mouth.
        // Broadcast reaches every agent member pane as before. A suppressed
        // (burst-active, not pierced) send skips this loop entirely —
        // including the protocol briefing — so nothing about a storm ever
        // touches a pane, only its transcript.
        let deliveries = if suppressed {
            Vec::new()
        } else {
            let targets: Vec<String> = if to_human {
                Vec::new()
            } else {
                match &to_pane {
                    Some(target) if target != &sender_pane => vec![target.clone()],
                    Some(_) => Vec::new(),
                    None => self.channel_agent_member_pane_ids(ws_idx),
                }
            };
            targets
                .into_iter()
                .map(|target| {
                    self.send_channel_protocol(&name, ws_idx, &target);
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
                .collect()
        };
        encode_success(
            id,
            ResponseResult::ChannelSent {
                deliveries,
                suppressed,
                seq: message.seq,
            },
        )
    }

    /// `channel.note`: append-only record, ZERO injection — the cheapest
    /// verb, for facts nobody needs to be woken for. Shares attribution and
    /// the per-(sender,channel) rate limit with `channel.send`, but skips
    /// addressing entirely (no `to`, no leading-mention parsing) and never
    /// touches the burst damper: there is no bell for it to suppress, so a
    /// note during an active burst appends exactly like one outside it.
    pub(super) fn handle_channel_note(&mut self, id: String, params: ChannelNoteParams) -> String {
        if params.text.is_empty() {
            return encode_error(
                id,
                "empty_channel_message",
                "channel message must not be empty",
            );
        }
        let name = channels::normalize_channel_name(&params.name);
        if self.find_channel_workspace(&name).is_none() {
            return encode_error(
                id,
                "channel_not_found",
                format!("channel #{name} not found"),
            );
        }
        let text = unescape_channel_text(&params.text);
        let sender_pane = params.from_pane.unwrap_or_default();
        let sender_name = self
            .pane_display_name(&sender_pane)
            .unwrap_or_else(|| "unknown".to_string());
        if !sender_pane.is_empty() {
            if let Err(remaining) = self.check_agent_prompt_rate_limit(
                &sender_pane,
                &format!("#{name}"),
                Instant::now(),
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
            from_kind: ChannelSenderKind::Agent,
            text,
            in_reply_to: None,
            to_pane: None,
            to_human: false,
        };
        if let Err(err) = channels::append_message(&name, &message) {
            return encode_error(id, "channel_send_failed", err.to_string());
        }
        self.state.push_chat_message(&name, message.clone());
        self.notify_chat_to_human(&name, &message);
        self.emit_event(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::ChannelMessage,
            data: crate::api::schema::EventData::ChannelMessage {
                channel: name.clone(),
                seq: message.seq,
                from_pane: (!sender_pane.is_empty()).then_some(sender_pane),
                from_name: sender_name,
                text: message.text.clone(),
                to_pane: None,
            },
        });
        encode_success(
            id,
            ResponseResult::ChannelSent {
                deliveries: Vec::new(),
                suppressed: false,
                seq: message.seq,
            },
        )
    }

    /// `channel.ask`'s append+inject half, run inline in `App`'s normal
    /// request dispatch — `wait::ask_channel` is the connection-thread poll
    /// that then blocks for the reply without holding up this (or any
    /// other) App request. Delegates straight to
    /// [`Self::handle_channel_send_inner`] with a mandatory `to` and
    /// `force_bell: true`: identical addressing errors, attribution, and
    /// single-target injection path as a targeted `channel.send`, just
    /// always piercing the burst damper. The caller reads the assigned
    /// `seq` off the `ChannelSent` response to correlate the reply.
    pub(super) fn handle_channel_ask_question(
        &mut self,
        id: String,
        params: crate::api::schema::ChannelAskParams,
    ) -> String {
        self.handle_channel_send_inner(
            id,
            ChannelSendParams {
                name: params.name,
                text: params.text,
                from_pane: params.from_pane,
                to: Some(params.to),
                in_reply_to: None,
                from_human: false,
            },
            true,
        )
    }

    /// Records `now` in `channel`'s burst-detection sliding window and
    /// reports whether the channel is (now, including this send) inside an
    /// active burst. Reads `ui.channel_burst_messages` /
    /// `ui.channel_burst_window_secs` from state (0 on either disables the
    /// damper — see `burst_active`). Prunes entries older than the window on
    /// every call, so the per-channel history never holds more than one
    /// window's worth of traffic.
    fn record_channel_burst_send(&mut self, channel: &str, now: Instant) -> bool {
        let n = self.state.channel_burst_messages;
        let window = self.state.channel_burst_window;
        if n == 0 || window.is_zero() {
            return false;
        }
        let times = self
            .channel_burst_history
            .entry(channel.to_string())
            .or_default();
        times.push_back(now);
        while times
            .front()
            .is_some_and(|t| now.duration_since(*t) >= window)
        {
            times.pop_front();
        }
        burst_active(times.make_contiguous(), now, n, window)
    }

    /// Appends the honest `[bora]` system line marking a channel's
    /// transition into burst — "recording without ringing" — using the same
    /// `from_name: "bora"` / `from_pane: "system"` shape as the dropped-
    /// delivery and protocol-sent notices.
    fn append_channel_burst_notice(&mut self, channel: &str) {
        let line = ChannelMessage {
            ts: now_rfc3339(),
            seq: channels::next_seq(channel),
            from_pane: "system".to_string(),
            from_name: "bora".to_string(),
            from_kind: ChannelSenderKind::Agent,
            text: format!(
                "canal em surto ({} msgs em {}s): gravando sem sino",
                self.state.channel_burst_messages,
                self.state.channel_burst_window.as_secs()
            ),
            in_reply_to: None,
            to_pane: None,
            to_human: false,
        };
        if let Err(err) = channels::append_message(channel, &line) {
            tracing::warn!(
                channel = %channel,
                error = %err,
                "failed to append channel burst notice"
            );
        } else {
            self.state.push_chat_message(channel, line);
        }
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

    /// `channel.join`: record an explicit membership so a pane living
    /// outside the channel's workspace still receives fan-out and can be
    /// addressed by nick. Idempotent — joining twice, or joining a pane that
    /// is already an implicit workspace member, succeeds and reports which
    /// kind of membership the caller actually ended up with. A `scope_write`
    /// and/or `scope_read` on the request records/replaces the pane's scope
    /// entry (CANAL-ESCOPO.md Shape 2) before the protocol briefing goes
    /// out, so a first-time briefing already names the directories.
    pub(super) fn handle_channel_join(&mut self, id: String, params: ChannelJoinParams) -> String {
        let name = channels::normalize_channel_name(&params.name);
        let Some(ws_idx) = self.find_channel_workspace(&name) else {
            return encode_error(
                id,
                "channel_not_found",
                format!("channel #{name} not found"),
            );
        };
        let Some((public_id, owner_ws_idx)) = self.resolve_public_pane(&params.pane) else {
            return encode_error(
                id,
                "pane_not_found",
                format!("pane {} not found", params.pane),
            );
        };
        if params.scope_write.is_some() || params.scope_read.is_some() {
            let write = params.scope_write.clone().unwrap_or_default();
            let read = params.scope_read.unwrap_or_default();
            if write.is_empty() && read.is_empty() {
                return encode_error(
                    id,
                    "channel_join_invalid_scope",
                    "scope_write/scope_read must name at least one directory",
                );
            }
            let entry = channels::ChannelScopeEntry {
                pane: public_id.clone(),
                nick: self.pane_display_name(&public_id),
                write,
                read,
            };
            if let Err(err) = channels::upsert_channel_scope(&name, entry) {
                return encode_error(id, "channel_join_failed", err.to_string());
            }
            tracing::info!(channel = %name, pane = %public_id, "pane scope recorded");
        }
        if owner_ws_idx == ws_idx {
            // A pane in the channel's own workspace is a member by
            // construction. Succeed, but say so: recording it would imply a
            // membership that `channel.leave` could take away.
            self.send_channel_protocol(&name, ws_idx, &public_id);
            return encode_success(
                id,
                ResponseResult::ChannelJoined {
                    pane_id: public_id,
                    source: ChannelMemberSource::Workspace,
                },
            );
        }
        let mut joined = self.joined_channel_members(&name);
        if !joined.iter().any(|member| member == &public_id) {
            joined.push(public_id.clone());
            if let Err(err) = channels::write_joined_members(&name, &joined) {
                return encode_error(id, "channel_join_failed", err.to_string());
            }
            tracing::info!(channel = %name, pane = %public_id, "pane joined channel");
        }
        self.send_channel_protocol(&name, ws_idx, &public_id);
        encode_success(
            id,
            ResponseResult::ChannelJoined {
                pane_id: public_id,
                source: ChannelMemberSource::Joined,
            },
        )
    }

    /// `channel.leave`: drop an explicit membership, and always drop the
    /// pane's scope entry too — a departed pane's declared directories must
    /// not outlive its membership. Idempotent — `removed: false` means
    /// there was nothing to drop from the roster, either because the pane
    /// never joined or because it lives in the channel's workspace and is a
    /// member by construction; scope removal is independent and silent
    /// either way.
    pub(super) fn handle_channel_leave(
        &mut self,
        id: String,
        params: ChannelLeaveParams,
    ) -> String {
        let name = channels::normalize_channel_name(&params.name);
        if self.find_channel_workspace(&name).is_none() {
            return encode_error(
                id,
                "channel_not_found",
                format!("channel #{name} not found"),
            );
        }
        let Some((public_id, _)) = self.resolve_public_pane(&params.pane) else {
            return encode_error(
                id,
                "pane_not_found",
                format!("pane {} not found", params.pane),
            );
        };
        let mut joined = self.joined_channel_members(&name);
        let before = joined.len();
        joined.retain(|member| member != &public_id);
        let removed = joined.len() != before;
        if removed {
            if let Err(err) = channels::write_joined_members(&name, &joined) {
                return encode_error(id, "channel_leave_failed", err.to_string());
            }
            tracing::info!(channel = %name, pane = %public_id, "pane left channel");
        }
        if let Err(err) = channels::remove_channel_scope_entry(&name, &public_id) {
            tracing::warn!(
                channel = %name,
                pane = %public_id,
                error = %err,
                "failed to remove channel scope entry"
            );
        }
        encode_success(
            id,
            ResponseResult::ChannelLeft {
                pane_id: public_id,
                removed,
            },
        )
    }

    /// Injects [`CHANNEL_PROTOCOL`] into `public_pane_id` once per channel,
    /// deduped and made durable across restarts by
    /// `channels::read_protocol_sent` / `channels::mark_protocol_sent`
    /// (keyed on `CHANNEL_PROTOCOL_VERSION`, so a version bump re-sends).
    /// When the pane has a recorded scope entry, the briefing text gets a
    /// suffix naming its write/read directories (`channel_scope_briefing`);
    /// a pane with no scope entry gets no suffix — never an invented empty
    /// section. Delivery goes through `handle_agent_prompt` with
    /// `from_pane: None` — exempt from the agent-prompt rate limit and
    /// carries no `[from ...]` prefix — and `when_idle: Some(true)`, so a
    /// `Working` pane gets it queued rather than dropped. Always appends
    /// one system line to the channel's transcript recording the delivery,
    /// mirroring `App::report_queued_prompt_dropped`'s drop-notice line.
    /// `ws_idx` is the channel's own workspace, kept only for tracing
    /// context — the pane is always addressed by its already-resolved
    /// public id.
    fn send_channel_protocol(&mut self, channel: &str, ws_idx: usize, public_pane_id: &str) {
        let already_sent = channels::read_protocol_sent(channel)
            .into_iter()
            .any(|entry| entry.pane == public_pane_id && entry.version >= CHANNEL_PROTOCOL_VERSION);
        if already_sent {
            return;
        }
        tracing::debug!(
            channel = %channel,
            ws_idx,
            pane = %public_pane_id,
            version = CHANNEL_PROTOCOL_VERSION,
            "sending channel protocol block"
        );
        let scope_suffix = channels::read_channel_scope(channel)
            .into_iter()
            .find(|entry| entry.pane == public_pane_id)
            .map(|entry| channel_scope_briefing(&entry))
            .unwrap_or_default();
        self.handle_agent_prompt(
            format!("channel-protocol:{channel}:{public_pane_id}"),
            AgentPromptParams {
                target: public_pane_id.to_string(),
                text: format!(
                    "[bora] channel protocol for #{channel} (v{CHANNEL_PROTOCOL_VERSION}):\n\n{CHANNEL_PROTOCOL}{scope_suffix}"
                ),
                wait: None,
                from_pane: None,
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        if let Err(err) =
            channels::mark_protocol_sent(channel, public_pane_id, CHANNEL_PROTOCOL_VERSION)
        {
            tracing::warn!(
                channel = %channel,
                pane = %public_pane_id,
                error = %err,
                "failed to record channel protocol delivery"
            );
        }
        let line = ChannelMessage {
            ts: now_rfc3339(),
            seq: channels::next_seq(channel),
            from_pane: "system".to_string(),
            from_name: "bora".to_string(),
            from_kind: ChannelSenderKind::Agent,
            text: format!("channel protocol v{CHANNEL_PROTOCOL_VERSION} sent to {public_pane_id}"),
            in_reply_to: None,
            to_pane: None,
            to_human: false,
        };
        if let Err(err) = channels::append_message(channel, &line) {
            tracing::warn!(
                channel = %channel,
                pane = %public_pane_id,
                error = %err,
                "failed to append channel protocol notice"
            );
        } else {
            self.state.push_chat_message(channel, line);
        }
    }

    /// Canonical public id and owning workspace of `pane`, accepting every
    /// form `parse_pane_id` does (raw id, alias, colon-free nick form) and
    /// normalizing to the one form the roster stores.
    fn resolve_public_pane(&self, pane: &str) -> Option<(String, usize)> {
        let (ws_idx, pane_id) = self.parse_pane_id(pane.trim())?;
        Some((self.public_pane_id(ws_idx, pane_id)?, ws_idx))
    }

    /// Persisted joined pane ids for `name`, minus any that no longer
    /// resolve to a live pane.
    fn joined_channel_members(&self, name: &str) -> Vec<String> {
        channels::read_joined_members(name, |pane| self.parse_pane_id(pane).is_some())
    }

    fn find_channel_workspace(&self, name: &str) -> Option<usize> {
        self.state
            .workspaces
            .iter()
            .position(|ws| workspace_channel_name(ws) == Some(name))
    }

    fn channel_summary(&self, ws_idx: usize, name: &str) -> ChannelSummary {
        let members = self.channel_member_panes(ws_idx);
        let agent_count = members
            .iter()
            .filter(|member| self.agent_info(member.ws_idx, member.pane_id).is_some())
            .count();
        ChannelSummary {
            name: format!("#{name}"),
            pane_count: members.len(),
            agent_count,
            member_status_counts: self.channel_member_status_counts(ws_idx),
        }
    }

    /// Every member pane of the channel owning `ws_idx`: the panes living in
    /// its `#name` workspace (implicit members), then panes elsewhere that
    /// joined explicitly. De-duplicated by canonical public pane id, so a
    /// pane that is both appears once, as `Workspace`. This is the single
    /// traversal every other member query is built on — members listing,
    /// summary counts, send fan-out, nick resolution — so the four can never
    /// disagree about who is in a channel.
    fn channel_member_panes(&self, ws_idx: usize) -> Vec<ChannelMemberPane> {
        let Some(ws) = self.state.workspaces.get(ws_idx) else {
            return Vec::new();
        };
        let mut members: Vec<ChannelMemberPane> = ws
            .tabs
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .filter_map(|pane_id| {
                Some(ChannelMemberPane {
                    ws_idx,
                    pane_id,
                    public_id: self.public_pane_id(ws_idx, pane_id)?,
                    source: ChannelMemberSource::Workspace,
                })
            })
            .collect();
        let Some(name) = workspace_channel_name(ws) else {
            return members;
        };
        for stored in self.joined_channel_members(name) {
            let Some((owner_ws_idx, pane_id)) = self.parse_pane_id(&stored) else {
                continue;
            };
            // Canonical form, so an alias or colon-free spelling of a pane
            // that is already a member can't slip past de-duplication.
            let Some(public_id) = self.public_pane_id(owner_ws_idx, pane_id) else {
                continue;
            };
            // ponytail: linear scan — member counts are tens, not thousands.
            if members.iter().any(|member| member.public_id == public_id) {
                continue;
            }
            members.push(ChannelMemberPane {
                ws_idx: owner_ws_idx,
                pane_id,
                public_id,
                source: ChannelMemberSource::Joined,
            });
        }
        members
    }

    /// Every member pane of the channel, as a `channel.members` listing —
    /// who would receive a `channel.send`, and how they got there.
    fn channel_members(&self, ws_idx: usize) -> Vec<ChannelMember> {
        self.channel_member_panes(ws_idx)
            .into_iter()
            .map(|member| {
                let agent = self.agent_info(member.ws_idx, member.pane_id);
                let name = agent
                    .as_ref()
                    .and_then(|info| info.display_agent.clone().or_else(|| info.name.clone()));
                ChannelMember {
                    pane_id: member.public_id,
                    name,
                    agent_status: agent.map(|info| info.agent_status),
                    source: member.source,
                }
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
        let mut counts = std::collections::HashMap::new();
        for member in self.channel_member_panes(ws_idx) {
            if let Some(info) = self.agent_info(member.ws_idx, member.pane_id) {
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

    /// Public pane ids of the channel's agent-hosting member panes —
    /// workspace panes and joined panes alike — which is the broadcast
    /// delivery set.
    fn channel_agent_member_pane_ids(&self, ws_idx: usize) -> Vec<String> {
        self.channel_member_panes(ws_idx)
            .into_iter()
            .filter(|member| self.agent_info(member.ws_idx, member.pane_id).is_some())
            .map(|member| member.public_id)
            .collect()
    }

    /// Resolves a nick (`channel.send`'s `to`, or a leading in-body
    /// `@nick`) against the channel's agent member panes — workspace panes
    /// and joined panes alike — plus the human seat at the TUI: exact match
    /// on the raw public pane id, case-insensitive match on any of the
    /// pane's display names (agent display name -> assigned name ->
    /// detected kind — the `pane_display_name` rungs that are per-pane; the
    /// workspace custom_name rung is the channel's own `#name` for every
    /// workspace member and carries no routing information), or on the
    /// effective human name (`ui.chat_name` -> `state.chat_name`). The
    /// human is a candidate in every channel: they read the chat view, not
    /// a pane, so there is no membership to check. Exactly one match ->
    /// `Unique` for a pane, `Human` for the seat; two or more ->
    /// `Ambiguous` with `pane (name)` candidate labels and `human (name)`
    /// for the seat; none -> `Unknown`.
    fn resolve_channel_nick(&self, ws_idx: usize, nick: &str) -> NickResolution {
        let mut matches: Vec<(String, Option<String>)> = Vec::new();
        for member in self.channel_member_panes(ws_idx) {
            let Some(info) = self.agent_info(member.ws_idx, member.pane_id) else {
                continue;
            };
            let named = info
                .display_agent
                .as_deref()
                .or(info.name.as_deref())
                .or(info.agent.as_deref());
            let named_matches = named.is_some_and(|name| name.eq_ignore_ascii_case(nick));
            if member.public_id == nick || named_matches {
                matches.push((member.public_id, named.map(str::to_string)));
            }
        }
        // The human seat collides with an agent of the same name exactly
        // like two same-named agents collide: genuine ambiguity, reported
        // rather than silently resolved in either's favour.
        let human_matches = self.state.chat_name.eq_ignore_ascii_case(nick);
        match matches.len() {
            0 if human_matches => return NickResolution::Human,
            0 => return NickResolution::Unknown,
            1 if !human_matches => {
                let (pane_id, _) = matches.swap_remove(0);
                return NickResolution::Unique(pane_id);
            }
            _ => {}
        }
        let mut candidates: Vec<String> = matches
            .into_iter()
            .map(|(pane_id, name)| match name {
                Some(name) => format!("{pane_id} ({name})"),
                None => pane_id,
            })
            .collect();
        if human_matches {
            candidates.push(format!("human ({})", self.state.chat_name));
        }
        NickResolution::Ambiguous(candidates)
    }
}

/// One resolved channel member pane: where it lives, its canonical public
/// id, and whether membership is workspace-implicit or explicitly joined.
struct ChannelMemberPane {
    ws_idx: usize,
    pane_id: crate::layout::PaneId,
    public_id: String,
    source: ChannelMemberSource,
}

/// Outcome of resolving a nick against a channel's member agents and the
/// human seat.
enum NickResolution {
    /// A single member pane, by its canonical public pane id.
    Unique(String),
    /// The human at the TUI chat view — a seat, not a pane, so nothing is
    /// delivered anywhere.
    Human,
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

/// Formats the per-pane scope suffix appended to [`CHANNEL_PROTOCOL`] when
/// `entry` names `entry`'s declared write/read directories — CANAL-ESCOPO.md
/// Shape 3's T1 layer: name the directories, then say where to ask for
/// anything outside them. Only called for a pane that has a scope entry;
/// a pane with none never gets this suffix, never an invented empty
/// section.
fn channel_scope_briefing(entry: &channels::ChannelScopeEntry) -> String {
    let mut lines = vec![
        String::new(),
        String::new(),
        "Your scope in this channel:".to_string(),
    ];
    if !entry.write.is_empty() {
        lines.push(format!("  write: {}", entry.write.join(", ")));
    }
    if !entry.read.is_empty() {
        lines.push(format!(
            "  read:  {} (write dirs are readable too)",
            entry.read.join(", ")
        ));
    }
    lines.push(
        "Anything outside these directories: ask, do not touch — address the owner \
         with @nick in this channel."
            .to_string(),
    );
    lines.join("\n")
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

/// Pure burst decision for the per-channel damper: `true` when at least `n`
/// of `times` fall within `window` of `now` — i.e. the last `n` channel
/// sends (however spread) all landed inside `window`. Equivalent to counting
/// how many landed in `[now - window, now]`: if that count is >= `n`, its
/// `n` most recent members are trivially among them; if the `n` most recent
/// are within `window`, everything else in the window is at least as
/// recent. Mirrors orc's `ORC_BURST_N`/`ORC_BURST_MIN`. `n == 0` or a zero
/// `window` disables the damper unconditionally. No clock reads: `now` is
/// supplied by the caller so this stays deterministic and unit-testable.
fn burst_active(times: &[Instant], now: Instant, n: u32, window: Duration) -> bool {
    if n == 0 || window.is_zero() {
        return false;
    }
    let n = n as usize;
    times
        .iter()
        .filter(|t| now.duration_since(**t) < window)
        .count()
        >= n
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

    /// Pre-marks `pane` as having already received the channel protocol
    /// block for `name`, so tests unrelated to `App::send_channel_protocol`
    /// can set up member panes without its injection consuming a runtime
    /// receiver slot or appending a system line into their assertions.
    fn skip_protocol(name: &str, pane: &str) {
        channels::mark_protocol_sent(name, pane, CHANNEL_PROTOCOL_VERSION)
            .expect("seeding the protocol record must not fail");
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
                from_human: false,
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
    async fn human_send_attributes_kind_and_name_without_pane() {
        let _isolated = IsolatedStateDir::new("send-human");
        let mut app = test_app();
        app.state.chat_name = "tester".into();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "hi".into(),
                from_pane: None,
                to: None,
                in_reply_to: None,
                from_human: true,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert!(sent["result"].is_object(), "{sent}");

        // Agent contrast: same channel, pane sender keeps agent attribution.
        let agent_send = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "and I am an agent".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
                from_human: false,
            },
        );
        let agent_send: serde_json::Value = serde_json::from_str(&agent_send).unwrap();
        assert!(agent_send["result"].is_object(), "{agent_send}");

        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].from_kind, ChannelSenderKind::Human);
        assert_eq!(history[0].from_pane, "");
        assert_eq!(history[0].from_name, "tester");
        assert_eq!(history[1].from_kind, ChannelSenderKind::Agent);
        assert_eq!(history[1].from_pane, "w1A:p9");

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn human_sends_stay_exempt_from_the_channel_rate_limit() {
        let _isolated = IsolatedStateDir::new("send-human-rate");
        let mut app = test_app();
        app.state.chat_name = "tester".into();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        // Two back-to-back sends inside one rate-limit window: a pane sender
        // would trip `channel_send_rate_limited` here (see the send-rate-limit
        // test). The human rides the existing no-sender-pane exemption —
        // there is no second rate-limit path for human sends.
        for text in ["first", "second"] {
            let sent = app.handle_channel_send(
                "req".into(),
                ChannelSendParams {
                    name: "#eng".into(),
                    text: text.into(),
                    from_pane: None,
                    to: None,
                    in_reply_to: None,
                    from_human: true,
                },
            );
            let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
            assert!(
                sent["result"].is_object(),
                "human send '{text}' was rejected: {sent}"
            );
        }
        assert_eq!(channels::read_tail("eng", 10).unwrap().len(), 2);

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[test]
    fn from_human_cannot_be_claimed_over_the_wire() {
        let params: ChannelSendParams = serde_json::from_str(
            r#"{"name":"eng","text":"spoofed","from_pane":"w1:p1","from_human":true}"#,
        )
        .unwrap();
        assert!(
            !params.from_human,
            "#[serde(skip)] must drop a wire-claimed from_human: {:?}",
            params
        );
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
        skip_protocol("eng", &public_pane_id);

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "hello".into(),
                from_pane: Some("w1A:p2".into()),
                to: None,
                in_reply_to: None,
                from_human: false,
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
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some("reviewer".into()),
                in_reply_to: None,
                from_human: false,
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
                from_human: false,
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
                from_human: false,
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
                from_human: false,
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
    async fn send_to_param_resolves_human_seat_case_insensitively_and_delivers_to_no_pane() {
        let _isolated = IsolatedStateDir::new("send-to-human");
        let mut app = test_app();
        app.state.chat_name = "arya".into();
        let (reviewer, _worker, mut rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "status?".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some("ARYA".into()),
                in_reply_to: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert!(
            sent["error"].is_null(),
            "addressing the human seat must succeed: {sent}"
        );
        assert!(
            sent["result"]["deliveries"].as_array().unwrap().is_empty(),
            "a human-addressed message is delivered to no pane"
        );
        assert!(
            rx.try_recv().is_err(),
            "no member agent may be injected for a human-addressed message"
        );
        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history.len(), 1, "the transcript still records it");
        assert!(history[0].to_human);
        assert_eq!(history[0].to_pane, None);
        assert_eq!(history[0].from_kind, ChannelSenderKind::Agent);
        assert_ne!(
            history[0].from_pane, reviewer,
            "sender attribution is untouched"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn send_to_param_name_shared_with_agent_is_ambiguous_between_human_and_agent() {
        let _isolated = IsolatedStateDir::new("send-to-human-collision");
        let mut app = test_app();
        app.state.chat_name = "reviewer".into();
        let (reviewer, _worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        let error = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "which one?".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some("reviewer".into()),
                in_reply_to: None,
                from_human: false,
            },
        );
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(
            error["error"]["code"],
            serde_json::json!("channel_nick_ambiguous")
        );
        let message = error["error"]["message"].as_str().unwrap();
        assert!(
            message.contains("human (reviewer)"),
            "candidates must label the human seat: {message}"
        );
        assert!(
            message.contains(&reviewer),
            "candidates must still list the agent pane: {message}"
        );
        assert!(
            channels::read_tail("eng", 10).unwrap().is_empty(),
            "an ambiguous structured addressing must append nothing"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn leading_mention_to_human_targets_the_seat_and_unknown_still_broadcasts() {
        let _isolated = IsolatedStateDir::new("mention-human");
        let mut app = test_app();
        app.state.chat_name = "arya".into();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@arya ping".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert!(
            sent["error"].is_null(),
            "in-body tokens never error: {sent}"
        );
        assert!(
            sent["result"]["deliveries"].as_array().unwrap().is_empty(),
            "a human-addressed mention delivers to no pane"
        );
        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].to_human);
        assert_eq!(history[0].to_pane, None);

        // A mention that matches neither the human nor any member still
        // degrades to literal broadcast — the human seat changed nothing.
        let degraded = app.handle_channel_send(
            "req2".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@ghost hi".into(),
                from_pane: Some("w1A:p8".into()),
                to: None,
                in_reply_to: None,
                from_human: false,
            },
        );
        let degraded: serde_json::Value = serde_json::from_str(&degraded).unwrap();
        assert!(degraded["error"].is_null());
        assert_eq!(
            degraded["result"]["deliveries"].as_array().unwrap().len(),
            2,
            "unknown mention broadcasts to every member agent"
        );
        let last = channels::read_tail("eng", 10).unwrap().pop().unwrap();
        assert!(!last.to_human);
        assert_eq!(last.to_pane, None);
        assert_eq!(last.text, "@ghost hi");
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_send_rate_limits_repeated_from_pane_but_exempts_missing_from_pane() {
        let _isolated = IsolatedStateDir::new("send-rate-limit");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        let first = app.handle_channel_send(
            "req1".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "primeira".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
                from_human: false,
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
                from_human: false,
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
                from_human: false,
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
                from_human: false,
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
                from_human: false,
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
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        // Unknown nick: literal broadcast, message unchanged, never an error.
        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@ghost are you here".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
                from_human: false,
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
        let (dup1, dup2, _rx2) = channel_with_two_agents(&mut app2, "dup", "dup");
        skip_protocol("eng", &dup1);
        skip_protocol("eng", &dup2);
        let sent2 = app2.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@dup pick one".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
                from_human: false,
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
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

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
                from_human: false,
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
                from_human: false,
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
                from_human: false,
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

    /// An idle agent pane in its own non-channel workspace — the
    /// pre-existing agent `channel.join` exists for. Returns its public pane
    /// id and the runtime receiver, which must stay alive for the pane to
    /// stay promptable.
    fn outside_agent_pane(
        app: &mut App,
        name: &str,
    ) -> (String, tokio::sync::mpsc::Receiver<bytes::Bytes>) {
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
        let ws_idx = app.state.workspaces.len() - 1;
        let pane = app.state.workspaces[ws_idx].tabs[0].root_pane;
        app.state.ensure_test_terminals();
        let terminal_id = app.state.workspaces[ws_idx]
            .pane_state(pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_agent_name(name.into());
        terminal.set_detected_state(
            Some(crate::detect::Agent::OpenCode),
            crate::detect::AgentState::Idle,
        );
        let (runtime, rx) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
        app.state.insert_test_runtime(pane, runtime);
        (app.public_pane_id(ws_idx, pane).unwrap(), rx)
    }

    fn join(app: &mut App, name: &str, pane: &str) -> serde_json::Value {
        let response = app.handle_channel_join(
            "req".into(),
            ChannelJoinParams {
                name: name.into(),
                pane: pane.into(),
                scope_write: None,
                scope_read: None,
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    fn join_with_scope(
        app: &mut App,
        name: &str,
        pane: &str,
        scope_write: Vec<&str>,
        scope_read: Vec<&str>,
    ) -> serde_json::Value {
        let response = app.handle_channel_join(
            "req".into(),
            ChannelJoinParams {
                name: name.into(),
                pane: pane.into(),
                scope_write: (!scope_write.is_empty())
                    .then(|| scope_write.into_iter().map(str::to_string).collect()),
                scope_read: (!scope_read.is_empty())
                    .then(|| scope_read.into_iter().map(str::to_string).collect()),
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    fn leave(app: &mut App, name: &str, pane: &str) -> serde_json::Value {
        let response = app.handle_channel_leave(
            "req".into(),
            ChannelLeaveParams {
                name: name.into(),
                pane: pane.into(),
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    fn broadcast(app: &mut App, from_pane: &str, text: &str) -> serde_json::Value {
        // Each send uses a distinct sender pane: the same sender would trip
        // the per-(pane, channel) rate limit.
        let response = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: text.into(),
                from_pane: Some(from_pane.into()),
                to: None,
                in_reply_to: None,
                from_human: false,
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    fn member_sources(app: &mut App) -> Vec<(String, String)> {
        let response =
            app.handle_channel_members("req".into(), ChannelMembersParams { name: "eng".into() });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        response["result"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .map(|member| {
                (
                    member["pane_id"].as_str().unwrap().to_string(),
                    member["source"].as_str().unwrap().to_string(),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn join_of_unknown_channel_errors() {
        let _isolated = IsolatedStateDir::new("join-unknown-channel");
        let mut app = test_app();
        let (outsider, _rx) = outside_agent_pane(&mut app, "brandos");
        let error = join(&mut app, "ghost", &outsider);
        assert_eq!(
            error["error"]["code"],
            serde_json::json!("channel_not_found")
        );
        let leave_error = leave(&mut app, "ghost", &outsider);
        assert_eq!(
            leave_error["error"]["code"],
            serde_json::json!("channel_not_found")
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn join_of_unknown_pane_errors() {
        let _isolated = IsolatedStateDir::new("join-unknown-pane");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let error = join(&mut app, "#eng", "w9Z:p9");
        assert_eq!(error["error"]["code"], serde_json::json!("pane_not_found"));
        assert!(
            channels::read_joined_members("eng", |_| true).is_empty(),
            "a rejected join must not record membership"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn join_of_workspace_pane_reports_implicit_membership() {
        let _isolated = IsolatedStateDir::new("join-implicit");
        let mut app = test_app();
        let (reviewer, _worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        let joined = join(&mut app, "#eng", &reviewer);
        assert_eq!(joined["result"]["pane_id"], serde_json::json!(reviewer));
        assert_eq!(
            joined["result"]["source"],
            serde_json::json!("workspace"),
            "a pane in the channel's own workspace was a member all along"
        );
        assert!(
            channels::read_joined_members("eng", |_| true).is_empty(),
            "implicit membership must not be recorded as explicit"
        );
        assert_eq!(member_sources(&mut app).len(), 2, "no member was added");
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn joined_pane_receives_broadcast_until_it_leaves() {
        let _isolated = IsolatedStateDir::new("join-delivery");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);
        let (outsider, mut outsider_rx) = outside_agent_pane(&mut app, "brandos");
        skip_protocol("eng", &outsider);

        let before = broadcast(&mut app, "w1A:p9", "before join");
        let before = before["result"]["deliveries"].as_array().unwrap();
        assert!(
            !before.iter().any(|d| d["pane_id"] == json_str(&outsider)),
            "a pane outside the workspace is not a member until it joins"
        );

        let joined = join(&mut app, "#eng", &outsider);
        assert_eq!(joined["result"]["source"], serde_json::json!("joined"));

        let after = broadcast(&mut app, "w1A:p8", "after join");
        let after = after["result"]["deliveries"].as_array().unwrap();
        let outsider_delivery = after
            .iter()
            .find(|d| d["pane_id"] == json_str(&outsider))
            .expect("joined pane must be in the fan-out");
        assert_eq!(
            outsider_delivery["status"],
            serde_json::json!("delivered"),
            "delivery detail: {:?}",
            outsider_delivery["detail"]
        );
        let injected = outsider_rx
            .try_recv()
            .expect("joined pane's runtime must receive the prefixed message");
        let injected = String::from_utf8_lossy(&injected);
        assert!(injected.contains("[#eng from "), "got {injected}");
        assert!(injected.contains("after join"), "got {injected}");
        assert!(
            after.iter().any(|d| d["pane_id"] == json_str(&reviewer)),
            "joining must not displace workspace members"
        );

        let left = leave(&mut app, "#eng", &outsider);
        assert_eq!(left["result"]["removed"], serde_json::json!(true));
        let post_leave = broadcast(&mut app, "w1A:p7", "after leave");
        assert!(
            !post_leave["result"]["deliveries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|d| d["pane_id"] == json_str(&outsider)),
            "leaving stops fan-out to the pane"
        );
        assert!(channels::read_joined_members("eng", |_| true).is_empty());
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn joined_pane_resolves_by_nick() {
        let _isolated = IsolatedStateDir::new("join-nick");
        let mut app = test_app();
        let (_reviewer, _worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        let (outsider, _outsider_rx) = outside_agent_pane(&mut app, "brandos");

        let unknown = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some("brandos".into()),
                in_reply_to: None,
                from_human: false,
            },
        );
        let unknown: serde_json::Value = serde_json::from_str(&unknown).unwrap();
        assert_eq!(
            unknown["error"]["code"],
            serde_json::json!("channel_nick_unknown"),
            "a non-member nick does not resolve"
        );

        join(&mut app, "#eng", &outsider);
        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: Some("w1A:p8".into()),
                to: Some("brandos".into()),
                in_reply_to: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(
            deliveries.len(),
            1,
            "targeted send reaches exactly one pane"
        );
        assert_eq!(deliveries[0]["pane_id"], json_str(&outsider));

        // The in-body `@nick` path resolves against the same member set.
        let mention = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@brandos please look".into(),
                from_pane: Some("w1A:p7".into()),
                to: None,
                in_reply_to: None,
                from_human: false,
            },
        );
        let mention: serde_json::Value = serde_json::from_str(&mention).unwrap();
        let deliveries = mention["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0]["pane_id"], json_str(&outsider));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn members_and_summary_report_joined_panes_once() {
        let _isolated = IsolatedStateDir::new("join-members");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        let (outsider, _outsider_rx) = outside_agent_pane(&mut app, "brandos");

        // Joining twice is a no-op success, and the roster keeps one entry.
        join(&mut app, "#eng", &outsider);
        let again = join(&mut app, "#eng", &outsider);
        assert_eq!(again["result"]["source"], serde_json::json!("joined"));
        assert_eq!(
            channels::read_joined_members("eng", |_| true),
            vec![outsider.clone()],
            "membership is persisted once, on disk, so it survives a restart"
        );

        let mut sources = member_sources(&mut app);
        sources.sort();
        let mut expected = vec![
            (reviewer, "workspace".to_string()),
            (worker, "workspace".to_string()),
            (outsider.clone(), "joined".to_string()),
        ];
        expected.sort();
        assert_eq!(sources, expected);

        let list = app.handle_channel_list("req".into());
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        let channel = &list["result"]["channels"][0];
        assert_eq!(
            channel["pane_count"],
            serde_json::json!(3),
            "summary counts the joined pane"
        );
        assert_eq!(channel["agent_count"], serde_json::json!(3));
        assert_eq!(
            channel["member_status_counts"]["idle"],
            serde_json::json!(2),
            "the joined idle agent is counted with the workspace's own"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn dead_joined_panes_are_pruned_and_leave_is_idempotent() {
        let _isolated = IsolatedStateDir::new("join-prune");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        let (outsider, _outsider_rx) = outside_agent_pane(&mut app, "brandos");

        // A roster written by a previous session: one live pane, one whose
        // pane no longer exists.
        channels::write_joined_members("eng", &[outsider.clone(), "w9Z:p9".into()]).unwrap();

        let panes: Vec<String> = member_sources(&mut app)
            .into_iter()
            .map(|(pane, _)| pane)
            .collect();
        assert!(panes.contains(&outsider));
        assert!(
            !panes.iter().any(|pane| pane == "w9Z:p9"),
            "a pane id that no longer resolves is not a member"
        );
        assert_eq!(panes.len(), 3);

        // Leaving rewrites the roster without the dead entry.
        let left = leave(&mut app, "eng", &outsider);
        assert_eq!(left["result"]["removed"], serde_json::json!(true));
        assert!(channels::read_joined_members("eng", |_| true).is_empty());

        // Leaving again, and leaving a workspace-implicit member, are
        // no-op successes rather than errors.
        let again = leave(&mut app, "eng", &outsider);
        assert_eq!(again["result"]["removed"], serde_json::json!(false));
        let implicit = leave(&mut app, "eng", &reviewer);
        assert_eq!(implicit["result"]["removed"], serde_json::json!(false));
        assert_eq!(member_sources(&mut app).len(), 2);
        assert!(worker.starts_with('w'));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn join_with_scope_persists_sidecar_and_leave_removes_it() {
        let _isolated = IsolatedStateDir::new("join-scope-roundtrip");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (outsider, _outsider_rx) = outside_agent_pane(&mut app, "brandos");

        let joined = join_with_scope(
            &mut app,
            "#eng",
            &outsider,
            vec!["/repo/work"],
            vec!["/repo/read"],
        );
        assert_eq!(joined["result"]["source"], serde_json::json!("joined"));

        let scope = channels::read_channel_scope("eng");
        assert_eq!(scope.len(), 1);
        assert_eq!(scope[0].pane, outsider);
        assert_eq!(scope[0].nick.as_deref(), Some("brandos"));
        assert_eq!(scope[0].write, vec!["/repo/work".to_string()]);
        assert_eq!(scope[0].read, vec!["/repo/read".to_string()]);

        let left = leave(&mut app, "eng", &outsider);
        assert_eq!(left["result"]["removed"], serde_json::json!(true));
        assert!(
            channels::read_channel_scope("eng").is_empty(),
            "leave must drop the pane's scope entry along with membership"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn rejoin_with_new_scope_replaces_and_canonicalizes_pane_id() {
        let _isolated = IsolatedStateDir::new("join-scope-replace");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (outsider, _outsider_rx) = outside_agent_pane(&mut app, "brandos");

        join_with_scope(&mut app, "#eng", &outsider, vec!["/repo/a"], vec![]);
        // Re-join through the colon-free spelling of the same pane id: must
        // land on the same entry, never a duplicate (CANAL-ESCOPO.md Shape 2:
        // "w2Ap1 and w2A:p1 land as one entry").
        let colonless = outsider.replace(':', "");
        join_with_scope(
            &mut app,
            "#eng",
            &colonless,
            vec!["/repo/b"],
            vec!["/repo/c"],
        );

        let scope = channels::read_channel_scope("eng");
        assert_eq!(scope.len(), 1, "re-join must replace, never duplicate");
        assert_eq!(scope[0].pane, outsider, "stored under the canonical id");
        assert_eq!(scope[0].write, vec!["/repo/b".to_string()]);
        assert_eq!(scope[0].read, vec!["/repo/c".to_string()]);
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn join_scope_rejects_empty_write_and_read() {
        let _isolated = IsolatedStateDir::new("join-scope-empty");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (outsider, _outsider_rx) = outside_agent_pane(&mut app, "brandos");

        let response = app.handle_channel_join(
            "req".into(),
            ChannelJoinParams {
                name: "#eng".into(),
                pane: outsider,
                scope_write: Some(Vec::new()),
                scope_read: Some(Vec::new()),
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(
            response["error"]["code"],
            serde_json::json!("channel_join_invalid_scope")
        );
        assert!(channels::read_channel_scope("eng").is_empty());
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_protocol_names_scope_when_present_and_stays_silent_otherwise() {
        let _isolated = IsolatedStateDir::new("protocol-scope");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (scoped, mut scoped_rx) = outside_agent_pane(&mut app, "brandos");
        let (unscoped, mut unscoped_rx) = outside_agent_pane(&mut app, "outro");

        join_with_scope(
            &mut app,
            "#eng",
            &scoped,
            vec!["/repo/work"],
            vec!["/repo/read"],
        );
        let scoped_injected = scoped_rx
            .try_recv()
            .expect("join must inject the channel protocol block");
        let scoped_injected = String::from_utf8_lossy(&scoped_injected);
        assert!(
            scoped_injected.contains("/repo/work"),
            "got: {scoped_injected}"
        );
        assert!(
            scoped_injected.contains("/repo/read"),
            "got: {scoped_injected}"
        );
        assert!(
            scoped_injected.contains('@') && scoped_injected.contains("this channel"),
            "must instruct asking via @nick in the channel: {scoped_injected}"
        );

        join(&mut app, "#eng", &unscoped);
        let unscoped_injected = unscoped_rx
            .try_recv()
            .expect("join must inject the channel protocol block");
        let unscoped_injected = String::from_utf8_lossy(&unscoped_injected);
        assert!(
            !unscoped_injected.contains("Your scope in this channel"),
            "a pane with no scope entry must not get an invented section: {unscoped_injected}"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    fn json_str(value: &str) -> serde_json::Value {
        serde_json::Value::String(value.to_string())
    }

    #[tokio::test]
    async fn joined_pane_receives_protocol_once_and_broadcast_never_re_injects() {
        let _isolated = IsolatedStateDir::new("protocol-join");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (outsider, mut outsider_rx) = outside_agent_pane(&mut app, "brandos");

        let joined = join(&mut app, "#eng", &outsider);
        assert_eq!(joined["result"]["source"], serde_json::json!("joined"));

        let injected = outsider_rx
            .try_recv()
            .expect("join must inject the channel protocol block");
        let injected = String::from_utf8_lossy(&injected);
        assert!(
            injected.contains("channel protocol for #eng"),
            "got: {injected}"
        );
        assert!(
            injected.contains(&format!("v{CHANNEL_PROTOCOL_VERSION}")),
            "got: {injected}"
        );

        let history = channels::read_tail("eng", 10).unwrap();
        let system_lines: Vec<_> = history.iter().filter(|m| m.from_pane == "system").collect();
        assert_eq!(
            system_lines.len(),
            1,
            "exactly one protocol system line: {history:?}"
        );
        assert_eq!(system_lines[0].from_name, "bora");

        // A subsequent broadcast must not re-inject the protocol block.
        let sent = broadcast(&mut app, "w1A:p9", "hello");
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        let outsider_delivery = deliveries
            .iter()
            .find(|d| d["pane_id"] == json_str(&outsider))
            .expect("outsider must be in the fan-out");
        assert_eq!(outsider_delivery["status"], serde_json::json!("delivered"));

        let delivered = outsider_rx
            .try_recv()
            .expect("broadcast delivery must reach the outsider pane");
        let delivered = String::from_utf8_lossy(&delivered);
        assert!(delivered.contains("hello"), "got: {delivered}");
        assert!(
            !delivered.contains("channel protocol"),
            "must not re-inject: {delivered}"
        );

        let history_after = channels::read_tail("eng", 10).unwrap();
        let system_lines_after: Vec<_> = history_after
            .iter()
            .filter(|m| m.from_pane == "system")
            .collect();
        assert_eq!(
            system_lines_after.len(),
            1,
            "no second protocol system line after a later send"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_protocol_survives_restart_without_resend() {
        let _isolated = IsolatedStateDir::new("protocol-restart");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        app.send_channel_protocol("eng", 0, "w1A:p2");
        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(
            history.iter().filter(|m| m.from_pane == "system").count(),
            1,
            "first send appends the protocol notice"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);

        // A fresh App ("restart") against the same on-disk state dir: the
        // persisted protocol.json record must suppress a resend for the
        // same pane, even though this App instance never saw that pane.
        let mut restarted = test_app();
        restarted.send_channel_protocol("eng", 0, "w1A:p2");
        let history_after = channels::read_tail("eng", 10).unwrap();
        assert_eq!(
            history_after
                .iter()
                .filter(|m| m.from_pane == "system")
                .count(),
            1,
            "restart must not re-inject or re-append the protocol notice"
        );

        // A pane that received an older protocol version (as if briefed
        // before a `CHANNEL_PROTOCOL_VERSION` bump) must get a resend: the
        // `entry.version >= CHANNEL_PROTOCOL_VERSION` gate is strict, not
        // pane-presence, so v1-briefed panes see the v2 scope-aware text.
        channels::mark_protocol_sent("eng", "w1A:p3", CHANNEL_PROTOCOL_VERSION - 1).unwrap();
        restarted.send_channel_protocol("eng", 0, "w1A:p3");
        let history_bump = channels::read_tail("eng", 10).unwrap();
        assert_eq!(
            history_bump
                .iter()
                .filter(|m| m.from_pane == "system")
                .count(),
            2,
            "a pane on an older protocol version must get a resend"
        );
        let recorded = channels::read_protocol_sent("eng")
            .into_iter()
            .find(|entry| entry.pane == "w1A:p3")
            .expect("resend must record the new version");
        assert_eq!(recorded.version, CHANNEL_PROTOCOL_VERSION);
        super::super::test_support::shutdown_test_runtimes(&mut restarted);
    }

    #[tokio::test]
    async fn channel_protocol_is_deferred_for_a_working_target() {
        let _isolated = IsolatedStateDir::new("protocol-deferred");
        let mut app = test_app();
        let (_reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        app.send_channel_protocol("eng", 0, &worker);

        assert!(
            app.pending_agent_prompts.contains_key(&worker),
            "a Working target must have the protocol block queued, not dropped"
        );
        assert_eq!(app.pending_agent_prompts[&worker].len(), 1);
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_protocol_send_never_records_a_rate_limit_entry() {
        let _isolated = IsolatedStateDir::new("protocol-rate-limit");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (outsider, mut rx) = outside_agent_pane(&mut app, "brandos");

        app.send_channel_protocol("eng", 0, &outsider);
        let injected = rx
            .try_recv()
            .expect("protocol block must be injected immediately");
        let injected = String::from_utf8_lossy(&injected);
        assert!(injected.contains("channel protocol"), "got: {injected}");

        assert!(
            !app.agent_prompt_rate_limits
                .keys()
                .any(|(_, target)| target == &outsider),
            "protocol delivery must use from_pane: None and never record a rate-limit entry"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[test]
    fn burst_active_counts_within_window_and_disables_at_zero_threshold() {
        let base = Instant::now();
        let window = Duration::from_secs(600);
        let times: Vec<Instant> = std::iter::repeat_n(base, 8).collect();

        // All 8 sends land at `base`, evaluated immediately: active.
        assert!(burst_active(&times, base, 8, window));
        // One fewer than the threshold: not active.
        assert!(!burst_active(&times[..7], base, 8, window));
        // The window has fully elapsed since every recorded send: inactive
        // again, even though the count itself never changed.
        let after_window = base + window + Duration::from_secs(1);
        assert!(!burst_active(&times, after_window, 8, window));
        // n == 0 or a zero window disables the damper unconditionally.
        assert!(!burst_active(&times, base, 0, window));
        assert!(!burst_active(&times, base, 8, Duration::ZERO));
    }

    #[tokio::test]
    async fn channel_burst_suppresses_injection_at_default_threshold_and_appends_one_notice() {
        let _isolated = IsolatedStateDir::new("burst-default");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);
        app.state.chat_name = "human".into();

        // Human sends stay exempt from the per-pane cooldown, so this
        // exercises the burst damper alone, the way a real storm of
        // several distinct sender panes would.
        let mut responses = Vec::new();
        for i in 0..9 {
            let sent = app.handle_channel_send(
                format!("req{i}"),
                ChannelSendParams {
                    name: "#eng".into(),
                    text: format!("msg {i}"),
                    from_pane: None,
                    to: None,
                    in_reply_to: None,
                    from_human: true,
                },
            );
            responses.push(serde_json::from_str::<serde_json::Value>(&sent).unwrap());
        }

        // Below the default threshold (8): the bell still rings.
        // `suppressed` is `skip_serializing_if` when false, so an absent
        // key (not a literal `false`) is the on-the-wire shape for "not
        // suppressed" — `as_bool().unwrap_or(false)` reads both the same.
        for (i, response) in responses.iter().take(7).enumerate() {
            assert!(
                !response["result"]["suppressed"].as_bool().unwrap_or(false),
                "send {i} must not be suppressed below the threshold: {response}"
            );
            assert!(
                !response["result"]["deliveries"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "send {i} must still fan out below the threshold"
            );
        }
        // At and past the threshold: burst active, bell cut, message still
        // recorded (asserted below via the transcript length).
        for (i, response) in responses.iter().enumerate().skip(7) {
            assert_eq!(
                response["result"]["suppressed"],
                serde_json::json!(true),
                "send {i} must be suppressed at/after the burst threshold: {response}"
            );
            assert!(response["result"]["deliveries"]
                .as_array()
                .unwrap()
                .is_empty());
        }

        let history = channels::read_tail("eng", 50).unwrap();
        assert_eq!(history.len(), 10, "9 messages + exactly one burst notice");
        let notices = history
            .iter()
            .filter(|m| m.from_pane == "system" && m.text.contains("surto"))
            .count();
        assert_eq!(
            notices, 1,
            "edge-triggered: one notice, not one per suppressed send: {history:?}"
        );

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_burst_force_bell_pierces_suppression() {
        let _isolated = IsolatedStateDir::new("burst-force-bell");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        // Drive the channel into burst first. `channels::normalize_channel_name`
        // strips the leading `#`, so the internal burst-tracking keys (and
        // this assertion) use the bare "eng" form, matching how
        // `channels::append_message`/`read_tail` are keyed elsewhere.
        for i in 0..8 {
            app.handle_channel_send(
                format!("req{i}"),
                ChannelSendParams {
                    name: "#eng".into(),
                    text: format!("msg {i}"),
                    from_pane: None,
                    to: None,
                    in_reply_to: None,
                    from_human: true,
                },
            );
        }
        assert!(app.channels_in_burst.contains("eng"));

        let pierced = app.handle_channel_send_inner(
            "req-pierce".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "urgent".into(),
                from_pane: None,
                to: None,
                in_reply_to: None,
                from_human: true,
            },
            true,
        );
        let pierced: serde_json::Value = serde_json::from_str(&pierced).unwrap();
        // Omitted (skip_serializing_if) when false, same as the unsuppressed
        // sends above.
        assert!(!pierced["result"]["suppressed"].as_bool().unwrap_or(false));
        assert!(!pierced["result"]["deliveries"]
            .as_array()
            .unwrap()
            .is_empty());

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn note_appends_with_zero_injections_outside_burst() {
        let _isolated = IsolatedStateDir::new("note-no-burst");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        let noted = app.handle_channel_note(
            "req".into(),
            ChannelNoteParams {
                name: "#eng".into(),
                text: "fact recorded".into(),
                from_pane: None,
            },
        );
        let noted: serde_json::Value = serde_json::from_str(&noted).unwrap();
        assert!(noted["result"]["deliveries"].as_array().unwrap().is_empty());
        assert!(!noted["result"]["suppressed"].as_bool().unwrap_or(false));
        assert_eq!(noted["result"]["seq"], serde_json::json!(1));

        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history.len(), 1, "{history:?}");
        assert_eq!(history[0].text, "fact recorded");
        assert_eq!(history[0].to_pane, None);

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn note_during_burst_still_appends_with_no_bell() {
        let _isolated = IsolatedStateDir::new("note-burst");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        for i in 0..8 {
            app.handle_channel_send(
                format!("req{i}"),
                ChannelSendParams {
                    name: "#eng".into(),
                    text: format!("msg {i}"),
                    from_pane: None,
                    to: None,
                    in_reply_to: None,
                    from_human: true,
                },
            );
        }
        assert!(app.channels_in_burst.contains("eng"));
        let burst_history_len_before = app
            .channel_burst_history
            .get("eng")
            .map_or(0, std::collections::VecDeque::len);

        let noted = app.handle_channel_note(
            "req-note".into(),
            ChannelNoteParams {
                name: "#eng".into(),
                text: "note during burst".into(),
                from_pane: None,
            },
        );
        let noted: serde_json::Value = serde_json::from_str(&noted).unwrap();
        assert!(noted["result"]["deliveries"].as_array().unwrap().is_empty());
        assert!(!noted["result"]["suppressed"].as_bool().unwrap_or(false));

        // channel.note never touches the burst-detection window: its
        // per-channel history length is unchanged by the note.
        assert_eq!(
            app.channel_burst_history
                .get("eng")
                .map_or(0, std::collections::VecDeque::len),
            burst_history_len_before,
            "channel.note must not record into the burst damper's sliding window"
        );

        let history = channels::read_tail("eng", 20).unwrap();
        // 8 sends + 1 burst-transition notice + 1 note = 10.
        assert_eq!(history.len(), 10, "{history:?}");
        assert_eq!(history.last().unwrap().text, "note during burst");

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn ask_to_unknown_nick_fails_before_anything_is_appended() {
        let _isolated = IsolatedStateDir::new("ask-unknown-nick");
        let mut app = test_app();
        create_channel(&mut app, "eng");

        let asked = app.handle_channel_ask_question(
            "req".into(),
            crate::api::schema::ChannelAskParams {
                name: "#eng".into(),
                to: "ghost".into(),
                text: "are you there?".into(),
                from_pane: None,
                timeout_ms: None,
            },
        );
        let asked: serde_json::Value = serde_json::from_str(&asked).unwrap();
        assert_eq!(
            asked["error"]["code"],
            serde_json::json!("channel_nick_unknown")
        );

        let history = channels::read_tail("eng", 10).unwrap();
        assert!(
            history.is_empty(),
            "an ask to an unknown nick must not append: {history:?}"
        );

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn ask_reuses_send_inner_single_target_path_and_reports_question_seq() {
        let _isolated = IsolatedStateDir::new("ask-single-target");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        let asked = app.handle_channel_ask_question(
            "req".into(),
            crate::api::schema::ChannelAskParams {
                name: "#eng".into(),
                to: "reviewer".into(),
                text: "ready to merge?".into(),
                from_pane: Some("w1A:p9".into()),
                timeout_ms: None,
            },
        );
        let asked: serde_json::Value = serde_json::from_str(&asked).unwrap();
        assert_eq!(asked["result"]["seq"], serde_json::json!(1));
        let deliveries = asked["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 1, "{asked}");
        assert_eq!(deliveries[0]["pane_id"], serde_json::json!(reviewer));

        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].to_pane.as_deref(), Some(reviewer.as_str()));

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn reply_to_seq_past_current_max_is_rejected_but_past_seqs_are_fine() {
        let _isolated = IsolatedStateDir::new("reply-to-future-seq");
        let mut app = test_app();
        create_channel(&mut app, "eng");

        app.handle_channel_send(
            "req1".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "hello".into(),
                from_pane: None,
                to: None,
                in_reply_to: None,
                from_human: true,
            },
        );

        let rejected = app.handle_channel_send(
            "req2".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "reply to the future".into(),
                from_pane: None,
                to: None,
                in_reply_to: Some(5),
                from_human: true,
            },
        );
        let rejected: serde_json::Value = serde_json::from_str(&rejected).unwrap();
        assert_eq!(
            rejected["error"]["code"],
            serde_json::json!("channel_reply_unknown_seq")
        );

        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(
            history.len(),
            1,
            "the rejected reply must not append: {history:?}"
        );

        // A reply threaded onto an in-range seq (even one rotation could
        // later drop) is accepted: history being gone is fine, the future
        // is not.
        let accepted = app.handle_channel_send(
            "req3".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "reply to the past".into(),
                from_pane: None,
                to: None,
                in_reply_to: Some(1),
                from_human: true,
            },
        );
        let accepted: serde_json::Value = serde_json::from_str(&accepted).unwrap();
        assert!(accepted["result"].is_object(), "{accepted}");

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }
}
