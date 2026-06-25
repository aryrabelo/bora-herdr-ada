use super::*;
use crate::config::Config;
use crate::detect::{Agent, AgentState};
use crate::terminal::TerminalRuntime;
use crate::workspace::Workspace;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Mutex;

fn raw_key(
    code: KeyCode,
    modifiers: KeyModifiers,
    kind: KeyEventKind,
) -> crate::raw_input::RawInputEvent {
    crate::raw_input::RawInputEvent::Key(
        crate::input::TerminalKey::new(code, modifiers).with_kind(kind),
    )
}

fn release_notes_state() -> state::ReleaseNotesState {
    state::ReleaseNotesState {
        version: "0.1.0".into(),
        body: "notes".into(),
        scroll: 0,
        preview: true,
    }
}

fn test_app() -> App {
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    App::new(
        &Config::default(),
        true,
        None,
        api_rx,
        crate::api::EventHub::default(),
    )
}

#[cfg(windows)]
fn exiting_test_command() -> &'static str {
    "C:\\Windows\\System32\\whoami.exe"
}

#[cfg(not(windows))]
fn exiting_test_command() -> &'static str {
    "/usr/bin/true"
}

#[derive(Clone, Default)]
struct FakePrefixInputSource {
    switch_calls: Rc<Cell<usize>>,
    restore_calls: Rc<Cell<usize>>,
    switched: Rc<Cell<bool>>,
    will_switch: bool,
}

impl FakePrefixInputSource {
    fn switching() -> Self {
        Self {
            will_switch: true,
            ..Self::default()
        }
    }

    fn no_op() -> Self {
        Self {
            will_switch: false,
            ..Self::default()
        }
    }
}

impl crate::platform::PrefixInputSource for FakePrefixInputSource {
    fn switch_to_ascii(&mut self) {
        self.switch_calls.set(self.switch_calls.get() + 1);
        if self.will_switch {
            self.switched.set(true);
        }
    }

    fn restore(&mut self) {
        if self.switched.replace(false) {
            self.restore_calls.set(self.restore_calls.get() + 1);
        }
    }
}

#[test]
fn sync_prefix_input_source_switches_then_restores_when_enabled() {
    let mut app = test_app();
    app.state.switch_ascii_input_source_in_prefix = true;
    let fake = FakePrefixInputSource::switching();
    let switch_calls = fake.switch_calls.clone();
    let restore_calls = fake.restore_calls.clone();
    app.set_prefix_input_source(Box::new(fake));

    // Terminal -> Prefix should switch to ASCII.
    app.state.mode = Mode::Prefix;
    app.sync_prefix_input_source(Mode::Terminal);
    assert_eq!(switch_calls.get(), 1);
    assert_eq!(restore_calls.get(), 0);

    // Prefix -> Terminal should restore the saved source.
    app.state.mode = Mode::Terminal;
    app.sync_prefix_input_source(Mode::Prefix);
    assert_eq!(switch_calls.get(), 1);
    assert_eq!(restore_calls.get(), 1);
}

#[test]
fn sync_prefix_input_source_is_noop_when_flag_disabled() {
    let mut app = test_app();
    app.state.switch_ascii_input_source_in_prefix = false;
    let fake = FakePrefixInputSource::switching();
    let switch_calls = fake.switch_calls.clone();
    let restore_calls = fake.restore_calls.clone();
    app.set_prefix_input_source(Box::new(fake));

    app.state.mode = Mode::Prefix;
    app.sync_prefix_input_source(Mode::Terminal);
    app.state.mode = Mode::Terminal;
    app.sync_prefix_input_source(Mode::Prefix);

    assert_eq!(switch_calls.get(), 0);
    assert_eq!(restore_calls.get(), 0);
}

#[test]
fn sync_prefix_input_source_restore_is_safe_when_switch_was_noop() {
    // Simulates the already-ASCII / failed-switch case: switch reports no
    // change, and the later restore on leave must stay harmless.
    let mut app = test_app();
    app.state.switch_ascii_input_source_in_prefix = true;
    let fake = FakePrefixInputSource::no_op();
    let switch_calls = fake.switch_calls.clone();
    let restore_calls = fake.restore_calls.clone();
    app.set_prefix_input_source(Box::new(fake));

    app.state.mode = Mode::Prefix;
    app.sync_prefix_input_source(Mode::Terminal);
    app.state.mode = Mode::Terminal;
    app.sync_prefix_input_source(Mode::Prefix);

    assert_eq!(switch_calls.get(), 1);
    assert_eq!(restore_calls.get(), 0);
}

#[tokio::test]
async fn raw_input_dispatch_restores_input_source_when_leaving_prefix() {
    // Leaving prefix mode happens inside the raw-input dispatch, not in
    // `handle_key` itself — the sync must sit at the dispatch layer so any
    // event that exits prefix (here Esc) still restores the host source.
    let mut app = test_app();
    app.state.switch_ascii_input_source_in_prefix = true;
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    let fake = FakePrefixInputSource::switching();
    let switch_calls = fake.switch_calls.clone();
    let restore_calls = fake.restore_calls.clone();
    app.set_prefix_input_source(Box::new(fake));

    // ctrl+b (the default prefix key) enters prefix mode → switch edge.
    app.handle_raw_input_event(raw_key(
        KeyCode::Char('b'),
        KeyModifiers::CONTROL,
        KeyEventKind::Press,
    ))
    .await;
    assert_eq!(app.state.mode, Mode::Prefix);
    assert_eq!(switch_calls.get(), 1);
    assert_eq!(restore_calls.get(), 0);

    // Esc leaves prefix mode → restore edge, even though the exit is decided
    // below `handle_key`.
    app.handle_raw_input_event(raw_key(
        KeyCode::Esc,
        KeyModifiers::empty(),
        KeyEventKind::Press,
    ))
    .await;
    assert_eq!(app.state.mode, Mode::Terminal);
    assert_eq!(restore_calls.get(), 1);
}

fn config_env_lock() -> &'static Mutex<()> {
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
    app.render_dirty.store(false, Ordering::Release);
    let workspace_id = app.state.workspaces[0].id.clone();
    let resolved_identity_cwd = app.state.workspaces[0].resolved_identity_cwd().unwrap();

    app.handle_internal_event(AppEvent::GitStatusRefreshed {
        results: vec![crate::workspace::WorkspaceGitStatus {
            workspace_id,
            resolved_identity_cwd,
            branch: Some("render-dirty-test".into()),
            ahead_behind: Some((1, 0)),
            space: None,
            change_set: None,
        }],
        cache_updates: Vec::new(),
    });

    assert!(app.render_dirty.load(Ordering::Acquire));
}

