use std::time::{Duration, Instant};

use bytes::Bytes;

use super::{terminal_targets::TerminalTargetError, App};
use crate::api::schema::AgentStartParams;

const DEFAULT_AGENT_START_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_AGENT_START_TIMEOUT: Duration = Duration::from_secs(300);
pub(crate) const AGENT_START_SETTLE_DELAY: Duration = Duration::from_secs(3);
const INVALID_AGENT_TIMEOUT_MESSAGE: &str =
    "agent start timeout must be greater than 3000ms and at most 300000ms";
const INVALID_AGENT_NAME_MESSAGE: &str = "agent name must start with a lowercase letter and contain only lowercase letters, digits, '-' or '_' (1-32 characters)";

fn valid_agent_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && name.len() <= 32
        && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '-' | '_'))
}

impl App {
    pub(super) fn collect_agent_infos(&self) -> Vec<crate::api::schema::AgentInfo> {
        self.state
            .workspaces
            .iter()
            .enumerate()
            .flat_map(|(ws_idx, ws)| {
                ws.tabs.iter().flat_map(move |tab| {
                    tab.layout
                        .pane_ids()
                        .into_iter()
                        .filter_map(move |pane_id| self.agent_info(ws_idx, pane_id))
                })
            })
            .collect()
    }

    pub(super) fn reconcile_managed_agent_target(&mut self, target: &str) {
        let Ok(resolved) = self.resolve_agent_target(target) else {
            return;
        };
        let Some(terminal_id) = self
            .state
            .workspaces
            .get(resolved.ws_idx)
            .and_then(|workspace| workspace.terminal_id(resolved.pane_id))
            .cloned()
        else {
            return;
        };
        let changed = self
            .state
            .terminals
            .get_mut(&terminal_id)
            .is_some_and(|terminal| terminal.reconcile_managed_agent_at(Instant::now(), false));
        if changed {
            self.state.mark_session_dirty();
            self.schedule_session_save();
            self.emit_pane_updated(resolved.ws_idx, resolved.pane_id);
        }
    }

    pub(super) fn agent_info_for_target(
        &self,
        target: &str,
    ) -> Result<crate::api::schema::AgentInfo, TerminalTargetError> {
        let resolved = self.resolve_agent_target(target)?;
        self.agent_info(resolved.ws_idx, resolved.pane_id)
            .ok_or_else(|| TerminalTargetError::NotFound {
                target: target.to_string(),
            })
    }

    pub(super) fn focus_agent_target(
        &mut self,
        target: &str,
    ) -> Result<crate::api::schema::AgentInfo, TerminalTargetError> {
        let resolved = self.resolve_agent_target(target)?;
        self.state
            .focus_pane_in_workspace(resolved.ws_idx, resolved.pane_id);
        self.state.mark_active_tab_seen();
        self.state.settle_terminal_mode_after_focus();
        self.agent_info(resolved.ws_idx, resolved.pane_id)
            .ok_or_else(|| TerminalTargetError::NotFound {
                target: target.to_string(),
            })
    }

    pub(super) fn rename_agent_target(
        &mut self,
        target: &str,
        name: Option<String>,
    ) -> Result<crate::api::schema::AgentInfo, AgentRenameError> {
        let resolved = self
            .resolve_agent_target(target)
            .map_err(AgentRenameError::Target)?;
        let normalized_name = match name {
            Some(name) if valid_agent_name(&name) => Some(name),
            Some(_) => return Err(AgentRenameError::InvalidName),
            None => None,
        };

        if let Some(name) = normalized_name.as_deref() {
            let conflicts = self.agent_name_conflicts(name, &resolved.terminal_id);
            if !conflicts.is_empty() {
                return Err(AgentRenameError::DuplicateName {
                    name: name.to_string(),
                    candidates: conflicts,
                });
            }
        }

        let Some(terminal) = self
            .state
            .terminals
            .values_mut()
            .find(|terminal| terminal.id.to_string() == resolved.terminal_id)
        else {
            return Err(AgentRenameError::Target(TerminalTargetError::NotFound {
                target: target.to_string(),
            }));
        };
        if terminal.managed_agent_launch_pending() {
            return Err(AgentRenameError::PendingLaunch);
        }
        if terminal.effective_agent_label().is_none() {
            return Err(AgentRenameError::NotAgent);
        }
        match normalized_name {
            Some(name) => terminal.set_agent_name(name),
            None => terminal.clear_agent_name(),
        }
        self.state.mark_session_dirty();
        self.schedule_session_save();
        self.emit_pane_updated(resolved.ws_idx, resolved.pane_id);
        self.agent_info(resolved.ws_idx, resolved.pane_id)
            .ok_or_else(|| {
                AgentRenameError::Target(TerminalTargetError::NotFound {
                    target: target.to_string(),
                })
            })
    }

