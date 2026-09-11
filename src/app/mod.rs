//! Application orchestration.
//!
//! - `state.rs` — AppState, Mode, and pure data structs
//! - `actions.rs` — state mutations (testable without PTYs/async)

pub(crate) mod actions;
mod agent_resume;
pub(crate) mod agent_view;
mod agents;
pub(crate) use agents::{AGENT_START_SETTLE_DELAY, MAX_AGENT_START_TIMEOUT};
mod api;
#[cfg(test)]
pub(crate) use api::test_support::exiting_test_command;
mod api_helpers;
pub(crate) use api_helpers::limit_snapshot_lines;
mod creation;
mod custom_commands;
mod git_refresh;
mod ids;
pub(crate) mod pane_graphics;
mod popup;
mod runtime;
mod session;
pub mod state;
mod tab_bar_status;
mod terminal_targets;
mod terminal_titles;
mod theme_sync;
mod window_title;
mod worktrees;

use std::collections::{HashMap, HashSet};
#[cfg(unix)]
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(16);
const GIT_REMOTE_STATUS_REFRESH_INTERVAL: Duration = Duration::from_millis(1500);
const GIT_REPO_DISCOVERY_REFRESH_INTERVAL: Duration = Duration::from_secs(5 * 60);
const AUTO_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
const PENDING_AGENT_RESUME_THEME_WAIT: Duration = Duration::from_millis(750);
const SESSION_SAVE_DEBOUNCE: Duration = Duration::from_secs(5);

/// Minimum spacing between two `agent.prompt` calls with the same
/// (from_pane, target_pane) pair, to break accidental agent-to-agent
/// prompt loops. Prompts without a `from_pane` (human/orchestrator path)
/// are exempt.
const AGENT_PROMPT_RATE_LIMIT: Duration = Duration::from_secs(2);
/// Entries older than this are pruned opportunistically on insert so the
/// rate-limit map cannot grow unbounded across a long session.
const AGENT_PROMPT_RATE_LIMIT_PRUNE_AGE: Duration = Duration::from_secs(60);
/// Per-target cap on `when_idle` prompts deferred by the in-process fast path
/// (`App::handle_agent_prompt`) while the target is `Working`. Past the cap,
/// the oldest queued prompt is dropped so a spammy sender cannot grow the
/// queue unbounded; the newest instruction is kept as the more useful one.
pub(crate) const PENDING_AGENT_PROMPT_CAP: usize = 32;
/// How long a target must be continuously observed away from `Working`
/// before a queued `when_idle` chat prompt (`PendingAgentPrompt`) is
/// replayed into it — see `App::sync_pending_agent_prompt_drain_deadline`.
pub(crate) const PENDING_AGENT_PROMPT_DRAIN_SETTLE: Duration = Duration::from_millis(1200);

/// A `when_idle` agent.prompt whose in-process fast path found the target
/// `Working` and deferred it instead of dropping it. Replayed through
/// `handle_agent_prompt` once the target is next observed to leave
/// `Working` — see `App::drain_pending_agent_prompts`. Every terminal fate
/// (delivered, or dropped by cap eviction / pane close / replay failure) is
/// reported via an `agent_prompt.delivered` / `agent_prompt.dropped` event
/// carrying `queue_id` — see `App::report_queued_prompt_dropped` — so the
/// `deferred` receipt never overpromises: the sender can always learn what
/// ultimately happened to a queued prompt.
pub(crate) struct PendingAgentPrompt {
    /// Stable for the lifetime of this entry in the queue; echoed on the
    /// deferred receipt (`AgentPrompted.queue_id`) and on its terminal-fate
    /// event so a caller can correlate the two.
    pub(crate) queue_id: u64,
    pub(crate) params: crate::api::schema::AgentPromptParams,
    pub(crate) enqueued_at: Instant,
}