#[test]
fn clipboard_write_event_shows_feedback_toast() {
    let mut app = test_app();

    app.handle_internal_event(AppEvent::ClipboardWrite {
        content: b"copied".to_vec(),
    });

    assert!(app.state.toast.is_none());
    let feedback = app.state.copy_feedback.as_ref().expect("copy feedback");
    assert_eq!(feedback.message, "copied to clipboard");
    assert!(app.copy_feedback_deadline.is_some());
}

#[test]
fn clipboard_feedback_can_be_disabled() {
    let mut app = test_app();
    app.state.toast_config.clipboard.enabled = false;

    app.handle_internal_event(AppEvent::ClipboardWrite {
        content: b"copied".to_vec(),
    });

    assert!(app.state.copy_feedback.is_none());
    assert!(app.copy_feedback_deadline.is_none());
}

#[test]
fn clipboard_feedback_does_not_replace_notification_toast() {
    let mut app = test_app();
    app.state.toast = Some(crate::app::state::ToastNotification {
        kind: crate::app::state::ToastKind::NeedsAttention,
        title: "pi needs attention".to_string(),
        context: "background · 2".to_string(),
        position: None,
        target: None,
    });
    let original_toast = app.state.toast.clone();

    app.handle_internal_event(AppEvent::ClipboardWrite {
        content: b"copied".to_vec(),
    });

    assert_eq!(app.state.toast, original_toast);
    assert_eq!(
        app.state
            .copy_feedback
            .as_ref()
            .map(|feedback| feedback.message.as_str()),
        Some("copied to clipboard")
    );
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
fn notification_show_api_herdr_toast_expires() {
    let mut app = test_app();
    app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;

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
            shown: true,
            reason: crate::api::schema::NotificationShowReason::Shown,
        }
    );
    let deadline = app.toast_deadline.expect("api toast deadline");
    assert!(app.handle_scheduled_tasks(deadline, false));
    assert!(app.state.toast.is_none());
    assert!(app.toast_deadline.is_none());
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
        method: crate::api::schema::Method::ServerStop(crate::api::schema::EmptyParams::default()),
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

    let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

    assert_eq!(app.state.agent_panel_sort, state::AgentPanelSort::Priority);
}

#[test]
fn startup_uses_redraw_on_focus_gained_config() {
    let mut config = Config::default();
    config.ui.redraw_on_focus_gained = false;
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

    let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

    assert!(!app.state.redraw_on_focus_gained);
}

#[test]
fn theme_auto_switch_is_opt_in_and_preserves_manual_default() {
    let mut config = Config::default();
    config.theme.name = Some("tokyo-night".to_string());
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

    let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

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
    let mut app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

    assert_eq!(app.state.theme_name, "tokyo-night");
    assert!(app.set_host_terminal_appearance(crate::terminal_theme::HostAppearance::Light, true));

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
    let mut app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

    app.set_host_terminal_appearance(crate::terminal_theme::HostAppearance::Light, true);

    assert_eq!(app.state.theme_name, "gruvbox-light");
    assert_eq!(
        app.state.palette.accent,
        ratatui::style::Color::Rgb(1, 2, 3)
    );
}

#[test]
fn inferred_background_appearance_does_not_override_explicit_report() {
    let mut config = Config::default();
    config.theme.name = Some("catppuccin".to_string());
    config.theme.auto_switch = true;
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

    app.set_host_terminal_appearance(crate::terminal_theme::HostAppearance::Dark, true);
    app.update_host_terminal_theme(
        crate::terminal_theme::DefaultColorKind::Background,
        crate::terminal_theme::RgbColor {
            r: 0xff,
            g: 0xff,
            b: 0xff,
        },
    );

    assert_eq!(
        app.state.host_terminal_appearance,
        Some(crate::terminal_theme::HostAppearance::Dark)
    );
    assert_eq!(app.state.theme_name, "catppuccin");
}