    pub(super) fn start_agent(
        &mut self,
        params: AgentStartParams,
    ) -> Result<(crate::api::schema::AgentInfo, Vec<String>), AgentStartError> {
        let name = params.name;
        if !valid_agent_name(&name) {
            return Err(AgentStartError::InvalidName);
        }
        let Some(kind) = crate::detect::parse_agent_label(&params.kind) else {
            return Err(AgentStartError::UnsupportedKind(params.kind));
        };
        if params
            .args
            .iter()
            .any(|arg| arg.chars().any(char::is_control))
        {
            return Err(AgentStartError::InvalidArgument);
        }
        let conflicts = self.agent_name_conflicts(&name, "");
        if !conflicts.is_empty() {
            return Err(AgentStartError::DuplicateName {
                name,
                candidates: conflicts,
            });
        }
        let Some((ws_idx, pane_id)) = self.parse_current_public_pane_id(&params.pane_id) else {
            return Err(AgentStartError::TargetNotFound(params.pane_id));
        };
        let terminal_id = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|workspace| workspace.terminal_id(pane_id))
            .cloned()
            .ok_or_else(|| AgentStartError::TargetNotFound(params.pane_id.clone()))?;
        let terminal = self
            .state
            .terminals
            .get(&terminal_id)
            .ok_or_else(|| AgentStartError::TargetNotFound(params.pane_id.clone()))?;
        if terminal.is_agent_terminal() || terminal.managed_agent_kind().is_some() {
            return Err(AgentStartError::TargetBusy(params.pane_id));
        }
        let runtime = self
            .terminal_runtimes
            .get(&terminal_id)
            .ok_or_else(|| AgentStartError::TargetUnavailable(params.pane_id.clone()))?;
        let shell_name = available_shell_name(runtime)
            .ok_or_else(|| AgentStartError::TargetBusy(params.pane_id.clone()))?;

        let mut argv = vec![self.state.agent_commands.command_for(kind).to_string()];
        argv.extend(params.args);
        let command = crate::platform::interactive_shell_command(&argv, &shell_name)
            .ok_or(AgentStartError::InvalidArgument)?;
        let bytes = crate::app::api_helpers::encode_api_submission(runtime, &command);
        let timeout = Duration::from_millis(
            params
                .timeout_ms
                .unwrap_or(DEFAULT_AGENT_START_TIMEOUT.as_millis() as u64),
        );
        if timeout <= AGENT_START_SETTLE_DELAY || timeout > MAX_AGENT_START_TIMEOUT {
            return Err(AgentStartError::InvalidTimeout);
        }

        let now = Instant::now();
        let terminal = self
            .state
            .terminals
            .get_mut(&terminal_id)
            .ok_or_else(|| AgentStartError::TargetUnavailable(params.pane_id.clone()))?;
        terminal.begin_managed_agent(name, kind, now, AGENT_START_SETTLE_DELAY, timeout);
        if let Err(err) = runtime.try_send_bytes(Bytes::from(bytes)) {
            terminal.clear_agent_name();
            return Err(AgentStartError::InputFailed(err.to_string()));
        }
        self.state.mark_session_dirty();
        self.schedule_session_save();

