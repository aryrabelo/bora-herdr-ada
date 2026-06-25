//! Application orchestration.
//!
//! - `state.rs` — AppState, Mode, and pure data structs
//! - `actions.rs` — state mutations (testable without PTYs/async)
//! - `input.rs` — key/mouse → action translation

pub(crate) mod actions;
mod agent_resume;
mod agents;
mod api;
mod api_helpers;
mod config_io;
mod creation;
mod ids;
mod input;
mod runtime;
mod session;
pub mod state;
mod terminal_targets;
mod theme_sync;
mod worktrees;

use std::collections::{HashMap, HashSet};
use std::future::pending;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const ANIMATION_INTERVAL: Duration = Duration::from_millis(16);
pub(crate) const HEADLESS_ANIMATION_INTERVAL: Duration = Duration::from_millis(128);
pub(crate) const HEADLESS_ANIMATION_TICK_STEP: u32 = 8;
pub(crate) const SELECTION_AUTOSCROLL_INTERVAL: Duration = Duration::from_millis(30);
const RESIZE_POLL_INTERVAL: Duration = Duration::from_millis(100);
const GIT_REMOTE_STATUS_REFRESH_INTERVAL: Duration = Duration::from_millis(1500);
const AUTO_UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(30 * 60);
const PENDING_AGENT_RESUME_THEME_WAIT: Duration = Duration::from_millis(750);
const SESSION_SAVE_DEBOUNCE: Duration = Duration::from_secs(5);
const SIDEBAR_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);
const PANE_DOUBLE_CLICK_WINDOW: Duration = Duration::from_millis(350);
const PANE_COPY_HIGHLIGHT_DURATION: Duration = Duration::from_millis(500);
const COPY_FEEDBACK_DURATION: Duration = Duration::from_secs(2);
pub(crate) const WORKSPACE_IDLE_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const WORKSPACE_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(5);

use crossterm::{
    event::{DisableMouseCapture, EnableMouseCapture},
    execute, terminal,
};
use ratatui::layout::Rect;
use ratatui::DefaultTerminal;
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct PaneClickState {
    pane_id: crate::layout::PaneId,
    viewport_row: u16,
    col: u16,
    at: Instant,
}

impl PaneClickState {
    fn is_double_click_for(self, next: Self) -> bool {
        self.pane_id == next.pane_id
            && next.at.duration_since(self.at) <= PANE_DOUBLE_CLICK_WINDOW
            && self.viewport_row.abs_diff(next.viewport_row) <= 1
            && self.col.abs_diff(next.col) <= 1
    }
}

pub struct App {
    pub state: AppState,
    pub(crate) terminal_runtimes: crate::terminal::TerminalRuntimeRegistry,
    pub event_tx: mpsc::Sender<AppEvent>,
    pub(crate) event_rx: mpsc::Receiver<AppEvent>,
    pub(crate) api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
    pub(crate) event_hub: crate::api::EventHub,
    pub(crate) last_focus: Option<(usize, crate::layout::PaneId)>,
    pub(crate) no_session: bool,
    pub(crate) input_rx: Option<mpsc::Receiver<crate::raw_input::RawInputEvent>>,
    pub(crate) last_terminal_size: Option<(u16, u16)>,
    pub(crate) config_diagnostic_deadline: Option<Instant>,
    pub(crate) toast_deadline: Option<Instant>,
    pub(crate) copy_feedback_deadline: Option<Instant>,
    pub(crate) last_api_notification_at: Option<Instant>,
    pub(crate) last_git_remote_status_refresh: Instant,
    pub(crate) git_refresh_in_flight: bool,
    pub(crate) git_refresh_due_after_in_flight: bool,
    pub(crate) git_status_cache: HashMap<std::path::PathBuf, crate::workspace::GitStatusCacheEntry>,
    pub(crate) pending_api_worktree_creates: HashMap<std::path::PathBuf, u64>,
    pub(crate) pending_api_worktree_removes: HashMap<String, u64>,
    pub(crate) pending_api_worktree_remove_paths: HashMap<std::path::PathBuf, u64>,
    pub(crate) next_api_worktree_operation_id: u64,
    pub(crate) last_sidebar_divider_click: Option<Instant>,
    pub(crate) last_pane_click: Option<PaneClickState>,
    pub(crate) next_resize_poll: Instant,
    pub(crate) next_animation_tick: Option<Instant>,
    pub(crate) next_auto_update_check: Option<Instant>,
    pub(crate) next_agent_manifest_update_check: Option<Instant>,
    pub(crate) update_version_check_enabled: bool,
    pub(crate) update_manifest_check_enabled: bool,
    pub(crate) agent_metadata_deadline: Option<Instant>,
    pub(crate) pending_agent_resume_deadline: Option<Instant>,
    pub(crate) selection_autoscroll_deadline: Option<Instant>,
    pub(crate) selection_highlight_clear_deadline: Option<Instant>,
    pub(crate) session_save_deadline: Option<Instant>,
    pub(crate) workspace_idle_check_deadline: Option<Instant>,
    pub(crate) persist_pane_history: bool,
    pub(crate) last_render_at: Option<Instant>,
    pub(crate) suppressed_repeat_keys:
        HashSet<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)>,
    pub render_notify: Arc<Notify>,
    pub render_dirty: Arc<AtomicBool>,
    pub(crate) full_redraw_pending: bool,
    pub(crate) overlay_panes: HashMap<crate::layout::PaneId, OverlayPaneState>,
    pub(crate) local_terminal_notifications: bool,
    pub(crate) config_reloaded_from_disk: bool,
    prefix_input_source: Box<dyn crate::platform::PrefixInputSource>,
}