#[test]
fn startup_restores_preview_update_available_from_saved_notes() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("startup-preview-update-available");
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    // Use a bogus far-future version so preview=true regardless of current binary version.
    crate::release_notes::save_pending("99.99.99", "### Changed\n- One").unwrap();

    let app = test_app();

    assert_eq!(app.state.update_available.as_deref(), Some("99.99.99"));
    assert!(app.state.latest_release_notes_available);

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn startup_does_not_restore_update_available_from_older_saved_notes() {
    let _guard = config_env_lock().lock().unwrap();
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
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("startup-pending-release-notes-no-auto-open");
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    crate::release_notes::save_pending(env!("CARGO_PKG_VERSION"), "### Changed\n- One").unwrap();
    let config = Config {
        onboarding: Some(false),
        ..Default::default()
    };
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();

    let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

    assert_eq!(app.state.mode, Mode::Navigate);
    assert!(app.state.release_notes.is_none());
    assert!(app.state.latest_release_notes_available);

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn startup_still_auto_opens_unseen_product_announcement() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("startup-product-announcement-auto-open");
    let state_home = path.parent().unwrap().join("state");
    let original_xdg_state_home = std::env::var_os("XDG_STATE_HOME");
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);
    std::env::set_var("XDG_STATE_HOME", &state_home);

    crate::release_notes::save_pending(env!("CARGO_PKG_VERSION"), "### Changed\n- One").unwrap();
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

    let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

    assert_eq!(app.state.mode, Mode::ProductAnnouncement);
    assert_eq!(
        app.state
            .product_announcement
            .as_ref()
            .map(|announcement| announcement.id.as_str()),
        Some("startup-announcement")
    );
    assert!(app.state.release_notes.is_none());

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    restore_xdg_state_home(original_xdg_state_home);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_updates_live_state() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("reload-config-success");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
            &path,
            "[terminal]\ndefault_shell = \"nu\"\nshell_mode = \"non_login\"\nnew_cwd = \"home\"\n[keys]\nnew_workspace = \"prefix+m\"\nprefix = \"ctrl+a\"\n[update]\nversion_check = false\nmanifest_check = false\n[ui]\nagent_panel_scope = \"current\"\nagent_panel_sort = \"priority\"\nredraw_on_focus_gained = false\nright_click_passthrough_modifier = \"ctrl\"\n[ui.toast]\ndelivery = \"herdr\"\n[experimental]\nswitch_ascii_input_source_in_prefix = true\n",
        )
        .unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    app.next_auto_update_check = Some(Instant::now());
    app.next_agent_manifest_update_check = Some(Instant::now());
    let report = app.reload_config();

    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
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
    assert!(!app.state.redraw_on_focus_gained);
    assert_eq!(
        app.state.right_click_passthrough_modifiers,
        Some(KeyModifiers::CONTROL)
    );
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
    assert!(app.state.switch_ascii_input_source_in_prefix);
    assert!(app.state.config_diagnostic.is_none());
    let toast = app.state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, crate::app::state::ToastKind::UpdateInstalled);
    assert_eq!(toast.title, "reloaded config");
    assert_eq!(toast.context, "using config.toml");

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_updates_sidebar_width_only_when_config_owned() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("reload-config-sidebar-width");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    assert_eq!(
        app.state.sidebar_width_source,
        state::SidebarWidthSource::ConfigDefault
    );

    std::fs::write(&path, "[ui]\nsidebar_width = 34\n").unwrap();
    let report = app.reload_config();
    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert_eq!(app.state.default_sidebar_width, 34);
    assert_eq!(app.state.sidebar_width, 34);

    app.state.sidebar_width = 31;
    app.state.sidebar_width_source = state::SidebarWidthSource::Manual;
    std::fs::write(&path, "[ui]\nsidebar_width = 35\n").unwrap();
    let report = app.reload_config();
    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert_eq!(app.state.default_sidebar_width, 35);
    assert_eq!(app.state.sidebar_width, 31);

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_updates_sidebar_bounds_and_reclamps() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("reload-config-sidebar-bounds");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    // Default bounds.
    assert_eq!(app.state.sidebar_min_width, 18);
    assert_eq!(app.state.sidebar_max_width, 36);
    assert_eq!(
        app.state.mobile_width_threshold,
        crate::config::DEFAULT_MOBILE_WIDTH_THRESHOLD
    );

    // Manually set a width and flip the source so the existing
    // sidebar_width-only-when-config-owned guard does NOT update it.
    app.state.sidebar_width = 30;
    app.state.sidebar_width_source = state::SidebarWidthSource::Manual;

    // Tightening max below the current width must re-clamp the live width
    // even when source is Manual — bounds always apply.
    std::fs::write(&path, "[ui]\nsidebar_max_width = 24\n").unwrap();
    let report = app.reload_config();
    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert_eq!(app.state.sidebar_max_width, 24);
    assert_eq!(
        app.state.sidebar_width, 24,
        "manual width must re-clamp to new max"
    );

    // Loosening max leaves the live width alone (it's already within bounds).
    app.state.sidebar_width = 24;
    std::fs::write(&path, "[ui]\nsidebar_max_width = 60\n").unwrap();
    let report = app.reload_config();
    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert_eq!(app.state.sidebar_max_width, 60);
    assert_eq!(app.state.sidebar_width, 24);

    // Raising min above the current width re-clamps upward.
    std::fs::write(&path, "[ui]\nsidebar_min_width = 30\n").unwrap();
    let report = app.reload_config();
    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert_eq!(app.state.sidebar_min_width, 30);
    assert_eq!(
        app.state.sidebar_width, 30,
        "manual width must re-clamp up to new min"
    );

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_updates_mobile_width_threshold() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("reload-config-mobile-width-threshold");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    assert_eq!(
        app.state.mobile_width_threshold,
        crate::config::DEFAULT_MOBILE_WIDTH_THRESHOLD
    );

    std::fs::write(&path, "[ui]\nmobile_width_threshold = 96\n").unwrap();
    let report = app.reload_config();

    assert_eq!(report.status, crate::config::ConfigReloadStatus::Applied);
    assert_eq!(app.state.mobile_width_threshold, 96);

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn app_new_falls_back_to_default_bounds_on_inverted_config() {
    let mut config = Config::default();
    config.ui.sidebar_min_width = 50;
    config.ui.sidebar_max_width = 30;

    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let app = App::new(&config, true, None, api_rx, crate::api::EventHub::default());

    assert_eq!(
        app.state.sidebar_min_width, 18,
        "App::new must fall back to default min when bounds are inverted"
    );
    assert_eq!(
        app.state.sidebar_max_width, 36,
        "App::new must fall back to default max when bounds are inverted"
    );
}

