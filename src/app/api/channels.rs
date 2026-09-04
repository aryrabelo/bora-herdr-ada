use crate::api::schema::{
    AgentInfo, AgentPromptParams, AgentStatus, ChannelCreateParams, ChannelDelivery,
    ChannelDeliveryStatus, ChannelHistoryParams, ChannelJoinParams, ChannelLeaveParams,
    ChannelListParams, ChannelMember, ChannelMemberSource, ChannelMembersParams, ChannelMessage,
    ChannelNoteParams, ChannelOpenParams, ChannelSendParams, ChannelSenderKind, ChannelSummary,
    PaneRightClickTarget, PaneSplitParams, ResponseResult, SplitDirection,
};
use crate::app::App;
use crate::persist::channels;
use bytes::Bytes;
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
/// `channel_scope_briefing` — CANAL-ESCOPO.md Shape 3's T1 layer. v3: the
/// injected channel prefix now carries `seq=N`, so every pane briefed
/// under v2 must be re-briefed — it was told a shape the runtime no
/// longer emits, and `--after <seq>`/`--reply-to <seq>` were unusable
/// without a visible seq. v4: an unresolved `@nick` is now a loud error
/// rather than a silent broadcast, and the `name` rungs start at the
/// workspace label (ceo-bora#30/#31) — v3 panes were told the OPPOSITE of
/// what the runtime now does ("never resend because of that; the message
/// already went out to everyone"), which is the one kind of stale briefing
/// that makes an agent act wrongly rather than merely miss a feature.
/// v5: channel fan-out delivery defaults to IMMEDIATE injection, even when
/// the recipient is mid-turn (steering semantics, matching `agent
/// prompt`'s default); `--when-idle` is the opt-in for hold-until-idle —
/// v4 panes were told messages are "deferred while the target is busy",
/// the opposite of what the runtime now does by default.
const CHANNEL_PROTOCOL_VERSION: u32 = 5;