        let agent = self
            .agent_info(ws_idx, pane_id)
            .ok_or(AgentStartError::TargetUnavailable(params.pane_id))?;
        self.auto_join_project_channel(ws_idx, pane_id, &agent.pane_id);
        Ok((agent, argv))
    }

    pub(super) fn agent_start_error_body(
        &self,
        err: AgentStartError,
    ) -> crate::api::schema::ErrorBody {
        match err {
            AgentStartError::InvalidName => crate::api::schema::ErrorBody {
                code: "invalid_agent_name".into(),
                message: INVALID_AGENT_NAME_MESSAGE.into(),
            },
            AgentStartError::UnsupportedKind(kind) => crate::api::schema::ErrorBody {
                code: "unsupported_agent_kind".into(),
                message: format!("unsupported interactive agent kind {kind}"),
            },
            AgentStartError::InvalidArgument => crate::api::schema::ErrorBody {
                code: "invalid_agent_argument".into(),
                message: "agent arguments cannot be encoded safely for the target shell".into(),
            },
            AgentStartError::InvalidTimeout => crate::api::schema::ErrorBody {
                code: "invalid_agent_timeout".into(),
                message: INVALID_AGENT_TIMEOUT_MESSAGE.into(),
            },
            AgentStartError::TargetNotFound(target) => crate::api::schema::ErrorBody {
                code: "agent_pane_not_found".into(),
                message: format!("agent target pane {target} not found"),
            },
            AgentStartError::TargetBusy(target) => crate::api::schema::ErrorBody {
                code: "agent_pane_busy".into(),
                message: format!("agent target pane {target} is not an available shell"),
            },
            AgentStartError::TargetUnavailable(target) => crate::api::schema::ErrorBody {
                code: "agent_pane_unavailable".into(),
                message: format!("agent target pane {target} has no live terminal"),
            },
            AgentStartError::InputFailed(message) => crate::api::schema::ErrorBody {
                code: "agent_start_input_failed".into(),
                message,
            },
            AgentStartError::DuplicateName { name, candidates } => crate::api::schema::ErrorBody {
                code: "agent_name_taken".into(),
                message: format!(
                    "agent name {name} is already used; candidates: {}",
                    candidates
                        .into_iter()
                        .map(|candidate| format!(
                            "terminal_id={} pane_id={} workspace_id={} tab_id={} cwd={} status={:?}",
                            candidate.terminal_id,
                            candidate.pane_id,
                            candidate.workspace_id,
                            candidate.tab_id,
                            candidate.cwd.unwrap_or_else(|| "unknown".into()),
                            candidate.agent_status,
                        ))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            },
        }
    }

    pub(super) fn agent_target_error_body(
        &self,
        err: TerminalTargetError,
    ) -> crate::api::schema::ErrorBody {
        match err {
            TerminalTargetError::NotFound { target } => crate::api::schema::ErrorBody {
                code: "agent_not_found".into(),
                message: format!("agent target {target} not found"),
            },
            TerminalTargetError::Ambiguous { target, candidates } => {
                crate::api::schema::ErrorBody {
                    code: "agent_target_ambiguous".into(),
                    message: format!(
                        "agent target {target} is ambiguous; candidates: {}",
                        candidates
                            .into_iter()
                            .map(|candidate| format!(
                                "terminal_id={} pane_id={} workspace_id={} tab_id={} cwd={} status={:?}",
                                candidate.terminal_id,
                                candidate.pane_id,
                                candidate.workspace_id,
                                candidate.tab_id,
                                candidate.cwd.unwrap_or_else(|| "unknown".into()),
                                candidate.agent_status,
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ),
                }
            }
        }
    }

    pub(super) fn agent_rename_error_body(
        &self,
        err: AgentRenameError,
    ) -> crate::api::schema::ErrorBody {
        match err {
            AgentRenameError::Target(err) => self.agent_target_error_body(err),
            AgentRenameError::InvalidName => crate::api::schema::ErrorBody {
                code: "invalid_agent_name".into(),
                message: INVALID_AGENT_NAME_MESSAGE.into(),
            },
            AgentRenameError::NotAgent => crate::api::schema::ErrorBody {
                code: "agent_not_found".into(),
                message: "agent target does not currently host an agent".into(),
            },
            AgentRenameError::PendingLaunch => crate::api::schema::ErrorBody {
                code: "agent_launch_pending".into(),
                message: "agent name cannot change while startup is pending".into(),
            },
            AgentRenameError::DuplicateName { name, candidates } => crate::api::schema::ErrorBody {
                code: "agent_name_taken".into(),
                message: format!(
                    "agent name {name} is already used; candidates: {}",
                    candidates
                        .into_iter()
                        .map(|candidate| format!(
                            "terminal_id={} pane_id={} workspace_id={} tab_id={} cwd={} status={:?}",
                            candidate.terminal_id,
                            candidate.pane_id,
                            candidate.workspace_id,
                            candidate.tab_id,
                            candidate.cwd.unwrap_or_else(|| "unknown".into()),
                            candidate.agent_status,
                        ))
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            },
        }
    }

    pub(super) fn agent_info(
        &self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
    ) -> Option<crate::api::schema::AgentInfo> {
        let ws = self.state.workspaces.get(ws_idx)?;
        let pane_state = ws.pane_state(pane_id)?;
        let terminal = self.state.terminals.get(&pane_state.attached_terminal_id)?;
        if !terminal.is_agent_terminal() {
            return None;
        }
        let pane = self.pane_info(ws_idx, pane_id)?;
        Some(crate::api::schema::AgentInfo {
            terminal_id: pane.terminal_id,
            name: terminal.agent_name.clone(),
            agent: pane.agent,
            title: pane.title,
            terminal_title: pane.terminal_title,
            terminal_title_stripped: pane.terminal_title_stripped,
            display_agent: pane.display_agent,
            agent_status: pane.agent_status,
            screen_detection_skipped: terminal.full_lifecycle_hook_authority_active(),
            state_labels: pane.state_labels,
            tokens: pane.tokens,
            agent_session: pane.agent_session,
            workspace_id: pane.workspace_id,
            tab_id: pane.tab_id,
            pane_id: pane.pane_id,
            focused: pane.focused,
            launch_pending: terminal.managed_agent_launch_pending(),
            interactive_ready: terminal.managed_agent_interactive_ready(),
            state_change_seq: terminal.last_agent_state_change_seq.unwrap_or(0),
            cwd: pane.cwd,
            foreground_cwd: pane.foreground_cwd,
            revision: pane.revision,
        })
    }

    fn agent_name_conflicts(
        &self,
        name: &str,
        except_terminal_id: &str,
    ) -> Vec<crate::api::schema::AgentInfo> {
        self.collect_agent_infos()
            .into_iter()
            .filter(|agent| {
                agent.name.as_deref() == Some(name) && agent.terminal_id != except_terminal_id
            })
            .collect()
    }

    /// bora-1le.1 "project = channel binding + auto-join": an agent
    /// started in a pane whose workspace's cwd lives inside a project
    /// member directory auto-joins that project's channel, going through
    /// the existing `channel.join` verb (`Method::ChannelJoin`) via
    /// `handle_api_request` — `app::api::channels` is a sibling module
    /// this one has no direct (`pub(super)`) access into, and routing
    /// through the public request surface means membership, the
    /// joined-pane roster, and the one-time protocol briefing all reuse
    /// `channel.join`'s own idempotency (`channels::read_protocol_sent`'s
    /// `(pane, version)` record) instead of reimplementing it: a second
    /// `agent.start` resolving to the same project, or a pane that
    /// already joined by hand, can never get double-joined or
    /// double-briefed.
    ///
    /// Matches by RESOLVED identity (`persist::projects::resolve_member`),
    /// never by string-comparing paths, so a `~`-relative member `dir:` or
    /// a worktree checkout still matches the pane's real cwd. Silently a
    /// no-op — never fails the agent start — when: the pane's cwd does not
    /// resolve to a git checkout, no project claims that directory, the
    /// owning project opted out via `auto_join: false`, or the project's
    /// channel workspace does not exist yet (`channel.join` itself reports
    /// `channel_not_found`, which this ignores).
    fn auto_join_project_channel(
        &mut self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        public_pane_id: &str,
    ) {
        let Some(cwd) = self.launch_cwd_for_pane_in_workspace(ws_idx, pane_id) else {
            return;
        };
        let crate::persist::projects::MemberResolution::Resolved(agent_dir) =
            crate::persist::projects::resolve_member(&cwd.to_string_lossy())
        else {
            return;
        };
        let Ok(file) = crate::persist::projects::load_projects_file_fresh() else {
            return;
        };
        let Some((slug, project)) = file.projects.iter().find(|(_, project)| {
            project.auto_join
                && project
                    .members
                    .iter()
                    .any(|member| member_covers(member, &agent_dir))
        }) else {
            return;
        };
        let channel = project.effective_channel(slug);
        let dbg_resp = self.handle_api_request(crate::api::schema::Request {
            id: "internal:project-auto-join".into(),
            method: crate::api::schema::Method::ChannelJoin(
                crate::api::schema::ChannelJoinParams {
                    name: channel,
                    pane: public_pane_id.to_string(),
                    scope_write: None,
                    scope_read: None,
                },
            ),
        });
        eprintln!("DBG join={dbg_resp}");
    }
}

/// Whether `member`'s resolved directory is `agent`'s checkout, or an
/// ancestor of it — same repo identity, same checkout (worktree), and
/// `agent`'s subdir sits at or beneath `member`'s. This is the "dir ->
/// project" mapping the bead requires: resolved identity, never a raw
/// path prefix compare, so `~/Sites/cnb_hono` as a member still matches
/// an agent started in `~/Sites/cnb_hono/packages/landing`.
fn member_covers(
    member: &crate::persist::projects::Member,
    agent: &crate::persist::projects::ResolvedMember,
) -> bool {
    let crate::persist::projects::MemberResolution::Resolved(candidate) = member.resolve() else {
        return false;
    };
    candidate.repo_identity == agent.repo_identity
        && candidate.checkout_key == agent.checkout_key
        && agent.subdir.starts_with(&candidate.subdir)
}

fn available_shell_name(runtime: &crate::terminal::TerminalRuntime) -> Option<String> {
    #[cfg(test)]
    if runtime.child_pid().is_none() {
        return Some("sh".into());
    }
    crate::platform::available_pane_shell(runtime.child_pid()?)
}

pub(super) fn runtime_hosts_agent(
    runtime: &crate::terminal::TerminalRuntime,
    expected: crate::detect::Agent,
) -> bool {
    #[cfg(test)]
    if runtime.child_pid().is_none() {
        return true;
    }
    live_runtime_agent(runtime) == Some(expected)
}

fn live_runtime_agent(runtime: &crate::terminal::TerminalRuntime) -> Option<crate::detect::Agent> {
    let job = crate::detect::foreground_job(runtime.child_pid()?)?;
    crate::detect::identify_agent_in_job(&job)
        .map(|(agent, _)| agent)
        .or_else(|| {
            job.processes
                .iter()
                .find_map(|process| crate::platform::process_agent_hint(process.pid))
        })
}

pub(super) enum AgentStartError {
    InvalidName,
    UnsupportedKind(String),
    InvalidArgument,
    InvalidTimeout,
    TargetNotFound(String),
    TargetBusy(String),
    TargetUnavailable(String),
    InputFailed(String),
    DuplicateName {
        name: String,
        candidates: Vec<crate::api::schema::AgentInfo>,
    },
}

pub(super) enum AgentRenameError {
    Target(TerminalTargetError),
    InvalidName,
    NotAgent,
    PendingLaunch,
    DuplicateName {
        name: String,
        candidates: Vec<crate::api::schema::AgentInfo>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{
        AgentStartParams, Method, ProjectCreateParams, ProjectMemberAddParams, Request,
    };
    use crate::config::{Config, IsolatedDirs};
    use crate::persist::projects::WorktreesScope;
    use crate::workspace::Workspace;

    #[test]
    fn agent_names_use_a_small_cli_safe_grammar() {
        for name in ["a", "reviewer-one", "reviewer_2", &"a".repeat(32)] {
            assert!(valid_agent_name(name), "expected {name:?} to be valid");
        }
        for name in [
            "",
            " reviewer",
            "reviewer ",
            "reviewer one",
            "Reviewer",
            "1reviewer",
            "reviewer.one",
            &"a".repeat(33),
        ] {
            assert!(!valid_agent_name(name), "expected {name:?} to be invalid");
        }
    }

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.default_shell = crate::app::api::test_support::long_running_test_command().into();
        // `project.create` binds a channel, which spawns a real channel
        // workspace. A login shell sources the developer's whole profile and
        // never exits, so workspace creation fails, `ensure_project_channel`
        // swallows it by design, and the auto-join then reports
        // `channel_not_found` — a failure that looks like the feature is
        // broken when it is only the fixture.
        app.state.shell_mode = crate::config::ShellModeConfig::NonLogin;
        app
    }

    /// A temp directory with a minimal `.git/HEAD`, enough for
    /// `persist::projects::resolve_member` to treat it as a real checkout
    /// (same fixture style `app::api::projects`'s own tests use).
    fn temp_repo(tag: &str) -> std::path::PathBuf {
        let repo =
            std::env::temp_dir().join(format!("bora-agents-project-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        repo
    }

    /// A synthetic (no real PTY) workspace whose sole pane reports `cwd`
    /// as its terminal's working directory — the field
    /// `launch_cwd_for_pane_in_workspace` falls back to when the runtime
    /// has no live process to query — plus a fresh, unoccupied test
    /// runtime so `agent.start` can succeed against it. Returns the
    /// workspace index and its root pane's public id.
    /// Returns the pty receiver alongside the ids: it MUST outlive the test.
    /// Dropping it closes the runtime's input channel, and `agent.start`
    /// writes into that channel, so a fixture that lets the receiver die at
    /// the end of this function fails with `agent_start_input_failed:
    /// channel closed` rather than doing anything the test is about.
    #[must_use]
    fn member_workspace(
        app: &mut App,
        name: &str,
        cwd: &std::path::Path,
    ) -> (usize, String, tokio::sync::mpsc::Receiver<bytes::Bytes>) {
        let workspace = Workspace::test_new(name);
        let root = workspace.tabs[0].root_pane;
        app.state.workspaces.push(workspace);
        let ws_idx = app.state.workspaces.len() - 1;
        app.state.ensure_test_terminals();
        let terminal_id = app.state.workspaces[ws_idx].tabs[0].panes[&root]
            .attached_terminal_id
            .clone();
        app.state.terminals.get_mut(&terminal_id).unwrap().cwd = cwd.to_path_buf();
        let (runtime, rx) = crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 4);
        app.terminal_runtimes.insert(terminal_id, runtime);
        app.state.active = Some(ws_idx);
        app.state.selected = ws_idx;
        let pane_id = app.public_pane_id(ws_idx, root).unwrap();
        (ws_idx, pane_id, rx)
    }

    /// Creates the project plus one member directory, and guarantees the
    /// project's `#slug` channel workspace exists for the auto-join to find.
    ///
    /// `project.create` does bind and spawn that channel workspace for real —
    /// `app::api::projects`'s own tests cover exactly that — but its two panes
    /// run a test shell with no live process behind them, so the workspace is
    /// reaped inside the very request that created it and is already gone by
    /// the time anything else looks. The auto-join then fails with
    /// `channel_not_found`, which reads like the feature is broken when the
    /// fixture is what is missing. These tests are about auto-join, not about
    /// channel binding, so the channel workspace is re-created synthetically
    /// here — same shape the rest of this module uses, no PTY to reap.
    fn create_project_with_member(
        app: &mut App,
        slug: &str,
        repo: &std::path::Path,
        auto_join: Option<bool>,
    ) {
        app.handle_api_request(Request {
            id: "req".into(),
            method: Method::ProjectCreate(ProjectCreateParams {
                slug: slug.into(),
                name: None,
                channel: None,
                auto_join,
            }),
        });
        app.handle_api_request(Request {
            id: "req".into(),
            method: Method::ProjectMemberAdd(ProjectMemberAddParams {
                slug: slug.into(),
                dir: repo.display().to_string(),
                worktrees: WorktreesScope::All,
            }),
        });
        let channel_name = format!("#{slug}");
        if !app
            .state
            .workspaces
            .iter()
            .any(|ws| ws.custom_name.as_deref() == Some(channel_name.as_str()))
        {
            let mut workspace = Workspace::test_new(slug);
            workspace.set_custom_name(channel_name);
            app.state.workspaces.push(workspace);
            app.state.ensure_test_terminals();
        }
    }

    fn start_agent_ok(app: &mut App, pane_id: &str, name: &str) -> serde_json::Value {
        let response = app.handle_api_request(Request {
            id: "req".into(),
            method: Method::AgentStart(AgentStartParams {
                name: name.into(),
                kind: "pi".into(),
                pane_id: pane_id.into(),
                args: Vec::new(),
                timeout_ms: Some(4_000),
            }),
        });
        serde_json::from_str(&response).unwrap()
    }

    #[tokio::test]
    async fn agent_start_in_project_member_workspace_auto_joins_channel() {
        let _isolated = IsolatedDirs::new("agents-auto-join");
        let mut app = test_app();
        let repo = temp_repo("auto-join");
        create_project_with_member(&mut app, "proj", &repo, None);

        let (_ws_idx, pane_id, _pty_rx) = member_workspace(&mut app, "member", &repo);
        let started = start_agent_ok(&mut app, &pane_id, "worker");
        assert_eq!(
            started["result"]["type"], "agent_started",
            "agent.start must succeed for this test to prove anything, got: {started}"
        );

        let members = crate::persist::channels::read_joined_members("proj", |_| true);
        assert!(
            members.contains(&pane_id),
            "an agent started in a project member workspace must auto-join the \
             project's channel, got members: {members:?}"
        );

        std::fs::remove_dir_all(&repo).ok();
    }

    #[tokio::test]
    async fn auto_join_does_not_double_join_or_double_brief_on_repeat() {
        let _isolated = IsolatedDirs::new("agents-auto-join-idempotent");
        let mut app = test_app();
        let repo = temp_repo("auto-join-idempotent");
        create_project_with_member(&mut app, "proj", &repo, None);

        let (ws_idx, pane_id, _pty_rx) = member_workspace(&mut app, "member", &repo);
        let root = app.state.workspaces[ws_idx].tabs[0].root_pane;
        start_agent_ok(&mut app, &pane_id, "worker");

        let members_after_first = crate::persist::channels::read_joined_members("proj", |_| true);
        assert_eq!(
            members_after_first.len(),
            1,
            "first auto-join must record exactly one roster entry"
        );
        let transcript_after_first = crate::persist::channels::read_tail("proj", 100)
            .unwrap()
            .len();

        // Whatever would trigger a second auto-join attempt for the same
        // already-joined pane (a config reload re-resolving the same
        // project, a caller retrying) must be a no-op — exercised
        // directly against the method under test.
        app.auto_join_project_channel(ws_idx, root, &pane_id);

        let members_after_second = crate::persist::channels::read_joined_members("proj", |_| true);
        assert_eq!(
            members_after_second, members_after_first,
            "a repeat auto-join must not add a duplicate roster entry"
        );
        let transcript_after_second = crate::persist::channels::read_tail("proj", 100)
            .unwrap()
            .len();
        assert_eq!(
            transcript_after_second, transcript_after_first,
            "a repeat auto-join must not send a second protocol briefing"
        );

        std::fs::remove_dir_all(&repo).ok();
    }

    #[tokio::test]
    async fn auto_join_respects_project_opt_out() {
        let _isolated = IsolatedDirs::new("agents-auto-join-opt-out");
        let mut app = test_app();
        let repo = temp_repo("auto-join-opt-out");
        create_project_with_member(&mut app, "proj", &repo, Some(false));

        let (_ws_idx, pane_id, _pty_rx) = member_workspace(&mut app, "member", &repo);
        let started = start_agent_ok(&mut app, &pane_id, "worker");
        assert_eq!(started["result"]["type"], "agent_started", "got: {started}");

        let members = crate::persist::channels::read_joined_members("proj", |_| true);
        assert!(
            members.is_empty(),
            "auto_join: false must keep the agent out of the channel roster, got: {members:?}"
        );

        std::fs::remove_dir_all(&repo).ok();
    }
}