#[test]
fn reload_config_invalid_sidebar_bounds_keeps_previous_ui_and_returns_partial() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("reload-config-invalid-sidebar-bounds");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    let original_min = app.state.sidebar_min_width;
    let original_max = app.state.sidebar_max_width;
    let original_mouse_capture = app.state.mouse_capture;
    // Pair the bad bounds with another `[ui]` field change to confirm the
    // entire section is treated as invalid (not just the bounds).
    let target_mouse_capture = !original_mouse_capture;
    std::fs::write(
        &path,
        format!(
            "[ui]\nsidebar_min_width = 50\nsidebar_max_width = 30\nmouse_capture = {}\n",
            target_mouse_capture
        ),
    )
    .unwrap();

    let report = app.reload_config();
    assert_eq!(report.status, crate::config::ConfigReloadStatus::Partial);
    assert_eq!(app.state.sidebar_min_width, original_min);
    assert_eq!(app.state.sidebar_max_width, original_max);
    assert_eq!(
        app.state.mouse_capture, original_mouse_capture,
        "[ui] is treated as invalid on bad bounds; mouse_capture must not apply"
    );
    assert!(app
        .state
        .config_diagnostic
        .as_deref()
        .is_some_and(|message| {
            message.contains("sidebar_min_width")
                && message.contains("sidebar_max_width")
                && message.contains("greater")
        }));

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_disables_invalid_binding_but_applies_valid_keymap_and_other_sections() {
    let _guard = config_env_lock().lock().unwrap();
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
    assert_eq!(
        (app.state.prefix_code, app.state.prefix_mods),
        original_prefix
    );
    assert!(app.state.keybinds.new_workspace.bindings.is_empty());
    assert_eq!(
        app.state.toast_config.delivery,
        crate::config::ToastDelivery::Terminal
    );
    assert!(app
        .state
        .config_diagnostic
        .as_deref()
        .is_some_and(|message| {
            message.contains("keys.new_workspace") && message.contains("disabling binding")
        }));

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_user_binding_displaces_default_without_rejecting_prefix() {
    let _guard = config_env_lock().lock().unwrap();
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
    let _guard = config_env_lock().lock().unwrap();
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
    assert!(app
        .state
        .keybinds
        .new_workspace
        .matches_prefix(&KeyEvent::new(KeyCode::Char('m'), KeyModifiers::empty())));
    assert_eq!(
        app.state.toast_config.delivery,
        crate::config::ToastDelivery::Herdr
    );
    assert!(app
        .state
        .config_diagnostic
        .as_deref()
        .is_some_and(|message| message.contains("invalid ui config")));

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_preserves_invalid_terminal_section_but_applies_valid_ui() {
    let _guard = config_env_lock().lock().unwrap();
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
    assert_eq!(app.state.default_shell, original_default_shell);
    assert_eq!(app.state.shell_mode, original_shell_mode);
    assert_eq!(app.state.new_terminal_cwd, original_new_cwd);
    assert_eq!(
        app.state.toast_config.delivery,
        crate::config::ToastDelivery::Terminal
    );
    assert!(app
        .state
        .config_diagnostic
        .as_deref()
        .is_some_and(|message| message.contains("invalid terminal config")));

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn settings_save_toast_delivery_persists_then_applies_live_config() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("settings-save-toast-delivery");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "onboarding = false\n").unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    assert_eq!(
        app.state.toast_config.delivery,
        crate::config::ToastDelivery::Off
    );

    app.save_toast_delivery(crate::config::ToastDelivery::Terminal);

    assert_eq!(
        app.state.toast_config.delivery,
        crate::config::ToastDelivery::Terminal
    );
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("delivery = \"terminal\""));
    assert!(app.state.config_diagnostic.is_none());

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn save_agent_panel_sort_persists_then_applies_live_config() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("save-agent-panel-sort");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "onboarding = false\n").unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    assert_eq!(app.state.agent_panel_sort, state::AgentPanelSort::Spaces);

    app.save_agent_panel_sort(state::AgentPanelSort::Priority);

    assert_eq!(app.state.agent_panel_sort, state::AgentPanelSort::Priority);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("agent_panel_sort = \"priority\""));
    assert!(app.state.config_diagnostic.is_none());

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn settings_save_pane_history_persists_then_applies_live_config() {
    let _guard = config_env_lock().lock().unwrap();
    let path = temp_config_path("settings-save-pane-history");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "onboarding = false\n").unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut app = test_app();
    assert!(!app.persist_pane_history);
    assert!(!app.state.pane_history_persistence);

    app.save_pane_history_persistence(true);

    assert!(app.persist_pane_history);
    assert!(app.state.pane_history_persistence);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[experimental]"));
    assert!(content.contains("pane_history = true"));
    assert!(app.state.config_diagnostic.is_none());

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn reload_config_keeps_current_state_on_invalid_toml() {
    let _guard = config_env_lock().lock().unwrap();
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
            message.contains("config parse error") && message.contains("keeping current config")
        }));
    assert!(app.state.toast.is_none());

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[tokio::test]
async fn raw_input_waits_when_reader_is_gone() {
    let result =
        tokio::time::timeout(Duration::from_millis(20), recv_raw_input_or_pending(None)).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn terminal_mode_handles_repeat_key_events() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;

    let handled = app
        .handle_raw_input_event(raw_key(
            KeyCode::Backspace,
            KeyModifiers::empty(),
            KeyEventKind::Repeat,
        ))
        .await;

    assert!(handled);
}

#[tokio::test]
async fn outer_focus_gained_marks_visible_done_panes_seen() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("test");
    let root_pane = workspace.tabs[0].root_pane;
    let split_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
    let background_tab = workspace.test_add_tab(Some("background"));
    let background_pane = workspace.tabs[background_tab].root_pane;

    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    let root_terminal_id = app.state.workspaces[0].tabs[0].panes[&root_pane]
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&root_terminal_id)
        .unwrap()
        .state = AgentState::Idle;
    app.state.workspaces[0].tabs[0]
        .panes
        .get_mut(&root_pane)
        .unwrap()
        .seen = false;
    let split_terminal_id = app.state.workspaces[0].tabs[0].panes[&split_pane]
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&split_terminal_id)
        .unwrap()
        .state = AgentState::Idle;
    app.state.workspaces[0].tabs[0]
        .panes
        .get_mut(&split_pane)
        .unwrap()
        .seen = false;
    let bg_terminal_id = app.state.workspaces[0].tabs[background_tab].panes[&background_pane]
        .attached_terminal_id
        .clone();
    app.state.terminals.get_mut(&bg_terminal_id).unwrap().state = AgentState::Idle;
    app.state.workspaces[0].tabs[background_tab]
        .panes
        .get_mut(&background_pane)
        .unwrap()
        .seen = false;

    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    app.state.outer_terminal_focus = Some(false);

    let handled = app
        .handle_raw_input_event(crate::raw_input::RawInputEvent::OuterFocusGained)
        .await;

    assert!(handled);
    assert_eq!(app.state.outer_terminal_focus, Some(true));
    assert!(app.state.workspaces[0].tabs[0].panes[&root_pane].seen);
    assert!(app.state.workspaces[0].tabs[0].panes[&split_pane].seen);
    assert!(!app.state.workspaces[0].tabs[background_tab].panes[&background_pane].seen);
}

#[tokio::test]
async fn outer_focus_gained_does_not_require_full_redraw_when_disabled() {
    let mut app = test_app();
    app.state.redraw_on_focus_gained = false;

    let handled = app
        .handle_raw_input_event(crate::raw_input::RawInputEvent::OuterFocusGained)
        .await;

    assert!(handled);
    assert_eq!(app.state.outer_terminal_focus, Some(true));
    assert!(!app.full_redraw_pending);
}

#[tokio::test]
async fn repeat_key_events_are_ignored_outside_terminal_mode() {
    let mut app = test_app();
    app.state.mode = Mode::ReleaseNotes;
    app.state.release_notes = Some(release_notes_state());

    let handled = app
        .handle_raw_input_event(raw_key(
            KeyCode::Enter,
            KeyModifiers::empty(),
            KeyEventKind::Repeat,
        ))
        .await;

    assert!(!handled);
    assert_eq!(app.state.mode, Mode::ReleaseNotes);
    assert!(app.state.release_notes.is_some());
}