/// Injected once per pane into every channel it joins or is already a
/// member of — see `App::send_channel_protocol`. Teaches an LLM agent, in
/// its own terminal, how to use `#channel` messaging.
const CHANNEL_PROTOCOL: &str = concat!(
    "You are now on a bora #channel. Messages you receive look like:\n",
    "  [#channel seq=N from <pane> <nick>] <text>   (channel)\n",
    "  [from <pane> <nick>] <text>            (direct)\n",
    "\n",
    "Reply in-channel:\n",
    "  bora channel send <name> \"<text>\" --current\n",
    "  Your own pane id resolves automatically; you never need to pass it.\n",
    "\n",
    "Address one member by a leading @nick (e.g. \"@rev please check this\").\n",
    "An unknown or ambiguous @nick FAILS the send — nothing is delivered and\n",
    "nothing is recorded. Read the error, pick a real nick, send again.\n",
    "Broadcast happens only when your text has no leading @ at all.\n",
    "\n",
    "`--to <nick>` is the same rule spelled as a flag:\n",
    "  bora channel send <name> \"<text>\" --to <nick>\n",
    "\n",
    "A nick is any ONE of three forms, and every member always has all\n",
    "three. `bora channel members <name> --json` gives you `pane_id` and\n",
    "`name`, which is all three:\n",
    "  w78:p1   the `pane_id` as printed\n",
    "  w78p1    that same id with the colon dropped — unique even when\n",
    "           every other form collides, so this one always works\n",
    "  rev      the `name` field: workspace label (what the sidebar shows),\n",
    "           else display name, else assigned name, else detected kind\n",
    "Names match case-insensitively. Two panes running the same agent kind\n",
    "share the third form, so `--to codex` with two codex panes is\n",
    "ambiguous and fails — address those by w78p1. Broadcasting because you\n",
    "concluded addressing was impossible is the one mistake to avoid: it is\n",
    "never impossible, the colon-free pane id always resolves.\n",
    "\n",
    "A human reads this channel too, addressable by name like any member.\n",
    "The name is at the end of this briefing. They hold no pane, so they do\n",
    "not appear in `channel members` — a message addressed to them lands in\n",
    "the transcript for them to read and is injected into no pane. Ask them\n",
    "directly rather than asking an agent to relay. Only their own chat view\n",
    "can send as them; anything else claiming to be them is not.\n",
    "\n",
    "Answering a channel.ask question: reply with\n",
    "  bora channel send <name> \"<text>\" --reply-to <seq>\n",
    "using the `seq=N` printed in that question's own prefix — that is how the\n",
    "asker's wait resolves.\n",
    "\n",
    "Catch up on history you missed:\n",
    "  bora channel tail <name> --after <seq>\n",
    "passing the highest `seq=N` you have already seen.\n",
    "\n",
    "Sends are rate-limited (2s per sender->target). A message addressed to\n",
    "you arrives WHILE YOU ARE WORKING, like steering — read it mid-turn;\n",
    "a sender who wants it held until you are free passes --when-idle, and\n",
    "then a `deferred` receipt means QUEUED for delivery — that is not a\n",
    "failure, do not resend it.\n",
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
                self.ensure_channel_tail_pane(index, &name);
                self.ensure_channel_shell_pane(index, &name);
                self.state.mark_session_dirty();
                self.emit_workspace_open_events(index);
                let channel = self.channel_summary(index, &name, None);
                encode_success(id, ResponseResult::ChannelCreated { channel })
            }
            Err(err) => encode_error(id, "channel_create_failed", err.to_string()),
        }
    }

    /// `channel.open`: focus the channel's own workspace and repair its
    /// two-pane shape — see [`Self::ensure_channel_tail_pane`] and
    /// [`Self::ensure_channel_shell_pane`] — adding only whatever pane is
    /// missing. The only path that fixes a channel workspace created
    /// before either half of the two-pane shape shipped (which otherwise
    /// stays exactly as broken as the day it was created — nothing else
    /// ever re-checks an existing channel's panes). Idempotent: called
    /// again on an already-repaired channel, both `ensure_*` calls are
    /// no-ops.
    pub(super) fn handle_channel_open(&mut self, id: String, params: ChannelOpenParams) -> String {
        let name = channels::normalize_channel_name(&params.name);
        let Some(ws_idx) = self.find_channel_workspace(&name) else {
            return encode_error(
                id,
                "channel_not_found",
                format!("channel #{name} not found"),
            );
        };
        self.state.switch_workspace(ws_idx);
        self.ensure_channel_tail_pane(ws_idx, &name);
        self.ensure_channel_shell_pane(ws_idx, &name);
        self.state.mark_session_dirty();
        let channel = self.channel_summary(ws_idx, &name, None);
        encode_success(id, ResponseResult::ChannelOpened { channel })
    }

    /// Ensures the channel workspace at `ws_idx` has a pane running
    /// `channel tail <name> --follow` — the transcript half of the
    /// two-pane shape (batch contract item 1). A no-op when one is already
    /// running, detected via real process info
    /// ([`Self::channel_workspace_has_tail_pane`], contract item 3) rather
    /// than a side file, which is what keeps this safe to call from both
    /// `channel.create` and every `channel.open` repair without ever
    /// stacking a second transcript pane (contract item 2). Splits the new
    /// pane off the workspace's root pane rather than retyping into it, so
    /// an existing plain shell — the `#runner-disk-full` repair case — is
    /// left alone and still typeable. Never fails the caller (contract
    /// item 5): [`Self::split_channel_pane`] already reduces every failure
    /// to a `tracing` warning and a no-op.
    fn ensure_channel_tail_pane(&mut self, ws_idx: usize, name: &str) {
        if self.channel_workspace_has_tail_pane(ws_idx, name) {
            return;
        }
        let Some(target_pane_id) = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs.first())
            .map(|tab| tab.root_pane)
        else {
            return;
        };
        if let Some((new_ws_idx, new_pane_id)) =
            self.split_channel_pane(ws_idx, target_pane_id, name, "transcript")
        {
            self.seed_channel_tail_pane(new_ws_idx, new_pane_id, name);
        }
    }

    /// Ensures the channel workspace at `ws_idx` has at least one pane that
    /// is *not* running `channel tail --follow` — somewhere to type
    /// `bora channel send`, the other half of the two-pane shape (contract
    /// item 1). Only fires when every existing pane is a transcript pane —
    /// the shape a channel created before this fix landed can be stuck in
    /// forever otherwise. A workspace that already has any non-tail pane
    /// (the common case, including one [`Self::ensure_channel_tail_pane`]
    /// just left alone above) is untouched.
    fn ensure_channel_shell_pane(&mut self, ws_idx: usize, name: &str) {
        let Some(pane_ids) = self.state.workspaces.get(ws_idx).map(|ws| {
            ws.tabs
                .iter()
                .flat_map(|tab| tab.layout.pane_ids())
                .collect::<Vec<_>>()
        }) else {
            return;
        };
        let has_shell = pane_ids
            .iter()
            .any(|&pane_id| !self.pane_runs_channel_tail(ws_idx, pane_id, name));
        if has_shell {
            return;
        }
        if let Some(&target_pane_id) = pane_ids.first() {
            self.split_channel_pane(ws_idx, target_pane_id, name, "shell");
        }
    }

    /// Whether the channel workspace at `ws_idx` already has a pane
    /// running `channel tail <name> --follow`, scanning every pane in the
    /// workspace (not just its root).
    fn channel_workspace_has_tail_pane(&self, ws_idx: usize, name: &str) -> bool {
        let Some(ws) = self.state.workspaces.get(ws_idx) else {
            return false;
        };
        ws.tabs
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .any(|pane_id| self.pane_runs_channel_tail(ws_idx, pane_id, name))
    }

    /// Whether `pane_id`'s live foreground process is running
    /// `channel tail <name> --follow` — real process info (the same
    /// [`crate::detect::foreground_job`] source `bora pane process-info`
    /// reports via `handle_pane_process_info`), not a persisted flag.
    fn pane_runs_channel_tail(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        name: &str,
    ) -> bool {
        let Some((runtime, _workspace_id)) = self.lookup_runtime(ws_idx, pane_id) else {
            return false;
        };
        let Some(shell_pid) = runtime.child_pid() else {
            return false;
        };
        let Some(job) = crate::detect::foreground_job(shell_pid) else {
            return false;
        };
        job.processes
            .iter()
            .any(|process| is_channel_tail_process(process, name))
    }

    /// Splits a new, unseeded pane off `target_pane_id` in the channel
    /// workspace, via [`Self::handle_pane_split`] — the same machinery
    /// `pane.split` itself calls — and resolves its id back to
    /// `(ws_idx, pane_id)`. `purpose` only labels the warning on failure
    /// (`"transcript"` or `"shell"`). Never fails the caller (contract item
    /// 5): a pane that can't be split is a `tracing` warning and `None`,
    /// leaving the workspace exactly as it was.
    fn split_channel_pane(
        &mut self,
        ws_idx: usize,
        target_pane_id: crate::layout::PaneId,
        name: &str,
        purpose: &'static str,
    ) -> Option<(usize, crate::layout::PaneId)> {
        let Some(target_public_id) = self.public_pane_id(ws_idx, target_pane_id) else {
            tracing::warn!(
                channel = %name,
                purpose,
                "could not resolve the channel workspace's target pane; cannot split a {purpose} pane"
            );
            return None;
        };
        let response = self.handle_pane_split(
            format!("internal:channel-{purpose}-split:{name}"),
            PaneSplitParams {
                workspace_id: None,
                target_pane_id: Some(target_public_id),
                direction: SplitDirection::Down,
                ratio: None,
                cwd: None,
                focus: false,
                right_click: PaneRightClickTarget::default(),
                env: std::collections::HashMap::new(),
            },
        );
        let new_pane_public_id = serde_json::from_str::<serde_json::Value>(&response)
            .ok()
            .and_then(|value| {
                value
                    .get("result")?
                    .get("pane")?
                    .get("pane_id")?
                    .as_str()
                    .map(str::to_string)
            });
        let Some(new_pane_public_id) = new_pane_public_id else {
            tracing::warn!(
                channel = %name,
                purpose,
                response = %response,
                "failed to split a pane for the channel; it stays without one"
            );
            return None;
        };
        self.parse_pane_id(&new_pane_public_id)
    }

    /// Types `<bora> channel tail <name> --follow` into `pane_id` and
    /// presses Enter — the exact internal path `pane.send_input`
    /// (`bora pane run`) already uses, which is also the documented manual
    /// workaround for a channel pane that otherwise sits at a bare login
    /// shell showing nothing about the channel. `<bora>` is resolved by
    /// absolute path via [`std::env::current_exe`] rather than trusted to
    /// be on the server's `PATH`.
    ///
    /// Never fails the caller: an unresolved binary, a not-yet-ready
    /// runtime, or a send failure is a `warn` log and a no-op — a channel
    /// that works but looks like today's plain shell is strictly better
    /// than a channel that cannot be created or repaired.
    fn seed_channel_tail_pane(
        &mut self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        name: &str,
    ) {
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(err) => {
                tracing::warn!(
                    channel = %name,
                    error = %err,
                    "could not resolve bora's own binary path; channel pane starts as a plain shell"
                );
                return;
            }
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            tracing::warn!(
                channel = %name,
                "no runtime for the freshly created channel pane; it starts as a plain shell"
            );
            return;
        };
        let command = format!(
            "{} channel tail {} --follow",
            shell_quote(&exe.display().to_string()),
            shell_quote(name),
        );
        let bytes = match super::super::api_helpers::encode_api_input(
            runtime,
            &command,
            &["Enter".to_string()],
        ) {
            Ok(bytes) => bytes,
            Err(key) => {
                tracing::warn!(
                    channel = %name,
                    key = %key,
                    "could not encode the channel seed command; channel pane starts as a plain shell"
                );
                return;
            }
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            tracing::warn!(
                channel = %name,
                error = %err,
                "failed to seed the channel pane with the tail command; it starts as a plain shell"
            );
        }
    }

    pub(super) fn handle_channel_list(&mut self, id: String, params: ChannelListParams) -> String {
        let channels = self
            .state
            .workspaces
            .iter()
            .enumerate()
            .filter_map(|(idx, ws)| {
                ws.channel_home_name()
                    .map(|name| self.channel_summary(idx, name, params.from_pane.as_deref()))
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

        // Addressing fails the send LOUDLY on both paths. `to` and a
        // leading in-body `@nick` are two spellings of one intent, so they
        // get one outcome: nothing is appended or delivered until the nick
        // resolves. Either path can land on the human seat, which has no
        // pane: the message is recorded as `to_human` and delivered to
        // nobody.
        //
        // The in-body path used to DEGRADE — `tracing::debug!` and then
        // broadcast the text literally. That is the root cause of
        // ceo-bora#30 ("mensagem duplicada pra todos e invadindo a sessão"),
        // and it is amplification rather than any echo or re-injection loop:
        // one intended recipient became every agent member pane, each
        // reached through `handle_agent_prompt`, i.e. typed into a live
        // session. It hid behind a collision it also caused — two panes
        // sharing the detected kind `omp` made `@omp` Ambiguous, and
        // Ambiguous meant broadcast. Owner's decision, ceo-bora#31 of
        // 2026-08-29: an unresolved mention is a loud error, never a silent
        // broadcast; broadcast happens only when there is no leading `@` at
        // all. Prose that genuinely opens with `@` escapes as `\@`.
        let mut to_pane: Option<String> = None;
        let mut to_human = false;
        let raw_text = params.text.clone();
        let to = params
            .to
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty());
        let addressed = to
            .map(str::to_string)
            .or_else(|| leading_mention_nick(&raw_text));
        if let Some(nick) = addressed {
            match self.resolve_channel_nick(ws_idx, &nick) {
                NickResolution::Unique(pane_id) => to_pane = Some(pane_id),
                NickResolution::Human => to_human = true,
                NickResolution::Ambiguous(candidates) => {
                    return encode_error(
                        id,
                        "channel_nick_ambiguous",
                        format!(
                            "nick '{nick}' matches {} channel members: {} — address one by pane id or a unique name",
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
                            "no channel member matches '{nick}' — channel.members lists pane ids and names"
                        ),
                    );
                }
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
        self.push_chat_message(&name, message.clone());
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
        //
        // `seq` is in the prefix because it is the only place an agent can
        // read it. The protocol block tells agents to catch up with `tail
        // --after <seq>` and to answer a `channel.ask` with `--reply-to
        // <seq>`, and before this both instructions named a number the
        // agent had no way to obtain — the seq existed only in the
        // `channel.send` response the SENDER got, never in what the
        // recipient was handed. Field order is otherwise unchanged, so a
        // reader parsing `from <pane> <nick>` positionally still works.
        let seq = message.seq;
        let prefixed = format!("[#{name} seq={seq} from {sender_pane} {sender_name}] {text}");

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
                    None => self.channel_agent_member_pane_ids(ws_idx, &sender_pane),
                }
            };
            targets
                .into_iter()
                .map(|target| {
                    self.send_channel_protocol(&name, ws_idx, &target, params.when_idle);
                    let response = self.handle_agent_prompt(
                        format!("{id}:channel:{target}"),
                        AgentPromptParams {
                            target: target.clone(),
                            text: prefixed.clone(),
                            wait: None,
                            from_pane: None,
                            when_idle: params.when_idle,
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
        self.push_chat_message(&name, message.clone());
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
                when_idle: None,
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
            self.push_chat_message(channel, line);
        }
    }

    /// Announces a join-time autorename (ceo-bora#34) on the channel, in
    /// the same `from_name: "bora"` / `from_pane: "system"` shape as the
    /// burst and protocol notices: who joined, and the name they answer to
    /// now.
    ///
    /// Silent when the joining pane's name did not collide, which is the
    /// ordinary case — a notice on every join would be noise, and the
    /// thing worth saying is precisely that `@base` no longer reaches this
    /// pane. Called only from the branch that actually grew the roster, so
    /// a repeated `channel.join` stays idempotent here too.
    fn append_channel_autorename_notice(&mut self, channel: &str, ws_idx: usize, public_id: &str) {
        let Some(member) = self
            .channel_member_names(ws_idx)
            .into_iter()
            .find(|member| member.public_id == public_id)
        else {
            return;
        };
        if member.addressable == member.base {
            return;
        }
        let line = ChannelMessage {
            ts: now_rfc3339(),
            seq: channels::next_seq(channel),
            from_pane: "system".to_string(),
            from_name: "bora".to_string(),
            from_kind: ChannelSenderKind::Agent,
            text: format!(
                "{public_id} joined as @{} — the name @{} is shared, so address this pane by @{}",
                member.addressable, member.base, member.addressable
            ),
            in_reply_to: None,
            to_pane: None,
            to_human: false,
        };
        if let Err(err) = channels::append_message(channel, &line) {
            tracing::warn!(
                channel = %channel,
                pane = %public_id,
                error = %err,
                "failed to append channel autorename notice"
            );
        } else {
            self.push_chat_message(channel, line);
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
            Ok(messages) => {
                if let Some(ws_idx) = self.find_channel_workspace(&name) {
                    if let Some(last) = messages.last() {
                        self.advance_channel_read_cursor(
                            &name,
                            ws_idx,
                            params.from_pane.as_deref(),
                            last.seq,
                        );
                    }
                }
                encode_success(id, ResponseResult::ChannelHistory { messages })
            }
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
        let members = self.channel_members(ws_idx, &name);
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
            self.send_channel_protocol(&name, ws_idx, &public_id, Some(true));
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
            // Only on the call that actually added the pane: joining twice
            // is idempotent, and so is the notice.
            self.append_channel_autorename_notice(&name, ws_idx, &public_id);
        }
        self.send_channel_protocol(&name, ws_idx, &public_id, Some(true));
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
    /// carries no `[from ...]` prefix — and the caller's `when_idle` mode:
    /// the `channel.send` fan-out passes the message's own mode so the
    /// briefing and the message travel together and the agent always reads
    /// the protocol BEFORE its first message, whichever mode the sender
    /// chose. Standalone callers (`channel join`) pass `Some(true)` so
    /// joining never types into a running turn. Always appends one system
    /// line to the channel's transcript recording the delivery.
    /// `ws_idx` is the channel's own workspace, kept only for tracing
    /// context — the pane is always addressed by its already-resolved
    /// public id.
    fn send_channel_protocol(
        &mut self,
        channel: &str,
        ws_idx: usize,
        public_pane_id: &str,
        when_idle: Option<bool>,
    ) {
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
        // The blob says a human is addressable and that their name is at
        // the end of the briefing; this is that end. Interpolated per
        // install rather than baked into the const, because it is
        // `ui.chat_name` (or the OS username). The seat existed and worked
        // long before this, but no briefing ever named it, so no agent
        // could address the human and every question for them got routed
        // through another agent instead.
        let human_suffix = format!(
            "\n\nThe human on this channel is @{}.",
            self.state.chat_name
        );
        self.handle_agent_prompt(
            format!("channel-protocol:{channel}:{public_pane_id}"),
            AgentPromptParams {
                target: public_pane_id.to_string(),
                text: format!(
                    "[bora] channel protocol for #{channel} (v{CHANNEL_PROTOCOL_VERSION}):\n\n{CHANNEL_PROTOCOL}{human_suffix}{scope_suffix}"
                ),
                wait: None,
                from_pane: None,
                when_idle,
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
            self.push_chat_message(channel, line);
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

    /// `pub(crate)`: also called from `app::input::chat`'s passive
    /// delivery badge writers (`set_channel_unread_badge` /
    /// `clear_channel_unread_badge`, ceo-bora#33) to resolve a channel
    /// name to the workspace whose `metadata_tokens` carry its badge.
    pub(crate) fn find_channel_workspace(&self, name: &str) -> Option<usize> {
        self.state
            .workspaces
            .iter()
            .position(|ws| ws.channel_home_name() == Some(name))
    }

    /// Resolves `from_pane` to a channel member and advances that member's
    /// stored read cursor to `high_water_seq` — the shared mechanism
    /// behind both `channel.history` and `channel.wait` ("channel tail")'s
    /// read-cursor tracking (the source `unread` counts against). Uses the
    /// same identity resolution as `channel.join`/`channel.leave`
    /// (`resolve_public_pane`) and the same membership test `channel.send`
    /// fan-out uses (`channel_member_panes`), so "who counts as a member"
    /// can never drift between sending, addressing, and read tracking. A
    /// caller with no pane identity, an unresolvable pane, or a pane that
    /// is not currently a member reads freely and advances nothing — only
    /// a verified member's cursor ever moves. Never fails the caller: a
    /// cursor-persistence error is a `tracing` warning, like every other
    /// channel sidecar write.
    fn advance_channel_read_cursor(
        &self,
        name: &str,
        ws_idx: usize,
        from_pane: Option<&str>,
        high_water_seq: u64,
    ) {
        let Some(from_pane) = from_pane else {
            return;
        };
        let Some((public_id, _)) = self.resolve_public_pane(from_pane) else {
            return;
        };
        let is_member = self
            .channel_member_panes(ws_idx)
            .iter()
            .any(|member| member.public_id == public_id);
        if !is_member {
            return;
        }
        if let Err(err) = channels::advance_channel_cursor(name, &public_id, high_water_seq) {
            tracing::warn!(
                channel = %name,
                pane = %public_id,
                error = %err,
                "failed to advance channel read cursor"
            );
        }
    }

    /// Builds a channel's room-level summary, including `unread` for
    /// `from_pane`'s caller. Uses the exact identity resolution and
    /// membership test [`Self::advance_channel_read_cursor`] uses
    /// (`resolve_public_pane` + `channel_member_panes`) so "who counts as a
    /// member" can never drift between advancing a cursor and reading it
    /// back here, then reads that member's stored cursor
    /// ([`channels::read_channel_cursor`]) on this same refresh pass — the
    /// same pass that already reads the tail for `last_message_seq` /
    /// `last_message_ts`, never on a render or per-pane path. A caller with
    /// no pane identity, an unresolvable pane, or a pane that isn't a
    /// member of this channel has no mailbox here and sees `0`.
    fn channel_summary(
        &self,
        ws_idx: usize,
        name: &str,
        from_pane: Option<&str>,
    ) -> ChannelSummary {
        let members = self.channel_member_panes(ws_idx);
        let agent_count = members
            .iter()
            .filter(|member| self.agent_info(member.ws_idx, member.pane_id).is_some())
            .count();
        let (last_message_seq, last_message_ts) = match channels::read_tail(name, 1) {
            Ok(tail) => tail
                .last()
                .map_or((0, None), |message| (message.seq, Some(message.ts.clone()))),
            Err(err) => {
                tracing::warn!(
                    channel = %name,
                    error = %err,
                    "failed to read channel tail for summary; reporting no last message"
                );
                (0, None)
            }
        };
        let unread = from_pane
            .and_then(|pane| self.resolve_public_pane(pane))
            .filter(|(public_id, _)| members.iter().any(|member| &member.public_id == public_id))
            .map_or(0, |(public_id, _)| {
                let cursor = channels::read_channel_cursor(name, &public_id).unwrap_or(0);
                last_message_seq.saturating_sub(cursor)
            });
        ChannelSummary {
            name: format!("#{name}"),
            pane_count: members.len(),
            agent_count,
            last_message_seq,
            last_message_ts,
            unread,
            member_status_counts: self.channel_member_status_counts(ws_idx),
        }
    }

    /// Every member pane of the channel owning `ws_idx`: the panes living in
    /// its `#name` workspace (implicit members), then panes elsewhere that
    /// joined explicitly. De-duplicated by canonical public pane id, so a
    /// pane that is both appears once, as `Workspace`. This is the single
    /// traversal every other member query is built on — members listing,
    /// summary counts, send fan-out, nick resolution — so the four can never
    /// disagree about who is in a channel. This includes the workspace's own
    /// transcript pane (running `channel tail --follow`): it lives in the
    /// channel's workspace like any other pane, so — consistent with every
    /// other workspace-implicit member — it counts too, just never as an
    /// agent (it hosts none, so `agent_count`/nick resolution/delivery
    /// fan-out — all of which filter on `agent_info` — naturally skip it).
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
        let Some(name) = ws.channel_home_name() else {
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

    /// Every agent-hosting member's addressable name, with collisions
    /// resolved: a base name held by two or more members becomes
    /// `{base}-1`, `{base}-2`, … by the member's position in
    /// [`Self::channel_member_panes`]'s order (ceo-bora#34).
    ///
    /// The ordinal is DERIVED from that order, never stored, and that is
    /// what makes it survive a restart: the order is the workspace's own
    /// pane layout followed by the joined roster, and the roster is an
    /// append-ordered file (`channels::write_joined_members`). Same join
    /// order in, same names out, with nothing added to storage. Recording
    /// the rename instead would need a second source of truth that
    /// `channel.leave`, a pane close, and a workspace rename could each
    /// leave stale.
    ///
    /// The rung chain in [`member_addressable_name`] is untouched: this is
    /// a layer above it, so the workspace label still wins rung 0 and both
    /// consumers below still read one identity.
    fn channel_member_names(&self, ws_idx: usize) -> Vec<ChannelMemberName> {
        let bases: Vec<(String, String)> = self
            .channel_member_panes(ws_idx)
            .into_iter()
            .filter_map(|member| {
                let info = self.agent_info(member.ws_idx, member.pane_id)?;
                let base = member_addressable_name(
                    self.workspace_label(member.ws_idx),
                    &info,
                    &member.public_id,
                );
                Some((member.public_id, base))
            })
            .collect();
        // Case-insensitively, because that is how a nick matches: `omp`
        // and `OMP` are one collision, not two unique names that `@omp`
        // would then refuse to resolve.
        let mut totals: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for (_, base) in &bases {
            *totals.entry(base.to_ascii_lowercase()).or_insert(0) += 1;
        }
        let mut ordinals: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        bases
            .into_iter()
            .map(|(public_id, base)| {
                let key = base.to_ascii_lowercase();
                let addressable = if totals.get(&key).copied().unwrap_or(0) < 2 {
                    base.clone()
                } else {
                    let ordinal = ordinals.entry(key).or_insert(0);
                    *ordinal += 1;
                    format!("{base}-{ordinal}")
                };
                ChannelMemberName {
                    public_id,
                    base,
                    addressable,
                }
            })
            .collect()
    }

    /// Every member pane of the channel, as a `channel.members` listing —
    /// who would receive a `channel.send`, and how they got there.
    fn channel_members(&self, ws_idx: usize, name: &str) -> Vec<ChannelMember> {
        let last_message_seq = match channels::read_tail(name, 1) {
            Ok(tail) => tail.last().map_or(0, |message| message.seq),
            Err(err) => {
                tracing::warn!(
                    channel = %name,
                    error = %err,
                    "failed to read channel tail for members; reporting no unread"
                );
                0
            }
        };
        let cursors = channels::read_channel_cursors(name);
        // ponytail: linear scan over tens of members, like the roster
        // de-duplication above.
        let names = self.channel_member_names(ws_idx);
        self.channel_member_panes(ws_idx)
            .into_iter()
            .map(|member| {
                let agent = self.agent_info(member.ws_idx, member.pane_id);
                let name = names
                    .iter()
                    .find(|named| named.public_id == member.public_id)
                    .map(|named| named.addressable.clone());
                let cursor = cursors
                    .iter()
                    .find(|entry| entry.pane == member.public_id)
                    .map_or(0, |entry| entry.seq);
                ChannelMember {
                    pane_id: member.public_id,
                    name,
                    agent_status: agent.map(|info| info.agent_status),
                    source: member.source,
                    unread: last_message_seq.saturating_sub(cursor),
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

    /// The workspace label of a pane's workspace — rung 0 of
    /// [`member_addressable_name`], and the owner's decision of 2026-08-29
    /// for what a channel identity is.
    ///
    /// A `#`-prefixed label is REFUSED: that is the form
    /// `find_channel_workspace` matches, so it names a CHANNEL, never an
    /// agent. Panes native to a channel workspace all share it, so honouring
    /// it would make every member of `#eng` answer to `@eng` and to nothing
    /// else — a collision manufactured by the very fix meant to remove one.
    /// It repairs attribution in the same stroke: such a sender used to be
    /// attributed `#eng` rather than by its own name.
    fn workspace_label(&self, ws_idx: usize) -> Option<&str> {
        self.state
            .workspaces
            .get(ws_idx)?
            .custom_name
            .as_deref()
            .filter(|label| !label.starts_with('#'))
    }

    /// How a sender is ATTRIBUTED — and deliberately the same chain that
    /// decides how it RESOLVES, so the name on the line is always a name
    /// `@` can address. Delegating rather than restating is the mechanism;
    /// the previous version restated the first rung and omitted the second,
    /// which is exactly how the two drifted (see
    /// [`member_addressable_name`]).
    ///
    /// `pub(crate)`: also the resolver for a message's `to_pane` in the
    /// chat view's Messages column (`app::input::chat` caches its result
    /// in `ChatViewState::to_names` at data-refresh time) — one identity
    /// chain, never restated at that callsite either.
    pub(crate) fn pane_display_name(&self, public_pane_id: &str) -> Option<String> {
        let (ws_idx, pane_id) = self.parse_pane_id(public_pane_id)?;
        let label = self.workspace_label(ws_idx);
        // A pane with no detected agent still has a workspace label, and
        // that label is a name: attribution must not degrade to "unknown"
        // just because detection has not landed yet.
        let Some(info) = self.agent_info(ws_idx, pane_id) else {
            return label.map(str::to_string);
        };
        Some(member_addressable_name(label, &info, public_pane_id))
    }

    /// Public pane ids of the channel's agent-hosting member panes —
    /// workspace panes and joined panes alike — which is the broadcast
    /// delivery set, minus `sender_pane`.
    ///
    /// The sender is excluded here rather than at the call site so that
    /// "who a broadcast reaches" has exactly one definition: a second
    /// caller cannot reintroduce the echo by forgetting the filter. A
    /// sender left in its own fan-out is delivered its own message and
    /// accumulates unread counts for text it wrote itself — measured on
    /// `#bun-nix`, where `w22:p6` sent and then held six unreads of its
    /// own sends. Targeted delivery already excluded the sender at the
    /// `to_pane` match; broadcast was the only path that did not, which is
    /// why the echo looked like it depended on addressing. `orc` draws the
    /// same line with `grep -vxF "$nick"` (`bin/orc:722,732`): same channel
    /// topology, same recipient list.
    fn channel_agent_member_pane_ids(&self, ws_idx: usize, sender_pane: &str) -> Vec<String> {
        self.channel_member_panes(ws_idx)
            .into_iter()
            .filter(|member| member.public_id != sender_pane)
            .filter(|member| self.agent_info(member.ws_idx, member.pane_id).is_some())
            .map(|member| member.public_id)
            .collect()
    }

    /// Resolves a nick (`channel.send`'s `to`, or a leading in-body
    /// `@nick`) against the channel's agent member panes — workspace panes
    /// and joined panes alike — plus the human seat at the TUI. A nick
    /// matches a member pane on any of three things: exact match on the
    /// raw public pane id (`w78:p1`), case-insensitive match on the compact
    /// colon-free pane id (`w78p1` — always available regardless of which
    /// rung the display name below settled on, so it is the disambiguator
    /// of last resort even when every other candidate collides), or
    /// case-insensitive match on the pane's addressable name from
    /// [`member_addressable_name`] (display name -> assigned name ->
    /// detected kind -> compact pane id — the same single fallback chain
    /// `channel_members` uses, so the two can never drift apart). It also
    /// matches on the effective human name (`ui.chat_name` ->
    /// `state.chat_name`). The human is a candidate in every channel: they
    /// read the chat view, not a pane, so there is no membership to check.
    /// Exactly one match -> `Unique` for a pane, `Human` for the seat; two
    /// or more -> `Ambiguous` with `pane (name)` candidate labels and
    /// `human (name)` for the seat — a nick matching multiple members
    /// through the same shared rung (two panes both detected as the same
    /// kind) stays `Ambiguous` rather than silently picking one; none ->
    /// `Unknown`.
    fn resolve_channel_nick(&self, ws_idx: usize, nick: &str) -> NickResolution {
        let mut matches: Vec<(String, String)> = Vec::new();
        for member in self.channel_member_names(ws_idx) {
            let compact_id = compact_pane_id(&member.public_id);
            // Both spellings match, and that is the anti-amplification
            // guarantee of ceo-bora#30 surviving ceo-bora#34: the SUFFIXED
            // name reaches exactly one pane, while the bare colliding base
            // still matches every holder and so still lands in
            // `Ambiguous` below. Dropping the base here would turn a
            // refusal into `Unknown` — quieter, and wrong.
            let matched = member.public_id == nick
                || compact_id.eq_ignore_ascii_case(nick)
                || member.base.eq_ignore_ascii_case(nick)
                || member.addressable.eq_ignore_ascii_case(nick);
            if matched {
                matches.push((member.public_id, member.addressable));
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
            .map(|(pane_id, name)| format!("{pane_id} ({name})"))
            .collect();
        if human_matches {
            candidates.push(format!("human ({})", self.state.chat_name));
        }
        NickResolution::Ambiguous(candidates)
    }
}

/// A member pane's addressable name: the single source of truth consumed
/// by both `channel_members` (what a `channel.members` listing shows) and
/// `resolve_channel_nick` (what `--to`/leading `@nick` actually match), so
/// the two fallback chains can never drift apart again. Order, most to
/// least specific: the **workspace label** (`custom_name` — the name the
/// sidebar shows, and the owner's decision of 2026-08-29 for what a
/// channel identity IS) -> `display_agent` (the agent's own self-reported
/// name) -> `name` (registered via `bora agent rename`) -> `agent`
/// (detected tool kind, e.g. "omp") -> the compact addressable pane id
/// (`w78p1` — the same colon-free form `workspace_agent_label` mints for
/// the sidebar badge in 0.26.0, see `src/ui/sidebar.rs`). The last rung
/// guarantees every member always has a non-null, unique, typeable name.
///
/// Rung 0 exists because attribution and resolution had drifted in the one
/// way the doc comment above did not cover: the sender line was built by
/// [`App::pane_display_name`], which starts at the workspace label, while
/// matching started at `display_agent` — so on `#metodo-pp` the two members
/// were ATTRIBUTED as `ceo-pp` and `ceo-bora` and yet `@ceo-bora` resolved
/// to nobody, while `@omp` (their shared detected kind) matched both. The
/// name you read was never the name that resolved. Sharing this one helper
/// is not enough when its INPUT omits the rung the other consumer starts
/// at.
fn member_addressable_name(
    workspace_label: Option<&str>,
    info: &AgentInfo,
    public_id: &str,
) -> String {
    workspace_label
        .map(str::to_string)
        .or_else(|| info.display_agent.clone())
        .or_else(|| info.name.clone())
        .or_else(|| info.agent.clone())
        .unwrap_or_else(|| compact_pane_id(public_id))
}

/// Strips the `:` from a canonical public pane id (`w78:p1` -> `w78p1`) —
/// the colon-free form `bora agent prompt`/`orc channel send` already
/// accept for other pane addressing, and the form the sidebar mints for
/// its agent identity badge (0.26.0).
fn compact_pane_id(public_id: &str) -> String {
    public_id.replace(':', "")
}

/// One resolved channel member pane: where it lives, its canonical public
/// id, and whether membership is workspace-implicit or explicitly joined.
struct ChannelMemberPane {
    ws_idx: usize,
    pane_id: crate::layout::PaneId,
    public_id: String,
    source: ChannelMemberSource,
}

/// One agent-hosting member's two names: the `base` its rung chain mints
/// and the `addressable` one a nick actually gets, which differ only when
/// the base collided and [`App::channel_member_names`] suffixed it.
/// Carrying both is what lets the resolver keep refusing a bare colliding
/// base while resolving its suffixed forms.
struct ChannelMemberName {
    public_id: String,
    base: String,
    addressable: String,
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

/// Single-quote a value for safe interpolation into a typed shell command
/// line — same idiom as `render_flow_command`'s quoting in `flow.rs`.
/// Needed because [`App::seed_channel_tail_pane`] types a channel name
/// (arbitrary API input) and a binary path into a live shell.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// True when `process`'s argv contains `channel tail <name> --follow` as a
/// contiguous run — the exact shape [`App::seed_channel_tail_pane`] types,
/// once the shell that ran it has parsed away the quoting. Matched on argv
/// (not the raw `cmdline` string) so shell quoting or incidental whitespace
/// can never produce a false negative or a false positive on an unrelated
/// process that merely mentions "channel tail" in passing.
fn is_channel_tail_process(process: &crate::platform::ForegroundProcess, name: &str) -> bool {
    let Some(argv) = process.argv.as_deref() else {
        return false;
    };
    argv.windows(4).any(|window| {
        window[0] == "channel"
            && window[1] == "tail"
            && window[2] == name
            && window[3] == "--follow"
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::ChannelDeliveryStatus;
    use crate::config::{IsolatedDirs, ShellModeConfig};

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

    #[tokio::test]
    async fn create_normalizes_name_and_rejects_duplicates() {
        let _isolated = IsolatedDirs::new("create-normalize");
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

    /// The core acceptance case: a fresh channel's root pane must run
    /// `bora channel tail <name> --follow`, not sit at a bare shell.
    /// `/bin/cat` stands in for an interactive shell here — it echoes
    /// whatever is typed into it back onto the pane's screen, which is
    /// what makes the seeded command observable without a real `bora`
    /// binary or a real channel-tail session.
    #[cfg(unix)]
    #[tokio::test]
    async fn create_seeds_the_pane_with_the_channel_tail_command() {
        let _isolated = IsolatedDirs::new("create-seed");
        let mut app = test_app();
        app.state.default_shell = "/bin/cat".into();
        create_channel(&mut app, "eng");

        // The transcript lives in its own pane now, not the tab root, so scan
        // every pane in the room for the seeded command.
        let pane_ids: Vec<_> = app.state.workspaces[0].tabs[0]
            .panes
            .keys()
            .copied()
            .collect();
        assert!(
            pane_ids.len() >= 2,
            "a created channel must have a transcript pane and a shell pane, got {pane_ids:?}"
        );

        // A long test-binary path plus the command can exceed the pane's
        // column width and wrap mid-word; strip whitespace from both sides
        // so a wrap-inserted newline can never break the match.
        let expected: String = format!("channel tail {} --follow", shell_quote("eng"))
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        let dense =
            |screen: &str| -> String { screen.chars().filter(|c| !c.is_whitespace()).collect() };
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut screens: Vec<String> = Vec::new();
        let mut seeded = false;
        while !seeded && std::time::Instant::now() < deadline {
            screens = pane_ids
                .iter()
                .filter_map(|pane_id| app.lookup_runtime_sender(0, *pane_id))
                .map(crate::terminal::TerminalRuntime::visible_text)
                .collect();
            seeded = screens
                .iter()
                .any(|screen| dense(screen).contains(&expected));
            if !seeded {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        }
        assert!(
            seeded,
            "one channel pane must run `bora channel tail eng --follow`, got: {screens:?}"
        );

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// A seed that cannot land — no registered runtime at all, standing in
    /// for the unresolved-binary and send-failure branches too, since they
    /// all collapse to the same early return — must never turn into a
    /// failed `channel.create`.
    #[tokio::test]
    async fn create_succeeds_even_when_the_channel_pane_cannot_be_seeded() {
        let _isolated = IsolatedDirs::new("create-noseed");
        let mut app = test_app();

        app.seed_channel_tail_pane(0, crate::layout::PaneId::alloc(), "ghost");

        let created = create_channel(&mut app, "ops");
        assert!(
            created["result"]["channel"].is_object(),
            "channel creation must succeed even if seeding the pane fails: {created}"
        );

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn create_gives_the_channel_both_a_transcript_and_a_shell_pane() {
        let _isolated = IsolatedDirs::new("create-panes");
        let mut app = test_app();
        create_channel(&mut app, "twopane");
        let ws_idx = app
            .find_channel_workspace("twopane")
            .expect("the channel workspace must exist");
        let pane_count = app.state.workspaces[ws_idx]
            .tabs
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .count();
        assert_eq!(
            pane_count, 2,
            "a freshly created channel must have both a transcript pane and a \
             shell pane, got {pane_count}"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// The `#runner-disk-full` repair case: a channel workspace built
    /// before the two-pane shape shipped stays a single bare-shell pane
    /// forever, since nothing but `channel.open` ever re-checks an
    /// existing channel's panes.
    #[tokio::test]
    async fn open_repairs_a_pre_two_pane_shape_channel_workspace() {
        let _isolated = IsolatedDirs::new("open-repair");
        let mut app = test_app();
        let ws_idx = app
            .create_workspace_with_launch_env(std::env::temp_dir(), false, Vec::new())
            .expect("workspace creation must succeed");
        app.state.workspaces[ws_idx].set_custom_name("#runner-disk-full".into());

        let pane_count_before = app.state.workspaces[ws_idx]
            .tabs
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .count();
        assert_eq!(
            pane_count_before, 1,
            "the pre-fix channel shape is a single bare-shell pane"
        );

        let response = app.handle_channel_open(
            "req".into(),
            ChannelOpenParams {
                name: "runner-disk-full".into(),
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert!(
            response["result"]["channel"].is_object(),
            "channel.open must succeed: {response}"
        );

        let pane_count_after = app.state.workspaces[ws_idx]
            .tabs
            .iter()
            .flat_map(|tab| tab.layout.pane_ids())
            .count();
        assert_eq!(
            pane_count_after, 2,
            "channel.open must repair a bare-shell channel workspace by \
             adding its transcript pane"
        );

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn open_on_unknown_channel_returns_channel_not_found() {
        let mut app = test_app();
        let response = app.handle_channel_open(
            "req".into(),
            ChannelOpenParams {
                name: "ghost-channel".into(),
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(
            response["error"]["code"],
            serde_json::json!("channel_not_found")
        );
        assert!(
            app.state.workspaces.is_empty(),
            "channel.open must never create a workspace for an unknown channel"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// `channel.open` on an already-repaired channel — both halves of the
    /// two-pane shape already present — must be a true no-op. Pane 2 is
    /// made to impersonate a real `channel tail idem --follow` process: a
    /// genuine `/bin/sh` subprocess whose own argv ends in exactly those
    /// four tokens (`sleep 999 & wait` keeps it alive so detection can
    /// never race its exit), so [`App::channel_workspace_has_tail_pane`]
    /// sees the same shape a real seeded pane produces — proving
    /// idempotency is driven by real process info (contract item 3), not
    /// by remembering that this test itself added the pane.
    #[cfg(unix)]
    #[tokio::test]
    async fn open_twice_on_an_already_repaired_channel_does_not_duplicate_the_transcript_pane() {
        let _isolated = IsolatedDirs::new("open-twice-idem");
        let mut app = test_app();
        app.state.default_shell = "/bin/sh".into();

        let ws_idx = app
            .create_workspace_with_launch_env(std::env::temp_dir(), false, Vec::new())
            .expect("workspace creation must succeed");
        app.state.workspaces[ws_idx].set_custom_name("#idem".into());

        let root_pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let root_public_id = app.public_pane_id(ws_idx, root_pane_id).unwrap();
        let split_response = app.handle_pane_split(
            "req:split".into(),
            PaneSplitParams {
                workspace_id: None,
                target_pane_id: Some(root_public_id),
                direction: SplitDirection::Down,
                ratio: None,
                cwd: None,
                focus: false,
                right_click: PaneRightClickTarget::default(),
                env: std::collections::HashMap::new(),
            },
        );
        let split_response: serde_json::Value = serde_json::from_str(&split_response).unwrap();
        let pane2_public_id = split_response["result"]["pane"]["pane_id"]
            .as_str()
            .expect("the setup split must succeed")
            .to_string();
        let (_, pane2_id) = app.parse_pane_id(&pane2_public_id).unwrap();

        let runtime = app
            .lookup_runtime_sender(ws_idx, pane2_id)
            .expect("pane 2 must have a runtime");
        let command = "/bin/sh -c 'sleep 999 & wait' channel tail idem --follow";
        let bytes =
            crate::app::api_helpers::encode_api_input(runtime, command, &["Enter".to_string()])
                .expect("encoding the marker command must not fail");
        runtime
            .try_send_bytes(Bytes::from(bytes))
            .expect("sending the marker command must not fail");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !app.channel_workspace_has_tail_pane(ws_idx, "idem")
            && std::time::Instant::now() < deadline
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(
            app.channel_workspace_has_tail_pane(ws_idx, "idem"),
            "the marker process must be detected as the channel's transcript pane"
        );

        let pane_count = |app: &App| -> usize {
            app.state.workspaces[ws_idx]
                .tabs
                .iter()
                .flat_map(|tab| tab.layout.pane_ids())
                .count()
        };
        assert_eq!(
            pane_count(&app),
            2,
            "sanity: root pane plus the marker pane"
        );

        app.handle_channel_open(
            "req1".into(),
            ChannelOpenParams {
                name: "idem".into(),
            },
        );
        assert_eq!(
            pane_count(&app),
            2,
            "an already-complete channel must not grow a pane on the first open"
        );

        app.handle_channel_open(
            "req2".into(),
            ChannelOpenParams {
                name: "idem".into(),
            },
        );
        assert_eq!(
            pane_count(&app),
            2,
            "a second channel.open must not duplicate the transcript pane either"
        );

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn list_reports_only_hash_named_ungrouped_workspaces() {
        let _isolated = IsolatedDirs::new("list-hash-only");
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

        let list = app.handle_channel_list("req".into(), ChannelListParams { from_pane: None });
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        let channels = list["result"]["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["name"], serde_json::json!("#eng"));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn send_appends_transcript_and_reports_history() {
        let _isolated = IsolatedDirs::new("send");
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
                when_idle: None,
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
                from_pane: None,
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
        let _isolated = IsolatedDirs::new("send-human");
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
                when_idle: None,
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
                when_idle: None,
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
        let _isolated = IsolatedDirs::new("send-human-rate");
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
                    when_idle: None,
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
        let _isolated = IsolatedDirs::new("missing");
        let mut app = test_app();
        let history = app.handle_channel_history(
            "req".into(),
            ChannelHistoryParams {
                name: "nope".into(),
                lines: Some(10),
                from_pane: None,
            },
        );
        let history: serde_json::Value = serde_json::from_str(&history).unwrap();
        assert!(history["result"]["messages"].as_array().unwrap().is_empty());
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn queued_prompt_drop_appends_system_line_to_originating_channel_history() {
        let _isolated = IsolatedDirs::new("drop-notice");
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
            // Busy target: delivery is immediate by default now, so the
            // opt-in `when_idle` is what defers here — the drop notice this
            // test exists for is only reachable for a QUEUED prompt.
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
                when_idle: Some(true),
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

    /// Default delivery: a member mid-turn receives the message NOW (like
    /// steering) — injected bytes, `delivered` receipt, nothing queued.
    /// This is the behavior the v5 briefing teaches.
    #[tokio::test]
    async fn channel_send_to_working_member_injects_immediately_by_default() {
        let _isolated = IsolatedDirs::new("send-midturn");
        let mut app = test_app();
        let ws_idx = create_bare_channel_workspace(&mut app, "eng");
        let first = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let second =
            app.state.workspaces[ws_idx].test_split(ratatui::layout::Direction::Horizontal);
        app.state.ensure_test_terminals();
        for (pane, state) in [
            (first, crate::detect::AgentState::Idle),
            (second, crate::detect::AgentState::Working),
        ] {
            let terminal_id = app.state.workspaces[ws_idx]
                .pane_state(pane)
                .unwrap()
                .attached_terminal_id
                .clone();
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.set_detected_state(Some(crate::detect::Agent::OpenCode), state);
        }
        // BOTH members get runtimes: the Working one too, so the send has a
        // real pane to type into mid-turn. The receiver must stay alive.
        let (_rx1, mut rx2) = {
            let (r1, rx1) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
            app.state.insert_test_runtime(first, r1);
            let (r2, rx2) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
            app.state.insert_test_runtime(second, r2);
            (rx1, rx2)
        };
        let worker = app.public_pane_id(ws_idx, second).unwrap();
        skip_protocol("eng", &worker);
        let first_public = app.public_pane_id(ws_idx, first).unwrap();
        skip_protocol("eng", &first_public);

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "mid-turn hello".into(),
                from_pane: Some(first_public),
                to: None,
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        let worker_delivery = deliveries
            .iter()
            .find(|d| d["pane_id"] == serde_json::json!(worker))
            .expect("the Working member must be a delivery target");
        assert_eq!(
            worker_delivery["status"],
            serde_json::json!("delivered"),
            "a Working member must receive the message immediately by default: {worker_delivery:?}"
        );
        let injected = rx2
            .try_recv()
            .expect("mid-turn delivery must type into the busy pane");
        let injected = String::from_utf8_lossy(&injected);
        assert!(injected.contains("[#eng seq=1 from "), "got: {injected}");
        assert!(injected.contains("mid-turn hello"), "got: {injected}");
        assert!(
            !app.pending_agent_prompts.contains_key(&worker),
            "immediate delivery must not queue anything"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// Opt-in: `when_idle` restores the hold-until-idle delivery for a
    /// Working member — queued, `deferred` receipt with the queue position.
    #[tokio::test]
    async fn channel_send_when_idle_defers_to_working_member() {
        let _isolated = IsolatedDirs::new("send-when-idle");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "hold this".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
                when_idle: Some(true),
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        let worker_delivery = deliveries
            .iter()
            .find(|d| d["pane_id"] == serde_json::json!(worker))
            .expect("the Working member must be a delivery target");
        assert_eq!(
            worker_delivery["status"],
            serde_json::json!("deferred"),
            "when_idle must hold delivery for the Working member: {worker_delivery:?}"
        );
        assert_eq!(
            worker_delivery["detail"],
            serde_json::json!("queued (pos 1)"),
            "the deferred receipt must carry the queue position"
        );
        assert!(app.pending_agent_prompts.contains_key(&worker));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// Fan-out order contract: an UNBRIEFED Working member receives the
    /// protocol block and the message in the SAME delivery mode — both
    /// injected immediately by default, briefing FIRST, nothing queued. A
    /// briefing that queued while the message injected would teach the
    /// agent a protocol it reads only after answering a message it was
    /// never taught to read.
    #[tokio::test]
    async fn channel_protocol_and_message_inject_together_for_working_member() {
        let _isolated = IsolatedDirs::new("send-unbriefed-working");
        let mut app = test_app();
        let ws_idx = create_bare_channel_workspace(&mut app, "eng");
        let first = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let second =
            app.state.workspaces[ws_idx].test_split(ratatui::layout::Direction::Horizontal);
        app.state.ensure_test_terminals();
        for (pane, state) in [
            (first, crate::detect::AgentState::Idle),
            (second, crate::detect::AgentState::Working),
        ] {
            let terminal_id = app.state.workspaces[ws_idx]
                .pane_state(pane)
                .unwrap()
                .attached_terminal_id
                .clone();
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
            terminal.set_detected_state(Some(crate::detect::Agent::OpenCode), state);
        }
        let (_rx1, mut rx2) = {
            let (r1, rx1) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
            app.state.insert_test_runtime(first, r1);
            let (r2, rx2) = crate::terminal::TerminalRuntime::test_with_channel(80, 24);
            app.state.insert_test_runtime(second, r2);
            (rx1, rx2)
        };
        let worker = app.public_pane_id(ws_idx, second).unwrap();
        // Deliberately NO skip_protocol: the member has never been briefed.

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "first words".into(),
                from_pane: None,
                to: None,
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 2);
        let worker_delivery = deliveries
            .iter()
            .find(|d| d["pane_id"] == serde_json::json!(worker))
            .expect("the Working member must be a delivery target");
        assert_eq!(worker_delivery["status"], serde_json::json!("delivered"));
        assert!(
            !app.pending_agent_prompts.contains_key(&worker),
            "briefing and message must BOTH inject to an unbriefed Working member, not queue"
        );

        // Order: the protocol block reaches the pane first, the message second.
        let first_bytes = rx2.try_recv().expect("briefing must inject");
        let first_write = String::from_utf8_lossy(&first_bytes);
        assert!(
            first_write.contains("channel protocol for #eng"),
            "briefing must be the FIRST write into an unbriefed pane: {first_write:?}"
        );
        let second_bytes = rx2.try_recv().expect("message must inject");
        let second_write = String::from_utf8_lossy(&second_bytes);
        assert!(
            second_write.contains("[#eng seq=") && second_write.contains("first words"),
            "the channel message must follow the briefing: {second_write:?}"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// The briefing must teach the delivery truth: immediate mid-turn
    /// arrival by default, `--when-idle` as the opt-in — and the version
    /// bump is what gets the correction re-briefed to already-briefed panes.
    #[test]
    fn channel_protocol_briefing_teaches_immediate_delivery() {
        assert!(
            CHANNEL_PROTOCOL.contains("--when-idle"),
            "briefing must name the --when-idle opt-in"
        );
        assert!(
            CHANNEL_PROTOCOL.contains("WHILE YOU ARE WORKING"),
            "briefing must state that messages arrive mid-turn"
        );
        assert!(
            !CHANNEL_PROTOCOL.contains("deferred while the"),
            "stale claim that sends wait for idle must be gone"
        );
    }

    #[tokio::test]
    async fn members_on_missing_channel_is_error() {
        let _isolated = IsolatedDirs::new("members-missing");
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
        let _isolated = IsolatedDirs::new("members-agent");
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
        assert_eq!(
            members.len(),
            2,
            "the channel's shell pane and its transcript pane are both members: {members:?}"
        );
        let agent_member = members
            .iter()
            .find(|member| member["agent_status"] == serde_json::json!("idle"))
            .expect("the agent-hosting pane must be listed with its status");
        assert!(agent_member["pane_id"].as_str().unwrap().contains(':'));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn members_reports_detected_kind_as_name_when_unregistered() {
        let _isolated = IsolatedDirs::new("members-detected-kind-name");
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
        // No `set_agent_name` (no `bora agent rename`) and no display_agent
        // — the fleet's common case: a detected tool with no registered
        // name, which used to report `name: null` here.
        terminal.detected_agent = Some(crate::detect::Agent::Claude);
        terminal.state = crate::detect::AgentState::Idle;

        let response =
            app.handle_channel_members("req".into(), ChannelMembersParams { name: "eng".into() });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        let members = response["result"]["members"].as_array().unwrap();
        assert_eq!(
            members.len(),
            2,
            "the channel's shell pane and its transcript pane are both members: {members:?}"
        );
        let agent_member = members
            .iter()
            .find(|member| member["name"] == serde_json::json!("claude"))
            .expect("an unnamed pane must still report its detected kind, not a null name");
        assert!(agent_member["pane_id"].as_str().unwrap().contains(':'));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn list_reports_member_status_counts() {
        let _isolated = IsolatedDirs::new("members-counts");
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

        let list = app.handle_channel_list("req".into(), ChannelListParams { from_pane: None });
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        let channels = list["result"]["channels"].as_array().unwrap();
        assert_eq!(
            channels[0]["member_status_counts"]["working"],
            serde_json::json!(1)
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn list_reports_last_message_seq_and_ts_and_zero_for_unmessaged_channel() {
        let _isolated = IsolatedDirs::new("list-last-message");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        create_channel(&mut app, "quiet");

        app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "hello".into(),
                from_pane: Some("w1A:p2".into()),
                to: None,
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );

        let list = app.handle_channel_list("req".into(), ChannelListParams { from_pane: None });
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        let channels = list["result"]["channels"].as_array().unwrap();
        let eng = channels
            .iter()
            .find(|channel| channel["name"] == serde_json::json!("#eng"))
            .expect("eng must be listed");
        assert_eq!(eng["last_message_seq"], serde_json::json!(1));
        assert!(
            eng["last_message_ts"]
                .as_str()
                .is_some_and(|ts| !ts.is_empty()),
            "a messaged channel must report a non-empty last_message_ts: {eng}"
        );

        let quiet = channels
            .iter()
            .find(|channel| channel["name"] == serde_json::json!("#quiet"))
            .expect("quiet must be listed");
        assert_eq!(quiet["last_message_seq"], serde_json::json!(0));
        assert_eq!(quiet["last_message_ts"], serde_json::Value::Null);

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

    #[test]
    fn member_addressable_name_falls_back_to_compact_pane_id_when_all_rungs_are_unset() {
        let info = AgentInfo {
            terminal_id: "t1".into(),
            name: None,
            agent: None,
            title: None,
            terminal_title: None,
            terminal_title_stripped: None,
            display_agent: None,
            agent_status: AgentStatus::Idle,
            screen_detection_skipped: false,
            state_labels: Default::default(),
            tokens: Default::default(),
            agent_session: None,
            workspace_id: "w1".into(),
            tab_id: "w1t1".into(),
            pane_id: "p1".into(),
            focused: false,
            launch_pending: false,
            interactive_ready: true,
            state_change_seq: 0,
            cwd: None,
            foreground_cwd: None,
            revision: 1,
        };
        // No workspace label and neither `display_agent`, `name`, nor
        // `agent` is set — every rung of the chain is empty, so the helper
        // must fall back to the compact colon-free pane id rather than
        // reporting nothing.
        assert_eq!(member_addressable_name(None, &info, "w78:p1"), "w78p1");
        // Rung 0 is the workspace label and outranks every other rung —
        // the owner's decision of 2026-08-29. Asserted on the same fixture
        // so the only difference is the label itself.
        assert_eq!(
            member_addressable_name(Some("ceo-bora"), &info, "w78:p1"),
            "ceo-bora"
        );
        let kinded = AgentInfo {
            agent: Some("omp".into()),
            ..info
        };
        assert_eq!(
            member_addressable_name(Some("ceo-bora"), &kinded, "w78:p1"),
            "ceo-bora"
        );
        assert_eq!(member_addressable_name(None, &kinded, "w78:p1"), "omp");
    }

    /// A `#name` channel workspace with exactly one bare pane — bypassing
    /// `channel.create`'s own two-pane seeding, so fixtures built on top
    /// (agent panes added via `test_split`) get to control every pane in
    /// the workspace themselves rather than sharing it with an auto-added
    /// transcript pane that is not part of what they are testing.
    fn create_bare_channel_workspace(app: &mut App, name: &str) -> usize {
        let index = app
            .create_workspace_with_launch_env(std::env::temp_dir(), false, Vec::new())
            .expect("workspace creation must succeed");
        if let Some(workspace) = app.state.workspaces.get_mut(index) {
            workspace.set_custom_name(format!("#{name}"));
        }
        index
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
        let ws_idx = create_bare_channel_workspace(app, "eng");
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

    /// Channel workspace with two agent member panes sharing a detected
    /// kind and no registered name — the fleet's common case (12 of 13
    /// live agents have no `bora agent rename`, only a detected tool).
    /// The first is idle with a test runtime (promptable -> `delivered`;
    /// its receiver is returned and must stay alive or the runtime's send
    /// channel closes); the second is working (no runtime needed ->
    /// `deferred`). Returns both public pane ids.
    fn channel_with_two_same_kind_agents(
        app: &mut App,
    ) -> (String, String, tokio::sync::mpsc::Receiver<bytes::Bytes>) {
        let ws_idx = create_bare_channel_workspace(app, "eng");
        let first = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let second =
            app.state.workspaces[ws_idx].test_split(ratatui::layout::Direction::Horizontal);
        app.state.ensure_test_terminals();
        for (pane, state) in [
            (first, crate::detect::AgentState::Idle),
            (second, crate::detect::AgentState::Working),
        ] {
            let terminal_id = app.state.workspaces[ws_idx]
                .pane_state(pane)
                .unwrap()
                .attached_terminal_id
                .clone();
            let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
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
    async fn send_to_shared_kind_nick_stays_ambiguous_without_registered_names() {
        let _isolated = IsolatedDirs::new("send-to-kind-ambiguous");
        let mut app = test_app();
        let (first, second, _rx) = channel_with_two_same_kind_agents(&mut app);

        let error = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "which one?".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some("opencode".into()),
                in_reply_to: None,
                when_idle: None,
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
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn send_to_compact_pane_id_resolves_uniquely_despite_shared_kind() {
        let _isolated = IsolatedDirs::new("send-to-compact-id-unique");
        let mut app = test_app();
        let (first, second, _rx) = channel_with_two_same_kind_agents(&mut app);
        skip_protocol("eng", &first);
        skip_protocol("eng", &second);
        let compact_id = first.replace(':', "");

        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: Some("w1A:p9".into()),
                to: Some(compact_id),
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(
            deliveries.len(),
            1,
            "the compact pane id must resolve to exactly the first pane, not both same-kind panes"
        );
        assert_eq!(deliveries[0]["pane_id"], serde_json::json!(first));
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn send_to_param_targets_unique_nick_and_threads_reply_by_pane_id() {
        let _isolated = IsolatedDirs::new("send-to-unique");
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
                when_idle: None,
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
        assert!(injected.contains("[#eng seq=1 from "), "got: {injected}");
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
                // The worker pane is mid-turn by fixture: opt into the
                // hold-until-idle delivery so the reply is QUEUED (the
                // `deferred` receipt asserted below), not injected.
                when_idle: Some(true),
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
        let _isolated = IsolatedDirs::new("send-to-ambiguous");
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
                when_idle: None,
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
        let _isolated = IsolatedDirs::new("send-to-unknown");
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
                when_idle: None,
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
        let _isolated = IsolatedDirs::new("send-to-human");
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
                when_idle: None,
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
        let _isolated = IsolatedDirs::new("send-to-human-collision");
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
                when_idle: None,
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

    /// Was `leading_mention_to_human_targets_the_seat_and_unknown_still_broadcasts`,
    /// which locked the degrade as intended behaviour. Owner's decision,
    /// ceo-bora#31 of 2026-08-29, reverses the second half: the human seat
    /// still resolves, and an unknown mention now REFUSES. The first half is
    /// unchanged and asserted identically, so the reversal is visibly scoped
    /// to the unknown case rather than to addressing as a whole.
    #[tokio::test]
    async fn leading_mention_to_human_targets_the_seat_and_unknown_refuses() {
        let _isolated = IsolatedDirs::new("mention-human");
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
                when_idle: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert!(
            sent["error"].is_null(),
            "a mention that resolves still sends: {sent}"
        );
        assert!(
            sent["result"]["deliveries"].as_array().unwrap().is_empty(),
            "a human-addressed mention delivers to no pane"
        );
        let history = channels::read_tail("eng", 10).unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].to_human);
        assert_eq!(history[0].to_pane, None);

        // A mention matching neither the human nor any member is a loud
        // error, and nothing is appended or delivered: this is the exact
        // amplification that made one intended recipient into every member
        // pane (ceo-bora#30).
        let refused = app.handle_channel_send(
            "req2".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@ghost hi".into(),
                from_pane: Some("w1A:p8".into()),
                to: None,
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let refused: serde_json::Value = serde_json::from_str(&refused).unwrap();
        assert_eq!(
            refused["error"]["code"],
            serde_json::json!("channel_nick_unknown"),
            "{refused}"
        );
        assert!(
            refused["result"].is_null(),
            "a refused send reports no deliveries: {refused}"
        );
        assert_eq!(
            channels::read_tail("eng", 10).unwrap().len(),
            1,
            "a refused mention must append nothing"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_send_rate_limits_repeated_from_pane_but_exempts_missing_from_pane() {
        let _isolated = IsolatedDirs::new("send-rate-limit");
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
                when_idle: None,
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
                when_idle: None,
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
                when_idle: None,
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
                when_idle: None,
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
        let _isolated = IsolatedDirs::new("mention-unique");
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
                when_idle: None,
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

    /// Was `leading_mention_degrades_to_broadcast_when_not_uniquely_resolvable`.
    /// This is the repro for ceo-bora#30: the `"dup"/"dup"` fixture IS the
    /// `#metodo-pp` case where both members were detected as kind `omp`, and
    /// the numbers it asserts are the before/after. BEFORE: `@dup` resolved
    /// Ambiguous, Ambiguous meant broadcast, and one intended recipient
    /// became `deliveries.len() == 2` — every member pane, each reached
    /// through `handle_agent_prompt`, i.e. text typed into a live session.
    /// AFTER: the send is refused, `deliveries` does not exist, and the
    /// transcript gains nothing. No echo and no re-injection were ever
    /// involved; the amplifier was the silent fallback itself.
    #[tokio::test]
    async fn unresolvable_leading_mention_refuses_instead_of_amplifying() {
        let _isolated = IsolatedDirs::new("mention-degrade");
        let mut app = test_app();
        let (reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");
        skip_protocol("eng", &reviewer);
        skip_protocol("eng", &worker);

        // Unknown nick: refused, nothing appended, nobody prompted.
        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@ghost are you here".into(),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert_eq!(
            sent["error"]["code"],
            serde_json::json!("channel_nick_unknown"),
            "{sent}"
        );
        assert!(
            channels::read_tail("eng", 10).unwrap().is_empty(),
            "a refused unknown mention must append nothing"
        );

        // Ambiguous nick — two agents sharing one name, the measured
        // `#metodo-pp` topology. Refused, and the error names both so the
        // sender can address one.
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
                when_idle: None,
                from_human: false,
            },
        );
        let sent2: serde_json::Value = serde_json::from_str(&sent2).unwrap();
        assert_eq!(
            sent2["error"]["code"],
            serde_json::json!("channel_nick_ambiguous"),
            "{sent2}"
        );
        let message = sent2["error"]["message"].as_str().unwrap();
        for pane in [&dup1, &dup2] {
            assert!(
                message.contains(pane.as_str()),
                "the refusal must name every candidate so one can be picked: {message}"
            );
        }
        // Both apps share the process-global isolated state dir, so this
        // reads the whole transcript: an amplified send would have left a
        // line here and prompted two panes.
        assert!(
            channels::read_tail("eng", 10).unwrap().is_empty(),
            "a refused ambiguous mention must append nothing"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
        super::super::test_support::shutdown_test_runtimes(&mut app2);
    }

    #[tokio::test]
    async fn escapes_unescape_to_literal_and_never_address() {
        let _isolated = IsolatedDirs::new("escapes");
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
                when_idle: None,
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
                when_idle: None,
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
        let _isolated = IsolatedDirs::new("self-target");
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
                when_idle: None,
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

    /// Join-time collision autorename (ceo-bora#34): two panes answering to
    /// the same base name become `@rev-1`/`@rev-2`, the channel is told, the
    /// suffixed names each reach exactly one pane, and the bare colliding
    /// base still REFUSES rather than amplifying to both (ceo-bora#30).
    #[tokio::test]
    async fn colliding_joins_are_autorenamed_by_join_order() {
        let _isolated = IsolatedDirs::new("autorename-join");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (first, _first_rx) = outside_agent_pane(&mut app, "rev");
        let (second, _second_rx) = outside_agent_pane(&mut app, "rev");
        skip_protocol("eng", &first);
        skip_protocol("eng", &second);

        assert_eq!(join(&mut app, "#eng", &first)["result"]["source"], "joined");
        assert_eq!(
            member_names(&mut app, "#eng"),
            vec!["rev".to_string()],
            "one holder of the base name is not a collision, so it keeps it"
        );

        assert_eq!(
            join(&mut app, "#eng", &second)["result"]["source"],
            "joined"
        );
        assert_eq!(
            member_names(&mut app, "#eng"),
            vec!["rev-1".to_string(), "rev-2".to_string()],
            "the second join collides, so both holders take an ordinal"
        );

        // Contract 2: the channel is told who joined and how it ended up.
        let notice = channels::read_tail("eng", 10)
            .unwrap()
            .into_iter()
            .find(|message| message.from_pane == "system" && message.text.contains("rev-2"))
            .expect("a colliding join must announce the rename");
        assert_eq!(notice.from_name, "bora");
        assert!(
            notice.text.contains(&second),
            "the notice names who joined: {}",
            notice.text
        );

        // Contract 3, both halves. A suffixed nick is unique...
        let sent = app.handle_channel_send(
            "req-suffixed".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: None,
                to: Some("rev-2".into()),
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 1, "@rev-2 reaches exactly one pane");
        assert_eq!(deliveries[0]["pane_id"], serde_json::json!(second));

        // ...and the bare base still refuses, which is the anti-amplification
        // guarantee autorename must not quietly retire.
        let refused = app.handle_channel_send(
            "req-bare".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: None,
                to: Some("rev".into()),
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let refused: serde_json::Value = serde_json::from_str(&refused).unwrap();
        assert_eq!(
            refused["error"]["code"],
            serde_json::json!("channel_nick_ambiguous"),
            "{refused}"
        );

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// The ordinal follows the PERSISTED join order, which is what makes it
    /// survive a restart (ceo-bora#34): nothing is stored per rename, so the
    /// roster file is the only thing the names can come from. Rewriting that
    /// file with the opposite join history must swap the ordinals — an
    /// implementation keying off pane id, or off a `HashMap`'s iteration
    /// order, would not move.
    #[tokio::test]
    async fn autorename_ordinals_follow_the_persisted_join_order() {
        let _isolated = IsolatedDirs::new("autorename-restart");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (first, _first_rx) = outside_agent_pane(&mut app, "rev");
        let (second, _second_rx) = outside_agent_pane(&mut app, "rev");
        skip_protocol("eng", &first);
        skip_protocol("eng", &second);
        join(&mut app, "#eng", &first);
        join(&mut app, "#eng", &second);

        assert_eq!(
            channels::read_joined_members("eng", |_| true),
            vec![first.clone(), second.clone()],
            "the roster is the append-ordered record of who joined when"
        );
        assert_eq!(
            member_names(&mut app, "#eng"),
            vec!["rev-1".to_string(), "rev-2".to_string()]
        );

        // Re-reading it unchanged is a restart: same order in, same names out.
        assert_eq!(
            member_names(&mut app, "#eng"),
            vec!["rev-1".to_string(), "rev-2".to_string()],
            "same join order must always mint the same names"
        );

        // The other join history, on disk, mints the other assignment.
        channels::write_joined_members("eng", &[second.clone(), first.clone()]).unwrap();
        let response = app.handle_channel_members(
            "req".into(),
            ChannelMembersParams {
                name: "#eng".into(),
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        let members = response["result"]["members"].as_array().unwrap();
        let by_pane = |pane: &str| -> String {
            members
                .iter()
                .find(|member| member["pane_id"] == serde_json::json!(pane))
                .and_then(|member| member["name"].as_str())
                .expect("member must be listed")
                .to_string()
        };
        assert_eq!(by_pane(&second), "rev-1");
        assert_eq!(by_pane(&first), "rev-2");

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }
    /// Three holders of one base name — the "colisão múltipla" path of
    /// ceo-bora#34's DoD. The ordinals keep counting: `rev-1`/`rev-2`/
    /// `rev-3`, the third colliding join announces its rename like the
    /// others, the highest suffix reaches exactly its own pane, and the
    /// bare base refuses naming all three candidates rather than
    /// amplifying (ceo-bora#30).
    #[tokio::test]
    async fn three_colliding_joins_take_ordinals_one_two_three() {
        let _isolated = IsolatedDirs::new("autorename-triple");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (first, _first_rx) = outside_agent_pane(&mut app, "rev");
        let (second, _second_rx) = outside_agent_pane(&mut app, "rev");
        let (third, _third_rx) = outside_agent_pane(&mut app, "rev");
        skip_protocol("eng", &first);
        skip_protocol("eng", &second);
        skip_protocol("eng", &third);

        join(&mut app, "#eng", &first);
        join(&mut app, "#eng", &second);
        assert_eq!(join(&mut app, "#eng", &third)["result"]["source"], "joined");
        assert_eq!(
            member_names(&mut app, "#eng"),
            vec![
                "rev-1".to_string(),
                "rev-2".to_string(),
                "rev-3".to_string()
            ],
            "each successive holder takes the next free ordinal"
        );

        let notice = channels::read_tail("eng", 10)
            .unwrap()
            .into_iter()
            .find(|message| message.from_pane == "system" && message.text.contains("rev-3"))
            .expect("the third colliding join must announce its rename too");
        assert!(
            notice.text.contains(&third),
            "the notice names who joined: {}",
            notice.text
        );

        let sent = app.handle_channel_send(
            "req-third".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: None,
                to: Some("rev-3".into()),
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 1, "@rev-3 reaches exactly one pane");
        assert_eq!(deliveries[0]["pane_id"], serde_json::json!(third));

        let refused = app.handle_channel_send(
            "req-bare".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "ping".into(),
                from_pane: None,
                to: Some("rev".into()),
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let refused: serde_json::Value = serde_json::from_str(&refused).unwrap();
        assert_eq!(
            refused["error"]["code"],
            serde_json::json!("channel_nick_ambiguous"),
            "{refused}"
        );
        assert!(
            refused["error"]["message"]
                .as_str()
                .unwrap()
                .contains("matches 3 channel members"),
            "all three holders are offered as candidates: {refused}"
        );

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// Addressable names of a channel's agent members, in listing order.
    fn member_names(app: &mut App, name: &str) -> Vec<String> {
        let response =
            app.handle_channel_members("req".into(), ChannelMembersParams { name: name.into() });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        response["result"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|member| member["name"].as_str().map(str::to_string))
            .collect()
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
                when_idle: None,
                from_human: false,
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    /// The other half of ceo-bora#30's root cause, and ceo-bora#31's first
    /// decision: the workspace label — what the sidebar shows — is what a
    /// mention addresses. This is the `#metodo-pp` topology exactly: two
    /// panes whose only distinct fact is the label of the workspace they
    /// live in, sharing every lower rung. Before, `@ceo-bora` matched
    /// nobody while the sender line already read `ceo-bora`, so the name a
    /// reader saw was not a name they could use; the only nick that
    /// resolved was the shared kind, and it resolved to both.
    #[tokio::test]
    async fn workspace_label_is_the_addressable_identity_and_the_channel_label_is_not() {
        let _isolated = IsolatedDirs::new("label-identity");
        let mut app = test_app();
        let channel_ws = create_bare_channel_workspace(&mut app, "eng");

        // Two outside agents sharing one lower-rung name ("omp"), told
        // apart only by their workspace labels.
        let mut labelled = Vec::new();
        let mut keep_alive = Vec::new();
        for label in ["ceo-bora", "ceo-pp"] {
            let (pane, rx) = outside_agent_pane(&mut app, "omp");
            let ws_idx = app.state.workspaces.len() - 1;
            app.state.workspaces[ws_idx].set_custom_name(label.to_string());
            assert!(join(&mut app, "eng", &pane)["error"].is_null());
            skip_protocol("eng", &pane);
            labelled.push((label, pane));
            keep_alive.push(rx);
        }

        // `channel.members` lists the labels, and resolution agrees with
        // that listing — the two consumers of the one chain.
        let members =
            app.handle_channel_members("req".into(), ChannelMembersParams { name: "eng".into() });
        let members: serde_json::Value = serde_json::from_str(&members).unwrap();
        let listed: Vec<String> = members["result"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|member| member["name"].as_str().map(str::to_string))
            .collect();
        for (label, _) in &labelled {
            assert!(
                listed.iter().any(|name| name == label),
                "members must list the workspace label: {listed:?}"
            );
        }

        // `@ceo-bora` reaches exactly that pane, and nobody else.
        let (label, target) = &labelled[0];
        let sent = app.handle_channel_send(
            "req".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: format!("@{label} your turn"),
                from_pane: Some("w1A:p9".into()),
                to: None,
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let sent: serde_json::Value = serde_json::from_str(&sent).unwrap();
        assert!(sent["error"].is_null(), "{sent}");
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        assert_eq!(deliveries.len(), 1, "{sent}");
        assert_eq!(deliveries[0]["pane_id"], serde_json::json!(target));

        // The shared kind still refuses rather than reaching both — the
        // amplification guard, now the only thing standing between a
        // collision and a broadcast.
        let shared = app.handle_channel_send(
            "req2".into(),
            ChannelSendParams {
                name: "#eng".into(),
                text: "@omp who is this".into(),
                from_pane: Some("w1A:p8".into()),
                to: None,
                in_reply_to: None,
                when_idle: None,
                from_human: false,
            },
        );
        let shared: serde_json::Value = serde_json::from_str(&shared).unwrap();
        assert_eq!(
            shared["error"]["code"],
            serde_json::json!("channel_nick_unknown"),
            "the label outranks the kind, so the kind matches nobody: {shared}"
        );

        // A pane native to the channel workspace does NOT inherit `#eng` as
        // an identity: the label names the channel, so rung 0 is skipped and
        // the pane keeps its own name.
        assert_eq!(app.workspace_label(channel_ws), None);

        super::super::test_support::shutdown_test_runtimes(&mut app);
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
        let _isolated = IsolatedDirs::new("join-unknown-channel");
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
        let _isolated = IsolatedDirs::new("join-unknown-pane");
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
        let _isolated = IsolatedDirs::new("join-implicit");
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
        let _isolated = IsolatedDirs::new("join-delivery");
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
        assert!(injected.contains("[#eng seq=2 from "), "got {injected}");
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

    /// The injected prefix carries the message's own `seq`, and that value
    /// is a usable cursor.
    ///
    /// The protocol block tells an agent to catch up with `tail --after
    /// <seq>` and to answer a `channel.ask` with `--reply-to <seq>`. Before
    /// the prefix carried it, the seq existed only in the `channel.send`
    /// response handed to the SENDER — a recipient was told to pass a
    /// number it could not obtain. So this test does not merely regex the
    /// prefix: it parses the seq out of the delivered bytes and feeds it to
    /// `channels::read_since`, the exact store call `channel.wait` /
    /// `channel tail --after` runs. A prefix printing a seq that does not
    /// resolve as a cursor would satisfy a regex and still be useless.
    #[tokio::test]
    async fn injected_prefix_carries_a_seq_that_works_as_a_tail_cursor() {
        let _isolated = IsolatedDirs::new("prefix-seq-cursor");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (first, _first_rx) = outside_agent_pane(&mut app, "first");
        let (second, mut second_rx) = outside_agent_pane(&mut app, "second");
        for pane in [&first, &second] {
            let joined = join(&mut app, "#eng", pane);
            assert_eq!(joined["result"]["source"], serde_json::json!("joined"));
        }
        while second_rx.try_recv().is_ok() {}

        let sent = broadcast(&mut app, &first, "first message");
        let reported_seq = sent["result"]["seq"].as_u64().expect("send reports a seq");

        let injected = second_rx
            .try_recv()
            .expect("broadcast must reach the other member");
        let injected = String::from_utf8_lossy(&injected);
        let parsed_seq: u64 = injected
            .split("seq=")
            .nth(1)
            .and_then(|rest| {
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                digits.parse().ok()
            })
            .unwrap_or_else(|| panic!("no parsable seq= in injected prefix: {injected}"));
        assert_eq!(
            parsed_seq, reported_seq,
            "the seq the recipient can read must be the message's own: {injected}"
        );

        // A second message, so the cursor has something to resolve to.
        broadcast(&mut app, &second, "second message");

        let since = channels::read_since("eng", parsed_seq).expect("cursor read");
        let texts: Vec<&str> = since
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec!["second message"],
            "the parsed seq must exclude its own message and yield the next one"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn joined_pane_resolves_by_nick() {
        let _isolated = IsolatedDirs::new("join-nick");
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
                when_idle: None,
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
                when_idle: None,
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
                when_idle: None,
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
        let _isolated = IsolatedDirs::new("join-members");
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

        let list = app.handle_channel_list("req".into(), ChannelListParams { from_pane: None });
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
        let _isolated = IsolatedDirs::new("join-prune");
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
        let _isolated = IsolatedDirs::new("join-scope-roundtrip");
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
        let _isolated = IsolatedDirs::new("join-scope-replace");
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
        let _isolated = IsolatedDirs::new("join-scope-empty");
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
        let _isolated = IsolatedDirs::new("protocol-scope");
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
        let _isolated = IsolatedDirs::new("protocol-join");
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

    /// The briefing an agent actually receives defines the nick namespace
    /// and names the human seat.
    ///
    /// Measured failure this defends: agents were broadcasting because
    /// `channel members --json` showed `name: "omp"` on every row, so
    /// addressing looked impossible — the protocol block recommended `--to
    /// <nick>` without ever saying what a nick may be. Broadcast is the
    /// only fan-out width there is, so ignorance here cost every member's
    /// context on every message. Separately, the human seat worked but was
    /// never mentioned, so no agent could address the human at all.
    ///
    /// Asserted against the delivered bytes, not against `CHANNEL_PROTOCOL`
    /// itself: the human name is interpolated at injection time, so a test
    /// reading the const would not see it, and this must fail if the suffix
    /// stops being appended.
    #[tokio::test]
    async fn protocol_briefing_defines_nick_forms_and_names_the_human() {
        let _isolated = IsolatedDirs::new("protocol-nick-namespace");
        let mut app = test_app();
        app.state.chat_name = "arya".into();
        create_channel(&mut app, "eng");
        let (outsider, mut outsider_rx) = outside_agent_pane(&mut app, "brandos");
        join(&mut app, "#eng", &outsider);

        let injected = outsider_rx
            .try_recv()
            .expect("join must inject the channel protocol block");
        let injected = String::from_utf8_lossy(&injected);

        // Each form asserted by its DEFINING line, not by the bare token: a
        // mutation run showed `contains("w78p1")` passes on an incidental
        // later mention ("address those by w78p1") even with the definition
        // deleted, so the loose version was blind to exactly the regression
        // it was written for.
        for definition in [
            "w78:p1   the `pane_id` as printed",
            "w78p1    that same id with the colon dropped",
            "rev      the `name` field",
        ] {
            assert!(
                injected.contains(definition),
                "briefing must define the nick form: {definition}\ngot: {injected}"
            );
        }
        // The one behaviour that made the namespace worth documenting.
        assert!(
            injected.contains("colon-free pane id always resolves"),
            "briefing must say the colon-free id is the always-works form: {injected}"
        );
        // The human seat, named — not merely alluded to.
        assert!(
            injected.contains("The human on this channel is @arya."),
            "briefing must name the human seat: {injected}"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// A broadcast reaches every agent member except its own sender.
    ///
    /// Three agent panes join `#eng`; `a` sends. Without the filter in
    /// `channel_agent_member_pane_ids`, `a` is delivered its own message
    /// and this test fails on both the length and the membership
    /// assertion — the failure mode measured on `#bun-nix`, where the
    /// sender accumulated unreads of its own sends. The receiver check is
    /// what makes the assertion about delivery rather than bookkeeping: a
    /// pane id absent from `deliveries` but still written to would pass a
    /// length-only test.
    #[tokio::test]
    async fn broadcast_never_delivers_to_its_own_sender() {
        let _isolated = IsolatedDirs::new("broadcast-no-echo");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (sender, mut sender_rx) = outside_agent_pane(&mut app, "sender");
        let (second, _second_rx) = outside_agent_pane(&mut app, "second");
        let (third, _third_rx) = outside_agent_pane(&mut app, "third");
        for pane in [&sender, &second, &third] {
            let joined = join(&mut app, "#eng", pane);
            assert_eq!(joined["result"]["source"], serde_json::json!("joined"));
        }

        // Drain the protocol block `join` injects, so a later `try_recv`
        // can only observe the broadcast itself.
        while sender_rx.try_recv().is_ok() {}

        let sent = broadcast(&mut app, &sender, "hello");
        let deliveries = sent["result"]["deliveries"].as_array().unwrap();
        let reached: Vec<&str> = deliveries
            .iter()
            .map(|delivery| delivery["pane_id"].as_str().unwrap())
            .collect();
        assert_eq!(
            reached.len(),
            2,
            "broadcast reaches the two other members, not the sender: {sent}"
        );
        assert!(
            !reached.contains(&sender.as_str()),
            "sender must not be in its own fan-out: {reached:?}"
        );
        assert!(reached.contains(&second.as_str()), "{reached:?}");
        assert!(reached.contains(&third.as_str()), "{reached:?}");

        assert!(
            sender_rx.try_recv().is_err(),
            "nothing may be written to the sender's own pane"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_protocol_survives_restart_without_resend() {
        let _isolated = IsolatedDirs::new("protocol-restart");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        app.send_channel_protocol("eng", 0, "w1A:p2", None);
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
        restarted.send_channel_protocol("eng", 0, "w1A:p2", None);
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
        restarted.send_channel_protocol("eng", 0, "w1A:p3", None);
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
        let _isolated = IsolatedDirs::new("protocol-deferred");
        let mut app = test_app();
        let (_reviewer, worker, _rx) = channel_with_two_agents(&mut app, "reviewer", "worker");

        app.send_channel_protocol("eng", 0, &worker, Some(true));

        assert!(
            app.pending_agent_prompts.contains_key(&worker),
            "a Working target must have the protocol block queued, not dropped"
        );
        assert_eq!(app.pending_agent_prompts[&worker].len(), 1);
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn channel_protocol_send_never_records_a_rate_limit_entry() {
        let _isolated = IsolatedDirs::new("protocol-rate-limit");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let (outsider, mut rx) = outside_agent_pane(&mut app, "brandos");

        app.send_channel_protocol("eng", 0, &outsider, None);
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
        let _isolated = IsolatedDirs::new("burst-default");
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
                    when_idle: None,
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
        let _isolated = IsolatedDirs::new("burst-force-bell");
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
                    when_idle: None,
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
                when_idle: None,
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
        let _isolated = IsolatedDirs::new("note-no-burst");
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
        let _isolated = IsolatedDirs::new("note-burst");
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
                    when_idle: None,
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
        let _isolated = IsolatedDirs::new("ask-unknown-nick");
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
        let _isolated = IsolatedDirs::new("ask-single-target");
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
        let _isolated = IsolatedDirs::new("reply-to-future-seq");
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
                when_idle: None,
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
                when_idle: None,
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
                when_idle: None,
                from_human: true,
            },
        );
        let accepted: serde_json::Value = serde_json::from_str(&accepted).unwrap();
        assert!(accepted["result"].is_object(), "{accepted}");

        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    // --- Read-cursor / unread tests (CANAL-NAO-LIDO) ---

    fn member_unread(app: &mut App, name: &str, pane_id: &str) -> u64 {
        let response =
            app.handle_channel_members("req".into(), ChannelMembersParams { name: name.into() });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        response["result"]["members"]
            .as_array()
            .unwrap()
            .iter()
            .find(|member| member["pane_id"] == serde_json::json!(pane_id))
            .unwrap_or_else(|| panic!("{pane_id} must be a listed member: {response}"))["unread"]
            .as_u64()
            .unwrap()
    }

    #[tokio::test]
    async fn fresh_member_sees_every_message_as_unread() {
        let _isolated = IsolatedDirs::new("unread-fresh");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let public_id = app.public_pane_id(ws_idx, pane_id).unwrap();

        app.handle_channel_note(
            "req1".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "first".into(),
                from_pane: None,
            },
        );
        app.handle_channel_note(
            "req2".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "second".into(),
                from_pane: None,
            },
        );

        assert_eq!(
            member_unread(&mut app, "eng", &public_id),
            2,
            "a member who has never read must see every message as unread"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    /// `channel tail`'s cursor advance ultimately lands through the same
    /// shared persistence primitive (`channels::advance_channel_cursor`)
    /// this test exercises directly, after confirming the pane is a member
    /// through the same `channel.members` listing `wait_for_channel_message`
    /// (`src/api/wait.rs`) dispatches to over the App's request channel —
    /// the wire handler runs on the connection thread, off the `App` this
    /// test module drives directly, so the identity check is reproduced
    /// here rather than round-tripped through a socket.
    #[tokio::test]
    async fn channel_tail_read_marks_member_caught_up() {
        let _isolated = IsolatedDirs::new("unread-tail-read");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let public_id = app.public_pane_id(ws_idx, pane_id).unwrap();

        app.handle_channel_note(
            "req1".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "first".into(),
                from_pane: None,
            },
        );
        assert_eq!(member_unread(&mut app, "eng", &public_id), 1);

        let members =
            app.handle_channel_members("req".into(), ChannelMembersParams { name: "eng".into() });
        let members: serde_json::Value = serde_json::from_str(&members).unwrap();
        assert!(
            members["result"]["members"]
                .as_array()
                .unwrap()
                .iter()
                .any(|member| member["pane_id"] == serde_json::json!(public_id)),
            "pane must resolve as a member before its cursor advances"
        );
        let last_seq = channels::read_tail("eng", 1).unwrap().last().unwrap().seq;
        channels::advance_channel_cursor("eng", &public_id, last_seq).unwrap();

        assert_eq!(
            member_unread(&mut app, "eng", &public_id),
            0,
            "reading via channel tail must catch the member up to the last seq"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn note_after_tail_read_makes_unread_exactly_one() {
        let _isolated = IsolatedDirs::new("unread-note-after-read");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let public_id = app.public_pane_id(ws_idx, pane_id).unwrap();

        app.handle_channel_note(
            "req1".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "first".into(),
                from_pane: None,
            },
        );
        let last_seq = channels::read_tail("eng", 1).unwrap().last().unwrap().seq;
        channels::advance_channel_cursor("eng", &public_id, last_seq).unwrap();
        assert_eq!(member_unread(&mut app, "eng", &public_id), 0);

        // A zero-injection `channel.note` still registers as mail — the
        // actual point of the unread primitive.
        app.handle_channel_note(
            "req2".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "second".into(),
                from_pane: None,
            },
        );
        assert_eq!(member_unread(&mut app, "eng", &public_id), 1);
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn non_member_history_read_does_not_change_any_members_unread() {
        let _isolated = IsolatedDirs::new("unread-non-member");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let public_id = app.public_pane_id(ws_idx, pane_id).unwrap();

        app.handle_channel_note(
            "req1".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "first".into(),
                from_pane: None,
            },
        );
        assert_eq!(member_unread(&mut app, "eng", &public_id), 1);

        // A caller with no pane identity — a human shell without
        // `HERDR_PANE_ID` — reads the same history but is nobody's cursor.
        let history = app.handle_channel_history(
            "req2".into(),
            ChannelHistoryParams {
                name: "eng".into(),
                lines: None,
                from_pane: None,
            },
        );
        let history: serde_json::Value = serde_json::from_str(&history).unwrap();
        assert_eq!(history["result"]["messages"].as_array().unwrap().len(), 1);

        assert_eq!(
            member_unread(&mut app, "eng", &public_id),
            1,
            "a non-member (or identity-less) read must not touch any member's cursor"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn corrupt_cursor_file_yields_full_unread_not_error() {
        let _isolated = IsolatedDirs::new("unread-corrupt-cursor");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let public_id = app.public_pane_id(ws_idx, pane_id).unwrap();

        app.handle_channel_note(
            "req1".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "first".into(),
                from_pane: None,
            },
        );

        let cursor_path = channels::channel_cursors_file_path("eng");
        std::fs::create_dir_all(cursor_path.parent().unwrap()).unwrap();
        std::fs::write(&cursor_path, b"not json").unwrap();

        assert_eq!(
            member_unread(&mut app, "eng", &public_id),
            1,
            "a corrupt cursor file must read as no cursor (full unread), never an error"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn list_reports_callers_own_unread_and_clears_after_reading() {
        let _isolated = IsolatedDirs::new("list-caller-unread");
        let mut app = test_app();
        create_channel(&mut app, "eng");
        let ws_idx = app.state.workspaces.len() - 1;
        let pane_id = app.state.workspaces[ws_idx].tabs[0].root_pane;
        let public_id = app.public_pane_id(ws_idx, pane_id).unwrap();

        app.handle_channel_note(
            "req1".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "first".into(),
                from_pane: None,
            },
        );

        let unread = |app: &mut App, pane: &str| -> u64 {
            let list = app.handle_channel_list(
                "req".into(),
                ChannelListParams {
                    from_pane: Some(pane.to_string()),
                },
            );
            let list: serde_json::Value = serde_json::from_str(&list).unwrap();
            list["result"]["channels"]
                .as_array()
                .unwrap()
                .iter()
                .find(|channel| channel["name"] == serde_json::json!("#eng"))
                .expect("eng must be listed")["unread"]
                .as_u64()
                .unwrap()
        };

        assert_eq!(
            unread(&mut app, &public_id),
            1,
            "a member calling channel.list from its own pane sees its own mailbox"
        );

        // Reading via channel.history from the same pane advances that
        // member's stored cursor, same as channel tail.
        app.handle_channel_history(
            "req2".into(),
            ChannelHistoryParams {
                name: "eng".into(),
                lines: None,
                from_pane: Some(public_id.clone()),
            },
        );
        assert_eq!(unread(&mut app, &public_id), 0);

        app.handle_channel_note(
            "req3".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "second".into(),
                from_pane: None,
            },
        );
        assert_eq!(
            unread(&mut app, &public_id),
            1,
            "unread rises by exactly one after a later note"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }

    #[tokio::test]
    async fn list_reports_zero_unread_for_caller_with_no_pane_identity() {
        let _isolated = IsolatedDirs::new("list-no-pane-identity");
        let mut app = test_app();
        create_channel(&mut app, "eng");

        app.handle_channel_note(
            "req1".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "first".into(),
                from_pane: None,
            },
        );
        app.handle_channel_note(
            "req2".into(),
            ChannelNoteParams {
                name: "eng".into(),
                text: "second".into(),
                from_pane: None,
            },
        );

        let list = app.handle_channel_list("req3".into(), ChannelListParams { from_pane: None });
        let list: serde_json::Value = serde_json::from_str(&list).unwrap();
        let eng = list["result"]["channels"]
            .as_array()
            .unwrap()
            .iter()
            .find(|channel| channel["name"] == serde_json::json!("#eng"))
            .expect("eng must be listed");
        assert_eq!(eng["last_message_seq"], serde_json::json!(2));
        assert_eq!(
            eng["unread"],
            serde_json::json!(0),
            "a caller with no pane identity must never see the room's message count as unread"
        );
        super::super::test_support::shutdown_test_runtimes(&mut app);
    }
}