fn rfc3339_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Renders a `QueuedAgentPromptDropReason` (plus optional detail, e.g. an
/// error code/message from a failed replay) as the human-readable text used in
/// both the sender's PTY notice and the channel history system line.
fn queued_prompt_drop_reason_text(
    reason: crate::api::schema::QueuedAgentPromptDropReason,
    detail: Option<&str>,
) -> String {
    use crate::api::schema::QueuedAgentPromptDropReason as Reason;
    let base = match reason {
        Reason::Capacity => "queue full",
        Reason::PaneClosed => "target pane closed",
        Reason::AgentChanged => "target unavailable",
    };
    match detail {
        Some(detail) if !detail.is_empty() => format!("{base} ({detail})"),
        _ => base.to_string(),
    }
}

use ratatui::layout::Rect;
use tokio::sync::{mpsc, Notify};
use tracing::info;

use crate::config::Config;
use crate::events::AppEvent;

pub use state::{AppState, Mode, ToastKind, ViewState};

pub(crate) fn load_plugin_manifest(
    path: &str,
    enabled: bool,
) -> Result<crate::api::schema::InstalledPluginInfo, (&'static str, String)> {
    api::plugins::load_plugin_manifest(path, enabled)
}

/// Full application: AppState + runtime concerns (event channels, async I/O).
#[derive(Debug, Clone)]
pub(crate) struct OverlayPaneState {
    ws_idx: usize,
    tab_idx: usize,
    previous_focus: crate::layout::PaneId,
    previous_zoomed: bool,
    temp_files: Vec<std::path::PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AppPolicy {
    pub(crate) restore_session: bool,
    pub(crate) persist_session: bool,
    pub(crate) persist_plugin_registry: bool,
    pub(crate) background_updates: bool,
}

impl AppPolicy {
    pub(crate) const PRODUCTION: Self = Self {
        restore_session: true,
        persist_session: true,
        persist_plugin_registry: true,
        background_updates: true,
    };

    #[cfg(test)]
    pub(crate) const TEST: Self = Self {
        restore_session: false,
        persist_session: false,
        persist_plugin_registry: false,
        background_updates: false,
    };

    #[cfg(unix)]
    pub(crate) const HANDOFF_REPLACEMENT: Self = Self {
        restore_session: false,
        persist_session: true,
        persist_plugin_registry: true,
        background_updates: true,
    };
}

pub struct App {
    pub state: AppState,
    pub(crate) pane_graphics: pane_graphics::Runtime,
    pub(crate) pane_graphics_files: Arc<crate::pane_graphics_files::FileStore>,
    pub(crate) direct_graphics_available: bool,
    pub(crate) pixel_mouse_available: bool,
    pub(crate) terminal_runtimes: crate::terminal::TerminalRuntimeRegistry,
    pub event_tx: mpsc::Sender<AppEvent>,
    pub(crate) event_rx: mpsc::Receiver<AppEvent>,
    pub(crate) api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
    pub(crate) event_hub: crate::api::EventHub,
    pub(crate) last_focus: Option<(usize, crate::layout::PaneId)>,
    pub(crate) policy: AppPolicy,
    pub(crate) config_diagnostic_deadline: Option<Instant>,
    pub(crate) toast_deadline: Option<Instant>,
    pub(crate) last_api_notification_at: Option<Instant>,
    pub(crate) last_git_remote_status_refresh: Instant,
    pub(crate) last_git_repo_discovery_refresh: Instant,
    pub(crate) git_refresh_in_flight: bool,
    pub(crate) git_refresh_due_after_in_flight: bool,
    pub(crate) git_identity_refresh_requested: bool,
    pub(crate) git_status_cache: HashMap<std::path::PathBuf, crate::workspace::GitStatusCacheEntry>,
    pub(crate) pending_api_worktree_creates: HashMap<std::path::PathBuf, u64>,
    pub(crate) pending_api_worktree_removes: HashMap<String, u64>,
    pub(crate) pending_api_worktree_remove_paths: HashMap<std::path::PathBuf, u64>,
    pub(crate) pending_worktree_remove_runtime_exits: HashMap<crate::layout::PaneId, usize>,
    pub(crate) pending_worktree_remove_runtime_restores: HashMap<crate::layout::PaneId, u64>,
    pub(crate) next_api_worktree_operation_id: u64,
    pub(crate) next_auto_update_check: Option<Instant>,
    pub(crate) next_agent_manifest_update_check: Option<Instant>,
    pub(crate) update_version_check_enabled: bool,
    pub(crate) update_manifest_check_enabled: bool,
    pub(crate) loaded_host_cursor: crate::config::HostCursorModeConfig,
    pub(crate) agent_metadata_deadline: Option<Instant>,
    pub(crate) pending_agent_resume_deadline: Option<Instant>,
    pub(crate) session_save_deadline: Option<Instant>,
    pub(crate) session_save_thread: Option<std::thread::JoinHandle<()>>,
    pane_exit_checkpoint_pending: bool,
    pub(crate) detached_process_children: Vec<std::process::Child>,
    tab_bar_status_generation: u64,
    tab_bar_datetimes: Vec<tab_bar_status::TabBarDatetimeRuntime>,
    tab_bar_commands: Vec<tab_bar_status::TabBarCommandRuntime>,
    next_tab_bar_datetime_refresh: Option<Instant>,
    /// Parsed `ui.window_title` plus the hostname resolved when it was applied.
    window_title_template: Option<(crate::config::WindowTitleTemplate, String)>,
    pub(crate) persist_pane_history: bool,
    /// Last render-loop attempt, including a throttled hidden-only PTY skip.
    pub(crate) last_render_at: Option<Instant>,
    /// Last attempt that could update a connected presentation surface.
    pub(crate) last_presentation_at: Option<Instant>,
    pub render_notify: Arc<Notify>,
    pub(crate) render_dirty: Arc<crate::render_signal::RenderSignal>,
    pub(crate) full_redraw_pending: bool,
    pub(crate) overlay_panes: HashMap<crate::layout::PaneId, OverlayPaneState>,
    pub(crate) config_reloaded_from_disk: bool,
    client_shell_keybindings_profile: Option<String>,
    pub(crate) channels_in_burst: HashSet<String>,
    pub(crate) agent_prompt_rate_limits: HashMap<(String, String), Instant>,
    /// Per-channel send timestamps used to detect a burst (`ui.channel_burst_messages` within
    /// `ui.channel_burst_window_secs`) — see `App::record_channel_burst_send` in
    /// `app::api::channels`. Pruned to the burst window on every send.
    pub(crate) pending_agent_prompts:
        HashMap<String, std::collections::VecDeque<PendingAgentPrompt>>,
    pub(crate) pending_agent_prompt_drain_deadlines: HashMap<String, Instant>,
    /// Per-channel send timestamps used to detect a burst (`ui.channel_burst_messages` within
    /// `ui.channel_burst_window_secs`) — see `App::record_channel_burst_send` in
    /// `app::api::channels`. Pruned to the burst window on every send.
    pub(crate) channel_burst_history: HashMap<String, std::collections::VecDeque<Instant>>,
    pub(crate) next_pending_agent_prompt_queue_id: u64,
    endpoint_commands: custom_commands::EndpointCommandRegistry,
}

pub(crate) const APP_EVENT_CHANNEL_CAPACITY: usize = 256;
pub(crate) const APP_EVENT_DRAIN_LIMIT: usize = 64;

fn auto_updates_enabled(background_updates: bool) -> bool {
    background_updates && !cfg!(debug_assertions)
}

fn background_update_check_enabled(background_updates: bool, check_enabled: bool) -> bool {
    auto_updates_enabled(background_updates) && check_enabled
}

fn load_plugin_registry(
    persist_plugin_registry: bool,
) -> crate::app::state::InstalledPluginRegistry {
    if !persist_plugin_registry {
        return std::collections::HashMap::new();
    }
    let entries = crate::persist::plugin_registry::load();
    let entries = crate::persist::plugin_registry::reload_manifests(entries, |path, enabled| {
        crate::app::api::plugins::load_plugin_manifest(path, enabled).map_err(|(_, msg)| msg)
    });
    entries
        .into_iter()
        .map(|plugin| (plugin.plugin_id.clone(), plugin))
        .collect()
}

fn agent_panel_sort_from_config(
    sort: crate::config::AgentPanelSortConfig,
) -> state::AgentPanelSort {
    match sort {
        crate::config::AgentPanelSortConfig::Spaces => state::AgentPanelSort::Spaces,
        crate::config::AgentPanelSortConfig::Priority => state::AgentPanelSort::Priority,
    }
}

/// Parse the configured agent name list into a deduplicated set of `Agent`
/// values. Unknown agent names are silently dropped so a typo cannot disable
/// other valid entries.
fn parse_cjk_ime_agents(names: &[String]) -> Vec<crate::detect::Agent> {
    let mut out = Vec::with_capacity(names.len());
    for name in names {
        if let Some(agent) = crate::detect::parse_agent_label(name) {
            if !out.contains(&agent) {
                out.push(agent);
            }
        }
    }
    out
}

fn normalize_theme_name(name: &str) -> String {
    name.to_lowercase().replace([' ', '_'], "-")
}

fn sibling_theme_names(name: &str) -> (String, String) {
    match normalize_theme_name(name).as_str() {
        "catppuccin" | "catppuccin-mocha" | "catppuccin-latte" | "latte" | "light" => {
            ("catppuccin".to_string(), "catppuccin-latte".to_string())
        }
        "tokyo-night" | "tokyonight" | "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => {
            ("tokyo-night".to_string(), "tokyo-night-day".to_string())
        }
        "gruvbox" | "gruvbox-dark" | "gruvbox-light" => {
            ("gruvbox".to_string(), "gruvbox-light".to_string())
        }
        "one-dark" | "onedark" | "one-light" | "onelight" => {
            ("one-dark".to_string(), "one-light".to_string())
        }
        "solarized" | "solarized-dark" | "solarized-light" => {
            ("solarized".to_string(), "solarized-light".to_string())
        }
        "kanagawa" | "kanagawa-lotus" | "lotus" => {
            ("kanagawa".to_string(), "kanagawa-lotus".to_string())
        }
        "rose-pine" | "rosepine" | "rose-pine-dawn" | "rosepine-dawn" | "dawn" => {
            ("rose-pine".to_string(), "rose-pine-dawn".to_string())
        }
        _ => (name.to_string(), name.to_string()),
    }
}

fn theme_runtime_config(
    config: &crate::config::Config,
    use_legacy_ui_accent: bool,
) -> state::ThemeRuntimeConfig {
    let manual_name = config
        .theme
        .name
        .clone()
        .unwrap_or_else(|| "catppuccin".to_string());
    let (default_dark, default_light) = sibling_theme_names(&manual_name);
    state::ThemeRuntimeConfig {
        manual_name,
        dark_name: config.theme.dark_name.clone().unwrap_or(default_dark),
        light_name: config.theme.light_name.clone().unwrap_or(default_light),
        auto_switch: config.theme.auto_switch,
        custom: config.theme.custom.clone(),
        legacy_accent: (use_legacy_ui_accent
            && config.ui.accent != "cyan"
            && config
                .theme
                .custom
                .as_ref()
                .and_then(|c| c.accent.as_ref())
                .is_none())
        .then(|| config.ui.accent.clone()),
    }
}

fn resolve_palette_for_theme_name(
    name: &str,
    fallback_name: &str,
    runtime: &state::ThemeRuntimeConfig,
    mode_custom: Option<&crate::config::ModeThemeColors>,
) -> state::Palette {
    let mut palette = state::Palette::from_name(name).unwrap_or_else(|| {
        tracing::warn!(
            theme = name,
            fallback = fallback_name,
            "unknown theme, falling back"
        );
        state::Palette::from_name(fallback_name).unwrap_or_else(state::Palette::catppuccin)
    });

    if let Some(custom) = &runtime.custom {
        palette = palette.with_overrides(custom);
    }
    if let Some(accent) = &runtime.legacy_accent {
        palette.accent = crate::config::parse_color(accent);
    }
    if let Some(custom) = mode_custom {
        palette = palette.with_mode_overrides(custom);
    }

    palette
}

fn resolve_effective_theme(
    runtime: &state::ThemeRuntimeConfig,
    appearance: Option<crate::terminal_theme::HostAppearance>,
) -> (state::Palette, String) {
    let (name, fallback, mode_custom) = if runtime.auto_switch {
        match appearance.unwrap_or(crate::terminal_theme::HostAppearance::Dark) {
            crate::terminal_theme::HostAppearance::Dark => (
                &runtime.dark_name,
                "catppuccin",
                runtime
                    .custom
                    .as_ref()
                    .and_then(|custom| custom.dark.as_ref()),
            ),
            crate::terminal_theme::HostAppearance::Light => (
                &runtime.light_name,
                "catppuccin-latte",
                runtime
                    .custom
                    .as_ref()
                    .and_then(|custom| custom.light.as_ref()),
            ),
        }
    } else {
        (&runtime.manual_name, "catppuccin", None)
    };
    (
        resolve_palette_for_theme_name(name, fallback, runtime, mode_custom),
        name.clone(),
    )
}

pub(crate) fn client_theme_runtime_from_config(config: &Config) -> state::ThemeRuntimeConfig {
    theme_runtime_config(config, true)
}

pub(crate) fn client_palette_for_theme(
    runtime: &state::ThemeRuntimeConfig,
    name: &str,
) -> state::Palette {
    resolve_palette_for_theme_name(name, "catppuccin", runtime, None)
}

pub(crate) fn client_palette_from_config(config: &Config) -> state::Palette {
    let runtime = client_theme_runtime_from_config(config);
    resolve_effective_theme(&runtime, None).0
}

pub(crate) fn client_palette_for_appearance(
    runtime: &state::ThemeRuntimeConfig,
    appearance: crate::terminal_theme::HostAppearance,
) -> state::Palette {
    resolve_effective_theme(runtime, Some(appearance)).0
}

impl App {
    pub fn new(
        config: &Config,
        policy: AppPolicy,
        config_diagnostic: Option<String>,
        api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
        event_hub: crate::api::EventHub,
    ) -> Self {
        let (prefix_code, prefix_mods) = config.prefix_key();
        crate::kitty_graphics::set_enabled(config.kitty_graphics_enabled());
        let (event_tx, event_rx) = mpsc::channel::<AppEvent>(APP_EVENT_CHANNEL_CAPACITY);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(crate::render_signal::RenderSignal::new());

        // Try to restore previous session
        let mut restored_terminals = std::collections::HashMap::new();
        let mut restored_terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let (workspaces, active, selected) = if !policy.restore_session {
            (Vec::new(), None, 0)
        } else if let Some(snap) = crate::persist::load() {
            let history = config
                .experimental
                .pane_history
                .then(crate::persist::load_history)
                .flatten();
            let (ws, terminals, terminal_runtimes) = crate::persist::restore(
                &snap,
                history.as_ref(),
                24,
                80,
                config.advanced.scrollback_limit_bytes,
                &config.terminal.default_shell,
                config.terminal.shell_mode,
                config.session.resume_agents_on_restore,
                event_tx.clone(),
                render_notify.clone(),
                render_dirty.clone(),
            );
            restored_terminals = terminals;
            restored_terminal_runtimes = terminal_runtimes.into();
            if ws.is_empty() {
                crate::logging::session_restored(0, "empty");
                (Vec::new(), None, 0)
            } else {
                crate::logging::session_restored(ws.len(), "ok");
                let active = snap.active.filter(|&i| i < ws.len());
                let selected = snap.selected.min(ws.len().saturating_sub(1));
                (ws, active, selected)
            }
        } else {
            (Vec::new(), None, 0)
        };

        let agent_panel_sort = agent_panel_sort_from_config(config.ui.agent_panel_sort);

        let worktree_directory =
            crate::worktree::expand_tilde_absolute_path(&config.worktrees.directory);

        info!(
            pane_scrollback_limit_bytes = config.advanced.scrollback_limit_bytes,
            "using pane scrollback configuration"
        );

        let latest_release_notes = crate::release_notes::load_latest();
        let update_available = latest_release_notes
            .as_ref()
            .filter(|notes| notes.preview)
            .map(|notes| notes.version.clone());
        let latest_release_notes_available = latest_release_notes.is_some();
        let update_install_command = crate::update::update_install_command().to_string();
        let startup_product_announcement =
            crate::product_announcements::load_unseen_for_current_version();

        let mode = if active.is_some() {
            state::Mode::Terminal
        } else {
            state::Mode::Navigate
        };

        #[cfg(not(test))]
        let agent_manifest_summaries = crate::detect::manifest::reload_manifests();
        // Nextest runs each unit test in a fresh process. Manifest-sensitive tests reload
        // explicitly; unrelated App tests should not recompile every bundled regex.
        #[cfg(test)]
        let agent_manifest_summaries = Vec::new();
        let theme_runtime = theme_runtime_config(config, true);
        let (theme_palette, theme_name) = resolve_effective_theme(&theme_runtime, None);

        let mut state = AppState {
            terminals: std::collections::HashMap::new(),
            direct_attach_resize_locks: std::collections::HashSet::new(),
            pane_id_aliases: std::collections::HashMap::new(),
            public_pane_id_aliases: std::collections::HashMap::new(),
            workspaces,
            active,
            previous_pane_focus: None,
            selected,
            mode,
            should_quit: false,
            request_client_config_reload: false,
            worktree_directory,
            latest_release_notes,
            product_announcement: startup_product_announcement.map(|announcement| {
                state::ProductAnnouncementState {
                    version: announcement.version,
                    id: announcement.id,
                    title: announcement.title,
                    body: announcement.body,
                    scroll: 0,
                    preview: announcement.preview,
                }
            }),
            view: state::ViewState {
                terminal_area: Rect::default(),
                pane_infos: Vec::new(),
            },
            update_available,
            update_install_command,
            latest_release_notes_available,
            update_dismissed: false,
            config_diagnostic,
            toast: None,
            pending_agent_notifications: std::collections::HashMap::new(),
            outer_terminal_focus: None,
            prefix_code,
            prefix_mods,
            headless_size: config.headless_size(),
            agent_panel_sort,
            agent_view_override: None,
            sidebar_agents: config.ui.sidebar.agents.clone(),
            sidebar_spaces: config.ui.sidebar.spaces.clone(),
            next_agent_state_change_seq: 0,
            pane_borders: config.ui.pane_borders,
            pane_outer_borders: config.ui.pane_outer_borders,
            pane_scrollbars: config.ui.pane_scrollbars,
            pane_gaps: config.ui.pane_gaps,
            show_agent_labels_on_pane_borders: config.ui.show_agent_labels_on_pane_borders,
            show_pane_ids_on_pane_borders: config.ui.show_pane_ids_on_pane_borders,
            tab_bar_right: Vec::new(),
            tab_bar_right_separator: String::new(),
            reveal_hidden_cursor_for_cjk_ime: config.experimental.reveal_hidden_cursor_for_cjk_ime,
            cjk_ime_agent_filter_configured: !config.experimental.cjk_ime_agents.is_empty(),
            cjk_ime_agents: parse_cjk_ime_agents(&config.experimental.cjk_ime_agents),
            cjk_ime_cursor_shape: config.experimental.cjk_ime_cursor_shape.to_decscusr(),
            kitty_graphics_enabled: config.kitty_graphics_enabled(),
            default_shell: config.terminal.default_shell.clone(),
            shell_mode: config.terminal.shell_mode,
            new_terminal_cwd: config.terminal.new_cwd.clone(),
            pane_scrollback_limit_bytes: config.advanced.scrollback_limit_bytes,
            sound: config.ui.sound.clone(),
            toast_config: config.ui.toast.clone(),
            keybinds: config.keybinds(),
            palette: theme_palette,
            theme_name,
            theme_runtime,
            host_terminal_appearance: None,
            host_terminal_appearance_explicit: false,
            integration_recommendations: crate::integration::integration_recommendations(),
            agent_manifest_summaries,
            agent_manifest_update_status: crate::detect::manifest_update::load_status(),
            installed_plugins: load_plugin_registry(policy.persist_plugin_registry),
            plugin_panes: std::collections::HashMap::new(),
            popup_pane: None,
            plugin_command_logs: Vec::new(),
            next_plugin_command_log_id: 1,
            plugin_commands_in_flight: 0,
            host_terminal_theme: crate::terminal_theme::TerminalTheme::default(),
            host_cell_size: crate::kitty_graphics::HostCellSize::default(),
            session_dirty: false,
            terminal_runtime_shutdowns: Vec::new(),
            agent_commands: config.agents.clone(),
            chat_name: config.ui.effective_chat_name(),
            channel_burst_messages: config.ui.channel_burst_messages,
            channel_burst_window: Duration::from_secs(config.ui.channel_burst_window_secs),
            sidebar_width: config.ui.sidebar_width,
            sidebar_section_split: 0.7,
            collapsed_space_keys: std::collections::HashSet::new(),
            view_mode: config.ui.view_mode,
        };

        state.terminals = restored_terminals;

        for ws_idx in 0..state.workspaces.len() {
            let cwd = state.workspaces[ws_idx]
                .resolved_identity_cwd_from(&state.terminals, &restored_terminal_runtimes);
            state.workspaces[ws_idx].cached_git_branch =
                cwd.as_deref().and_then(crate::workspace::git_branch);
        }

        // Background auto-update is disabled for non-persistent test apps
        // and in debug/test builds so local development never mutates the
        // running binary out from under spawned test processes.
        let version_check_enabled =
            background_update_check_enabled(policy.background_updates, config.update.version_check);
        let manifest_check_enabled = background_update_check_enabled(
            policy.background_updates,
            config.update.manifest_check,
        );
        if version_check_enabled {
            let update_tx = event_tx.clone();
            std::thread::spawn(move || crate::update::auto_update(update_tx));
        }
        if manifest_check_enabled {
            let manifest_update_tx = event_tx.clone();
            std::thread::spawn(move || {
                crate::detect::manifest_update::auto_update(manifest_update_tx)
            });
        }

        let last_focus = state.active.and_then(|idx| {
            state
                .workspaces
                .get(idx)
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
        let client_shell_keybindings_profile = config.local_keybindings_profile_toml().ok();
        let endpoint_commands =
            custom_commands::EndpointCommandRegistry::new(&state.keybinds.custom_commands);

        let mut app = Self {
            config_diagnostic_deadline: None,
            toast_deadline: None,
            last_api_notification_at: None,
            state,
            pane_graphics: pane_graphics::Runtime::default(),
            pane_graphics_files: Arc::new(crate::pane_graphics_files::FileStore::default()),
            direct_graphics_available: false,
            pixel_mouse_available: false,
            terminal_runtimes: restored_terminal_runtimes,
            event_tx,
            event_rx,
            last_git_remote_status_refresh: Instant::now() - GIT_REMOTE_STATUS_REFRESH_INTERVAL,
            last_git_repo_discovery_refresh: Instant::now(),
            git_refresh_in_flight: false,
            git_refresh_due_after_in_flight: false,
            git_identity_refresh_requested: false,
            git_status_cache: HashMap::new(),
            pending_api_worktree_creates: HashMap::new(),
            pending_api_worktree_removes: HashMap::new(),
            pending_api_worktree_remove_paths: HashMap::new(),
            pending_worktree_remove_runtime_exits: HashMap::new(),
            pending_worktree_remove_runtime_restores: HashMap::new(),
            next_api_worktree_operation_id: 1,
            next_auto_update_check: version_check_enabled
                .then_some(Instant::now() + AUTO_UPDATE_CHECK_INTERVAL),
            next_agent_manifest_update_check: manifest_check_enabled
                .then_some(Instant::now() + AUTO_UPDATE_CHECK_INTERVAL),
            update_version_check_enabled: config.update.version_check,
            update_manifest_check_enabled: config.update.manifest_check,
            loaded_host_cursor: config.ui.host_cursor,
            agent_metadata_deadline: None,
            pending_agent_resume_deadline: None,
            session_save_deadline: None,
            session_save_thread: None,
            pane_exit_checkpoint_pending: false,
            detached_process_children: Vec::new(),
            tab_bar_status_generation: 0,
            tab_bar_datetimes: Vec::new(),
            tab_bar_commands: Vec::new(),
            next_tab_bar_datetime_refresh: None,
            window_title_template: None,
            persist_pane_history: config.experimental.pane_history,
            last_render_at: None,
            last_presentation_at: None,
            api_rx,
            event_hub,
            last_focus,
            policy,
            render_notify,
            render_dirty,
            full_redraw_pending: false,
            overlay_panes: HashMap::new(),
            config_reloaded_from_disk: false,
            agent_prompt_rate_limits: HashMap::new(),
            pending_agent_prompts: HashMap::new(),
            next_pending_agent_prompt_queue_id: 1,
            pending_agent_prompt_drain_deadlines: HashMap::new(),
            channels_in_burst: HashSet::new(),
            client_shell_keybindings_profile,
            endpoint_commands,
            channel_burst_history: HashMap::new(),
        };
        app.configure_tab_bar_status(&config.ui.tab_bar_right, &config.ui.tab_bar_right_separator);
        app.configure_window_title(&config.ui.window_title);
        app.load_pending_agent_prompts();
        app
    }

    #[cfg(unix)]
    pub fn new_from_handoff(
        config: &Config,
        config_diagnostic: Option<String>,
        api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
        event_hub: crate::api::EventHub,
        snapshot: &crate::persist::SessionSnapshot,
        imports: &mut std::collections::HashMap<
            u32,
            crate::handoff_runtime::ImportedHandoffRuntime,
        >,
    ) -> io::Result<Self> {
        let mut app = Self::new(
            config,
            AppPolicy::HANDOFF_REPLACEMENT,
            config_diagnostic,
            api_rx,
            event_hub,
        );
        let (workspaces, terminals, runtimes) = crate::persist::restore_handoff(
            snapshot,
            config.advanced.scrollback_limit_bytes,
            &config.terminal.default_shell,
            config.terminal.shell_mode,
            imports,
            app.event_tx.clone(),
            app.render_notify.clone(),
            app.render_dirty.clone(),
        )?;
        let pane_id_aliases = crate::persist::handoff_pane_aliases(snapshot, &workspaces);

        app.state.pane_id_aliases = pane_id_aliases;
        app.state.workspaces = workspaces;
        app.state.terminals = terminals;
        app.terminal_runtimes = runtimes.into();
        app.state.active = snapshot
            .active
            .filter(|&idx| idx < app.state.workspaces.len());
        app.state.selected = snapshot
            .selected
            .min(app.state.workspaces.len().saturating_sub(1));
        app.state.mode = if app.state.active.is_some() {
            state::Mode::Terminal
        } else {
            state::Mode::Navigate
        };
        app.last_focus = app.state.active.and_then(|idx| {
            app.state
                .workspaces
                .get(idx)
                .and_then(|ws| ws.focused_pane_id().map(|pane_id| (idx, pane_id)))
        });
        Ok(app)
    }

    #[cfg(unix)]
    pub fn unpause_handoff_readers(&self) {
        self.terminal_runtimes.set_handoff_readers_paused(false);
    }

    #[cfg(unix)]
    pub fn assume_handoff_ownership(&mut self) {
        self.terminal_runtimes.assume_handoff_ownership();
    }

    pub(crate) fn ensure_default_workspace(&mut self) -> bool {
        if !self.state.workspaces.is_empty() {
            return false;
        }

        let cwd = self.resolve_new_terminal_cwd(None);
        let preserve_checkpoint = self.pane_exit_checkpoint_pending && !self.state.session_dirty;

        match self.create_workspace_with_options(cwd, true) {
            Ok(_) => {
                if preserve_checkpoint {
                    // Automatic replacement is part of pane removal, not a new user mutation.
                    self.pane_exit_checkpoint_pending = true;
                    self.finish_checkpointed_pane_exit();
                }
                true
            }
            Err(err) => {
                tracing::error!(err = %err, "failed to create default workspace");
                self.state.mode = Mode::Navigate;
                false
            }
        }
    }

    fn mark_release_notes_seen(&mut self, preview: bool) {
        if !preview {
            if let Err(err) = crate::release_notes::mark_current_version_seen() {
                self.state.config_diagnostic =
                    Some(format!("failed to update release notes status: {err}"));
                self.config_diagnostic_deadline = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }

    pub(crate) fn dismiss_product_announcement(&mut self) {
        if let Some(announcement) = self.state.product_announcement.take() {
            if !announcement.preview {
                if let Err(err) =
                    crate::product_announcements::mark_seen(&announcement.version, &announcement.id)
                {
                    self.state.config_diagnostic =
                        Some(format!("failed to update announcement status: {err}"));
                    self.config_diagnostic_deadline = Some(Instant::now() + Duration::from_secs(5));
                }
            }
        }
    }

    pub(crate) fn reload_config(&mut self) -> crate::config::ConfigReloadReport {
        self.apply_config_from_disk(true)
    }

    pub(crate) fn take_config_reloaded_from_disk(&mut self) -> bool {
        let reloaded = self.config_reloaded_from_disk;
        self.config_reloaded_from_disk = false;
        reloaded
    }

    pub(crate) fn apply_config_from_disk(
        &mut self,
        notify_success: bool,
    ) -> crate::config::ConfigReloadReport {
        self.config_reloaded_from_disk = true;
        let previous_toast = self.state.toast.clone();
        let report = match crate::config::load_live_config() {
            Ok(loaded) => self.apply_live_config(
                &loaded.config,
                &loaded.diagnostics,
                &loaded.invalid_sections,
                notify_success,
            ),
            Err(diagnostics) => {
                self.state.toast = None;
                self.state.config_diagnostic =
                    crate::config::config_diagnostic_summary(&diagnostics);
                self.config_diagnostic_deadline = None;
                crate::config::ConfigReloadReport {
                    status: crate::config::ConfigReloadStatus::Failed,
                    diagnostics,
                }
            }
        };
        self.endpoint_commands =
            custom_commands::EndpointCommandRegistry::new(&self.state.keybinds.custom_commands);
        self.sync_toast_deadline(previous_toast);
        report
    }

    fn apply_live_config(
        &mut self,
        config: &crate::config::Config,
        load_diagnostics: &[String],
        invalid_sections: &[String],
        notify_success: bool,
    ) -> crate::config::ConfigReloadReport {
        let mut diagnostics = load_diagnostics.to_vec();
        let invalid_section =
            |section: &str| invalid_sections.iter().any(|invalid| invalid == section);

        if !invalid_section("keys") {
            match config.live_keybinds_with_diagnostics() {
                Ok((live, keybind_diagnostics)) => {
                    self.state.prefix_code = live.prefix.0;
                    self.state.prefix_mods = live.prefix.1;
                    self.state.keybinds = live.keybinds;
                    match config.local_keybindings_profile_toml() {
                        Ok(profile) => self.client_shell_keybindings_profile = Some(profile),
                        Err(err) => diagnostics.push(format!(
                            "failed to publish server keybindings: {err}; kept previous keybindings"
                        )),
                    }
                    diagnostics.extend(keybind_diagnostics);
                }
                Err(keybind_diagnostics) => {
                    diagnostics.extend(
                        keybind_diagnostics
                            .into_iter()
                            .map(|diagnostic| format!("{diagnostic}; kept current keybinds")),
                    );
                }
            }
        }

        if !invalid_section("ui") {
            // Validate sidebar bounds before they reach any `u16::clamp` call.
            // On `min > max`, treat the entire `[ui]` section as invalid: keep
            // the previous settings and skip the section so the re-clamp below
            // — and every subsequent render/drag — can never panic.
            if let Some(diagnostic) = config.invalid_sidebar_bounds_diagnostic() {
                diagnostics.push(format!("{diagnostic}; keeping previous [ui] settings"));
            } else {
                diagnostics.extend(config.ui.sound.diagnostics());
                diagnostics.extend(crate::config::tab_bar_right_diagnostics(
                    &config.ui.tab_bar_right,
                ));
                diagnostics.extend(crate::config::window_title_diagnostics(
                    &config.ui.window_title,
                ));

                self.loaded_host_cursor = config.ui.host_cursor;
                self.state.pane_borders = config.ui.pane_borders;
                self.state.pane_outer_borders = config.ui.pane_outer_borders;
                self.state.pane_scrollbars = config.ui.pane_scrollbars;
                self.state.pane_gaps = config.ui.pane_gaps;
                self.state.show_agent_labels_on_pane_borders =
                    config.ui.show_agent_labels_on_pane_borders;
                self.configure_tab_bar_status(
                    &config.ui.tab_bar_right,
                    &config.ui.tab_bar_right_separator,
                );
                self.configure_window_title(&config.ui.window_title);
                self.state.agent_panel_sort =
                    agent_panel_sort_from_config(config.ui.agent_panel_sort);
                self.state.sidebar_agents = config.ui.sidebar.agents.clone();
                self.state.sidebar_spaces = config.ui.sidebar.spaces.clone();
                self.state.sound = config.ui.sound.clone();
                self.state.toast_config = config.ui.toast.clone();
                self.state.view_mode = config.ui.view_mode;
                self.state.chat_name = config.ui.effective_chat_name();
                self.state.channel_burst_messages = config.ui.channel_burst_messages;
                self.state.channel_burst_window =
                    Duration::from_secs(config.ui.channel_burst_window_secs);
            }
        }

        let graphics_config_valid = !invalid_section("terminal")
            && (config.terminal.kitty_graphics.is_some() || !invalid_section("experimental"));
        if graphics_config_valid
            && config.kitty_graphics_enabled() != self.state.kitty_graphics_enabled
        {
            diagnostics.push(
                "terminal.kitty_graphics changes require restarting Herdr; kept current setting"
                    .into(),
            );
        }

        if !invalid_section("experimental") {
            self.state.reveal_hidden_cursor_for_cjk_ime =
                config.experimental.reveal_hidden_cursor_for_cjk_ime;
            self.state.cjk_ime_agent_filter_configured =
                !config.experimental.cjk_ime_agents.is_empty();
            self.state.cjk_ime_agents = parse_cjk_ime_agents(&config.experimental.cjk_ime_agents);
            self.state.cjk_ime_cursor_shape =
                config.experimental.cjk_ime_cursor_shape.to_decscusr();
            self.persist_pane_history = config.experimental.pane_history;
            if !self.persist_pane_history {
                crate::persist::clear_history();
            }
        }

        if !invalid_section("server") {
            if let Some(diagnostic) = config.invalid_headless_size_diagnostic() {
                diagnostics.push(format!("{diagnostic}; keeping current [server] settings"));
            } else {
                self.state.headless_size = config.headless_size();
            }
        }

        if !invalid_section("advanced") {
            self.state.pane_scrollback_limit_bytes = config.advanced.scrollback_limit_bytes;
        }

        if !invalid_section("update") {
            let now = Instant::now();
            let previous_version_check_enabled = self.update_version_check_enabled;
            let previous_manifest_check_enabled = self.update_manifest_check_enabled;
            self.update_version_check_enabled = config.update.version_check;
            self.update_manifest_check_enabled = config.update.manifest_check;

            if !self.update_version_check_enabled {
                self.next_auto_update_check = None;
            } else if !previous_version_check_enabled
                && background_update_check_enabled(
                    self.policy.background_updates,
                    self.update_version_check_enabled,
                )
                && self.state.update_available.is_none()
            {
                self.next_auto_update_check = Some(now);
            }

            if !self.update_manifest_check_enabled {
                self.next_agent_manifest_update_check = None;
            } else if !previous_manifest_check_enabled
                && background_update_check_enabled(
                    self.policy.background_updates,
                    self.update_manifest_check_enabled,
                )
            {
                self.next_agent_manifest_update_check = Some(now);
            }
        }

        if !invalid_section("terminal") {
            self.state.default_shell = config.terminal.default_shell.clone();
            self.state.shell_mode = config.terminal.shell_mode;
            self.state.new_terminal_cwd = config.terminal.new_cwd.clone();
        }

        if !invalid_section("worktrees") {
            self.state.worktree_directory =
                crate::worktree::expand_tilde_absolute_path(&config.worktrees.directory);
        }

        if !invalid_section("theme") {
            self.state.theme_runtime = theme_runtime_config(config, !invalid_section("ui"));
            self.refresh_effective_app_theme();
        }

        let status = if diagnostics.is_empty() {
            crate::config::ConfigReloadStatus::Applied
        } else {
            crate::config::ConfigReloadStatus::Partial
        };

        if diagnostics.is_empty() {
            self.state.config_diagnostic = None;
            self.config_diagnostic_deadline = None;
            if notify_success {
                self.state.toast = Some(crate::app::state::ToastNotification {
                    kind: crate::app::state::ToastKind::UpdateInstalled,
                    title: "reloaded config".to_string(),
                    context: "using config.toml".to_string(),
                    position: None,
                    target: None,
                });
            }
        } else {
            self.state.config_diagnostic = crate::config::config_diagnostic_summary(&diagnostics);
            self.config_diagnostic_deadline = None;
            if notify_success {
                self.state.toast = Some(crate::app::state::ToastNotification {
                    kind: crate::app::state::ToastKind::UpdateInstalled,
                    title: "reloaded config".to_string(),
                    context: "with warnings".to_string(),
                    position: None,
                    target: None,
                });
            }
        }

        self.state.request_client_config_reload = true;
        crate::config::ConfigReloadReport {
            status,
            diagnostics,
        }
    }
}

impl App {
    /// Rehydrates `pending_agent_prompts` from
    /// `persist::pending_prompts`. Without this the `deferred` receipt was a
    /// promise the server broke on every restart: the queue was memory-only,
    /// so a restart silently dropped messages a sender had been told were
    /// queued for delivery.
    pub(crate) fn load_pending_agent_prompts(&mut self) {
        let records = crate::persist::pending_prompts::read_pending_prompts();
        if records.is_empty() {
            return;
        }
        let now = Instant::now();
        let mut highest_queue_id = 0;
        for record in records {
            highest_queue_id = highest_queue_id.max(record.queue_id);
            self.pending_agent_prompts
                .entry(record.target)
                .or_default()
                .push_back(PendingAgentPrompt {
                    queue_id: record.queue_id,
                    params: record.params,
                    enqueued_at: now,
                });
        }
        self.next_pending_agent_prompt_queue_id = self
            .next_pending_agent_prompt_queue_id
            .max(highest_queue_id + 1);
        tracing::info!(
            targets = self.pending_agent_prompts.len(),
            "restored deferred agent prompts"
        );
    }

    /// Mirrors `pending_agent_prompts` to disk. Called after every mutation of
    /// the map — enqueue, cap eviction, drain, pane-close drop — so the stored
    /// queue is never a stale superset that would replay an already-delivered
    /// prompt after a restart. A write failure is logged, never propagated:
    /// losing durability is strictly better than failing the delivery that was
    /// already accepted.
    pub(crate) fn persist_pending_agent_prompts(&self) {
        let records: Vec<crate::persist::pending_prompts::PendingPromptRecord> = self
            .pending_agent_prompts
            .iter()
            .flat_map(|(target, queue)| {
                queue.iter().map(move |pending| {
                    crate::persist::pending_prompts::PendingPromptRecord {
                        target: target.clone(),
                        queue_id: pending.queue_id,
                        params: pending.params.clone(),
                    }
                })
            })
            .collect();
        if let Err(err) = crate::persist::pending_prompts::write_pending_prompts(&records) {
            tracing::warn!(error = %err, "failed to persist deferred agent prompts");
        }
    }

    /// Checks and records an `agent.prompt` call from `from_pane` to
    /// `target_pane` against the loop-guard rate limit.
    ///
    /// Returns `Ok(())` and records `now` for the pair when the call is
    /// allowed. Returns `Err(remaining)` — the cooldown still left — when
    /// the same pair fired within `AGENT_PROMPT_RATE_LIMIT` and the call
    /// must be rejected without recording a new timestamp.
    pub(crate) fn check_agent_prompt_rate_limit(
        &mut self,
        from_pane: &str,
        target_pane: &str,
        now: Instant,
    ) -> Result<(), Duration> {
        self.agent_prompt_rate_limits
            .retain(|_, at| now.duration_since(*at) < AGENT_PROMPT_RATE_LIMIT_PRUNE_AGE);
        let key = (from_pane.to_string(), target_pane.to_string());
        if let Some(last) = self.agent_prompt_rate_limits.get(&key) {
            let elapsed = now.duration_since(*last);
            if elapsed < AGENT_PROMPT_RATE_LIMIT {
                return Err(AGENT_PROMPT_RATE_LIMIT - elapsed);
            }
        }
        self.agent_prompt_rate_limits.insert(key, now);
        Ok(())
    }

    /// Queues `params` for replay once `target_pane` (its public pane id) is next
    /// observed to leave `Working`. Bounded per target at
    /// `PENDING_AGENT_PROMPT_CAP`: past the cap, the oldest queued prompt is dropped
    /// (with a tracing warning, and a `agent_prompt.dropped` event / best-effort
    /// sender notice — see `report_queued_prompt_dropped`) to make room — a full
    /// queue means the target is falling behind, so keeping the newest instruction
    /// is more useful than keeping the oldest. Returns `(1-based queue position,
    /// queue_id)`, surfaced to callers as `AgentPrompted.queue_position` /
    /// `AgentPrompted.queue_id`.
    pub(crate) fn enqueue_pending_agent_prompt(
        &mut self,
        target_pane: String,
        params: crate::api::schema::AgentPromptParams,
    ) -> (usize, u64) {
        let queue_id = self.next_pending_agent_prompt_queue_id();
        let evicted = {
            let queue = self
                .pending_agent_prompts
                .entry(target_pane.clone())
                .or_default();
            if queue.len() >= PENDING_AGENT_PROMPT_CAP {
                queue.pop_front()
            } else {
                None
            }
        };
        if let Some(evicted) = evicted {
            tracing::warn!(
                target = %target_pane,
                cap = PENDING_AGENT_PROMPT_CAP,
                "pending agent prompt queue full; dropped oldest queued prompt"
            );
            self.report_queued_prompt_dropped(
                evicted.queue_id,
                &target_pane,
                evicted.params.from_pane,
                evicted.params.origin_channel,
                crate::api::schema::QueuedAgentPromptDropReason::Capacity,
                None,
            );
        }
        let queue = self
            .pending_agent_prompts
            .entry(target_pane.clone())
            .or_default();
        queue.push_back(PendingAgentPrompt {
            queue_id,
            params,
            enqueued_at: Instant::now(),
        });
        let depth = queue.len();
        self.persist_pending_agent_prompts();
        (depth, queue_id)
    }

    fn next_pending_agent_prompt_queue_id(&mut self) -> u64 {
        let id = self.next_pending_agent_prompt_queue_id;
        self.next_pending_agent_prompt_queue_id =
            self.next_pending_agent_prompt_queue_id.saturating_add(1);
        id
    }

    /// Replays every prompt queued for `target_pane`, oldest first, through
    /// `handle_agent_prompt`. A queued prompt that lands mid-`Working` again (the
    /// target flipped back busy between drains) is simply re-queued by
    /// `handle_agent_prompt`'s own busy check — reported as an `outcome: "deferred"`
    /// receipt, not an error — so ordering degrades but nothing is lost; not a
    /// terminal fate, so no event yet. A successful replay is terminal: reports
    /// `agent_prompt.delivered`. Any other failure (rate-limited, pane gone, agent
    /// swapped out) is also terminal — a retry cannot fix it — so it is logged and
    /// reported via `report_queued_prompt_dropped`.
    pub(crate) fn drain_pending_agent_prompts(&mut self, target_pane: &str) {
        let Some(queue) = self.pending_agent_prompts.remove(target_pane) else {
            return;
        };
        // Persist the removal BEFORE replaying: a crash mid-drain must not
        // leave a stored queue that redelivers prompts already written to the
        // pane. A re-queue (target flipped busy again) goes back through
        // `enqueue_pending_agent_prompt`, which persists on its own.
        self.persist_pending_agent_prompts();
        for pending in queue {
            let queue_id = pending.queue_id;
            let from_pane = pending.params.from_pane.clone();
            let origin_channel = pending.params.origin_channel.clone();
            let waited_ms = pending.enqueued_at.elapsed().as_millis();
            let request_id = format!("deferred:{target_pane}:{waited_ms}");
            let response = self.handle_agent_prompt(request_id, pending.params);
            let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&response) else {
                continue;
            };
            if let Some(error) = parsed.get("error") {
                let code = error.get("code").and_then(|c| c.as_str()).unwrap_or("");
                let message = error.get("message").and_then(|m| m.as_str()).unwrap_or("");
                tracing::warn!(
                    target = %target_pane,
                    waited_ms,
                    code,
                    message,
                    "deferred agent prompt dropped; replay failed"
                );
                let detail = if message.is_empty() {
                    code.to_string()
                } else {
                    message.to_string()
                };
                self.report_queued_prompt_dropped(
                    queue_id,
                    target_pane,
                    from_pane,
                    origin_channel,
                    crate::api::schema::QueuedAgentPromptDropReason::AgentChanged,
                    Some(detail),
                );
                continue;
            }
            let outcome = parsed
                .get("result")
                .and_then(|result| result.get("outcome"))
                .and_then(|outcome| outcome.as_str());
            if outcome == Some("deferred") {
                tracing::debug!(
                    target = %target_pane,
                    waited_ms,
                    "deferred agent prompt re-queued; target busy again"
                );
                continue;
            }
            self.report_queued_prompt_delivered(queue_id, target_pane, from_pane);
        }
    }

    /// Called on every `pane.agent_status_changed` observation for
    /// `target_pane`. Starts or cancels the settle window that gates
    /// `drain_settled_pending_agent_prompts`.
    pub(crate) fn sync_pending_agent_prompt_drain_deadline(
        &mut self,
        target_pane: &str,
        agent_status: crate::api::schema::AgentStatus,
        now: Instant,
    ) {
        if agent_status == crate::api::schema::AgentStatus::Working {
            self.pending_agent_prompt_drain_deadlines
                .remove(target_pane);
            return;
        }
        if self
            .pending_agent_prompts
            .get(target_pane)
            .is_none_or(std::collections::VecDeque::is_empty)
        {
            return;
        }
        self.pending_agent_prompt_drain_deadlines
            .entry(target_pane.to_string())
            .or_insert_with(|| now + PENDING_AGENT_PROMPT_DRAIN_SETTLE);
    }

    /// Drains every target whose settle deadline (see
    /// `sync_pending_agent_prompt_drain_deadline`) has elapsed by `now`.
    /// Called from the scheduled-task tick (`handle_scheduled_tasks` /
    /// `handle_scheduled_tasks_headless`) so a target that goes quiet and
    /// never produces another status-change event still gets drained once
    /// settled, not just on the next incidental status flip. Returns
    /// whether any target was drained, for the caller's render-dirty flag.
    pub(crate) fn drain_settled_pending_agent_prompts(&mut self, now: Instant) -> bool {
        let due: Vec<String> = self
            .pending_agent_prompt_drain_deadlines
            .iter()
            .filter(|(_, deadline)| now >= **deadline)
            .map(|(target, _)| target.clone())
            .collect();
        for target in &due {
            self.pending_agent_prompt_drain_deadlines.remove(target);
            self.drain_pending_agent_prompts(target);
        }
        !due.is_empty()
    }

    /// Drops every prompt queued for `target_pane` (its public pane id) — the
    /// recipient disappeared (pane closed) before it could be delivered.
    pub(crate) fn fail_pending_agent_prompts(&mut self, target_pane: &str) {
        let Some(queue) = self.pending_agent_prompts.remove(target_pane) else {
            return;
        };
        self.persist_pending_agent_prompts();
        for pending in queue {
            tracing::warn!(
                target = %target_pane,
                waited_ms = pending.enqueued_at.elapsed().as_millis(),
                "deferred agent prompt dropped; target pane closed before delivery"
            );
            self.report_queued_prompt_dropped(
                pending.queue_id,
                target_pane,
                pending.params.from_pane,
                pending.params.origin_channel,
                crate::api::schema::QueuedAgentPromptDropReason::PaneClosed,
                None,
            );
        }
    }

    /// Emits the durable `agent_prompt.delivered` event for a queued prompt that
    /// was successfully drained and injected.
    fn report_queued_prompt_delivered(
        &mut self,
        queue_id: u64,
        target_pane: &str,
        from_pane: Option<String>,
    ) {
        let workspace_id = self
            .resolve_agent_target(target_pane)
            .ok()
            .map(|resolved| self.public_workspace_id(resolved.ws_idx));
        self.emit_event(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::QueuedPromptDelivered,
            data: crate::api::schema::EventData::QueuedPromptDelivered {
                queue_id,
                target_pane: target_pane.to_string(),
                workspace_id,
                from_pane,
            },
        });
    }

    /// Reports the terminal drop of a queued prompt — this is what keeps a
    /// `deferred` receipt honest, since the queue itself otherwise only logs
    /// internally. Always emits the durable `agent_prompt.dropped` event; when
    /// the sender is a known, resolvable pane, also best-effort injects a
    /// one-line PTY notice (see `notify_pane_of_queue_drop` — that path can
    /// never itself be queued, so this never recurses); when the prompt was a
    /// `channel.send` fan-out delivery, appends an honest system line to that
    /// channel's history.
    fn report_queued_prompt_dropped(
        &mut self,
        queue_id: u64,
        target_pane: &str,
        from_pane: Option<String>,
        origin_channel: Option<String>,
        reason: crate::api::schema::QueuedAgentPromptDropReason,
        detail: Option<String>,
    ) {
        let workspace_id = self
            .resolve_agent_target(target_pane)
            .ok()
            .map(|resolved| self.public_workspace_id(resolved.ws_idx));
        self.emit_event(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::QueuedPromptDropped,
            data: crate::api::schema::EventData::QueuedPromptDropped {
                queue_id,
                target_pane: target_pane.to_string(),
                workspace_id,
                from_pane: from_pane.clone(),
                reason,
                detail: detail.clone(),
            },
        });
        let reason_text = queued_prompt_drop_reason_text(reason, detail.as_deref());
        if let Some(from_pane) = from_pane.as_deref() {
            self.notify_pane_of_queue_drop(from_pane, target_pane, &reason_text);
        }
        if let Some(channel) = origin_channel {
            let line = crate::api::schema::ChannelMessage {
                ts: rfc3339_now(),
                seq: crate::persist::channels::next_seq(&channel),
                from_pane: "system".to_string(),
                from_name: "bora".to_string(),
                from_kind: crate::api::schema::ChannelSenderKind::Agent,
                text: format!("delivery to {target_pane} dropped: {reason_text}"),
                in_reply_to: None,
                to_pane: None,
                to_human: false,
            };
            if let Err(err) = crate::persist::channels::append_message(&channel, &line) {
                tracing::warn!(
                    channel = %channel,
                    target_pane = %target_pane,
                    error = %err,
                    "failed to append delivery-drop notice to channel history"
                );
            }
        }
    }

    /// Best-effort direct PTY notice to `from_pane` that its queued prompt to
    /// `target_pane` was dropped. Deliberately bypasses the deferred queue and
    /// rate limiting entirely — it calls `handle_agent_prompt` with
    /// `from_pane: None` and `when_idle: None`, so neither gate applies — which is
    /// what keeps a notice from ever being queued itself: a drop can never
    /// recurse into reporting another drop. Skips silently (the event emitted by
    /// `report_queued_prompt_dropped` remains the durable record) when `from_pane`
    /// no longer resolves to a live agent pane, or that pane is currently
    /// `Working` — a courtesy notice must never interrupt an agent mid-task.
    fn notify_pane_of_queue_drop(&mut self, from_pane: &str, target_pane: &str, reason: &str) {
        let Ok(resolved) = self.resolve_agent_target(from_pane) else {
            return;
        };
        if let Some(agent) = self.agent_info(resolved.ws_idx, resolved.pane_id) {
            if agent.agent_status == crate::api::schema::AgentStatus::Working {
                tracing::debug!(
                    from_pane,
                    target_pane,
                    "queued prompt drop notice skipped; sender pane busy"
                );
                return;
            }
        }
        let notice = format!("[bora] prompt to {target_pane} dropped: {reason}");
        let response = self.handle_agent_prompt(
            format!("system:queue_drop_notice:{target_pane}"),
            crate::api::schema::AgentPromptParams {
                target: from_pane.to_string(),
                text: notice,
                wait: None,
                from_pane: None,
                when_idle: None,
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        );
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&response) {
            if parsed.get("error").is_some() {
                tracing::debug!(
                    from_pane,
                    target_pane,
                    "queued prompt drop notice failed to inject"
                );
            }
        }
    }

    /// Chat view feed hook. The chat view itself is temporarily absent in this
    /// merge (see ceo-bora#274); the channel transcript on disk remains the
    /// source of truth, so this is a deliberate no-op until the re-port in
    /// ceo-bora#276.
    pub(crate) fn push_chat_message(
        &mut self,
        _channel: &str,
        _message: crate::api::schema::ChannelMessage,
    ) {
    }

    /// Human-seat chat arrival notice: passive delivery per ceo-bora#33 —
    /// patches the channel workspace's `chat_unread` metadata token (the dim
    /// one-line preview the sidebar row renders) and never switches the view
    /// or injects a pane. The toast raise returns with the chat view (#276).
    pub(crate) fn notify_chat_to_human(
        &mut self,
        channel: &str,
        message: &crate::api::schema::ChannelMessage,
    ) {
        if !message.to_human {
            return;
        }
        self.set_channel_unread_badge(channel, message);
    }

    /// Writes the dim one-line preview + unread badge onto `channel`'s own
    /// workspace row (`Workspace::metadata_tokens`), keyed by
    /// `CHAT_UNREAD_TOKEN`. A channel whose workspace can't be found (e.g.
    /// gone) is a silent no-op, same posture as every other channel
    /// sidecar write.
    fn set_channel_unread_badge(
        &mut self,
        channel: &str,
        message: &crate::api::schema::ChannelMessage,
    ) {
        let normalized = crate::persist::channels::normalize_channel_name(channel);
        let Some(ws_idx) = self.find_channel_workspace(&normalized) else {
            return;
        };
        let Some(ws) = self.state.workspaces.get_mut(ws_idx) else {
            return;
        };
        let preview = chat_human_preview(&message.from_name, &message.text);
        ws.metadata_tokens.patch(
            std::collections::HashMap::from([(CHAT_UNREAD_TOKEN.to_string(), Some(preview))]),
            None,
            std::time::Instant::now(),
        );
    }
}

/// Collapses `text` to one line (channel messages can carry newlines) and
/// caps its length, so one very long message can't grow the sidebar
/// badge without bound.
fn chat_human_preview(from_name: &str, text: &str) -> String {
    const MAX_LEN: usize = 48;
    let collapsed: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .take(MAX_LEN)
        .collect();
    let ellipsis = if text.chars().count() > MAX_LEN {
        "…"
    } else {
        ""
    };
    format!("{from_name}: {collapsed}{ellipsis}")
}

/// Metadata-token key `App::notify_chat_to_human` patches onto a
/// channel's own workspace to carry its unread badge + one-line dim
/// preview — the same `Workspace::metadata_tokens` store
/// `bora workspace report-metadata` patches, so the sidebar row renders it
/// with zero extra wiring.
const CHAT_UNREAD_TOKEN: &str = "chat_unread";
#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::detect::{Agent, AgentState};
    use crate::workspace::Workspace;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.default_shell = exiting_test_command().into();
        app
    }

    fn unique_temp_path(name: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("herdr-{name}-{}-{stamp}", std::process::id()))
    }

    fn config_env_lock() -> &'static parking_lot::Mutex<()> {
        crate::config::test_config_env_lock()
    }

    fn temp_config_path(name: &str) -> std::path::PathBuf {
        let unique = format!(
            "herdr-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique).join("config.toml")
    }

    fn restore_xdg_state_home(original: Option<std::ffi::OsString>) {
        if let Some(value) = original {
            std::env::set_var("XDG_STATE_HOME", value);
        } else {
            std::env::remove_var("XDG_STATE_HOME");
        }
    }

    #[test]
    fn git_refresh_deadline_is_suppressed_while_in_flight() {
        let mut app = test_app();
        app.state.workspaces.push(Workspace::test_new("one"));
        app.git_refresh_in_flight = true;

        assert_eq!(app.git_refresh_deadline(), None);
    }

    #[test]
    fn unchanged_git_status_event_has_no_render_impact() {
        let mut app = test_app();
        app.git_refresh_in_flight = true;

        let changed = app.handle_internal_event_with_render_impact(AppEvent::GitStatusRefreshed {
            results: Vec::new(),
            cache_updates: Vec::new(),
        });

        assert!(!changed);
        assert!(!app.git_refresh_in_flight);
    }

    #[test]
    fn tab_bar_command_events_render_only_when_visible_output_changes() {
        if !crate::platform::status_commands_supported() {
            return;
        }

        let mut app = test_app();
        app.configure_tab_bar_status(
            &[crate::config::TabBarRightEntryConfig::Command {
                command: "status".into(),
                interval_seconds: 5,
                timeout_seconds: 2,
            }],
            " ",
        );
        let generation = app.tab_bar_status_generation;
        let event = |generation, output: Option<&str>| AppEvent::TabBarCommandFinished {
            generation,
            segment_index: 0,
            result: Ok(output.map(str::to_string)),
        };

        assert!(!app.handle_internal_event_with_render_impact(event(generation, None)));
        assert!(app.handle_internal_event_with_render_impact(event(generation, Some("ready"))));
        assert!(!app.handle_internal_event_with_render_impact(event(generation, Some("ready"))));
        assert!(!app.handle_internal_event_with_render_impact(event(
            generation.wrapping_add(1),
            Some("stale"),
        )));
    }

    #[test]
    fn git_status_event_clears_in_flight_refresh() {
        let mut app = test_app();
        app.git_refresh_in_flight = true;
        let previous_refresh = Instant::now() - Duration::from_secs(10);
        app.last_git_remote_status_refresh = previous_refresh;

        app.handle_internal_event(AppEvent::GitStatusRefreshed {
            results: Vec::new(),
            cache_updates: Vec::new(),
        });

        assert!(!app.git_refresh_in_flight);
        assert!(app.last_git_remote_status_refresh > previous_refresh);
    }

    #[test]
    fn git_status_event_marks_render_dirty_when_status_changes() {
        let mut app = test_app();
        app.state.workspaces.push(Workspace::test_new("one"));
        let _ = app.render_dirty.take();
        let workspace_id = app.state.workspaces[0].id.clone();
        let resolved_identity_cwd = app.state.workspaces[0].resolved_identity_cwd().unwrap();

        app.handle_internal_event(AppEvent::GitStatusRefreshed {
            results: vec![crate::workspace::WorkspaceGitStatus {
                workspace_id,
                resolved_identity_cwd: resolved_identity_cwd.clone(),
                status_cache_key: resolved_identity_cwd,
                demand: crate::workspace::GitStatusRefreshDemand::ALL,
                auto_label: "one".into(),
                branch: Some("render-dirty-test".into()),
                ahead_behind: Some((1, 0)),
                space: None,
            }],
            cache_updates: Vec::new(),
        });

        assert!(app.render_dirty.is_pending());
    }

    #[test]
    fn notification_show_api_creates_herdr_toast_with_position() {
        let mut app = test_app();
        app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;

        let response =
            app.handle_api_request_after_internal_events_drained(crate::api::schema::Request {
                id: "notify".into(),
                method: crate::api::schema::Method::NotificationShow(
                    crate::api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: Some("api workspace".into()),
                        position: Some(crate::config::ToastHerdrPosition::TopLeft),
                        sound: crate::api::schema::NotificationShowSound::None,
                    },
                ),
            });

        let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(
            parsed.result,
            crate::api::schema::ResponseResult::NotificationShow {
                shown: true,
                reason: crate::api::schema::NotificationShowReason::Shown,
            }
        );
        let toast = app.state.toast.as_ref().expect("api toast");
        assert_eq!(toast.title, "build failed");
        assert_eq!(toast.context, "api workspace");
        assert_eq!(
            toast.position,
            Some(crate::config::ToastHerdrPosition::TopLeft)
        );
        assert!(app.toast_deadline.is_some());
    }

    #[test]
    fn notification_show_api_respects_off_delivery() {
        let mut app = test_app();
        app.state.toast_config.delivery = crate::config::ToastDelivery::Off;

        let response =
            app.handle_api_request_after_internal_events_drained(crate::api::schema::Request {
                id: "notify".into(),
                method: crate::api::schema::Method::NotificationShow(
                    crate::api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: None,
                        position: None,
                        sound: crate::api::schema::NotificationShowSound::None,
                    },
                ),
            });

        let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(
            parsed.result,
            crate::api::schema::ResponseResult::NotificationShow {
                shown: false,
                reason: crate::api::schema::NotificationShowReason::Disabled,
            }
        );
        assert!(app.state.toast.is_none());
    }

    #[test]
    fn notification_show_api_does_not_replace_existing_toast() {
        let mut app = test_app();
        app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
        app.state.toast = Some(crate::app::state::ToastNotification {
            kind: crate::app::state::ToastKind::NeedsAttention,
            title: "pi needs attention".to_string(),
            context: "background · 2".to_string(),
            position: None,
            target: None,
        });

        let response =
            app.handle_api_request_after_internal_events_drained(crate::api::schema::Request {
                id: "notify".into(),
                method: crate::api::schema::Method::NotificationShow(
                    crate::api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: None,
                        position: None,
                        sound: crate::api::schema::NotificationShowSound::None,
                    },
                ),
            });

        let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(
            parsed.result,
            crate::api::schema::ResponseResult::NotificationShow {
                shown: false,
                reason: crate::api::schema::NotificationShowReason::Busy,
            }
        );
        assert_eq!(
            app.state.toast.as_ref().map(|toast| toast.title.as_str()),
            Some("pi needs attention")
        );
    }

    #[test]
    fn notification_show_api_is_rate_limited() {
        let mut app = test_app();
        app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
        app.mark_api_notification_shown(Instant::now());

        let response =
            app.handle_api_request_after_internal_events_drained(crate::api::schema::Request {
                id: "notify".into(),
                method: crate::api::schema::Method::NotificationShow(
                    crate::api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: None,
                        position: None,
                        sound: crate::api::schema::NotificationShowSound::None,
                    },
                ),
            });

        let parsed: crate::api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
        assert_eq!(
            parsed.result,
            crate::api::schema::ResponseResult::NotificationShow {
                shown: false,
                reason: crate::api::schema::NotificationShowReason::RateLimited,
            }
        );
        assert!(app.state.toast.is_none());
    }

    #[test]
    fn unchanged_git_status_drain_has_no_render_impact() {
        let mut app = test_app();
        app.git_refresh_in_flight = true;
        app.event_tx
            .try_send(AppEvent::GitStatusRefreshed {
                results: Vec::new(),
                cache_updates: Vec::new(),
            })
            .unwrap();

        assert!(!app.drain_internal_events());
        assert!(!app.git_refresh_in_flight);
    }

    #[test]
    fn internal_event_drain_limits_work_per_tick() {
        let mut app = test_app();
        for i in 0..=APP_EVENT_DRAIN_LIMIT {
            app.event_tx
                .try_send(AppEvent::UpdateReady {
                    version: format!("2.0.{i}"),
                    install_command: "herdr install".into(),
                })
                .unwrap();
        }

        assert!(app.drain_internal_events());

        let expected_version = format!("2.0.{}", APP_EVENT_DRAIN_LIMIT - 1);
        assert_eq!(
            app.state.update_available.as_deref(),
            Some(expected_version.as_str())
        );
        assert!(app.event_rx.try_recv().is_ok());
    }

    #[test]
    fn api_request_drains_all_pending_internal_events_before_reading_state() {
        let mut app = test_app();
        for i in 0..=APP_EVENT_DRAIN_LIMIT {
            app.event_tx
                .try_send(AppEvent::UpdateReady {
                    version: format!("3.0.{i}"),
                    install_command: "herdr install".into(),
                })
                .unwrap();
        }

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_server_stop_after_events".into(),
            method: crate::api::schema::Method::ServerStop(
                crate::api::schema::EmptyParams::default(),
            ),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        let expected_version = format!("3.0.{APP_EVENT_DRAIN_LIMIT}");
        assert_eq!(
            app.state.update_available.as_deref(),
            Some(expected_version.as_str())
        );
        assert!(app.event_rx.try_recv().is_err());
    }

    #[test]
    fn startup_uses_configured_agent_panel_sort() {
        let mut config = Config::default();
        config.ui.agent_panel_sort = crate::config::AgentPanelSortConfig::Priority;
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

        let app = App::new(
            &config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );

        assert_eq!(app.state.agent_panel_sort, state::AgentPanelSort::Priority);
    }

    #[test]
    fn theme_auto_switch_is_opt_in_and_preserves_manual_default() {
        let mut config = Config::default();
        config.theme.name = Some("tokyo-night".to_string());
        config.theme.custom = Some(crate::config::CustomThemeColors {
            light: Some(crate::config::ModeThemeColors {
                accent: Some("#010203".to_string()),
                ..Default::default()
            }),
            dark: Some(crate::config::ModeThemeColors {
                accent: Some("#040506".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

        let app = App::new(
            &config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );

        assert!(!app.state.theme_runtime.auto_switch);
        assert_eq!(app.state.theme_name, "tokyo-night");
        assert_eq!(app.state.palette, state::Palette::tokyo_night());
    }

    #[test]
    fn theme_auto_switch_uses_sibling_map_and_explicit_appearance() {
        let mut config = Config::default();
        config.theme.name = Some("tokyo-night".to_string());
        config.theme.auto_switch = true;
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );

        assert_eq!(app.state.theme_name, "tokyo-night");
        assert!(app.set_host_terminal_appearance_state(
            Some(crate::terminal_theme::HostAppearance::Light),
            true,
        ));

        assert_eq!(app.state.theme_name, "tokyo-night-day");
        assert_eq!(app.state.palette, state::Palette::tokyo_night_day());
    }

    #[test]
    fn theme_auto_switch_applies_custom_overrides_after_active_base() {
        let mut config = Config::default();
        config.theme.name = Some("gruvbox".to_string());
        config.theme.auto_switch = true;
        config.theme.custom = Some(crate::config::CustomThemeColors {
            accent: Some("#010203".to_string()),
            ..Default::default()
        });
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );

        app.set_host_terminal_appearance_state(
            Some(crate::terminal_theme::HostAppearance::Light),
            true,
        );

        assert_eq!(app.state.theme_name, "gruvbox-light");
        assert_eq!(
            app.state.palette.accent,
            ratatui::style::Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn theme_auto_switch_layers_active_mode_overrides_last() {
        let mut config = Config::default();
        config.theme.name = Some("gruvbox".to_string());
        config.theme.auto_switch = true;
        config.theme.custom = Some(crate::config::CustomThemeColors {
            accent: Some("#010203".to_string()),
            text: Some("#040506".to_string()),
            light: Some(crate::config::ModeThemeColors {
                accent: Some("#070809".to_string()),
                ..Default::default()
            }),
            dark: Some(crate::config::ModeThemeColors {
                text: Some("#0a0b0c".to_string()),
                sidebar_bg: Some("#0d0e0f".to_string()),
                active_row_bg: Some("#101112".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        });
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );

        assert_eq!(
            app.state.palette.accent,
            ratatui::style::Color::Rgb(1, 2, 3)
        );
        assert_eq!(
            app.state.palette.text,
            ratatui::style::Color::Rgb(10, 11, 12)
        );
        assert_eq!(
            app.state.palette.sidebar_bg,
            ratatui::style::Color::Rgb(13, 14, 15)
        );
        assert_eq!(
            app.state.palette.active_row_bg,
            ratatui::style::Color::Rgb(16, 17, 18)
        );

        app.set_host_terminal_appearance_state(
            Some(crate::terminal_theme::HostAppearance::Light),
            true,
        );

        assert_eq!(
            app.state.palette.accent,
            ratatui::style::Color::Rgb(7, 8, 9)
        );
        assert_eq!(app.state.palette.text, ratatui::style::Color::Rgb(4, 5, 6));
    }

    #[test]
    fn startup_restores_preview_update_available_from_saved_notes() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("startup-preview-update-available");
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        // Use a bogus far-future version so preview=true regardless of current binary version.
        crate::release_notes::save_pending("99.99.99", "### Changed\n- One").unwrap();

        let app = test_app();

        assert_eq!(app.state.update_available.as_deref(), Some("99.99.99"));
        assert!(app.state.latest_release_notes_available);
        assert_eq!(
            app.state
                .latest_release_notes
                .as_ref()
                .map(|notes| notes.version.as_str()),
            Some("99.99.99")
        );

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn update_ready_refreshes_cached_release_notes() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("update-ready-refreshes-release-notes");
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);
        let mut app = test_app();
        assert!(app.state.latest_release_notes.is_none());

        crate::release_notes::save_pending("99.99.99", "### Changed\n- One").unwrap();
        app.handle_internal_event(AppEvent::UpdateReady {
            version: "99.99.99".into(),
            install_command: "herdr update".into(),
        });

        assert_eq!(
            app.state.latest_release_notes.as_ref().map(|notes| (
                notes.version.as_str(),
                notes.body.as_str(),
                notes.preview
            )),
            Some(("99.99.99", "### Changed\n- One", true))
        );

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn release_notes_dismiss_api_marks_current_seen_but_keeps_preview_unseen() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("release-notes-dismiss-persistence");
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let dismiss = |app: &mut App, version: &str| {
            let response = app.handle_api_request(crate::api::schema::Request {
                id: format!("dismiss-{version}"),
                method: crate::api::schema::Method::ReleaseNotesDismiss(
                    crate::api::schema::ReleaseNotesDismissParams {
                        version: version.to_owned(),
                    },
                ),
            });
            let response: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(response["result"]["type"], "ok");
        };
        let show_on_startup = || {
            let stored: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(crate::release_notes::pending_path()).unwrap(),
            )
            .unwrap();
            stored["show_on_startup"].as_bool()
        };

        let current = env!("CARGO_PKG_VERSION");
        crate::release_notes::save_pending(current, "### Changed\n- Current").unwrap();
        let mut app = test_app();
        dismiss(&mut app, current);
        assert_eq!(show_on_startup(), Some(false));

        crate::release_notes::save_pending("99.99.99", "### Changed\n- Preview").unwrap();
        let mut app = test_app();
        dismiss(&mut app, "99.99.99");
        assert_eq!(show_on_startup(), Some(true));

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn startup_does_not_restore_update_available_from_older_saved_notes() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("startup-stale-update-notes");
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        crate::release_notes::save_pending("0.4.9", "### Changed\n- One").unwrap();

        let app = test_app();

        assert_eq!(app.state.update_available, None);
        assert!(app.state.latest_release_notes_available);

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn startup_keeps_pending_release_notes_available_without_auto_opening() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("startup-pending-release-notes-no-auto-open");
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        crate::release_notes::save_pending(env!("CARGO_PKG_VERSION"), "### Changed\n- One")
            .unwrap();
        let config = Config {
            onboarding: Some(false),
            ..Default::default()
        };
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

        let app = App::new(
            &config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );

        assert_eq!(app.state.mode, Mode::Navigate);
        assert!(app.state.latest_release_notes_available);

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn startup_loads_unseen_product_announcement_for_clients() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("startup-product-announcement-auto-open");
        let state_home = path.parent().unwrap().join("state");
        let original_xdg_state_home = std::env::var_os("XDG_STATE_HOME");
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);
        std::env::set_var("XDG_STATE_HOME", &state_home);

        crate::release_notes::save_pending(env!("CARGO_PKG_VERSION"), "### Changed\n- One")
            .unwrap();
        crate::product_announcements::save_manifest_announcement(
            env!("CARGO_PKG_VERSION"),
            Some(&crate::product_announcements::ManifestAnnouncement {
                id: "startup-announcement".into(),
                title: Some("Startup announcement".into()),
                body: "### Announcement\n- One".into(),
            }),
        )
        .unwrap();

        let config = Config {
            onboarding: Some(false),
            ..Default::default()
        };
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

        let app = App::new(
            &config,
            crate::app::AppPolicy::TEST,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );

        assert_eq!(app.state.mode, Mode::Navigate);
        assert_eq!(
            app.state
                .product_announcement
                .as_ref()
                .map(|announcement| announcement.id.as_str()),
            Some("startup-announcement")
        );

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        restore_xdg_state_home(original_xdg_state_home);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_updates_live_state() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-success");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[terminal]\ndefault_shell = \"nu\"\nshell_mode = \"non_login\"\nnew_cwd = \"home\"\n[keys]\nnew_workspace = \"prefix+m\"\nprefix = \"ctrl+a\"\n[update]\nversion_check = false\nmanifest_check = false\n[server]\nheadless_cols = 160\nheadless_rows = 50\n[ui]\nagent_panel_sort = \"priority\"\n[ui.toast]\ndelivery = \"herdr\"\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        app.next_auto_update_check = Some(Instant::now());
        app.next_agent_manifest_update_check = Some(Instant::now());
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.headless_size, (160, 50));
        assert_eq!(app.state.prefix_code, KeyCode::Char('a'));
        assert_eq!(app.state.prefix_mods, KeyModifiers::CONTROL);
        assert!(app
            .state
            .keybinds
            .new_workspace
            .matches_prefix(&KeyEvent::new(KeyCode::Char('m'), KeyModifiers::empty())));
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Herdr
        );
        assert_eq!(app.state.agent_panel_sort, state::AgentPanelSort::Priority);
        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert!(app.state.request_client_config_reload);
        assert_eq!(app.state.default_shell, "nu");
        assert_eq!(
            app.state.shell_mode,
            crate::config::ShellModeConfig::NonLogin
        );
        assert_eq!(
            app.state.new_terminal_cwd,
            crate::config::NewTerminalCwdConfig::Home
        );
        assert!(!app.update_version_check_enabled);
        assert!(!app.update_manifest_check_enabled);
        assert!(app.next_auto_update_check.is_none());
        assert!(app.next_agent_manifest_update_check.is_none());
        assert!(app.state.config_diagnostic.is_none());
        let toast = app.state.toast.as_ref().unwrap();
        assert_eq!(toast.kind, crate::app::state::ToastKind::UpdateInstalled);
        assert_eq!(toast.title, "reloaded config");
        assert_eq!(toast.context, "using config.toml");

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_keeps_kitty_graphics_until_restart() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-kitty-graphics");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[terminal]\nkitty_graphics = false\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        assert!(app.state.kitty_graphics_enabled);

        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert!(app.state.kitty_graphics_enabled);
        assert_eq!(
            report.diagnostics,
            vec![
                "terminal.kitty_graphics changes require restarting Herdr; kept current setting"
                    .to_owned()
            ]
        );

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_requests_client_reload_for_key_only_change() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-key-only");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[keys]\nprefix = \"ctrl+a\"\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        app.state.request_client_config_reload = false;
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.prefix_code, KeyCode::Char('a'));
        assert!(app.state.request_client_config_reload);

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_requests_client_reload_for_host_cursor_only_change() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-host-cursor");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[ui]\nhost_cursor = \"native\"\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        app.state.request_client_config_reload = false;

        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(
            app.loaded_host_cursor,
            crate::config::HostCursorModeConfig::Native
        );
        assert!(app.state.request_client_config_reload);

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_updates_sidebar_token_rows() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-sidebar-tokens");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);
        let mut app = test_app();

        std::fs::write(
            &path,
            "[ui.sidebar.agents]\nrows = [[\"state_icon\", \"$summary\"]]\nrow_gap = 1\n\n[ui.sidebar.agents.rows_by_agent]\nclaude = [[\"terminal_title_stripped\"]]\n\n[ui.sidebar.spaces]\nrows = [[\"workspace\", \"$jj_status\"]]\nrow_gap = 3\n",
        )
        .unwrap();
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(
            app.state.sidebar_agents.rows,
            vec![vec![
                crate::config::AgentSidebarToken::StateIcon,
                crate::config::AgentSidebarToken::Custom("summary".into()),
            ]]
        );
        assert_eq!(
            app.state.sidebar_agents.rows_by_agent["claude"],
            vec![vec![
                crate::config::AgentSidebarToken::TerminalTitleStripped,
            ]]
        );
        assert_eq!(app.state.sidebar_agents.row_gap, 1);
        assert_eq!(
            app.state.sidebar_spaces.rows,
            vec![vec![
                crate::config::SpaceSidebarToken::Workspace,
                crate::config::SpaceSidebarToken::Custom("jj_status".into()),
            ]]
        );
        assert_eq!(app.state.sidebar_spaces.row_gap, 3);

        let conditional = "[ui.sidebar.agents]\nrows = [[{ token = '$load', rules = [{ gt = 80, bold = true }] }]]\n";
        std::fs::write(&path, conditional).unwrap();
        assert_eq!(
            app.reload_config().status,
            crate::config::ConfigReloadStatus::Applied
        );
        assert_eq!(
            app.state.sidebar_agents.rows[0][0]
                .style_for_value("90")
                .bold,
            Some(true)
        );
        let previous = app.state.sidebar_agents.clone();
        std::fs::write(&path, conditional.replace("gt = 80", "gt = 'invalid'")).unwrap();
        assert_eq!(
            app.reload_config().status,
            crate::config::ConfigReloadStatus::Partial
        );
        assert_eq!(app.state.sidebar_agents, previous);

        let previous_agents = app.state.sidebar_agents.clone();
        std::fs::write(
            &path,
            "[ui.sidebar.agents]\nrows = [[\"agent\"]]\n\n[ui.sidebar.agents.rows_by_agent]\nclaude-code = [[\"terminal_title\"]]\n",
        )
        .unwrap();
        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert_eq!(app.state.sidebar_agents, previous_agents);

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_invalid_sidebar_bounds_keeps_previous_ui_and_returns_partial() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-invalid-sidebar-bounds");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let original_pane_borders = app.state.pane_borders;
        // Pair the bad bounds with another `[ui]` field change to confirm the
        // entire section is treated as invalid (not just the bounds).
        std::fs::write(
            &path,
            "[ui]\nsidebar_min_width = 50\nsidebar_max_width = 30\npane_borders = \"always\"\n",
        )
        .unwrap();

        let report = app.reload_config();
        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.contains("sidebar_min_width")
                && diagnostic.contains("sidebar_max_width")
                && diagnostic.contains("greater")
        }));
        assert_eq!(
            app.state.pane_borders, original_pane_borders,
            "[ui] is treated as invalid on bad bounds; pane_borders must not apply"
        );
        assert_eq!(
            app.state.config_diagnostic.as_deref(),
            Some("config.toml; herdr config check")
        );

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_disables_invalid_binding_but_applies_valid_keymap_and_other_sections() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-invalid-keybind");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[keys]\nnew_workspace = \"wat\"\n[ui.toast]\ndelivery = \"terminal\"\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let original_prefix = (app.state.prefix_code, app.state.prefix_mods);
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic.contains("keys.new_workspace") && diagnostic.contains("disabling binding")
        }));
        assert_eq!(
            (app.state.prefix_code, app.state.prefix_mods),
            original_prefix
        );
        assert!(app.state.keybinds.new_workspace.bindings.is_empty());
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Terminal
        );
        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_applies_known_sibling_and_summarizes_unknown_key() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-unknown-key");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let target_pane_borders = crate::config::PaneBordersConfig::Always;
        std::fs::write(
            &path,
            "[ui]\npane_borders = \"always\"\nmouse_captur = false\n",
        )
        .unwrap();

        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert_eq!(
            report.diagnostics,
            vec!["unknown config key ui.mouse_captur; ignoring key"]
        );
        assert_eq!(app.state.pane_borders, target_pane_borders);
        assert_eq!(
            app.state.config_diagnostic.as_deref(),
            Some("config.toml has unknown keys; herdr config check")
        );

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_user_binding_displaces_default_without_rejecting_prefix() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-user-binding-displaces-default");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[keys]\nprefix = \"ctrl+space\"\nprevious_workspace = \"prefix+shift+l\"\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
        assert_eq!(app.state.prefix_code, KeyCode::Char(' '));
        assert_eq!(app.state.prefix_mods, KeyModifiers::CONTROL);
        assert!(app
            .state
            .keybinds
            .previous_workspace
            .matches_prefix(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::SHIFT)));
        assert!(app.state.keybinds.swap_pane_right.bindings.is_empty());
        assert!(app.state.config_diagnostic.is_none());

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_preserves_invalid_ui_section_but_applies_valid_keys() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-invalid-ui-section");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[keys]\nnew_workspace = \"prefix+m\"\n[ui.toast]\ndelivery = \"desktop\"\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("invalid ui config")));
        assert!(app
            .state
            .keybinds
            .new_workspace
            .matches_prefix(&KeyEvent::new(KeyCode::Char('m'), KeyModifiers::empty())));
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Herdr
        );
        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_config_preserves_invalid_terminal_section_but_applies_valid_ui() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-invalid-terminal-section");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[terminal]\ndefault_shell = \"nu\"\nshell_mode = \"sideways\"\nnew_cwd = \"home\"\n[ui.toast]\ndelivery = \"terminal\"\n",
        )
        .unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let original_default_shell = app.state.default_shell.clone();
        let original_shell_mode = app.state.shell_mode;
        let original_new_cwd = app.state.new_terminal_cwd.clone();
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("invalid terminal config")));
        assert_eq!(app.state.default_shell, original_default_shell);
        assert_eq!(app.state.shell_mode, original_shell_mode);
        assert_eq!(app.state.new_terminal_cwd, original_new_cwd);
        assert_eq!(
            app.state.toast_config.delivery,
            crate::config::ToastDelivery::Terminal
        );
        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
    #[test]
    fn reload_config_keeps_current_state_on_invalid_toml() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("reload-config-invalid-toml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[keys\nnew_workspace = \"g\"\n").unwrap();
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

        let mut app = test_app();
        let original_prefix = (app.state.prefix_code, app.state.prefix_mods);
        let original_keybinds = app.state.keybinds.new_workspace.clone();
        let original_toast_delivery = app.state.toast_config.delivery;
        let report = app.reload_config();

        assert_eq!(report.status, crate::config::ConfigReloadStatus::Failed);
        assert_eq!(
            (app.state.prefix_code, app.state.prefix_mods),
            original_prefix
        );
        assert_eq!(app.state.keybinds.new_workspace, original_keybinds);
        assert_eq!(app.state.toast_config.delivery, original_toast_delivery);
        assert!(app
            .state
            .config_diagnostic
            .as_deref()
            .is_some_and(|message| {
                message == "config.toml invalid; keeping current config; herdr config check"
            }));
        assert!(app.state.toast.is_none());

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
    #[test]
    fn read_only_api_requests_do_not_force_rerender() {
        let read_only = crate::api::schema::Request {
            id: "req_1".into(),
            method: crate::api::schema::Method::WorkspaceList(
                crate::api::schema::EmptyParams::default(),
            ),
        };
        let mutating = crate::api::schema::Request {
            id: "req_2".into(),
            method: crate::api::schema::Method::WorkspaceFocus(
                crate::api::schema::WorkspaceTarget {
                    workspace_id: "w1".into(),
                },
            ),
        };
        let pane_rename = crate::api::schema::Request {
            id: "req_3".into(),
            method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
                pane_id: "w1:p1".into(),
                label: Some("logs".into()),
            }),
        };
        let worktree_list = crate::api::schema::Request {
            id: "req_4".into(),
            method: crate::api::schema::Method::WorktreeList(
                crate::api::schema::WorktreeListParams::default(),
            ),
        };
        let worktree_create = crate::api::schema::Request {
            id: "req_5".into(),
            method: crate::api::schema::Method::WorktreeCreate(
                crate::api::schema::WorktreeCreateParams::default(),
            ),
        };
        let pane_swap = crate::api::schema::Request {
            id: "req_6".into(),
            method: crate::api::schema::Method::PaneSwap(crate::api::schema::PaneSwapParams {
                pane_id: Some("w1:p1".into()),
                direction: Some(crate::api::schema::PaneDirection::Right),
                ..crate::api::schema::PaneSwapParams::default()
            }),
        };
        let pane_focus_direction = crate::api::schema::Request {
            id: "req_7".into(),
            method: crate::api::schema::Method::PaneFocusDirection(
                crate::api::schema::PaneFocusDirectionParams {
                    pane_id: Some("w1:p1".into()),
                    direction: crate::api::schema::PaneDirection::Right,
                },
            ),
        };
        let pane_resize = crate::api::schema::Request {
            id: "req_8".into(),
            method: crate::api::schema::Method::PaneResize(crate::api::schema::PaneResizeParams {
                pane_id: Some("w1:p1".into()),
                direction: crate::api::schema::PaneDirection::Right,
                amount: Some(0.05),
            }),
        };
        let agent_view = crate::api::schema::Request {
            id: "req_9".into(),
            method: crate::api::schema::Method::AgentViewClear(
                crate::api::schema::AgentViewClearParams::default(),
            ),
        };
        let command_invoke = crate::api::schema::Request {
            id: "req_10".into(),
            method: crate::api::schema::Method::CommandInvoke(
                crate::api::schema::CommandInvokeParams {
                    command_id: "cmd_boot_1_0".into(),
                    workspace_id: Some("w1".into()),
                    tab_id: Some("w1:t1".into()),
                    pane_id: Some("w1:p1".into()),
                    selection: None,
                },
            ),
        };
        let announcement_dismiss = crate::api::schema::Request {
            id: "req_11".into(),
            method: crate::api::schema::Method::ProductAnnouncementDismiss(
                crate::api::schema::ProductAnnouncementDismissParams {
                    version: "0.8.2".into(),
                    id: "client-shell".into(),
                },
            ),
        };
        let release_notes_dismiss = crate::api::schema::Request {
            id: "req_12".into(),
            method: crate::api::schema::Method::ReleaseNotesDismiss(
                crate::api::schema::ReleaseNotesDismissParams {
                    version: "0.8.2".into(),
                },
            ),
        };

        assert!(!crate::api::request_changes_ui(&read_only));
        assert!(!crate::api::request_changes_ui(&worktree_list));
        assert!(crate::api::request_changes_ui(&mutating));
        assert!(crate::api::request_changes_ui(&pane_rename));
        assert!(crate::api::request_changes_ui(&worktree_create));
        assert!(crate::api::request_changes_ui(&pane_swap));
        assert!(crate::api::request_changes_ui(&pane_focus_direction));
        assert!(crate::api::request_changes_ui(&pane_resize));
        assert!(crate::api::request_changes_ui(&agent_view));
        assert!(crate::api::request_changes_ui(&command_invoke));
        assert!(crate::api::request_changes_ui(&announcement_dismiss));
        assert!(crate::api::request_changes_ui(&release_notes_dismiss));
    }

    #[test]
    fn workspace_create_response_includes_initial_tab_and_root_pane() {
        let mut app = test_app();
        app.state.workspaces = vec![Workspace::test_new("api-root-pane")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;

        let crate::api::schema::ResponseResult::WorkspaceCreated {
            workspace,
            tab,
            root_pane,
        } = app.workspace_created_result(0).unwrap()
        else {
            panic!("expected workspace_created response");
        };

        assert_eq!(workspace.label, "api-root-pane");
        assert_eq!(tab.workspace_id, workspace.workspace_id);
        assert_eq!(root_pane.workspace_id, workspace.workspace_id);
        assert_eq!(root_pane.tab_id, tab.tab_id);
        assert!(root_pane.terminal_id.starts_with("term_"));
        assert_ne!(root_pane.terminal_id, root_pane.pane_id);
    }

    #[test]
    fn tab_create_response_includes_root_pane() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("api-tab-root-pane");
        workspace.test_add_tab(None);
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;

        let crate::api::schema::ResponseResult::TabCreated { tab, root_pane } =
            app.tab_created_result(0, 1).unwrap()
        else {
            panic!("expected tab_created response");
        };

        assert_eq!(tab.workspace_id, root_pane.workspace_id);
        assert_eq!(root_pane.tab_id, tab.tab_id);
        assert_eq!(tab.pane_count, 1);
    }

    #[test]
    fn tab_info_number_uses_stable_public_tab_number() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("api-tab-public-number");
        let removed_tab = workspace.test_add_tab(None);
        let survivor_tab = workspace.test_add_tab(None);
        let survivor_pane = workspace.tabs[survivor_tab].root_pane;
        assert!(workspace.close_tab(removed_tab));
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let survivor_idx = app.state.workspaces[0]
            .find_tab_index_for_pane(survivor_pane)
            .unwrap();

        let tab = app.tab_info(0, survivor_idx).unwrap();

        assert_eq!(tab.tab_id, format!("{}:t3", app.state.workspaces[0].id));
        assert_eq!(tab.number, 3);
        assert_eq!(tab.label, "2");
    }

    #[test]
    fn legacy_bare_tab_id_uses_tab_position_not_public_tab_number() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("legacy-tab-id");
        let removed_tab = workspace.test_add_tab(None);
        workspace.test_add_tab(None);
        let public_four_tab = workspace.test_add_tab(None);
        let fourth_position_tab = workspace.test_add_tab(None);
        let public_four_pane = workspace.tabs[public_four_tab].root_pane;
        let fourth_position_pane = workspace.tabs[fourth_position_tab].root_pane;
        assert!(workspace.close_tab(removed_tab));
        app.state.workspaces = vec![workspace];

        let public_four_idx = app.state.workspaces[0]
            .find_tab_index_for_pane(public_four_pane)
            .unwrap();
        let fourth_position_idx = app.state.workspaces[0]
            .find_tab_index_for_pane(fourth_position_pane)
            .unwrap();

        assert_eq!(app.state.workspaces[0].tabs[public_four_idx].number, 4);
        assert_eq!(app.state.workspaces[0].tabs[fourth_position_idx].number, 5);
        assert_eq!(
            app.parse_tab_id(&format!("{}:t4", app.state.workspaces[0].id)),
            Some((0, public_four_idx))
        );
        assert_eq!(
            app.parse_tab_id(&format!("{}:4", app.state.workspaces[0].id)),
            Some((0, fourth_position_idx))
        );
    }

    #[test]
    fn workspace_creation_in_navigate_mode_uses_selected_workspace_seed_cwd() {
        let mut app = test_app();
        let mut first = Workspace::test_new("herdr");
        first.identity_cwd = std::path::PathBuf::from("/tmp/herdr");
        let mut second = Workspace::test_new("pion");
        second.identity_cwd = std::path::PathBuf::from("/tmp/pion");

        app.state.workspaces = vec![first, second];
        app.state.active = Some(0);
        app.state.selected = 1;
        app.state.mode = Mode::Navigate;

        let ws_idx = app.workspace_creation_source().unwrap();
        let seed_cwd = app.seed_cwd_from_workspace(ws_idx).unwrap();

        assert_eq!(ws_idx, 1);
        assert_eq!(seed_cwd, std::path::PathBuf::from("/tmp/pion"));
    }

    #[test]
    fn new_terminal_cwd_follow_uses_source_cwd() {
        let cwd = creation::resolve_new_terminal_cwd(
            &crate::config::NewTerminalCwdConfig::Follow,
            Some(std::path::PathBuf::from("/tmp/herdr-source")),
        );

        assert_eq!(cwd, std::path::PathBuf::from("/tmp/herdr-source"));
    }

    #[test]
    fn new_terminal_cwd_follow_without_source_uses_home() {
        let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
            return;
        };

        let cwd =
            creation::resolve_new_terminal_cwd(&crate::config::NewTerminalCwdConfig::Follow, None);

        assert_eq!(cwd, home);
    }

    #[test]
    fn new_terminal_cwd_path_uses_configured_path() {
        let cwd = creation::resolve_new_terminal_cwd(
            &crate::config::NewTerminalCwdConfig::Path("/tmp/herdr-fixed".into()),
            Some(std::path::PathBuf::from("/tmp/herdr-source")),
        );

        assert_eq!(cwd, std::path::PathBuf::from("/tmp/herdr-fixed"));
    }

    #[test]
    fn server_stop_request_sets_should_quit_flag() {
        let mut app = test_app();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_server_stop".into(),
            method: crate::api::schema::Method::ServerStop(
                crate::api::schema::EmptyParams::default(),
            ),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert!(app.state.should_quit);
    }

    #[test]
    fn pane_rename_request_sets_and_clears_manual_label() {
        let mut app = test_app();
        let workspace = Workspace::test_new("api-pane-rename");
        let pane = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;

        let pane_id = app.pane_info(0, pane).unwrap().pane_id;
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_rename".into(),
            method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
                pane_id: pane_id.clone(),
                label: Some("reviewer".into()),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(response["result"]["pane"]["label"], "reviewer");
        let terminal_id = app.state.workspaces[0]
            .pane_state(pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        assert_eq!(
            app.state
                .terminals
                .get(&terminal_id)
                .unwrap()
                .manual_label
                .as_deref(),
            Some("reviewer")
        );

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_rename_clear".into(),
            method: crate::api::schema::Method::PaneRename(crate::api::schema::PaneRenameParams {
                pane_id,
                label: None,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert!(response["result"]["pane"].get("label").is_none());
        assert!(app
            .state
            .terminals
            .get(&terminal_id)
            .unwrap()
            .manual_label
            .is_none());
    }

    #[test]
    fn terminal_and_agent_targets_treat_terminal_ids_differently() {
        let mut app = test_app();
        let workspace = Workspace::test_new("terminal-target-id");
        let pane = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane).unwrap().to_string();
        app.state.workspaces = vec![workspace];
        app.state.active = Some(0);
        app.state.selected = 0;

        let resolved = app.resolve_terminal_target(&terminal_id).unwrap();
        assert_eq!(resolved.pane_id, pane);
        assert_eq!(resolved.terminal_id, terminal_id);

        assert!(matches!(
            app.resolve_agent_target(&resolved.terminal_id),
            Err(crate::app::terminal_targets::TerminalTargetError::NotFound { .. })
        ));
    }

    #[test]
    fn agent_target_rejects_a_pane_that_only_has_a_launch_command() {
        let mut app = test_app();
        let workspace = Workspace::test_new("terminal-target-command");
        let pane = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane).unwrap().clone();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .launch_argv = Some(vec!["just".into(), "dev".into()]);
        let pane_id = app.public_pane_id(0, pane).unwrap();

        assert!(app.resolve_terminal_target(&pane_id).is_ok());
        assert!(matches!(
            app.resolve_agent_target(&pane_id),
            Err(crate::app::terminal_targets::TerminalTargetError::NotFound { .. })
        ));
    }

    #[test]
    fn terminal_target_resolves_pane_id_for_an_agent() {
        let mut app = test_app();
        let workspace = Workspace::test_new("terminal-target-pane");
        let pane = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane).unwrap().to_string();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        let attached_terminal_id = app.state.workspaces[0].terminal_id(pane).cloned().unwrap();
        app.state
            .terminals
            .get_mut(&attached_terminal_id)
            .unwrap()
            .set_detected_state(
                Some(crate::detect::Agent::Pi),
                crate::detect::AgentState::Idle,
            );
        app.state.active = Some(0);
        app.state.selected = 0;
        let pane_id = app.public_pane_id(0, pane).unwrap();

        let resolved = app.resolve_terminal_target(&pane_id).unwrap();

        assert_eq!(resolved.pane_id, pane);
        assert_eq!(resolved.terminal_id, terminal_id);
    }

    #[test]
    fn terminal_target_resolves_unique_agent_name() {
        let mut app = test_app();
        let workspace = Workspace::test_new("terminal-target-name");
        let pane = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane).unwrap().to_string();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        let attached_terminal_id = app.state.workspaces[0]
            .pane_state(pane)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&attached_terminal_id)
            .unwrap()
            .set_agent_name("reviewer".into());
        app.state.active = Some(0);
        app.state.selected = 0;

        let resolved = app.resolve_terminal_target("reviewer").unwrap();

        assert_eq!(resolved.pane_id, pane);
        assert_eq!(resolved.terminal_id, terminal_id);
    }

    #[test]
    fn agent_target_treats_legacy_pane_syntax_as_a_name() {
        let mut app = test_app();
        let workspace = Workspace::test_new("agent-target-name");
        let pane = workspace.tabs[0].root_pane;
        let terminal_id = workspace.terminal_id(pane).unwrap().clone();
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        let terminal = app.state.terminals.get_mut(&terminal_id).unwrap();
        terminal.set_detected_state(
            Some(crate::detect::Agent::Pi),
            crate::detect::AgentState::Idle,
        );
        terminal.set_agent_name("p_1".into());

        let resolved = app.resolve_agent_target("p_1").unwrap();

        assert_eq!(resolved.pane_id, pane);
        assert_eq!(resolved.terminal_id, terminal_id.to_string());
    }

    #[test]
    fn terminal_target_reports_missing_target() {
        let mut app = test_app();
        app.state.workspaces = vec![Workspace::test_new("terminal-target-missing")];
        app.state.active = Some(0);
        app.state.selected = 0;

        let err = app.resolve_terminal_target("missing-agent").unwrap_err();

        assert_eq!(
            err,
            crate::app::terminal_targets::TerminalTargetError::NotFound {
                target: "missing-agent".into()
            }
        );
    }

    #[test]
    fn terminal_target_reports_ambiguous_duplicate_agent_name() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("terminal-target-ambiguous");
        let first = workspace.tabs[0].root_pane;
        let second = workspace.test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        let first_terminal_id = app.state.workspaces[0]
            .pane_state(first)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .set_agent_name("worker".into());
        let second_terminal_id = app.state.workspaces[0]
            .pane_state(second)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&second_terminal_id)
            .unwrap()
            .set_agent_name("worker".into());
        app.state.active = Some(0);
        app.state.selected = 0;

        let err = app.resolve_terminal_target("worker").unwrap_err();

        let crate::app::terminal_targets::TerminalTargetError::Ambiguous { target, candidates } =
            err
        else {
            panic!("expected ambiguous terminal target");
        };
        assert_eq!(target, "worker");
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().all(|candidate| {
            candidate.terminal_id.starts_with("term_")
                && candidate.pane_id.starts_with(&app.state.workspaces[0].id)
                && candidate.workspace_id == app.state.workspaces[0].id
                && candidate.cwd.is_some()
        }));
    }

    #[tokio::test]
    async fn pane_split_request_focuses_new_pane_when_requested() {
        let _guard = config_env_lock().lock();
        let original_shell = std::env::var_os("SHELL");
        std::env::set_var("SHELL", exiting_test_command());

        let mut app = test_app();
        let mut workspace = Workspace::test_new("api-pane-split-focus-background-tab");
        let background_tab = workspace.test_add_tab(Some("worker"));
        workspace.switch_tab(0);
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;

        let target_pane = app.state.workspaces[0].tabs[background_tab].root_pane;
        let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;
        let target_tab_id = app.public_tab_id(0, background_tab).unwrap();

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_split_focus_background_tab".into(),
            method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
                workspace_id: None,
                target_pane_id: Some(target_pane_id),
                direction: crate::api::schema::SplitDirection::Right,
                ratio: None,
                cwd: None,
                focus: true,
                right_click: Default::default(),
                env: Default::default(),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(response["result"]["pane"]["tab_id"], target_tab_id);
        assert_eq!(response["result"]["pane"]["focused"], true);
        assert_eq!(app.state.active, Some(0));
        assert_eq!(app.state.workspaces[0].active_tab, background_tab);

        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
        match original_shell {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
    }

    #[tokio::test]
    async fn pane_split_request_applies_ratio() {
        let _guard = config_env_lock().lock();
        let original_shell = std::env::var_os("SHELL");
        std::env::set_var("SHELL", exiting_test_command());

        let mut app = test_app();
        let workspace = Workspace::test_new("api-pane-split-ratio");
        let target_pane = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;

        let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_split_ratio".into(),
            method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
                workspace_id: None,
                target_pane_id: Some(target_pane_id),
                direction: crate::api::schema::SplitDirection::Right,
                ratio: Some(0.333),
                cwd: None,
                focus: false,
                right_click: crate::api::schema::PaneRightClickTarget::Pane,
                env: Default::default(),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        let splits = app.state.workspaces[0].tabs[0]
            .layout
            .splits(ratatui::layout::Rect::new(0, 0, 100, 20));
        assert_eq!(splits.len(), 1);
        assert!((splits[0].ratio - 0.333).abs() < f32::EPSILON);
        let response_pane_id = response["result"]["pane"]["pane_id"].as_str().unwrap();
        let (_, response_pane_id) = app.parse_pane_id(response_pane_id).unwrap();
        assert!(
            app.state.workspaces[0]
                .pane_state(response_pane_id)
                .unwrap()
                .right_click_passthrough
        );

        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
        match original_shell {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
    }

    #[tokio::test]
    async fn pane_split_request_uses_active_focused_pane_when_target_is_omitted() {
        let _guard = config_env_lock().lock();
        let original_shell = std::env::var_os("SHELL");
        std::env::set_var("SHELL", exiting_test_command());

        let mut app = test_app();
        let workspace = Workspace::test_new("api-pane-split-current");
        let target_pane = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.focus_pane_in_workspace(0, target_pane);

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_split_current".into(),
            method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
                workspace_id: None,
                target_pane_id: None,
                direction: crate::api::schema::SplitDirection::Right,
                ratio: None,
                cwd: None,
                focus: false,
                right_click: Default::default(),
                env: Default::default(),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "pane_info");
        assert_eq!(app.state.workspaces[0].tabs[0].layout.pane_count(), 2);
        assert_eq!(
            app.state.workspaces[0].tabs[0].layout.focused(),
            target_pane
        );

        let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
        for (_terminal_id, runtime) in runtimes {
            runtime.shutdown();
        }
        match original_shell {
            Some(value) => std::env::set_var("SHELL", value),
            None => std::env::remove_var("SHELL"),
        }
    }

    #[tokio::test]
    async fn unavailable_agent_start_does_not_mutate_topology() {
        let mut app = test_app();
        let workspace = Workspace::test_new("agent-start-target");
        let root = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let pane_id = app.pane_info(0, root).unwrap().pane_id;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_agent_start_target".into(),
            method: crate::api::schema::Method::AgentStart(crate::api::schema::AgentStartParams {
                name: "worker".into(),
                kind: "pi".into(),
                pane_id,
                args: Vec::new(),
                timeout_ms: Some(1_000),
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["error"]["code"], "agent_pane_unavailable");
        assert_eq!(app.state.workspaces[0].tabs[0].layout.pane_count(), 1);
        assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(root));
    }

    #[tokio::test]
    async fn failed_agent_start_input_rolls_back_and_can_retry() {
        let mut app = test_app();
        let workspace = Workspace::test_new("agent-start-input-failure");
        let root = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let pane_id = app.pane_info(0, root).unwrap().pane_id;
        let terminal_id = app.state.workspaces[0].tabs[0].panes[&root]
            .attached_terminal_id
            .clone();
        app.state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .set_manual_label("shell".into());
        let (runtime, mut receiver) =
            crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 1);
        runtime
            .try_send_bytes(bytes::Bytes::from_static(b"occupied"))
            .unwrap();
        app.terminal_runtimes.insert(terminal_id.clone(), runtime);

        let request = || crate::api::schema::Request {
            id: "req_agent_start_input".into(),
            method: crate::api::schema::Method::AgentStart(crate::api::schema::AgentStartParams {
                name: "worker".into(),
                kind: "codex".into(),
                pane_id: pane_id.clone(),
                args: vec!["resume".into(), "codex-session".into()],
                timeout_ms: Some(4_000),
            }),
        };
        let response = app.handle_api_request(request());
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], "agent_start_input_failed");
        assert_eq!(app.state.terminals[&terminal_id].agent_name, None);
        assert!(app.state.terminals[&terminal_id]
            .persisted_agent_session
            .is_none());
        assert_eq!(
            app.state.terminals[&terminal_id].manual_label.as_deref(),
            Some("shell")
        );

        assert_eq!(
            receiver.try_recv().unwrap(),
            bytes::Bytes::from_static(b"occupied")
        );
        let retry = app.handle_api_request(request());
        let retry: serde_json::Value = serde_json::from_str(&retry).unwrap();
        assert_eq!(retry["result"]["type"], "agent_started");
        assert_eq!(
            retry["result"]["agent"]["agent_session"],
            serde_json::json!({
                "source": "herdr:codex",
                "agent": "codex",
                "kind": "id",
                "value": "codex-session",
            })
        );
        assert_eq!(
            app.state.terminals[&terminal_id].agent_name.as_deref(),
            Some("worker")
        );
        let rename = app.handle_api_request(crate::api::schema::Request {
            id: "req_agent_rename_pending".into(),
            method: crate::api::schema::Method::AgentRename(
                crate::api::schema::AgentRenameParams {
                    target: pane_id,
                    name: Some("replacement".into()),
                },
            ),
        });
        let rename: serde_json::Value = serde_json::from_str(&rename).unwrap();
        assert_eq!(rename["error"]["code"], "agent_launch_pending");
        assert_eq!(
            app.state.terminals[&terminal_id].agent_name.as_deref(),
            Some("worker")
        );
    }

    #[test]
    fn pane_close_request_closes_only_the_target_tab_when_other_tabs_exist() {
        let mut app = test_app();
        let mut workspace = Workspace::test_new("api-pane-close");
        let second_tab = workspace.test_add_tab(Some("logs"));
        workspace.switch_tab(second_tab);
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;

        let target_pane = app.state.workspaces[0].tabs[second_tab].root_pane;
        let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_close".into(),
            method: crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
                pane_id: target_pane_id,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].tabs.len(), 1);
        assert_eq!(app.state.workspaces[0].display_name(), "api-pane-close");
    }

    #[test]
    fn pane_close_request_closes_workspace_when_it_removes_the_last_pane() {
        let mut app = test_app();
        let workspace = Workspace::test_new("api-pane-close-last");
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;

        let target_pane = app.state.workspaces[0].tabs[0].root_pane;
        let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_pane_close_last".into(),
            method: crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
                pane_id: target_pane_id,
            }),
        });
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();

        assert_eq!(response["result"]["type"], "ok");
        assert!(app.state.workspaces.is_empty());
    }

    #[test]
    fn session_dirty_flag_schedules_debounced_save() {
        let mut app = test_app();
        app.policy.persist_session = true;
        app.state.session_dirty = true;

        app.sync_session_save_schedule();

        assert!(!app.state.session_dirty);
        assert!(app.session_save_deadline.is_some());
    }

    #[test]
    fn headless_next_loop_deadline_ignores_resize_poll() {
        let mut app = test_app();
        let now = Instant::now();
        app.session_save_deadline = Some(now + Duration::from_secs(2));
        app.next_auto_update_check = Some(now + Duration::from_secs(6));

        assert_eq!(
            app.next_headless_loop_deadline_with_git_refresh(now, false, true),
            app.session_save_deadline
        );
    }

    #[test]
    fn headless_next_loop_deadline_returns_none_when_resize_poll_is_only_deadline() {
        let mut app = test_app();
        let now = Instant::now();
        app.config_diagnostic_deadline = None;
        app.toast_deadline = None;
        app.next_auto_update_check = None;
        app.session_save_deadline = None;
        app.state.workspaces.clear();

        assert_eq!(
            app.next_headless_loop_deadline_with_git_refresh(now, false, true),
            None
        );
    }

    #[test]
    fn due_session_save_starts_background_writer() {
        let _guard = crate::config::test_config_env_lock().lock();
        let config_home = unique_temp_path("background-session-save");
        std::env::set_var("XDG_CONFIG_HOME", &config_home);
        std::env::remove_var(crate::session::SESSION_ENV_VAR);

        let mut app = test_app();
        app.policy.persist_session = true;
        app.state.workspaces = vec![Workspace::test_new("autosave")];
        app.state.ensure_test_terminals();
        app.session_save_deadline = Some(Instant::now() - Duration::from_secs(1));

        app.start_background_session_save();

        assert!(app.session_save_thread.is_some());
        assert!(app.session_save_deadline.is_none());
        app.save_session_now();
        assert!(crate::session::data_dir().join("session.json").exists());

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(config_home);
    }

    #[test]
    fn background_session_save_reschedules_when_writer_is_busy() {
        let mut app = test_app();
        app.policy.persist_session = true;
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        app.session_save_thread = Some(std::thread::spawn(move || {
            let _ = release_rx.recv();
        }));

        app.start_background_session_save();

        assert!(app.session_save_thread.is_some());
        assert!(app.session_save_deadline.is_some());

        release_tx.send(()).unwrap();
        app.policy.persist_session = false;
        app.save_session_now();
    }

    #[test]
    fn final_session_save_joins_background_writer_before_returning() {
        let mut app = test_app();
        app.policy.persist_session = false;
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        app.session_save_thread = Some(std::thread::spawn(move || {
            let _ = release_rx.recv();
            done_tx.send(()).unwrap();
        }));
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            release_tx.send(()).unwrap();
        });

        app.save_session_now();

        releaser.join().unwrap();
        done_rx.try_recv().unwrap();
        assert!(app.session_save_thread.is_none());
    }

    #[tokio::test]
    async fn pane_exit_checkpoint_survives_automatic_workspace_creation_on_shutdown() {
        let _guard = crate::config::test_config_env_lock().lock();
        let config_home = unique_temp_path("signaled-pane-session-checkpoint");
        std::env::set_var("XDG_CONFIG_HOME", &config_home);
        std::env::remove_var(crate::session::SESSION_ENV_VAR);

        let mut app = test_app();
        app.policy.persist_session = true;
        let mut workspace = Workspace::test_new("preserved");
        let first_pane = workspace.tabs[0].root_pane;
        let second_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces = vec![workspace];
        app.state.active = Some(0);
        app.state.ensure_test_terminals();

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id: first_pane,
            exit_reason: crate::platform::ChildExitReason::Interrupted,
        });
        app.handle_internal_event(AppEvent::PaneDied {
            pane_id: second_pane,
            exit_reason: crate::platform::ChildExitReason::Interrupted,
        });
        assert!(app.state.workspaces.is_empty());
        assert!(app.ensure_default_workspace());

        app.save_session_on_shutdown();

        let snapshot = crate::persist::load().expect("checkpointed session should survive");
        assert_eq!(snapshot.workspaces.len(), 1);
        assert_eq!(snapshot.workspaces[0].tabs[0].panes.len(), 2);

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(config_home);
    }

    #[test]
    fn normal_autosave_replaces_a_signaled_exit_checkpoint() {
        let _guard = crate::config::test_config_env_lock().lock();
        let config_home = unique_temp_path("signaled-pane-autosave");
        std::env::set_var("XDG_CONFIG_HOME", &config_home);
        std::env::remove_var(crate::session::SESSION_ENV_VAR);

        let mut app = test_app();
        app.policy.persist_session = true;
        let workspace = Workspace::test_new("closed");
        let pane_id = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.active = Some(0);
        app.state.ensure_test_terminals();

        app.handle_internal_event(AppEvent::PaneDied {
            pane_id,
            exit_reason: crate::platform::ChildExitReason::Interrupted,
        });
        assert!(crate::persist::load().is_some());

        app.start_background_session_save();
        if let Some(thread) = app.session_save_thread.take() {
            thread.join().unwrap();
        }
        app.save_session_on_shutdown();

        assert!(crate::persist::load().is_none());

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(config_home);
    }

    #[test]
    fn durable_mutation_after_pane_exit_checkpoint_wins_on_shutdown() {
        let _guard = crate::config::test_config_env_lock().lock();
        let config_home = unique_temp_path("pane-exit-newer-session-state");
        std::env::set_var("XDG_CONFIG_HOME", &config_home);
        std::env::remove_var(crate::session::SESSION_ENV_VAR);

        for another_interrupted_exit in [false, true] {
            let mut app = test_app();
            app.policy.persist_session = true;
            let workspace = Workspace::test_new("old");
            let pane_id = workspace.tabs[0].root_pane;
            app.state.workspaces = vec![workspace];
            app.state.active = Some(0);
            app.state.ensure_test_terminals();

            app.handle_internal_event(AppEvent::PaneDied {
                pane_id,
                exit_reason: crate::platform::ChildExitReason::Interrupted,
            });
            app.state.workspaces = vec![Workspace::test_new("newer")];
            app.state.active = Some(0);
            app.state.ensure_test_terminals();
            app.state.mark_session_dirty();
            if another_interrupted_exit {
                app.handle_internal_event(AppEvent::PaneDied {
                    pane_id: app.state.workspaces[0].tabs[0].root_pane,
                    exit_reason: crate::platform::ChildExitReason::Interrupted,
                });
            }
            app.save_session_on_shutdown();

            let snapshot = crate::persist::load().expect("newer session should be saved");
            assert_eq!(snapshot.workspaces.len(), 1);
            assert_eq!(snapshot.workspaces[0].custom_name.as_deref(), Some("newer"));
        }

        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(config_home);
    }

    #[tokio::test]
    async fn full_internal_event_queue_eventually_applies_working_to_idle_transition() {
        let mut app = test_app();
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;

        app.state.workspaces = vec![ws];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.mode = Mode::Terminal;

        let terminal_id = app.state.workspaces[0]
            .pane_state(pane_id)
            .unwrap()
            .attached_terminal_id
            .clone();
        app.handle_internal_event(AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Pi),
            state: AgentState::Working,
            visible_blocker: false,
            visible_working: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });
        assert_eq!(
            app.state.terminals.get(&terminal_id).unwrap().state,
            AgentState::Working
        );

        for i in 0..APP_EVENT_CHANNEL_CAPACITY {
            app.event_tx
                .try_send(AppEvent::UpdateReady {
                    version: format!("9.9.{i}"),
                    install_command: "herdr update".into(),
                })
                .unwrap();
        }

        let tx = app.event_tx.clone();
        let send = tx.send(AppEvent::StateChanged {
            pane_id,
            agent: Some(Agent::Pi),
            state: AgentState::Idle,
            visible_blocker: false,
            visible_working: false,
            process_exited: false,
            observed_at: std::time::Instant::now(),
        });
        tokio::pin!(send);

        let blocked =
            tokio::time::timeout(Duration::from_millis(20), async { (&mut send).await }).await;
        assert!(
            blocked.is_err(),
            "state change sender should wait for queue space instead of failing"
        );

        app.drain_internal_events();

        tokio::time::timeout(Duration::from_millis(50), async { (&mut send).await })
            .await
            .expect("state change should enqueue once queue space is available")
            .expect("app event receiver should still be alive");

        let max_drains = (APP_EVENT_CHANNEL_CAPACITY / APP_EVENT_DRAIN_LIMIT) + 2;
        for _ in 0..max_drains {
            if app.state.terminals.get(&terminal_id).unwrap().state == AgentState::Idle {
                break;
            }
            app.drain_internal_events();
        }

        assert_eq!(
            app.state.terminals.get(&terminal_id).unwrap().state,
            AgentState::Idle,
            "Working→Idle should still apply after temporary queue pressure"
        );
    }
}