#[tokio::test]
async fn modal_press_does_not_leak_repeat_into_terminal_mode() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::ReleaseNotes;
    app.state.release_notes = Some(release_notes_state());

    let press_handled = app
        .handle_raw_input_event(raw_key(
            KeyCode::Enter,
            KeyModifiers::empty(),
            KeyEventKind::Press,
        ))
        .await;
    let repeat_handled = app
        .handle_raw_input_event(raw_key(
            KeyCode::Enter,
            KeyModifiers::empty(),
            KeyEventKind::Repeat,
        ))
        .await;
    let release_handled = app
        .handle_raw_input_event(raw_key(
            KeyCode::Enter,
            KeyModifiers::empty(),
            KeyEventKind::Release,
        ))
        .await;
    let next_press_handled = app
        .handle_raw_input_event(raw_key(
            KeyCode::Enter,
            KeyModifiers::empty(),
            KeyEventKind::Press,
        ))
        .await;

    assert!(press_handled);
    assert_eq!(app.state.mode, Mode::Terminal);
    assert!(!repeat_handled);
    assert!(!release_handled);
    assert!(next_press_handled);
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
        method: crate::api::schema::Method::WorkspaceFocus(crate::api::schema::WorkspaceTarget {
            workspace_id: "w1".into(),
        }),
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

    assert!(!crate::api::request_changes_ui(&read_only));
    assert!(!crate::api::request_changes_ui(&worktree_list));
    assert!(crate::api::request_changes_ui(&mutating));
    assert!(crate::api::request_changes_ui(&pane_rename));
    assert!(crate::api::request_changes_ui(&worktree_create));
    assert!(crate::api::request_changes_ui(&pane_swap));
    assert!(crate::api::request_changes_ui(&pane_focus_direction));
    assert!(crate::api::request_changes_ui(&pane_resize));
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
        method: crate::api::schema::Method::ServerStop(crate::api::schema::EmptyParams::default()),
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
fn terminal_target_resolves_terminal_id() {
    let mut app = test_app();
    let workspace = Workspace::test_new("terminal-target-id");
    let pane = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane).unwrap().to_string();
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;

    let resolved = app.resolve_terminal_target(&terminal_id).unwrap();

    assert_eq!(resolved.ws_idx, 0);
    assert_eq!(resolved.pane_id, pane);
    assert_eq!(resolved.terminal_id, terminal_id);
}