pub(crate) const APP_EVENT_CHANNEL_CAPACITY: usize = 256;
pub(crate) const APP_EVENT_DRAIN_LIMIT: usize = 64;

pub(crate) enum LoopEvent {
    Timer,
    Internal(AppEvent),
    Api(Box<crate::api::ApiRequestMessage>),
    RawInput(crate::raw_input::RawInputEvent),
    InputClosed,
    RenderRequested,
}

struct SyncOutputGuard;

impl SyncOutputGuard {
    fn begin() -> io::Result<Self> {
        let mut stdout = io::stdout().lock();
        stdout.write_all(b"\x1b[?2026h")?;
        stdout.flush()?;
        Ok(Self)
    }
}

impl Drop for SyncOutputGuard {
    fn drop(&mut self) {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(b"\x1b[?2026l");
        let _ = stdout.flush();
    }
}

async fn recv_raw_input_or_pending(
    input_rx: Option<&mut mpsc::Receiver<crate::raw_input::RawInputEvent>>,
) -> Option<crate::raw_input::RawInputEvent> {
    match input_rx {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}

async fn sleep_until_or_pending(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await,
        None => pending().await,
    }
}

fn repeat_key_identity(
    key: &crate::input::TerminalKey,
) -> (crossterm::event::KeyCode, crossterm::event::KeyModifiers) {
    (key.code, key.modifiers)
}

fn auto_updates_enabled(no_session: bool) -> bool {
    !no_session && !cfg!(debug_assertions)
}

fn background_update_check_enabled(no_session: bool, check_enabled: bool) -> bool {
    auto_updates_enabled(no_session) && check_enabled
}