#[test]
fn terminal_target_resolves_legacy_pane_id() {
    let mut app = test_app();
    let workspace = Workspace::test_new("terminal-target-pane");
    let pane = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane).unwrap().to_string();
    app.state.workspaces = vec![workspace];
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

    let crate::app::terminal_targets::TerminalTargetError::Ambiguous { target, candidates } = err
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
async fn pane_split_request_targets_pane_in_background_tab() {
    let _guard = config_env_lock().lock().unwrap();
    let original_shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", exiting_test_command());

    let mut app = test_app();
    let mut workspace = Workspace::test_new("api-pane-split-background-tab");
    let active_pane = workspace.tabs[0].root_pane;
    let background_tab = workspace.test_add_tab(Some("worker"));
    let target_pane = workspace.tabs[background_tab].root_pane;
    workspace.switch_tab(background_tab);
    let background_previous_focus = workspace.test_split(ratatui::layout::Direction::Horizontal);
    workspace.switch_tab(0);
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    let split_cwd = std::env::temp_dir();
    let target_terminal_id = app.state.workspaces[0]
        .pane_state(target_pane)
        .unwrap()
        .attached_terminal_id
        .clone();
    app.state
        .terminals
        .get_mut(&target_terminal_id)
        .unwrap()
        .cwd = split_cwd.clone();
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state
        .focus_pane_in_workspace(0, background_previous_focus);
    app.state.focus_pane_in_workspace(0, active_pane);

    let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;
    let target_tab_id = app.public_tab_id(0, background_tab).unwrap();

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_split_background_tab".into(),
        method: crate::api::schema::Method::PaneSplit(crate::api::schema::PaneSplitParams {
            workspace_id: None,
            target_pane_id: Some(target_pane_id),
            direction: crate::api::schema::SplitDirection::Right,
            ratio: None,
            cwd: None,
            focus: false,
            env: Default::default(),
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "pane_info");
    assert_eq!(response["result"]["pane"]["tab_id"], target_tab_id);
    let response_cwd =
        std::path::PathBuf::from(response["result"]["pane"]["cwd"].as_str().unwrap());
    assert_eq!(
        crate::worktree::canonical_or_original(&response_cwd),
        crate::worktree::canonical_or_original(&split_cwd)
    );
    assert_eq!(response["result"]["pane"]["focused"], false);
    assert_eq!(app.state.active, Some(0));
    assert_eq!(app.state.workspaces[0].active_tab, 0);
    assert_eq!(
        app.state.workspaces[0].tabs[0].layout.focused(),
        active_pane
    );
    assert_eq!(app.state.workspaces[0].tabs[0].layout.pane_count(), 1);
    assert_eq!(
        app.state.workspaces[0].tabs[background_tab]
            .layout
            .focused(),
        background_previous_focus
    );
    assert_eq!(
        app.state.workspaces[0].tabs[background_tab]
            .layout
            .pane_count(),
        3
    );
    app.state.last_pane();
    assert_eq!(app.state.workspaces[0].active_tab, background_tab);
    assert_eq!(
        app.state.workspaces[0].tabs[background_tab]
            .layout
            .focused(),
        background_previous_focus
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
async fn pane_split_request_focuses_new_pane_when_requested() {
    let _guard = config_env_lock().lock().unwrap();
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
    let _guard = config_env_lock().lock().unwrap();
    let original_shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/usr/bin/true");

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
    let _guard = config_env_lock().lock().unwrap();
    let original_shell = std::env::var_os("SHELL");
    std::env::set_var("SHELL", "/usr/bin/true");

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
async fn focused_agent_start_records_previous_pane() {
    let mut app = test_app();
    let workspace = Workspace::test_new("agent-start-focus");
    let root = workspace.tabs[0].root_pane;
    app.state.workspaces = vec![workspace];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 0;

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_agent_start_focus".into(),
        method: crate::api::schema::Method::AgentStart(crate::api::schema::AgentStartParams {
            name: "worker".into(),
            cwd: None,
            workspace_id: None,
            tab_id: None,
            split: Some(crate::api::schema::SplitDirection::Right),
            focus: true,
            argv: vec![exiting_test_command().into()],
            env: Default::default(),
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "agent_started");
    assert_ne!(app.state.workspaces[0].focused_pane_id(), Some(root));

    app.state.last_pane();

    assert_eq!(app.state.active, Some(0));
    assert_eq!(app.state.workspaces[0].focused_pane_id(), Some(root));

    let runtimes: Vec<_> = app.terminal_runtimes.drain().collect();
    for (_terminal_id, runtime) in runtimes {
        runtime.shutdown();
    }
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
fn pane_close_request_requires_confirmation_before_closing_parent_worktree_group() {
    let mut app = test_app();
    let mut parent = Workspace::test_new("api-pane-close-parent");
    parent.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
        key: "repo-key".into(),
        label: "herdr".into(),
        repo_root: "/repo/herdr".into(),
        checkout_path: "/repo/herdr".into(),
        is_linked_worktree: false,
    });
    let mut child = Workspace::test_new("api-pane-close-child");
    child.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
        key: "repo-key".into(),
        label: "herdr".into(),
        repo_root: "/repo/herdr".into(),
        checkout_path: "/repo/herdr-child".into(),
        is_linked_worktree: true,
    });
    app.state.workspaces = vec![parent, child];
    app.state.ensure_test_terminals();
    app.state.active = Some(0);
    app.state.selected = 1;

    let target_pane = app.state.workspaces[0].tabs[0].root_pane;
    let target_pane_id = app.pane_info(0, target_pane).unwrap().pane_id;

    let response = app.handle_api_request(crate::api::schema::Request {
        id: "req_pane_close_parent_group".into(),
        method: crate::api::schema::Method::PaneClose(crate::api::schema::PaneTarget {
            pane_id: target_pane_id,
        }),
    });
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["error"]["code"], "confirmation_required");
    assert_eq!(app.state.mode, Mode::ConfirmClose);
    assert_eq!(app.state.selected, 0);
    assert_eq!(app.state.workspaces.len(), 2);
}

#[test]
fn session_dirty_flag_schedules_debounced_save() {
    let mut app = test_app();
    app.no_session = false;
    app.state.session_dirty = true;

    app.sync_session_save_schedule();

    assert!(!app.state.session_dirty);
    assert!(app.session_save_deadline.is_some());
}

#[test]
fn next_loop_deadline_includes_session_save_deadline() {
    let mut app = test_app();
    let now = Instant::now();
    app.session_save_deadline = Some(now + Duration::from_secs(2));
    app.next_resize_poll = now + Duration::from_secs(5);
    app.next_auto_update_check = Some(now + Duration::from_secs(6));

    assert_eq!(
        app.next_loop_deadline(now, false),
        app.session_save_deadline
    );
}

#[test]
fn headless_next_loop_deadline_ignores_resize_poll() {
    let mut app = test_app();
    let now = Instant::now();
    app.next_resize_poll = now + Duration::from_millis(100);
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
    app.next_resize_poll = now - Duration::from_millis(1);
    app.config_diagnostic_deadline = None;
    app.toast_deadline = None;
    app.next_animation_tick = None;
    app.next_auto_update_check = None;
    app.session_save_deadline = None;
    app.state.workspaces.clear();

    assert_eq!(
        app.next_headless_loop_deadline_with_git_refresh(now, false, true),
        None
    );
}

#[test]
fn due_session_save_deadline_is_cleared() {
    let mut app = test_app();
    app.session_save_deadline = Some(Instant::now() - Duration::from_secs(1));

    app.handle_scheduled_tasks(Instant::now(), false);

    assert!(app.session_save_deadline.is_none());
}

#[test]
fn next_loop_deadline_includes_selection_autoscroll_deadline() {
    let mut app = test_app();
    let now = Instant::now();
    app.next_resize_poll = now + Duration::from_millis(300);
    app.selection_autoscroll_deadline = Some(now + Duration::from_millis(5));
    app.next_animation_tick = Some(now + Duration::from_millis(100));
    app.session_save_deadline = Some(now + Duration::from_millis(200));
    assert_eq!(
        app.next_loop_deadline(now, false),
        app.selection_autoscroll_deadline
    );
}

#[test]
fn tick_selection_autoscroll_self_heals_when_state_cleared() {
    let mut app = test_app();
    let now = Instant::now();
    app.state.selection_autoscroll = None;
    app.selection_autoscroll_deadline = Some(now);
    app.tick_selection_autoscroll(now);
    assert!(app.selection_autoscroll_deadline.is_none());
}

#[test]
fn tick_selection_autoscroll_stops_on_rect_change() {
    let mut app = test_app();
    let now = Instant::now();
    let ws = Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    app.state.workspaces.push(ws);
    app.state.active = Some(0);
    app.state.selection = Some(crate::selection::Selection::anchor(pane_id, 0, 0, None));
    // Set autoscroll with a stale inner_rect that doesn't match pane_infos
    app.state.selection_autoscroll = Some(state::SelectionAutoscroll {
        direction: state::SelectionAutoscrollDirection::Down,
        last_mouse_screen_col: 0,
        last_mouse_screen_row: 999,
        inner_rect: ratatui::layout::Rect::new(0, 0, 1, 1), // wrong rect
    });
    app.selection_autoscroll_deadline = Some(now);
    app.tick_selection_autoscroll(now);
    assert!(app.state.selection_autoscroll.is_none());
    assert!(app.selection_autoscroll_deadline.is_none());
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
                install_command: "bora update".into(),
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

#[test]
fn route_client_input_dispatches_navigate_mode_keybinds() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;

    // Start in navigate mode.
    app.state.mode = Mode::Navigate;

    // Send Ctrl+B then Esc (prefix → leave navigate mode).
    // Ctrl+B is 0x02 in raw terminal input.
    // After entering navigate mode and pressing Esc, we should leave navigate mode.
    let esc_bytes = vec![0x1b]; // Esc
    app.route_client_input(esc_bytes);
    // Esc in navigate mode should leave navigate mode.
    assert_eq!(
        app.state.mode,
        Mode::Terminal,
        "Esc should leave navigate mode and return to Terminal mode"
    );
}

#[test]
fn route_client_input_q_detaches_in_persistence_mode() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.detach_exits = false;

    // Start in navigate mode.
    app.state.mode = Mode::Navigate;
    assert!(!app.state.detach_requested);

    let q_bytes = b"q".to_vec();
    app.route_client_input(q_bytes);

    assert!(
        app.state.detach_requested,
        "q should detach in persistence mode"
    );
    assert_eq!(
        app.state.mode,
        Mode::Terminal,
        "q should leave navigate mode"
    );
}

#[test]
fn route_client_input_prefix_then_q_detaches_in_persistence_mode() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.detach_exits = false;

    // Start in terminal mode (default after workspace creation).
    app.state.mode = Mode::Terminal;
    assert!(!app.state.detach_requested);

    // Send Ctrl+B (prefix key, raw byte 0x02).
    let prefix_bytes = vec![0x02];
    app.route_client_input(prefix_bytes);

    assert_eq!(
        app.state.mode,
        Mode::Prefix,
        "prefix key should enter prefix mode"
    );
    assert!(
        !app.state.detach_requested,
        "prefix key should not set detach flag"
    );

    let q_bytes = b"q".to_vec();
    app.route_client_input(q_bytes);

    assert!(
        app.state.detach_requested,
        "q should detach in persistence mode"
    );
    assert_eq!(
        app.state.mode,
        Mode::Terminal,
        "q should leave navigate mode"
    );
}

#[test]
fn route_client_input_prefix_tab_dispatches_global_last_pane() {
    let config: Config = toml::from_str(
        r#"
[keys]
last_pane = "prefix+tab"
"#,
    )
    .unwrap();
    let mut app = test_app();
    let mut first = Workspace::test_new("one");
    let first_second_tab = first.test_add_tab(Some("logs"));
    let first_second_root = first.tabs[first_second_tab].root_pane;
    let second = Workspace::test_new("two");
    let second_root = second.tabs[0].root_pane;
    app.state.workspaces = vec![first, second];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.keybinds = config.keybinds();
    app.state.mode = Mode::Terminal;
    app.state.switch_workspace_tab(0, first_second_tab);
    app.state.switch_workspace_tab(1, 0);

    app.route_client_input(vec![0x02, b'\t']);

    assert_eq!(app.state.mode, Mode::Terminal);
    assert_eq!(app.state.active, Some(0));
    assert_eq!(app.state.workspaces[0].active_tab, first_second_tab);
    assert_eq!(
        app.state.workspaces[0].focused_pane_id(),
        Some(first_second_root)
    );

    app.route_client_input(vec![0x02, b'\t']);

    assert_eq!(app.state.active, Some(1));
    assert_eq!(app.state.workspaces[1].focused_pane_id(), Some(second_root));
}

#[tokio::test]
async fn route_client_input_double_prefix_passes_prefix_through_to_focused_pane() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("test");
    let focused = workspace.focused_pane_id().unwrap();
    let (runtime, mut rx) = TerminalRuntime::test_with_channel(80, 24);
    workspace.tabs[0].runtimes.insert(focused, runtime);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;
    app.state.prefix_code = KeyCode::Char('l');
    app.state.prefix_mods = KeyModifiers::CONTROL;

    app.route_client_input(vec![0x0c]);
    assert_eq!(app.state.mode, Mode::Prefix);

    app.route_client_input(vec![0x0c]);
    assert_eq!(app.state.mode, Mode::Terminal);
    assert_eq!(rx.recv().await.unwrap(), bytes::Bytes::from(vec![0x0c]));
}

#[tokio::test]
async fn route_client_input_reencodes_terminal_keys_for_focused_pane_protocol() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("test");
    let focused = workspace.focused_pane_id().unwrap();
    let (runtime, mut rx) = TerminalRuntime::test_with_channel(80, 24);
    workspace.tabs[0].runtimes.insert(focused, runtime);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;

    // Ghostty/kitty-style Ctrl-C should be normalized back to the pane's
    // negotiated encoding instead of being forwarded verbatim.
    app.route_client_input(b"\x1b[99;5u".to_vec());

    assert_eq!(rx.recv().await.unwrap(), bytes::Bytes::from(vec![3]));

    // iTerm2 and rxvt-style hosts may send F4 as CSI 14~. Normalize it
    // through the same semantic key path instead of leaking host bytes.
    app.route_client_input(b"\x1b[14~".to_vec());

    assert_eq!(
        rx.recv().await.unwrap(),
        bytes::Bytes::from_static(b"\x1bOS")
    );
}

#[tokio::test]
async fn route_client_input_preserves_shift_enter_for_modify_other_keys_pane() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("test");
    let focused = workspace.focused_pane_id().unwrap();
    let (runtime, mut rx) =
        TerminalRuntime::test_with_channel_and_scrollback_bytes(80, 24, 0, b"\x1b[>4;1m", 4);
    workspace.tabs[0].runtimes.insert(focused, runtime);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;

    app.route_client_input(b"\x1b[13;2u".to_vec());

    assert_eq!(
        rx.recv().await.unwrap(),
        bytes::Bytes::from_static(b"\x1b[27;2;13~")
    );
}

#[tokio::test]
async fn route_client_input_splits_multi_event_payloads_before_forwarding() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("test");
    let focused = workspace.focused_pane_id().unwrap();
    let (runtime, mut rx) = TerminalRuntime::test_with_channel(80, 24);
    workspace.tabs[0].runtimes.insert(focused, runtime);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;

    app.route_client_input(b"ab".to_vec());

    assert_eq!(rx.recv().await.unwrap(), bytes::Bytes::from_static(b"a"));
    assert_eq!(rx.recv().await.unwrap(), bytes::Bytes::from_static(b"b"));
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn route_client_input_forwards_multilingual_ime_text_to_focused_pane() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("test");
    let focused = workspace.focused_pane_id().unwrap();
    let text = "中日한🙂";
    let (runtime, mut rx) =
        TerminalRuntime::test_with_channel_capacity(80, 24, text.chars().count());
    workspace.tabs[0].runtimes.insert(focused, runtime);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;

    app.route_client_input(text.as_bytes().to_vec());

    let mut forwarded = Vec::new();
    for _ in text.chars() {
        let chunk = rx.recv().await.unwrap();
        forwarded.extend_from_slice(&chunk);
    }
    assert_eq!(forwarded, text.as_bytes());
    assert!(rx.try_recv().is_err());
}