fn load_plugin_registry(no_session: bool) -> crate::app::state::InstalledPluginRegistry {
    if no_session {
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

    palette
}

fn resolve_effective_theme(
    runtime: &state::ThemeRuntimeConfig,
    appearance: Option<crate::terminal_theme::HostAppearance>,
) -> (state::Palette, String) {
    let (name, fallback) = if runtime.auto_switch {
        match appearance.unwrap_or(crate::terminal_theme::HostAppearance::Dark) {
            crate::terminal_theme::HostAppearance::Dark => (&runtime.dark_name, "catppuccin"),
            crate::terminal_theme::HostAppearance::Light => {
                (&runtime.light_name, "catppuccin-latte")
            }
        }
    } else {
        (&runtime.manual_name, "catppuccin")
    };
    (
        resolve_palette_for_theme_name(name, fallback, runtime),
        name.clone(),
    )
}

impl App {
    pub fn new(
        config: &Config,
        no_session: bool,
        config_diagnostic: Option<String>,
        api_rx: tokio::sync::mpsc::UnboundedReceiver<crate::api::ApiRequestMessage>,
        event_hub: crate::api::EventHub,
    ) -> Self {
        let (prefix_code, prefix_mods) = config.prefix_key();
        crate::kitty_graphics::set_enabled(config.experimental.kitty_graphics);
        let (event_tx, event_rx) = mpsc::channel::<AppEvent>(APP_EVENT_CHANNEL_CAPACITY);
        let render_notify = Arc::new(Notify::new());
        let render_dirty = Arc::new(AtomicBool::new(false));

        // Try to restore previous session
        let mut restored_terminals = std::collections::HashMap::new();
        let mut restored_terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let (
            workspaces,
            active,
            selected,
            sidebar_width,
            sidebar_width_source,
            sidebar_section_split,
            collapsed_space_keys,
        ) = if no_session {
            (
                Vec::new(),
                None,
                0,
                config.ui.sidebar_width,
                state::SidebarWidthSource::ConfigDefault,
                0.5_f32,
                std::collections::HashSet::new(),
            )
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
                (
                    Vec::new(),
                    None,
                    0,
                    snap.sidebar_width.unwrap_or(config.ui.sidebar_width),
                    if snap.sidebar_width.is_some() {
                        state::SidebarWidthSource::Persisted
                    } else {
                        state::SidebarWidthSource::ConfigDefault
                    },
                    snap.sidebar_section_split.unwrap_or(0.5),
                    snap.collapsed_space_keys,
                )
            } else {
                crate::logging::session_restored(ws.len(), "ok");
                let active = snap.active.filter(|&i| i < ws.len());
                let selected = snap.selected.min(ws.len().saturating_sub(1));
                (
                    ws,
                    active,
                    selected,
                    snap.sidebar_width.unwrap_or(config.ui.sidebar_width),
                    if snap.sidebar_width.is_some() {
                        state::SidebarWidthSource::Persisted
                    } else {
                        state::SidebarWidthSource::ConfigDefault
                    },
                    snap.sidebar_section_split.unwrap_or(0.5),
                    snap.collapsed_space_keys,
                )
            }
        } else {
            (
                Vec::new(),
                None,
                0,
                config.ui.sidebar_width,
                state::SidebarWidthSource::ConfigDefault,
                0.5_f32,
                std::collections::HashSet::new(),
            )
        };

        let agent_panel_sort = agent_panel_sort_from_config(config.ui.agent_panel_sort);

        // Validate sidebar bounds before they reach any `u16::clamp(min, max)`
        // call: `clamp` panics when `min > max`. On bad config, fall back to
        // the built-in defaults rather than crashing on the first render.
        let (sidebar_min_width, sidebar_max_width) = crate::config::validated_sidebar_bounds(
            config.ui.sidebar_min_width,
            config.ui.sidebar_max_width,
        )
        .unwrap_or_else(|| {
            tracing::warn!(
                min = config.ui.sidebar_min_width,
                max = config.ui.sidebar_max_width,
                "ui.sidebar_min_width is greater than sidebar_max_width; falling back to default bounds (18, 36)"
            );
            (18, 36)
        });

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

        let mode = if config.should_show_onboarding() {
            state::Mode::Onboarding
        } else if startup_product_announcement.is_some() {
            state::Mode::ProductAnnouncement
        } else if active.is_some() {
            state::Mode::Terminal
        } else {
            state::Mode::Navigate
        };

        let agent_manifest_summaries = crate::detect::manifest::reload_manifests();
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
            detach_exits: no_session,
            detach_requested: false,
            request_new_workspace: false,
            request_new_tab: false,
            request_new_linked_worktree: None,
            request_open_existing_worktree: None,
            request_new_workspace_cwd: None,
            request_remove_linked_worktree: None,
            request_merge_worktree_to_main: None,
            request_open_worktree_pr: None,
            request_sync_workspace_git: None,
            request_submit_worktree_create: false,
            request_submit_worktree_open: false,
            request_submit_worktree_remove: false,
            request_submit_worktree_merge: false,
            request_reload_config: false,
            request_client_config_reload: false,
            request_clipboard_write: None,
            creating_new_tab: false,
            requested_new_tab_name: None,
            rename_pane_target: None,
            worktree_create: None,
            worktree_open: None,
            worktree_remove: None,
            worktree_directory,
            collapsed_space_keys,
            request_complete_onboarding: false,
            name_input: String::new(),
            name_input_replace_on_type: false,
            release_notes: None,
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
            keybind_help: state::KeybindHelpState { scroll: 0 },
            navigator: state::NavigatorState::default(),
            copy_mode: None,
            workspace_scroll: 0,
            agent_panel_scroll: 0,
            tab_scroll: 0,
            tab_scroll_follow_active: true,
            mobile_switcher_scroll: 0,
            view: state::ViewState {
                layout: state::ViewLayout::Desktop,
                sidebar_rect: Rect::default(),
                workspace_card_areas: Vec::new(),
                workspace_group_header_areas: Vec::new(),
                tab_bar_rect: Rect::default(),
                tab_hit_areas: Vec::new(),
                tab_scroll_left_hit_area: Rect::default(),
                tab_scroll_right_hit_area: Rect::default(),
                new_tab_hit_area: Rect::default(),
                terminal_area: Rect::default(),
                mobile_header_rect: Rect::default(),
                mobile_menu_hit_area: Rect::default(),
                mobile_prev_tab_hit_area: Rect::default(),
                mobile_next_tab_hit_area: Rect::default(),
                toast_hit_area: Rect::default(),
                pane_infos: Vec::new(),
                split_borders: Vec::new(),
            },
            drag: None,
            workspace_press: None,
            tab_press: None,
            selection: None,
            selection_autoscroll: None,
            context_menu: None,
            update_available,
            update_install_command,
            latest_release_notes_available,
            update_dismissed: false,
            config_diagnostic,
            toast: None,
            pending_agent_notifications: std::collections::HashMap::new(),
            copy_feedback: None,
            outer_terminal_focus: None,
            prefix_code,
            prefix_mods,
            default_sidebar_width: config.ui.sidebar_width,
            sidebar_width,
            sidebar_min_width,
            sidebar_max_width,
            mobile_width_threshold: config.ui.mobile_width_threshold,
            sidebar_width_source,
            sidebar_width_auto: false,
            sidebar_collapsed: false,
            sidebar_section_split,
            agent_panel_sort,
            next_agent_state_change_seq: 0,
            mouse_capture: config.ui.mouse_capture,
            right_click_passthrough_modifiers: config.ui.right_click_passthrough_modifiers(),
            right_click_passthrough: None,
            redraw_on_focus_gained: config.ui.redraw_on_focus_gained,
            mouse_scroll_lines: config.ui.mouse_scroll_lines(),
            confirm_close: config.ui.confirm_close,
            prompt_new_tab_name: config.ui.prompt_new_tab_name,
            pane_borders: config.ui.pane_borders,
            pane_gaps: config.ui.pane_gaps,
            show_agent_labels_on_pane_borders: config.ui.show_agent_labels_on_pane_borders,
            pane_history_persistence: config.experimental.pane_history,
            reveal_hidden_cursor_for_cjk_ime: config.experimental.reveal_hidden_cursor_for_cjk_ime,
            cjk_ime_agent_filter_configured: !config.experimental.cjk_ime_agents.is_empty(),
            cjk_ime_agents: parse_cjk_ime_agents(&config.experimental.cjk_ime_agents),
            cjk_ime_cursor_shape: config.experimental.cjk_ime_cursor_shape.to_decscusr(),
            switch_ascii_input_source_in_prefix: config
                .experimental
                .switch_ascii_input_source_in_prefix,
            kitty_graphics_enabled: config.experimental.kitty_graphics,
            default_shell: config.terminal.default_shell.clone(),
            shell_mode: config.terminal.shell_mode,
            new_terminal_cwd: config.terminal.new_cwd.clone(),
            pane_scrollback_limit_bytes: config.advanced.scrollback_limit_bytes,
            accent: crate::config::parse_color(&config.ui.accent),
            sound: config.ui.sound.clone(),
            local_sound_playback: true,
            toast_config: config.ui.toast.clone(),
            keybinds: config.keybinds(),
            spinner_tick: 0,
            palette: theme_palette,
            theme_name,
            theme_runtime,
            host_terminal_appearance: None,
            host_terminal_appearance_explicit: false,
            settings: state::SettingsState {
                section: state::SettingsSection::Theme,
                list: state::SelectionListState::new(0),
                original_palette: None,
                original_theme: None,
            },
            integration_recommendations: crate::integration::integration_recommendations(),
            agent_manifest_summaries,
            agent_manifest_update_status: crate::detect::manifest_update::load_status(),
            integration_install_messages: Vec::new(),
            installed_plugins: load_plugin_registry(no_session),
            plugin_panes: std::collections::HashMap::new(),
            plugin_command_logs: Vec::new(),
            next_plugin_command_log_id: 1,
            plugin_commands_in_flight: 0,
            global_menu: state::MenuListState::new(0),
            host_terminal_theme: crate::terminal_theme::TerminalTheme::default(),
            session_dirty: false,
            terminal_runtime_shutdowns: Vec::new(),
        };

        state.terminals = restored_terminals;

        for ws_idx in 0..state.workspaces.len() {
            let cwd = state.workspaces[ws_idx]
                .resolved_identity_cwd_from(&state.terminals, &restored_terminal_runtimes);
            state.workspaces[ws_idx].cached_git_branch =
                cwd.as_deref().and_then(crate::workspace::git_branch);
        }

        // Background auto-update is disabled in monolithic no-session mode
        // and in debug/test builds so local development never mutates the
        // running binary out from under spawned test processes.
        let version_check_enabled =
            background_update_check_enabled(no_session, config.update.version_check);
        let manifest_check_enabled =
            background_update_check_enabled(no_session, config.update.manifest_check);
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

        Self {
            config_diagnostic_deadline: None,
            toast_deadline: None,
            copy_feedback_deadline: None,
            last_api_notification_at: None,
            state,
            terminal_runtimes: restored_terminal_runtimes,
            event_tx,
            event_rx,
            last_git_remote_status_refresh: Instant::now() - GIT_REMOTE_STATUS_REFRESH_INTERVAL,
            git_refresh_in_flight: false,
            git_refresh_due_after_in_flight: false,
            git_status_cache: HashMap::new(),
            pending_api_worktree_creates: HashMap::new(),
            pending_api_worktree_removes: HashMap::new(),
            pending_api_worktree_remove_paths: HashMap::new(),
            next_api_worktree_operation_id: 1,
            last_sidebar_divider_click: None,
            last_pane_click: None,
            next_resize_poll: Instant::now() + RESIZE_POLL_INTERVAL,
            next_animation_tick: None,
            next_auto_update_check: version_check_enabled
                .then_some(Instant::now() + AUTO_UPDATE_CHECK_INTERVAL),
            next_agent_manifest_update_check: manifest_check_enabled
                .then_some(Instant::now() + AUTO_UPDATE_CHECK_INTERVAL),
            update_version_check_enabled: config.update.version_check,
            update_manifest_check_enabled: config.update.manifest_check,
            agent_metadata_deadline: None,
            pending_agent_resume_deadline: None,
            session_save_deadline: None,
            workspace_idle_check_deadline: None,
            selection_autoscroll_deadline: None,
            selection_highlight_clear_deadline: None,
            persist_pane_history: config.experimental.pane_history,
            last_render_at: None,
            suppressed_repeat_keys: HashSet::new(),
            api_rx,
            event_hub,
            last_focus,
            no_session,
            input_rx: None,
            last_terminal_size: terminal::size().ok(),
            render_notify,
            render_dirty,
            full_redraw_pending: false,
            overlay_panes: HashMap::new(),
            local_terminal_notifications: true,
            config_reloaded_from_disk: false,
            prefix_input_source: Box::new(crate::platform::RealPrefixInputSource::default()),
        }
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
        let mut app = Self::new(config, true, config_diagnostic, api_rx, event_hub);
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

        app.no_session = false;
        let now = Instant::now();
        if background_update_check_enabled(app.no_session, app.update_version_check_enabled) {
            app.next_auto_update_check = app
                .state
                .update_available
                .is_none()
                .then_some(now + AUTO_UPDATE_CHECK_INTERVAL);
        }
        if background_update_check_enabled(app.no_session, app.update_manifest_check_enabled) {
            app.next_agent_manifest_update_check = Some(now + AUTO_UPDATE_CHECK_INTERVAL);
        }
        app.state.detach_exits = false;
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
        if let Some(width) = snapshot.sidebar_width {
            app.state.sidebar_width = width;
            app.state.sidebar_width_source = state::SidebarWidthSource::Persisted;
        }
        if let Some(split) = snapshot.sidebar_section_split {
            app.state.sidebar_section_split = split;
        }
        app.state.collapsed_space_keys = snapshot.collapsed_space_keys.clone();
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

    fn request_full_redraw(&mut self) {
        self.full_redraw_pending = true;
    }

    pub(crate) fn sync_prefix_input_source(&mut self, previous_mode: Mode) {
        match (
            previous_mode == Mode::Prefix,
            self.state.mode == Mode::Prefix,
        ) {
            (false, true) if self.state.switch_ascii_input_source_in_prefix => {
                self.prefix_input_source.switch_to_ascii();
            }
            (true, false) => self.prefix_input_source.restore(),
            _ => {}
        }
    }

    pub(crate) fn handle_internal_event_with_prefix_sync(
        &mut self,
        event: crate::events::AppEvent,
    ) {
        let previous_mode = self.state.mode;
        self.handle_internal_event(event);
        self.sync_prefix_input_source(previous_mode);
    }

    #[cfg(test)]
    pub(crate) fn set_prefix_input_source(
        &mut self,
        source: Box<dyn crate::platform::PrefixInputSource>,
    ) {
        self.prefix_input_source = source;
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        if self.input_rx.is_none() {
            self.input_rx = Some(crate::raw_input::spawn_input_reader());
        }
        self.query_host_terminal_theme();

        let mut needs_render = true;
        let mut host_mouse_capture_active = self.state.mouse_capture;

        while !self.state.should_quit {
            if self.render_dirty.load(Ordering::Acquire) {
                needs_render = true;
            }

            // Drain a bounded internal-event batch for responsiveness. API handlers
            // perform an exhaustive drain before reading pane/runtime state.
            if self.drain_internal_events() {
                needs_render = true;
            }
            if self.drain_api_requests() {
                needs_render = true;
            }

            self.sync_focus_events();
            self.sync_session_save_schedule();

            let now = Instant::now();
            if self.handle_scheduled_tasks(now, needs_render) {
                needs_render = true;
            }

            if self.state.request_complete_onboarding {
                self.state.request_complete_onboarding = false;
                self.open_settings_from_onboarding();
                needs_render = true;
            }

            if self.state.request_new_workspace {
                self.state.request_new_workspace = false;
                self.create_workspace();
                needs_render = true;
            }

            if self.state.request_new_tab {
                self.state.request_new_tab = false;
                self.create_tab();
                needs_render = true;
            }

            if let Some(ws_idx) = self.state.request_new_linked_worktree.take() {
                self.open_new_linked_worktree_dialog(ws_idx);
                needs_render = true;
            }

            if let Some(ws_idx) = self.state.request_open_existing_worktree.take() {
                self.open_existing_worktree_dialog(ws_idx);
                needs_render = true;
            }

            if let Some(cwd) = self.state.request_new_workspace_cwd.take() {
                if let Err(err) = self.create_workspace_with_events(cwd, true) {
                    tracing::error!(err = %err, "failed to create workspace at requested cwd");
                    self.state.mode = Mode::Navigate;
                }
                needs_render = true;
            }

            if let Some(ws_idx) = self.state.request_remove_linked_worktree.take() {
                self.open_remove_linked_worktree_confirmation(ws_idx);
                needs_render = true;
            }

            if let Some(ws_idx) = self.state.request_merge_worktree_to_main.take() {
                self.start_worktree_merge_to_main(ws_idx);
                needs_render = true;
            }

            if let Some(ws_idx) = self.state.request_open_worktree_pr.take() {
                self.start_worktree_open_pr(ws_idx);
                needs_render = true;
            }

            if let Some(ws_idx) = self.state.request_sync_workspace_git.take() {
                self.start_workspace_git_sync(ws_idx);
                needs_render = true;
            }

            if self.state.request_submit_worktree_create {
                self.state.request_submit_worktree_create = false;
                self.start_worktree_add();
                needs_render = true;
            }

            if self.state.request_submit_worktree_open {
                self.state.request_submit_worktree_open = false;
                self.open_selected_existing_worktree();
                needs_render = true;
            }

            if self.state.request_submit_worktree_remove {
                self.state.request_submit_worktree_remove = false;
                self.start_worktree_remove();
                needs_render = true;
            }

            if self.state.request_submit_worktree_merge {
                self.state.request_submit_worktree_merge = false;
                self.start_worktree_merge_and_remove();
                needs_render = true;
            }

            if self.state.request_reload_config {
                self.state.request_reload_config = false;
                self.reload_config();
                needs_render = true;
            }

            if self.ensure_default_workspace() {
                needs_render = true;
            }

            let now = Instant::now();
            self.sync_animation_timer(now);
            self.sync_host_mouse_capture(&mut host_mouse_capture_active)?;

            if needs_render && self.can_render_now(now) {
                self.render_dirty.swap(false, Ordering::AcqRel);
                let _sync_output = SyncOutputGuard::begin()?;
                let kitty_graphics_enabled = self.state.kitty_graphics_enabled;
                if self.full_redraw_pending {
                    if kitty_graphics_enabled {
                        crate::kitty_graphics::clear_all_host_graphics()?;
                    }
                    terminal.clear()?;
                    self.full_redraw_pending = false;
                }
                let mut cell_size = crate::kitty_graphics::HostCellSize::default();
                terminal.draw(|frame| {
                    let area = frame.area();
                    if kitty_graphics_enabled {
                        cell_size = crate::kitty_graphics::HostCellSize::from_terminal(area);
                        crate::ui::compute_view_with_cell_size(
                            &mut self.state,
                            &self.terminal_runtimes,
                            area,
                            cell_size,
                        );
                    } else {
                        crate::ui::compute_view_with_runtime_registry(
                            &mut self.state,
                            &self.terminal_runtimes,
                            area,
                        );
                    }
                    crate::ui::render_with_runtime_registry(
                        &self.state,
                        &self.terminal_runtimes,
                        frame,
                    );
                })?;
                if kitty_graphics_enabled {
                    crate::kitty_graphics::paint_local_pane_graphics(
                        &self.state,
                        &self.terminal_runtimes,
                        cell_size,
                    )?;
                }
                self.sync_pending_agent_resume_deadline(now);
                if self.start_pending_agent_resumes(self.pending_agent_resume_due(now)) {
                    self.render_dirty.store(true, Ordering::Release);
                    self.render_notify.notify_one();
                }
                self.last_render_at = Some(now);
                needs_render = false;
                continue;
            }

            let next_deadline = self.next_loop_deadline(now, needs_render);
            let event = {
                let input_rx = self.input_rx.as_mut();
                tokio::select! {
                    maybe_api = self.api_rx.recv() => match maybe_api {
                        Some(msg) => LoopEvent::Api(Box::new(msg)),
                        None => LoopEvent::Timer,
                    },
                    maybe_ev = self.event_rx.recv() => match maybe_ev {
                        Some(ev) => LoopEvent::Internal(ev),
                        None => LoopEvent::Timer,
                    },
                    maybe_input = recv_raw_input_or_pending(input_rx) => match maybe_input {
                        Some(input) => LoopEvent::RawInput(input),
                        None => LoopEvent::InputClosed,
                    },
                    _ = sleep_until_or_pending(next_deadline) => LoopEvent::Timer,
                    _ = self.render_notify.notified() => LoopEvent::RenderRequested,
                }
            };

            match event {
                LoopEvent::Timer => {}
                LoopEvent::Internal(ev) => {
                    self.handle_internal_event_with_prefix_sync(ev);
                    needs_render = true;
                }
                LoopEvent::Api(msg) => {
                    if self.handle_api_request_message(*msg) {
                        needs_render = true;
                    }
                }
                LoopEvent::RawInput(input) => {
                    if self.handle_raw_input_batch(input).await {
                        needs_render = true;
                    }
                }
                LoopEvent::InputClosed => {
                    self.input_rx = None;
                }
                LoopEvent::RenderRequested => {
                    if self.render_dirty.load(Ordering::Acquire) {
                        needs_render = true;
                    }
                }
            }
        }

        // Save session on exit (skip in --no-session mode)
        if !self.no_session {
            self.save_session_now();
        }

        Ok(())
    }

    fn sync_host_mouse_capture(&self, active: &mut bool) -> io::Result<()> {
        let desired = self
            .state
            .should_capture_host_mouse_from(&self.terminal_runtimes);
        if desired == *active {
            return Ok(());
        }
        if desired {
            execute!(io::stdout(), EnableMouseCapture)?;
        } else {
            execute!(io::stdout(), DisableMouseCapture)?;
        }
        *active = desired;
        Ok(())
    }

    pub(crate) fn ensure_default_workspace(&mut self) -> bool {
        if !self.state.workspaces.is_empty() || self.state.mode == Mode::Onboarding {
            return false;
        }

        let previous_mode = self.state.mode;
        let preserve_mode = matches!(
            previous_mode,
            Mode::ReleaseNotes | Mode::ProductAnnouncement | Mode::Settings
        );
        let cwd = self.resolve_new_terminal_cwd(None);

        match self.create_workspace_with_options(cwd, true) {
            Ok(_) => {
                if preserve_mode {
                    self.state.mode = previous_mode;
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

    pub(crate) fn dismiss_release_notes(&mut self) {
        let preview = self
            .state
            .release_notes
            .as_ref()
            .is_some_and(|notes| notes.preview);

        self.state.release_notes = None;
        if !preview {
            if let Err(err) = crate::release_notes::mark_current_version_seen() {
                self.state.config_diagnostic =
                    Some(format!("failed to update release notes status: {err}"));
                self.config_diagnostic_deadline = Some(Instant::now() + Duration::from_secs(5));
            }
        }

        if self.state.product_announcement.is_some() {
            self.state.mode = Mode::ProductAnnouncement;
        } else {
            self.state.mode = if self.state.active.is_some() {
                Mode::Terminal
            } else {
                Mode::Navigate
            };
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

        self.state.mode = if self.state.active.is_some() {
            Mode::Terminal
        } else {
            Mode::Navigate
        };
    }

    pub(crate) fn scroll_release_notes(&mut self, delta: i16) {
        let max_scroll = self.state.release_notes_max_scroll();
        if let Some(notes) = &mut self.state.release_notes {
            notes.scroll = if delta.is_negative() {
                notes.scroll.saturating_sub(delta.unsigned_abs())
            } else {
                notes.scroll.saturating_add(delta as u16)
            }
            .min(max_scroll);
        }
    }

    pub(crate) fn scroll_product_announcement(&mut self, delta: i16) {
        let max_scroll = self.state.product_announcement_max_scroll();
        if let Some(announcement) = &mut self.state.product_announcement {
            announcement.scroll = if delta.is_negative() {
                announcement.scroll.saturating_sub(delta.unsigned_abs())
            } else {
                announcement.scroll.saturating_add(delta as u16)
            }
            .min(max_scroll);
        }
    }

    pub(crate) fn open_settings_from_onboarding(&mut self) {
        self.mark_onboarding_complete();
        self.refresh_integration_recommendations();
        crate::app::input::open_settings_at(&mut self.state, state::SettingsSection::Integrations);
    }

    pub(crate) fn refresh_integration_recommendations(&mut self) {
        self.state.integration_recommendations = crate::integration::integration_recommendations();
    }

    pub(crate) fn install_recommended_integrations(&mut self) {
        let targets = self
            .state
            .integration_recommendations
            .iter()
            .filter(|recommendation| recommendation.needs_install())
            .map(|recommendation| recommendation.target)
            .collect::<Vec<_>>();

        self.state.integration_install_messages.clear();
        if targets.is_empty() {
            self.state
                .integration_install_messages
                .push("all detected integrations are current".to_string());
            return;
        }

        for target in targets {
            let label = crate::integration::integration_target_label(target);
            match crate::integration::install_target(target) {
                Ok(messages) => {
                    self.state
                        .integration_install_messages
                        .push(format!("installed {label}"));
                    self.state
                        .integration_install_messages
                        .extend(messages.into_iter().filter(|message| {
                            message.starts_with(crate::integration::INSTALL_WARNING_PREFIX)
                        }));
                }
                Err(err) => self
                    .state
                    .integration_install_messages
                    .push(format!("{label}: {err}")),
            }
        }

        self.state.integration_recommendations = crate::integration::integration_recommendations();
        self.state.mark_session_dirty();
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
            if crate::config::validated_sidebar_bounds(
                config.ui.sidebar_min_width,
                config.ui.sidebar_max_width,
            )
            .is_none()
            {
                diagnostics.push(format!(
                    "ui.sidebar_min_width ({}) is greater than sidebar_max_width ({}); keeping previous [ui] settings",
                    config.ui.sidebar_min_width, config.ui.sidebar_max_width,
                ));
            } else {
                diagnostics.extend(config.ui.sound.diagnostics());

                self.state.default_sidebar_width = config.ui.sidebar_width;
                if self.state.sidebar_width_source == state::SidebarWidthSource::ConfigDefault {
                    self.state.sidebar_width = config.ui.sidebar_width;
                }
                self.state.sidebar_min_width = config.ui.sidebar_min_width;
                self.state.sidebar_max_width = config.ui.sidebar_max_width;
                self.state.mobile_width_threshold = config.ui.mobile_width_threshold;
                // Re-clamp the live width to the new bounds. No source guard — bounds
                // always apply, including to widths owned by Persisted or Manual.
                self.state.sidebar_width = self
                    .state
                    .sidebar_width
                    .clamp(self.state.sidebar_min_width, self.state.sidebar_max_width);
                self.state.mouse_capture = config.ui.mouse_capture;
                if self.state.redraw_on_focus_gained != config.ui.redraw_on_focus_gained {
                    self.state.request_client_config_reload = true;
                }
                self.state.redraw_on_focus_gained = config.ui.redraw_on_focus_gained;
                self.state.mouse_scroll_lines = config.ui.mouse_scroll_lines();
                self.state.right_click_passthrough_modifiers =
                    config.ui.right_click_passthrough_modifiers();
                self.state.confirm_close = config.ui.confirm_close;
                self.state.prompt_new_tab_name = config.ui.prompt_new_tab_name;
                self.state.pane_borders = config.ui.pane_borders;
                self.state.pane_gaps = config.ui.pane_gaps;
                self.state.show_agent_labels_on_pane_borders =
                    config.ui.show_agent_labels_on_pane_borders;
                self.state.agent_panel_sort =
                    agent_panel_sort_from_config(config.ui.agent_panel_sort);
                self.state.agent_panel_scroll = 0;
                self.state.accent = crate::config::parse_color(&config.ui.accent);
                if !self.state.local_sound_playback && self.state.sound != config.ui.sound {
                    self.state.request_client_config_reload = true;
                }
                self.state.sound = config.ui.sound.clone();
                self.state.toast_config = config.ui.toast.clone();
            }
        }

        if !invalid_section("experimental") {
            let was_kitty_graphics_enabled = self.state.kitty_graphics_enabled;
            self.state.kitty_graphics_enabled = config.experimental.kitty_graphics;
            crate::kitty_graphics::set_enabled(config.experimental.kitty_graphics);
            if was_kitty_graphics_enabled && !config.experimental.kitty_graphics {
                let _ = crate::kitty_graphics::clear_all_host_graphics();
            }
            self.state.reveal_hidden_cursor_for_cjk_ime =
                config.experimental.reveal_hidden_cursor_for_cjk_ime;
            self.state.cjk_ime_agent_filter_configured =
                !config.experimental.cjk_ime_agents.is_empty();
            self.state.cjk_ime_agents = parse_cjk_ime_agents(&config.experimental.cjk_ime_agents);
            self.state.cjk_ime_cursor_shape =
                config.experimental.cjk_ime_cursor_shape.to_decscusr();
            self.state.switch_ascii_input_source_in_prefix =
                config.experimental.switch_ascii_input_source_in_prefix;
            self.persist_pane_history = config.experimental.pane_history;
            self.state.pane_history_persistence = config.experimental.pane_history;
            if !self.persist_pane_history {
                crate::persist::clear_history();
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
                    self.no_session,
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
                    self.no_session,
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

        crate::config::ConfigReloadReport {
            status,
            diagnostics,
        }
    }
}

// ---------------------------------------------------------------------------
// Input routing for headless server mode
// ---------------------------------------------------------------------------

impl App {
    /// Routes raw input bytes from a client through the existing input pipeline.
    ///
    /// The input bytes are parsed into `RawInputEvent`s and then processed.
    /// In terminal mode, keys are routed through the same semantic
    /// key-handling path as monolithic herdr so they are re-encoded for the
    /// focused pane's negotiated keyboard protocol instead of passing host
    /// terminal escape sequences through unchanged.
    #[cfg(test)]
    pub(crate) fn route_client_input(&mut self, data: Vec<u8>) {
        let events = crate::raw_input::parse_raw_input_bytes_sync(&data);
        self.route_client_events(events, true);
    }

    pub(crate) fn route_client_events(
        &mut self,
        events: Vec<crate::raw_input::RawInputEvent>,
        apply_host_terminal_theme: bool,
    ) {
        for event in events {
            let previous_mode = self.state.mode;
            match event {
                crate::raw_input::RawInputEvent::Key(key) => {
                    let key_id = repeat_key_identity(&key);
                    match key.kind {
                        crossterm::event::KeyEventKind::Press => {
                            if self.state.mode == Mode::Terminal {
                                self.suppressed_repeat_keys.remove(&key_id);
                                self.handle_terminal_key_headless(key);
                            } else {
                                self.suppressed_repeat_keys.insert(key_id);
                                self.handle_non_terminal_key_headless(key);
                            }
                        }
                        crossterm::event::KeyEventKind::Repeat => {
                            if self.state.mode == Mode::Terminal
                                && !self.suppressed_repeat_keys.contains(&key_id)
                            {
                                self.handle_terminal_key_headless(key);
                            }
                            // Repeats in non-terminal modes are ignored
                            // (same as monolithic behavior).
                        }
                        crossterm::event::KeyEventKind::Release => {
                            self.suppressed_repeat_keys.remove(&key_id);
                        }
                    }
                }
                crate::raw_input::RawInputEvent::Mouse(mouse) => {
                    if self.state.mouse_capture {
                        self.handle_mouse_event_headless(mouse);
                    } else {
                        self.state
                            .handle_pane_mouse_only(&self.terminal_runtimes, mouse);
                    }
                }
                crate::raw_input::RawInputEvent::Paste(text) => {
                    if self.state.mode != Mode::Terminal {
                        self.paste_into_active_text_input(&text);
                    } else if let Some(ws_idx) = self.state.active {
                        if let Some(ws) = self.state.workspaces.get(ws_idx) {
                            if let Some(focused) = ws.focused_pane_id() {
                                if let Some(runtime) = self.state.runtime_for_pane_in_workspace(
                                    &self.terminal_runtimes,
                                    ws_idx,
                                    focused,
                                ) {
                                    let _ = runtime.try_send_bytes(bytes::Bytes::from(
                                        if runtime
                                            .input_state()
                                            .map(|s| s.bracketed_paste)
                                            .unwrap_or(false)
                                        {
                                            format!("\x1b[200~{text}\x1b[201~")
                                        } else {
                                            text
                                        },
                                    ));
                                }
                            }
                        }
                    }
                }
                crate::raw_input::RawInputEvent::OuterFocusGained
                | crate::raw_input::RawInputEvent::OuterFocusLost => {}
                crate::raw_input::RawInputEvent::HostDefaultColor { kind, color } => {
                    if apply_host_terminal_theme {
                        self.update_host_terminal_theme(kind, color);
                    }
                }
                crate::raw_input::RawInputEvent::HostColorSchemeChanged(appearance) => {
                    if apply_host_terminal_theme {
                        self.set_host_terminal_appearance(appearance, true);
                    }
                }
                crate::raw_input::RawInputEvent::Unsupported => {}
            }
            self.sync_prefix_input_source(previous_mode);
        }
    }

    /// Handles a key event in non-terminal mode for the headless server.
    ///
    /// Uses the standalone handler functions that work on `&mut AppState`
    /// since the server doesn't have the async context of the monolithic App.
    fn handle_non_terminal_key_headless(&mut self, key: crate::input::TerminalKey) {
        let key_event = key.as_key_event();
        if input::modal_paste_target_active(&self.state)
            && input::is_modal_paste_shortcut(&key_event)
        {
            if let Some(text) = crate::platform::read_clipboard_text() {
                self.paste_into_active_text_input(&text);
            }
            return;
        }

        match self.state.mode {
            Mode::Prefix => {
                self.handle_prefix_key(key);
            }
            Mode::Navigate => {
                self.handle_navigate_key(key);
            }
            Mode::Copy => {
                self.handle_copy_mode_key(key);
            }
            Mode::RenameWorkspace
            | Mode::RenameTab
            | Mode::RenamePane
            | Mode::SetWorkspaceGroup => {
                input::handle_rename_key(&mut self.state, key_event);
            }
            Mode::NewLinkedWorktree => {
                self.handle_worktree_create_key(key_event);
            }
            Mode::OpenExistingWorktree => {
                self.handle_worktree_open_key(key_event);
            }
            Mode::ConfirmRemoveWorktree => {
                self.handle_worktree_remove_key(key_event);
            }
            Mode::Resize => {
                input::handle_resize_key(&mut self.state, key);
            }
            Mode::ConfirmClose => {
                input::handle_confirm_close_key(&mut self.state, key_event);
            }
            Mode::ContextMenu => {
                input::handle_context_menu_key(
                    &mut self.state,
                    &mut self.terminal_runtimes,
                    key_event,
                );
            }
            Mode::KeybindHelp => {
                input::handle_keybind_help_key(&mut self.state, key_event);
            }
            Mode::GlobalMenu => {
                input::handle_global_menu_key(&mut self.state, key_event);
            }
            Mode::Onboarding => {
                self.handle_onboarding_key(key_event);
            }
            Mode::ReleaseNotes => {
                self.handle_release_notes_key(key_event);
            }
            Mode::ProductAnnouncement => {
                self.handle_product_announcement_key(key_event);
            }
            Mode::Settings => {
                self.handle_settings_key(key_event);
            }
            Mode::Navigator => {
                input::handle_navigator_key(&mut self.state, &self.terminal_runtimes, key_event);
            }
            Mode::Terminal => {
                // Should not be called in terminal mode.
            }
        }
    }

    /// Handles a mouse event for the headless server.
    ///
    /// Delegates to the same mouse handling logic used in the monolithic
    /// mode (hit-testing against the rendered UI), which works because
    /// the server's AppState maintains view geometry from virtual rendering.
    fn handle_mouse_event_headless(&mut self, mouse: crossterm::event::MouseEvent) {
        self.handle_mouse(mouse);
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