#[tokio::test]
async fn route_client_input_forwards_long_voice_like_cjk_text_without_truncation() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("test");
    let focused = workspace.focused_pane_id().unwrap();
    let text = "你好，今天我们测试一段比较长的语音输入。こんにちは。안녕하세요.🙂".repeat(64);
    let char_count = text.chars().count();
    let (runtime, mut rx) = TerminalRuntime::test_with_channel_capacity(80, 24, char_count);
    workspace.tabs[0].runtimes.insert(focused, runtime);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;

    app.route_client_input(text.as_bytes().to_vec());

    let mut forwarded = Vec::new();
    for _ in 0..char_count {
        let chunk = rx.recv().await.unwrap();
        forwarded.extend_from_slice(&chunk);
    }
    assert_eq!(forwarded, text.as_bytes());
    assert!(rx.try_recv().is_err());
}

#[test]
fn route_client_input_handles_mouse_events() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;

    // Send a mouse scroll-up event via SGR encoding.
    let mouse_bytes = b"\x1b[<64;10;5M".to_vec();
    // This should not panic even though mouse handling is simplified
    // in headless mode.
    app.route_client_input(mouse_bytes);
    // No assertions on specific behavior — just no panic.
}

#[test]
fn route_client_input_advances_onboarding_modal() {
    let mut app = test_app();
    app.state.mode = Mode::Onboarding;

    app.route_client_input(b"\r".to_vec());

    assert_eq!(app.state.mode, Mode::Settings);
    assert_eq!(
        app.state.settings.section,
        state::SettingsSection::Integrations
    );
}

#[test]
fn route_client_input_pastes_bracketed_text_into_rename_modal() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::RenameTab;
    app.state.name_input = "2".into();
    app.state.name_input_replace_on_type = true;

    app.route_client_input(b"\x1b[200~feature/logs\x1b[201~".to_vec());

    assert_eq!(app.state.name_input, "feature/logs");
    assert!(!app.state.name_input_replace_on_type);
}

#[test]
fn raw_ctrl_v_decodes_as_modal_paste_shortcut() {
    let events = crate::raw_input::parse_raw_input_bytes_sync(&[0x16]);
    let Some(crate::raw_input::RawInputEvent::Key(key)) = events.first() else {
        panic!("expected ctrl-v key event");
    };

    assert!(input::is_modal_paste_shortcut(&key.as_key_event()));
}

#[test]
fn route_client_events_pastes_text_into_new_linked_worktree_modal() {
    let mut app = test_app();
    app.state.mode = Mode::NewLinkedWorktree;
    app.state.name_input = "generated-branch".into();
    app.state.name_input_replace_on_type = true;
    app.state.worktree_create = Some(state::WorktreeCreateState {
        source_workspace_id: "source".into(),
        source_checkout_path: "/repo/herdr".into(),
        source_existing_membership: None,
        source_repo_root: "/repo/herdr".into(),
        repo_key: "repo-key".into(),
        repo_name: "herdr".into(),
        branch: "generated-branch".into(),
        checkout_path: "/repo/herdr-generated-branch".into(),
        error: None,
        creating: false,
    });

    app.route_client_events(
        vec![crate::raw_input::RawInputEvent::Paste(
            "feature/linear-302".into(),
        )],
        true,
    );

    assert_eq!(app.state.name_input, "feature/linear-302");
    assert_eq!(
        app.state
            .worktree_create
            .as_ref()
            .map(|create| create.branch.as_str()),
        Some("feature/linear-302")
    );
}

#[test]
fn route_client_input_closes_release_notes_modal() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::ReleaseNotes;
    app.state.release_notes = Some(release_notes_state());

    app.route_client_input(b"\x1b".to_vec());

    assert_eq!(app.state.mode, Mode::Terminal);
    assert!(app.state.release_notes.is_none());
}

#[test]
fn route_client_input_closes_settings_modal() {
    let mut app = test_app();
    app.state.workspaces = vec![Workspace::test_new("test")];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Settings;
    app.state.settings.original_theme = Some(app.state.theme_name.clone());
    app.state.settings.original_palette = Some(app.state.palette.clone());

    app.route_client_input(b"\x1b".to_vec());

    assert_eq!(app.state.mode, Mode::Terminal);
}

#[test]
fn route_client_input_updates_host_terminal_theme_from_osc_response() {
    let mut app = test_app();

    app.route_client_input(b"\x1b]11;#123456\x07".to_vec());

    assert_eq!(
        app.state.host_terminal_theme.background,
        Some(crate::terminal_theme::RgbColor {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        })
    );
}

#[tokio::test]
async fn route_client_input_does_not_forward_incomplete_osc_introducer_to_pane() {
    let mut app = test_app();
    let mut workspace = Workspace::test_new("test");
    let focused = workspace.focused_pane_id().unwrap();
    let (runtime, mut rx) = TerminalRuntime::test_with_channel_capacity(80, 24, 1);
    workspace.tabs[0].runtimes.insert(focused, runtime);
    app.state.workspaces = vec![workspace];
    app.state.active = Some(0);
    app.state.selected = 0;
    app.state.mode = Mode::Terminal;

    app.route_client_input(b"\x1b]".to_vec());

    assert!(rx.try_recv().is_err());
}

#[test]
fn parse_raw_input_bytes_with_ranges_tracks_offsets() {
    // Verify that the range-aware parser correctly tracks byte offsets
    // for events within a multi-event input buffer.
    let input = b"\x1b[Aa".to_vec(); // Up arrow + 'a'
    let events = crate::raw_input::parse_raw_input_bytes_with_ranges(&input);

    assert_eq!(events.len(), 2, "should parse Up arrow and 'a'");
    // Up arrow: \x1b[A = 3 bytes starting at offset 0
    assert_eq!(events[0].start, 0);
    assert_eq!(events[0].len, 3);
    // 'a': 1 byte starting at offset 3
    assert_eq!(events[1].start, 3);
    assert_eq!(events[1].len, 1);

    // Verify the raw bytes for each event are correct.
    assert_eq!(
        &input[events[0].start..events[0].start + events[0].len],
        b"\x1b[A"
    );
    assert_eq!(
        &input[events[1].start..events[1].start + events[1].len],
        b"a"
    );
}
