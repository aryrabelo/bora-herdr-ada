use super::*;

use crate::app::AppState;
use crate::protocol::CursorState;
use crate::protocol::RenderEncoding;
use crate::server::client_transport::ClientWriter;
use std::fs;

fn test_headless_server() -> HeadlessServer {
    let config = crate::config::Config::default();
    let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = crate::app::App::new(&config, true, None, api_rx, api::EventHub::default());
    app.state.local_sound_playback = false;
    app.local_terminal_notifications = false;

    let dir = std::env::temp_dir().join(format!(
        "hh-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::create_dir_all(&dir);
    let socket_path = dir.join("client.sock");
    let _ = fs::remove_file(&socket_path);
    let listener = bind_local_listener(&socket_path).expect("bind test listener");
    let client_socket_identity =
        socket_file_identity(&socket_path).expect("test listener socket identity");
    #[cfg(unix)]
    listener
        .set_nonblocking(ListenerNonblockingMode::Accept)
        .expect("set listener nonblocking");
    let (server_event_tx, server_event_rx) = mpsc::channel(64);
    #[cfg(windows)]
    let should_quit = Arc::new(AtomicBool::new(false));
    #[cfg(windows)]
    spawn_windows_client_accept_thread(listener, should_quit.clone(), server_event_tx.clone());
    let server_keybindings = app_keybindings(&app);

    HeadlessServer {
        app,
        #[cfg(unix)]
        api_tx: None,
        #[cfg(unix)]
        api_server: None,
        #[cfg(unix)]
        client_listener: listener,
        client_socket_path: socket_path,
        client_socket_identity,
        clients: HashMap::new(),
        #[cfg(unix)]
        next_client_id: 1,
        foreground_client_id: None,
        server_keybindings,
        server_config_diagnostic: None,
        server_config_diagnostic_without_keybindings: None,
        terminal_attach_owners: HashMap::new(),
        next_activity_stamp: 1,
        effective_size: (MIN_COLS, MIN_ROWS),
        shutting_down: false,
        handoff_in_progress: false,
        #[cfg(unix)]
        pending_handoff_repaint_nudge: false,
        #[cfg(unix)]
        should_quit: Arc::new(AtomicBool::new(false)),
        #[cfg(windows)]
        should_quit,
        server_event_rx,
        server_event_tx,
    }
}

fn read_server_message(bytes: Vec<u8>) -> ServerMessage {
    let mut cursor = std::io::Cursor::new(bytes);
    protocol::read_message(&mut cursor, MAX_FRAME_SIZE).expect("decode server message")
}

fn read_server_frame(bytes: Vec<u8>) -> FrameData {
    match read_server_message(bytes) {
        ServerMessage::Frame(frame) => frame,
        other => panic!("expected frame, got {other:?}"),
    }
}

fn frame_text(frame: &FrameData) -> String {
    frame
        .cells
        .chunks(usize::from(frame.width))
        .map(|row| {
            row.iter()
                .map(|cell| cell.symbol.as_str())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn read_server_shutdown_reason(bytes: Vec<u8>) -> Option<String> {
    match read_server_message(bytes) {
        ServerMessage::ServerShutdown { reason } => reason,
        other => panic!("expected shutdown, got {other:?}"),
    }
}

#[test]
fn headless_api_request_drains_all_pending_internal_events_before_reading_state() {
    let mut server = test_headless_server();
    for i in 0..=crate::app::APP_EVENT_DRAIN_LIMIT {
        server
            .app
            .event_tx
            .try_send(AppEvent::UpdateReady {
                version: format!("4.0.{i}"),
                install_command: "herdr install".into(),
            })
            .unwrap();
    }

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    assert!(
        server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
            request: api::schema::Request {
                id: "headless_stop_after_events".into(),
                method: api::schema::Method::ServerStop(api::schema::EmptyParams::default()),
            },
            respond_to,
        })
    );
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();

    assert_eq!(response["result"]["type"], "ok");
    let expected_version = format!("4.0.{}", crate::app::APP_EVENT_DRAIN_LIMIT);
    assert_eq!(
        server.app.state.update_available.as_deref(),
        Some(expected_version.as_str())
    );
    assert!(server.app.event_rx.try_recv().is_err());
}

fn test_client_writer() -> (
    ClientWriter,
    std::sync::mpsc::Receiver<Vec<u8>>,
    std::sync::mpsc::Receiver<Vec<u8>>,
) {
    let (control_tx, control_rx) = std::sync::mpsc::channel();
    let (render_tx, render_rx) = std::sync::mpsc::sync_channel(1);
    (
        ClientWriter::test_channel(control_tx, render_tx),
        control_rx,
        render_rx,
    )
}

fn retained_test_server(
    initial_screen: &[u8],
) -> (
    HeadlessServer,
    std::sync::mpsc::Receiver<Vec<u8>>,
    crate::layout::PaneId,
) {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("test");
    let pane_id = workspace.focused_pane_id().expect("focused pane");
    workspace.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, initial_screen),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;

    let (client_tx, _client_control_rx, client_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    server.resize_shared_runtime_to_effective_size();

    (server, client_rx, pane_id)
}

fn assert_frame_data_eq(actual: &FrameData, expected: &FrameData) {
    assert_eq!(
        (actual.width, actual.height),
        (expected.width, expected.height)
    );
    assert_eq!(actual.cursor, expected.cursor, "cursor mismatch");
    assert_eq!(actual.hyperlinks, expected.hyperlinks, "hyperlink mismatch");
    assert_eq!(actual.graphics, expected.graphics, "graphics mismatch");
    assert_eq!(
        actual.cells.len(),
        expected.cells.len(),
        "cell length mismatch"
    );
    for (idx, (actual_cell, expected_cell)) in
        actual.cells.iter().zip(expected.cells.iter()).enumerate()
    {
        assert_eq!(
            actual_cell,
            expected_cell,
            "cell mismatch at index {idx} (x={}, y={})",
            idx % usize::from(actual.width),
            idx / usize::from(actual.width),
        );
    }
}

#[test]
fn foreground_client_applies_client_keybindings() {
    let mut server = test_headless_server();
    let local_config: crate::config::Config = toml::from_str(
        r#"
[keys]
prefix = "ctrl+a"
new_tab = "prefix+t"
"#,
    )
    .unwrap();
    let local_keybindings = local_config.live_keybinds().unwrap();
    let (writer_a, _control_a, _render_a) = test_client_writer();
    let (writer_b, _control_b, _render_b) = test_client_writer();

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 1,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::SemanticFrame,
        keybindings: Some(Box::new(local_keybindings)),
        direct_attach_requested: false,
        writer: writer_a,
    }));
    assert_eq!(
        server.app.state.prefix_code,
        crossterm::event::KeyCode::Char('a')
    );
    assert!(server
        .app
        .state
        .keybinds
        .new_tab
        .bindings
        .iter()
        .any(|binding| binding.label == "prefix+t"));

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 2,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::SemanticFrame,
        keybindings: None,
        direct_attach_requested: false,
        writer: writer_b,
    }));
    assert_eq!(
        server.app.state.prefix_code,
        crossterm::event::KeyCode::Char('b')
    );
    assert!(server
        .app
        .state
        .keybinds
        .new_tab
        .bindings
        .iter()
        .any(|binding| binding.label == "prefix+c"));
}

#[test]
fn local_keybinding_client_hides_server_keybinding_warnings() {
    let mut server = test_headless_server();
    let diagnostics = vec![
        "unsafe direct keybinding: keys.close_pane = \"x\" would intercept typing".to_owned(),
        "theme warning".to_owned(),
    ];
    let (full, without_keybindings) = server_config_diagnostic_summaries(&diagnostics);
    server.server_config_diagnostic = full.clone();
    server.server_config_diagnostic_without_keybindings = without_keybindings.clone();
    server.app.state.config_diagnostic = full;
    let local_keybindings = crate::config::Config::default().live_keybinds().unwrap();
    let (writer_a, _control_a, _render_a) = test_client_writer();
    let (writer_b, _control_b, _render_b) = test_client_writer();

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 1,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::SemanticFrame,
        keybindings: Some(Box::new(local_keybindings)),
        direct_attach_requested: false,
        writer: writer_a,
    }));
    assert_eq!(server.app.state.config_diagnostic, without_keybindings);

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 2,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::SemanticFrame,
        keybindings: None,
        direct_attach_requested: false,
        writer: writer_b,
    }));
    assert_eq!(
        server.app.state.config_diagnostic,
        server.server_config_diagnostic
    );
}

#[test]
fn local_keybinding_client_keeps_local_keybindings_after_settings_save() {
    let path = std::env::temp_dir().join(format!(
        "herdr-headless-settings-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(&path, "onboarding = false\n").unwrap();
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut server = test_headless_server();
    let local_config: crate::config::Config = toml::from_str(
        r#"
[keys]
prefix = "ctrl+a"
new_workspace = "prefix+n"
next_tab = ""
"#,
    )
    .unwrap();
    let local_keybindings = local_config.live_keybinds().unwrap();
    let (writer, _control, _render) = test_client_writer();
    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 1,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::SemanticFrame,
        keybindings: Some(Box::new(local_keybindings)),
        direct_attach_requested: false,
        writer,
    }));
    server.app.state.mode = crate::app::Mode::Settings;
    server.app.state.settings.section = crate::app::state::SettingsSection::Toast;
    server.app.state.settings.list.selected = 1;

    assert!(server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\r".to_vec(),
    }));

    assert_eq!(
        server.app.state.prefix_code,
        crossterm::event::KeyCode::Char('a')
    );
    assert!(server
        .app
        .state
        .keybinds
        .new_workspace
        .bindings
        .iter()
        .any(|binding| binding.label == "prefix+n"));
    assert!(server.app.state.toast.is_none());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("delivery = \"herdr\""));

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_file(path);
}

#[test]
fn invalid_server_keybindings_apply_valid_subset_after_settings_save_without_caching_local_keybindings(
) {
    let path = std::env::temp_dir().join(format!(
        "herdr-headless-invalid-settings-{}-{}.toml",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::write(
        &path,
        "onboarding = false\n[keys]\nnew_workspace = \"x\"\n[ui.toast]\ndelivery = \"off\"\n",
    )
    .unwrap();
    let _guard = crate::config::test_config_env_lock().lock().unwrap();
    std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);

    let mut server = test_headless_server();
    let previous_server_config: crate::config::Config =
        toml::from_str("[keys]\nprefix = \"ctrl+c\"\nnew_workspace = \"prefix+m\"\n").unwrap();
    server.server_keybindings = previous_server_config.live_keybinds().unwrap();
    let local_config: crate::config::Config = toml::from_str(
        r#"
[keys]
prefix = "ctrl+a"
new_workspace = "prefix+n"
next_tab = ""
"#,
    )
    .unwrap();
    let (writer_a, _control_a, _render_a) = test_client_writer();
    let (writer_b, _control_b, _render_b) = test_client_writer();

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 1,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::SemanticFrame,
        keybindings: Some(Box::new(local_config.live_keybinds().unwrap())),
        direct_attach_requested: false,
        writer: writer_a,
    }));
    server.app.state.mode = crate::app::Mode::Settings;
    server.app.state.settings.section = crate::app::state::SettingsSection::Toast;
    server.app.state.settings.list.selected = 1;

    assert!(server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\r".to_vec(),
    }));

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 2,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::SemanticFrame,
        keybindings: None,
        direct_attach_requested: false,
        writer: writer_b,
    }));
    assert_eq!(
        server.app.state.prefix_code,
        crossterm::event::KeyCode::Char('b')
    );
    assert!(!server
        .app
        .state
        .keybinds
        .new_workspace
        .bindings
        .iter()
        .any(|binding| binding.label == "prefix+n"));
    assert!(server.app.state.keybinds.new_workspace.bindings.is_empty());

    std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
    let _ = std::fs::remove_file(path);
}

#[test]
fn terminal_attach_rejects_missing_terminal_and_removes_client() {
    let mut server = test_headless_server();
    let (writer, control_rx, _render_rx) = test_client_writer();

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 7,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::TerminalAnsi,
        keybindings: None,
        direct_attach_requested: true,
        writer,
    }));
    assert!(server.clients.contains_key(&7));

    assert!(
        !server.handle_server_event(ServerEvent::ClientAttachTerminal {
            client_id: 7,
            terminal_id: "term_missing".to_owned(),
            takeover: false,
        })
    );
    assert!(!server.clients.contains_key(&7));
    let reason = read_server_shutdown_reason(control_rx.recv().expect("shutdown message"));
    assert_eq!(
        reason,
        Some("terminal attach failed: terminal term_missing not found".to_owned())
    );
}

fn app_client_marks_git_refresh_due_on_first_attach(render_encoding: RenderEncoding) {
    let mut server = test_headless_server();
    server
        .app
        .state
        .workspaces
        .push(crate::workspace::Workspace::test_new("test"));
    let future = Instant::now() + Duration::from_secs(60);
    server.app.last_git_remote_status_refresh = future;
    let (writer, _control_rx, _render_rx) = test_client_writer();

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 7,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding,
        keybindings: None,
        direct_attach_requested: false,
        writer,
    }));

    assert!(server.has_app_client());
    assert!(server
        .app
        .git_refresh_deadline()
        .is_some_and(|deadline| deadline <= Instant::now()));
}

#[test]
fn terminal_ansi_app_client_enables_headless_git_refresh() {
    app_client_marks_git_refresh_due_on_first_attach(RenderEncoding::TerminalAnsi);
}

#[test]
fn pending_terminal_attach_client_does_not_enable_headless_git_refresh() {
    let mut server = test_headless_server();
    server
        .app
        .state
        .workspaces
        .push(crate::workspace::Workspace::test_new("test"));
    let (writer, _control_rx, _render_rx) = test_client_writer();

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 7,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::TerminalAnsi,
        keybindings: None,
        direct_attach_requested: true,
        writer,
    }));

    assert!(!server.has_app_client());
    assert_eq!(
        server.app.next_headless_loop_deadline_with_git_refresh(
            Instant::now(),
            false,
            server.has_app_client()
        ),
        None
    );
}

#[test]
fn writerless_app_client_does_not_enable_headless_git_refresh() {
    let mut server = test_headless_server();
    server
        .app
        .state
        .workspaces
        .push(crate::workspace::Workspace::test_new("test"));
    let (writer, _control_rx, _render_rx) = test_client_writer();

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 7,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::SemanticFrame,
        keybindings: None,
        direct_attach_requested: false,
        writer,
    }));
    assert!(server.has_app_client());

    server.clients.get_mut(&7).expect("client").writer = None;

    assert!(!server.has_app_client());
    assert_eq!(
        server.app.next_headless_loop_deadline_with_git_refresh(
            Instant::now(),
            false,
            server.has_app_client()
        ),
        None
    );
}

#[test]
fn semantic_app_client_marks_git_refresh_due_on_first_attach() {
    app_client_marks_git_refresh_due_on_first_attach(RenderEncoding::SemanticFrame);
}

#[test]
fn terminal_attach_client_exits_when_attached_pane_dies() {
    let mut server = test_headless_server();
    let workspace = crate::workspace::Workspace::test_new("attached");
    let pane_id = workspace.tabs[0].root_pane;
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    let terminal_id = server.app.state.workspaces[0]
        .pane_state(pane_id)
        .expect("pane")
        .attached_terminal_id
        .to_string();
    let (writer, control_rx, _render_rx) = test_client_writer();

    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 7,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::TerminalAnsi,
        keybindings: None,
        direct_attach_requested: true,
        writer,
    }));
    assert!(
        server.handle_server_event(ServerEvent::ClientAttachTerminal {
            client_id: 7,
            terminal_id: terminal_id.clone(),
            takeover: false,
        })
    );
    assert_eq!(server.terminal_attach_owners.get(&terminal_id), Some(&7));

    assert!(server.handle_internal_event_with_forwarding(AppEvent::PaneDied { pane_id }));

    assert!(!server.clients.contains_key(&7));
    assert!(!server.terminal_attach_owners.contains_key(&terminal_id));
    let reason = read_server_shutdown_reason(control_rx.recv().expect("shutdown message"));
    assert_eq!(reason, Some(format!("terminal {terminal_id} exited")));
}

#[test]
fn terminal_attach_scroll_moves_attached_runtime_viewport() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut bytes = Vec::new();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let runtime = crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 4096, &bytes);

    apply_terminal_attach_scroll(
        &runtime,
        AttachScrollSource::Wheel,
        AttachScrollDirection::Up,
        3,
        None,
        None,
        0,
    )
    .expect("scroll up");
    let metrics = runtime.scroll_metrics().expect("scroll metrics");
    assert_eq!(metrics.offset_from_bottom, 3);

    apply_terminal_attach_scroll(
        &runtime,
        AttachScrollSource::Wheel,
        AttachScrollDirection::Down,
        2,
        None,
        None,
        0,
    )
    .expect("scroll down");
    let metrics = runtime.scroll_metrics().expect("scroll metrics");
    assert_eq!(metrics.offset_from_bottom, 1);
    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn terminal_attach_input_resets_scrolled_viewport() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut bytes = Vec::new();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20, 5, 4096, &bytes, 4,
        );

    runtime.scroll_up(4);
    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        4
    );

    apply_terminal_attach_input(&runtime, b"x".to_vec()).expect("attach input");
    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        0
    );
    assert_eq!(
        input_rx.try_recv().expect("forwarded input"),
        Bytes::from("x")
    );

    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn terminal_attach_page_key_host_scrolls_plain_terminal() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut bytes = Vec::new();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20, 5, 4096, &bytes, 4,
        );

    apply_terminal_attach_scroll(
        &runtime,
        AttachScrollSource::PageKey {
            input: b"\x1b[5~".to_vec(),
        },
        AttachScrollDirection::Up,
        4,
        None,
        None,
        0,
    )
    .expect("page key scroll");

    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        4
    );
    assert!(input_rx.try_recv().is_err());
    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn terminal_attach_page_key_forwards_when_mouse_reporting() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut bytes = b"\x1b[?1000h".to_vec();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20, 5, 4096, &bytes, 4,
        );
    runtime.scroll_up(3);

    apply_terminal_attach_scroll(
        &runtime,
        AttachScrollSource::PageKey {
            input: b"\x1b[5~".to_vec(),
        },
        AttachScrollDirection::Up,
        4,
        None,
        None,
        0,
    )
    .expect("page key forward");

    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        0
    );
    assert_eq!(
        input_rx.try_recv().expect("forwarded page key"),
        Bytes::from_static(b"\x1b[5~")
    );
    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn terminal_attach_page_key_forwards_in_alternate_screen_without_mouse_reporting() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut bytes = b"\x1b[?1049h".to_vec();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let (runtime, mut input_rx) =
        crate::terminal::TerminalRuntime::test_with_channel_and_scrollback_bytes(
            20, 5, 4096, &bytes, 4,
        );
    runtime.scroll_up(3);

    apply_terminal_attach_scroll(
        &runtime,
        AttachScrollSource::PageKey {
            input: b"\x1b[5~".to_vec(),
        },
        AttachScrollDirection::Up,
        4,
        None,
        None,
        0,
    )
    .expect("page key forward");

    assert_eq!(
        runtime
            .scroll_metrics()
            .expect("scroll metrics")
            .offset_from_bottom,
        0
    );
    assert_eq!(
        input_rx.try_recv().expect("forwarded page key"),
        Bytes::from_static(b"\x1b[5~")
    );
    drop(runtime);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn headless_scheduled_tasks_expire_agent_metadata() {
    let mut server = test_headless_server();
    let workspace = crate::workspace::Workspace::test_new("metadata");
    let pane_id = workspace.tabs[0].root_pane;
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();

    assert!(
        server.handle_internal_event_with_forwarding(AppEvent::HookStateReported {
            pane_id,
            source: "herdr:pi".into(),
            agent_label: "pi".into(),
            state: crate::detect::AgentState::Working,
            message: None,
            custom_status: None,
            seq: None,
            session_ref: None,
        })
    );
    assert!(
        server.handle_internal_event_with_forwarding(AppEvent::HookMetadataReported {
            pane_id,
            source: "user:pi-display".into(),
            agent_label: Some("pi".into()),
            applies_to_source: Some("herdr:pi".into()),
            title: None,
            display_agent: None,
            custom_status: Some("short lived".into()),
            state_labels: HashMap::new(),
            clear_title: false,
            clear_display_agent: false,
            clear_custom_status: false,
            clear_state_labels: false,
            seq: None,
            ttl: Some(Duration::from_millis(1)),
        })
    );

    let deadline = server
        .app
        .agent_metadata_deadline
        .expect("metadata deadline");
    let terminal_id = server.app.state.workspaces[0]
        .pane_state(pane_id)
        .expect("pane")
        .attached_terminal_id
        .clone();
    assert_eq!(
        server
            .app
            .state
            .terminals
            .get(&terminal_id)
            .expect("terminal")
            .effective_custom_status()
            .as_deref(),
        Some("short lived")
    );

    assert!(server.handle_scheduled_tasks_headless(deadline + Duration::from_millis(1), false));

    assert_eq!(server.app.agent_metadata_deadline, None);
    assert_eq!(
        server
            .app
            .state
            .terminals
            .get(&terminal_id)
            .expect("terminal")
            .effective_custom_status(),
        None
    );
    assert!(server
        .app
        .event_hub
        .events_after(0)
        .iter()
        .any(|(_, event)| {
            event.event == crate::api::schema::EventKind::PaneAgentStatusChanged
                && matches!(
                    &event.data,
                    crate::api::schema::EventData::PaneAgentStatusChanged {
                        custom_status,
                        ..
                    } if custom_status.is_none()
                )
        }));
}

#[test]
fn headless_scheduled_tasks_clears_disabled_agent_manifest_update_deadline() {
    let mut server = test_headless_server();
    let now = Instant::now();
    server.app.next_agent_manifest_update_check = Some(now - Duration::from_millis(1));

    assert!(!server.handle_scheduled_tasks_headless(now, false));
    assert_eq!(server.app.next_agent_manifest_update_check, None);
}

#[tokio::test]
async fn headless_scheduled_tasks_do_not_start_pending_agent_resume_when_geometry_dirty() {
    let mut server = test_headless_server();
    let workspace = crate::workspace::Workspace::test_new("restored");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    server.app.state.view.pane_infos = workspace.tabs[0]
        .layout
        .panes(ratatui::layout::Rect::new(0, 0, 100, 30));
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.ensure_test_terminals();
    server.clients.insert(
        1,
        ClientConnection::new(
            (100, 30),
            crate::kitty_graphics::HostCellSize::default(),
            server.app.state.host_terminal_theme,
            Some(true),
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.effective_size = (100, 30);
    server.app.state.host_terminal_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
    };
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal should exist")
        .pending_agent_resume_plan = Some(crate::agent_resume::AgentResumePlan {
        agent: "codex".into(),
        argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
        dedupe_key: "herdr:codex\0codex\0Id\0codex-session".into(),
    });
    server.app.pending_agent_resume_deadline = Some(Instant::now() - Duration::from_millis(1));

    assert!(!server.handle_scheduled_tasks_headless(Instant::now(), true));
    assert!(server.app.terminal_runtimes.get(&terminal_id).is_none());
    assert!(server
        .app
        .state
        .terminals
        .get(&terminal_id)
        .expect("test terminal should still exist")
        .pending_agent_resume_plan
        .is_some());
    assert!(server.app.pending_agent_resume_deadline.is_none());
}

#[tokio::test]
async fn headless_scheduled_tasks_do_not_start_pending_agent_resume_without_foreground_client() {
    let mut server = test_headless_server();
    let workspace = crate::workspace::Workspace::test_new("restored");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    server.app.state.view.pane_infos = workspace.tabs[0]
        .layout
        .panes(ratatui::layout::Rect::new(0, 0, 80, 24));
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.ensure_test_terminals();
    server.foreground_client_id = None;
    server.effective_size = (80, 24);
    server.app.state.host_terminal_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
    };
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal should exist")
        .pending_agent_resume_plan = Some(crate::agent_resume::AgentResumePlan {
        agent: "codex".into(),
        argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
        dedupe_key: "herdr:codex\0codex\0Id\0codex-session".into(),
    });
    server.app.pending_agent_resume_deadline = Some(Instant::now() - Duration::from_millis(1));

    assert!(!server.handle_scheduled_tasks_headless(Instant::now(), false));
    assert!(server.app.terminal_runtimes.get(&terminal_id).is_none());
    assert!(server
        .app
        .state
        .terminals
        .get(&terminal_id)
        .expect("test terminal should still exist")
        .pending_agent_resume_plan
        .is_some());
    assert!(server.app.pending_agent_resume_deadline.is_none());
}

#[tokio::test]
async fn headless_pre_input_resize_does_not_start_pending_agent_resume() {
    let mut server = test_headless_server();
    let workspace = crate::workspace::Workspace::test_new("restored");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).cloned().unwrap();
    server.app.state.view.pane_infos = workspace.tabs[0]
        .layout
        .panes(ratatui::layout::Rect::new(0, 0, 100, 30));
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.ensure_test_terminals();
    server.clients.insert(
        1,
        ClientConnection::new(
            (100, 30),
            crate::kitty_graphics::HostCellSize::default(),
            server.app.state.host_terminal_theme,
            Some(true),
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.effective_size = (100, 30);
    server.app.state.host_terminal_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 220,
            g: 220,
            b: 220,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 20,
            g: 20,
            b: 20,
        }),
    };
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .expect("test terminal should exist")
        .pending_agent_resume_plan = Some(crate::agent_resume::AgentResumePlan {
        agent: "codex".into(),
        argv: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
        dedupe_key: "herdr:codex\0codex\0Id\0codex-session".into(),
    });
    server.app.pending_agent_resume_deadline = Some(Instant::now() - Duration::from_millis(1));

    server.resize_shared_runtime_to_effective_size_before_input();

    assert!(server.app.terminal_runtimes.get(&terminal_id).is_none());
    assert!(server
        .app
        .state
        .terminals
        .get(&terminal_id)
        .expect("test terminal should still exist")
        .pending_agent_resume_plan
        .is_some());
    assert!(server.app.pending_agent_resume_deadline.is_none());
}

#[test]
fn virtual_render_produces_nonempty_buffer() {
    let mut state = AppState::test_new();
    let area = Rect::new(0, 0, 80, 24);
    let (buffer, _cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);
    assert_eq!(buffer.area.width, 80);
    assert_eq!(buffer.area.height, 24);
}

#[test]
fn virtual_render_without_frame_cursor_keeps_cursor_hidden() {
    let mut state = AppState::test_new();
    let area = Rect::new(0, 0, 80, 24);
    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);

    assert_eq!(cursor, None);
}

#[tokio::test]
async fn virtual_render_preserves_explicit_frame_cursor_position() {
    let mut state = AppState::test_new();
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    ws.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left"),
    );

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);
    let pane = state
        .view
        .pane_infos
        .iter()
        .find(|info| info.id == pane_id)
        .expect("focused pane info");

    assert_eq!(
        cursor,
        Some(CursorState {
            x: pane.inner_rect.x + 4,
            y: pane.inner_rect.y,
            visible: true,
            shape: cursor.as_ref().map(|c| c.shape).unwrap_or(0),
        })
    );
}

#[tokio::test]
async fn virtual_render_preserves_hidden_focused_pane_cursor_position() {
    let mut state = AppState::test_new();
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    ws.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left\x1b[?25l"),
    );

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);
    let pane = state
        .view
        .pane_infos
        .iter()
        .find(|info| info.id == pane_id)
        .expect("focused pane info");

    assert_eq!(
        cursor,
        Some(CursorState {
            x: pane.inner_rect.x + 4,
            y: pane.inner_rect.y,
            visible: false,
            shape: cursor.as_ref().map(|c| c.shape).unwrap_or(0),
        })
    );
}

#[tokio::test]
async fn virtual_render_hides_focused_pane_cursor_during_synchronized_output() {
    let mut state = AppState::test_new();
    state.reveal_hidden_cursor_for_cjk_ime = true;
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left");
    ws.insert_test_runtime(pane_id, runtime);

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let _ = crate::server::render_stream::render_virtual(&mut state, area, true);
    let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
    let runtime = state
        .runtime_for_pane(&terminal_runtimes, pane_id)
        .expect("pane runtime after initial render");
    runtime.test_process_pty_bytes(b"\x1b[?2026h\x1b[2;3H");
    assert!(runtime.synchronized_output_active());

    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, false);

    assert_eq!(
        cursor, None,
        "child cursor positions are unstable while synchronized output is active"
    );
}

#[tokio::test]
async fn virtual_render_hides_focused_pane_cursor_during_synchronized_output_resize() {
    let mut state = AppState::test_new();
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    let runtime = crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left");
    ws.insert_test_runtime(pane_id, runtime);

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let initial_area = Rect::new(0, 0, 80, 24);
    let _ = crate::server::render_stream::render_virtual(&mut state, initial_area, true);
    let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
    let runtime = state
        .runtime_for_pane(&terminal_runtimes, pane_id)
        .expect("pane runtime after initial render");
    runtime.test_process_pty_bytes(b"\x1b[?2026h\x1b[2;3H");
    assert!(runtime.synchronized_output_active());

    let resized_area = Rect::new(0, 0, 100, 30);
    let (_buffer, cursor) =
        crate::server::render_stream::render_virtual(&mut state, resized_area, true);

    assert_eq!(
        cursor, None,
        "pre-resize synchronized output should suppress the cursor even if resize clears the mode"
    );
}

#[tokio::test]
async fn virtual_render_exposes_hidden_pane_cursor_when_reveal_hidden_for_cjk_ime() {
    let mut state = AppState::test_new();
    state.reveal_hidden_cursor_for_cjk_ime = true;
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    ws.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left\x1b[?25l"),
    );

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);
    let pane = state
        .view
        .pane_infos
        .iter()
        .find(|info| info.id == pane_id)
        .expect("focused pane info");

    assert_eq!(
        cursor,
        Some(CursorState {
            x: pane.inner_rect.x + 4,
            y: pane.inner_rect.y,
            visible: true,
            shape: state.cjk_ime_cursor_shape,
        })
    );
}

#[tokio::test]
async fn virtual_render_keeps_cursor_hidden_when_scrolled_back_even_with_reveal_hidden_for_cjk_ime()
{
    let mut state = AppState::test_new();
    state.reveal_hidden_cursor_for_cjk_ime = true;
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    let mut bytes = Vec::new();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let runtime = crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 4096, &bytes);
    ws.insert_test_runtime(pane_id, runtime);

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let _ = crate::server::render_stream::render_virtual(&mut state, area, true);
    let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
    let runtime = state
        .runtime_for_pane(&terminal_runtimes, pane_id)
        .expect("pane runtime after initial render");
    runtime.scroll_up(6);
    assert!(crate::ui::pane_is_scrolled_back(runtime));

    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);

    assert!(
        cursor.as_ref().is_none_or(|cursor| !cursor.visible),
        "scrolled-back focused pane should keep the cursor hidden even when reveal_hidden_cursor_for_cjk_ime is true; got {cursor:?}",
    );
}

#[tokio::test]
async fn virtual_render_fallback_cursor_when_viewport_none_and_reveal_hidden_for_cjk_ime() {
    let mut state = AppState::test_new();
    state.reveal_hidden_cursor_for_cjk_ime = true;
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    // Feed only ?25l with no prior cursor movement — exercises the fallback
    // path for TUIs whose viewport has no cursor position.
    ws.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"\x1b[?25l"),
    );

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);
    let pane = state
        .view
        .pane_infos
        .iter()
        .find(|info| info.id == pane_id)
        .expect("focused pane info");

    assert_eq!(
        cursor,
        Some(CursorState {
            x: pane.inner_rect.x,
            y: pane.inner_rect.y,
            visible: true,
            shape: state.cjk_ime_cursor_shape,
        }),
        "fallback should anchor at pane top-left with the configured shape",
    );
}

#[tokio::test]
async fn virtual_render_skips_reveal_when_focused_pane_has_no_detected_agent() {
    let mut state = AppState::test_new();
    state.reveal_hidden_cursor_for_cjk_ime = true;
    // Filter only Claude, but the test pane has no detected agent, so the
    // reveal must not apply.
    state.cjk_ime_agent_filter_configured = true;
    state.cjk_ime_agents = vec![crate::detect::Agent::Claude];
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    ws.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left\x1b[?25l"),
    );

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);

    assert!(
        cursor.as_ref().is_none_or(|cursor| !cursor.visible),
        "agent filter should suppress reveal when the focused pane's detected agent is not on the list; got {cursor:?}",
    );
}

#[tokio::test]
async fn virtual_render_skips_reveal_when_agent_filter_has_no_valid_entries() {
    let mut state = AppState::test_new();
    state.reveal_hidden_cursor_for_cjk_ime = true;
    state.cjk_ime_agent_filter_configured = true;
    state.cjk_ime_agents = Vec::new();
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    ws.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left\x1b[?25l"),
    );

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);

    assert!(
        cursor.as_ref().is_none_or(|cursor| !cursor.visible),
        "agent filter with no valid entries should suppress reveal; got {cursor:?}",
    );
}

#[tokio::test]
async fn virtual_render_omits_focused_pane_cursor_while_mobile_switcher_open() {
    let mut state = AppState::test_new();
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    ws.insert_test_runtime(
        pane_id,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(20, 5, b"left"),
    );

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Navigate;

    let area = Rect::new(0, 0, 44, 24);
    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);

    assert_eq!(cursor, None);
}

#[tokio::test]
async fn virtual_render_hides_focused_pane_cursor_while_scrolled_back() {
    let mut state = AppState::test_new();
    let mut ws = crate::workspace::Workspace::test_new("test");
    let pane_id = ws.tabs[0].root_pane;
    let mut bytes = Vec::new();
    for line in 0..80 {
        bytes.extend_from_slice(format!("line {line:02}\r\n").as_bytes());
    }
    let runtime = crate::terminal::TerminalRuntime::test_with_scrollback_bytes(20, 5, 4096, &bytes);
    ws.insert_test_runtime(pane_id, runtime);

    state.workspaces = vec![ws];
    state.active = Some(0);
    state.selected = 0;
    state.mode = crate::app::Mode::Terminal;

    let area = Rect::new(0, 0, 80, 24);
    let _ = crate::server::render_stream::render_virtual(&mut state, area, true);
    let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
    let runtime = state
        .runtime_for_pane(&terminal_runtimes, pane_id)
        .expect("pane runtime after initial render");
    runtime.scroll_up(6);
    assert!(crate::ui::pane_is_scrolled_back(runtime));

    let (_buffer, cursor) = crate::server::render_stream::render_virtual(&mut state, area, true);

    assert!(
        cursor.as_ref().is_none_or(|cursor| !cursor.visible),
        "cursor: {cursor:?}"
    );
}

#[test]
fn latest_active_client_drives_shared_size_theme_and_fallback() {
    let mut server = test_headless_server();

    server.clients.insert(
        1,
        ClientConnection::new(
            (160, 45),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme {
                foreground: Some(crate::terminal_theme::RgbColor {
                    r: 0xaa,
                    g: 0xbb,
                    b: 0xcc,
                }),
                background: Some(crate::terminal_theme::RgbColor {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                }),
            },
            None,
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme {
                foreground: Some(crate::terminal_theme::RgbColor {
                    r: 0x10,
                    g: 0x20,
                    b: 0x30,
                }),
                background: Some(crate::terminal_theme::RgbColor {
                    r: 0xdd,
                    g: 0xee,
                    b: 0xff,
                }),
            },
            None,
            2,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );

    assert!(server.promote_client_to_foreground(1));
    assert_eq!(server.foreground_client_id, Some(1));
    assert_eq!(server.effective_size, (160, 45));
    assert_eq!(
        server.app.state.host_terminal_theme,
        server.clients[&1].host_terminal_theme
    );

    assert!(server.promote_client_to_foreground(2));
    assert_eq!(server.foreground_client_id, Some(2));
    assert_eq!(server.effective_size, (80, 24));
    assert_eq!(
        server.app.state.host_terminal_theme,
        server.clients[&2].host_terminal_theme
    );

    assert!(server.remove_client(2));
    assert_eq!(server.foreground_client_id, Some(1));
    assert_eq!(server.effective_size, (160, 45));
    assert_eq!(
        server.app.state.host_terminal_theme,
        server.clients[&1].host_terminal_theme
    );
}

#[test]
fn foreground_client_without_host_theme_clears_previous_host_theme() {
    let mut server = test_headless_server();
    let known_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x10,
            g: 0x20,
            b: 0x30,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x40,
            g: 0x50,
            b: 0x60,
        }),
    };
    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            known_theme,
            None,
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            2,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );

    assert!(server.promote_client_to_foreground(1));
    assert_eq!(server.app.state.host_terminal_theme, known_theme);

    assert!(server.promote_client_to_foreground(2));
    assert_eq!(
        server.app.state.host_terminal_theme,
        crate::terminal_theme::TerminalTheme::default()
    );
}

#[test]
fn foreground_client_appearance_controls_auto_theme() {
    let mut server = test_headless_server();
    server.app.state.theme_runtime.auto_switch = true;
    server.app.state.theme_runtime.dark_name = "catppuccin".to_string();
    server.app.state.theme_runtime.light_name = "catppuccin-latte".to_string();
    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme {
                foreground: None,
                background: Some(crate::terminal_theme::RgbColor { r: 0, g: 0, b: 0 }),
            },
            None,
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme {
                foreground: None,
                background: Some(crate::terminal_theme::RgbColor {
                    r: 255,
                    g: 255,
                    b: 255,
                }),
            },
            None,
            2,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );

    assert!(server.promote_client_to_foreground(1));
    assert_eq!(server.app.state.theme_name, "catppuccin");

    assert!(server.promote_client_to_foreground(2));
    assert_eq!(server.app.state.theme_name, "catppuccin-latte");
}

#[test]
fn color_scheme_change_event_is_inert_on_server() {
    let mut server = test_headless_server();
    let initial_theme = crate::terminal_theme::TerminalTheme {
        foreground: Some(crate::terminal_theme::RgbColor {
            r: 0x10,
            g: 0x20,
            b: 0x30,
        }),
        background: Some(crate::terminal_theme::RgbColor {
            r: 0x40,
            g: 0x50,
            b: 0x60,
        }),
    };
    server.app.state.host_terminal_theme = initial_theme;
    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            initial_theme,
            None,
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );

    let changed = server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: crate::raw_input::GHOSTTY_COLOR_SCHEME_DARK_REPORT.to_vec(),
    });

    assert!(!changed);
    assert_eq!(server.foreground_client_id, None);
    assert_eq!(server.clients[&1].host_terminal_theme, initial_theme);
    assert_eq!(server.app.state.host_terminal_theme, initial_theme);
}

#[test]
fn focus_lost_updates_client_without_promoting_foreground() {
    let mut server = test_headless_server();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(true),
            2,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(2);
    server.sync_foreground_client_state();

    let changed = server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b[O".to_vec(),
    });

    assert!(!changed);
    assert_eq!(server.foreground_client_id, Some(2));
    assert_eq!(server.clients[&1].outer_terminal_focus, Some(false));
    assert_eq!(server.app.state.outer_terminal_focus, Some(true));
}

#[test]
fn focus_gained_promotes_client_to_foreground() {
    let mut server = test_headless_server();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(true),
            2,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(2);
    server.sync_foreground_client_state();

    let changed = server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b[I".to_vec(),
    });

    assert!(changed);
    assert_eq!(server.foreground_client_id, Some(1));
    assert_eq!(server.clients[&1].outer_terminal_focus, Some(true));
    assert_eq!(server.app.state.outer_terminal_focus, Some(true));
}

#[test]
fn foreground_client_focus_event_updates_app_focus_state() {
    let mut server = test_headless_server();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(true),
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();

    let changed = server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b[O".to_vec(),
    });

    assert!(!changed);
    assert_eq!(server.clients[&1].outer_terminal_focus, Some(false));
    assert_eq!(server.app.state.outer_terminal_focus, Some(false));
}

#[test]
fn app_client_lone_escape_closes_navigate_mode() {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("test")];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Navigate;
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(true),
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();

    assert!(server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b".to_vec(),
    }));

    assert_eq!(server.app.state.mode, crate::app::Mode::Terminal);
}

#[test]
fn semantic_client_input_events_route_through_app_input() {
    let mut server = test_headless_server();
    server.app.state.mode = crate::app::Mode::Onboarding;
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(true),
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();

    assert!(server.handle_server_event(ServerEvent::ClientInputEvents {
        client_id: 1,
        events: vec![crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Enter,
            modifiers: 0,
            kind: crate::protocol::ClientKeyKind::Press,
        }],
    }));

    assert_eq!(server.app.state.mode, crate::app::Mode::Settings);
    assert_eq!(
        server.app.state.settings.section,
        crate::app::state::SettingsSection::Integrations
    );
}

#[test]
fn semantic_client_escape_closes_keybind_help() {
    let mut server = test_headless_server();
    server.app.state.mode = crate::app::Mode::KeybindHelp;
    server.clients.insert(
        1,
        ClientConnection::new(
            (100, 30),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(true),
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    server.resize_shared_runtime_to_effective_size();

    assert!(server.handle_server_event(ServerEvent::ClientInputEvents {
        client_id: 1,
        events: vec![crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Esc,
            modifiers: 0,
            kind: crate::protocol::ClientKeyKind::Press,
        }],
    }));

    assert_eq!(server.app.state.mode, crate::app::Mode::Navigate);
}

#[test]
fn semantic_client_down_scrolls_keybind_help() {
    let mut server = test_headless_server();
    server.app.state.mode = crate::app::Mode::KeybindHelp;
    server.clients.insert(
        1,
        ClientConnection::new(
            (100, 30),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(true),
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    server.resize_shared_runtime_to_effective_size();

    assert!(server.app.state.keybind_help_max_scroll() > 0);
    assert!(server.handle_server_event(ServerEvent::ClientInputEvents {
        client_id: 1,
        events: vec![crate::protocol::ClientInputEvent::Key {
            code: crate::protocol::ClientKeyCode::Down,
            modifiers: 0,
            kind: crate::protocol::ClientKeyKind::Press,
        }],
    }));

    assert_eq!(server.app.state.mode, crate::app::Mode::KeybindHelp);
    assert_eq!(server.app.state.keybind_help.scroll, 1);
}

#[tokio::test]
async fn split_default_background_response_updates_theme_without_forwarding_tail() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("test");
    let focused = workspace.focused_pane_id().unwrap();
    let (runtime, mut rx) = crate::terminal::TerminalRuntime::test_with_channel_capacity(80, 24, 1);
    workspace.tabs[0].runtimes.insert(focused, runtime);
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(true),
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();

    let _ = server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b]".to_vec(),
    });
    assert!(rx.try_recv().is_err());

    assert!(server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"11;#123456\x07".to_vec(),
    }));

    assert!(rx.try_recv().is_err());
    assert_eq!(
        server.clients[&1].host_terminal_theme.background,
        Some(crate::terminal_theme::RgbColor {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        })
    );
    assert_eq!(
        server.app.state.host_terminal_theme.background,
        Some(crate::terminal_theme::RgbColor {
            r: 0x12,
            g: 0x34,
            b: 0x56,
        })
    );
}

#[test]
fn render_and_stream_uses_each_client_terminal_size() {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("test")];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;

    let (desktop_tx, _desktop_control_rx, desktop_rx) = test_client_writer();
    let (phone_tx, _phone_control_rx, phone_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(desktop_tx),
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            2,
            RenderEncoding::SemanticFrame,
            Some(phone_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    server.resize_shared_runtime_to_effective_size();

    server.render_and_stream();

    let desktop_frame = read_server_frame(desktop_rx.recv().expect("desktop frame"));
    let phone_frame = read_server_frame(phone_rx.recv().expect("phone frame"));

    assert_eq!((desktop_frame.width, desktop_frame.height), (120, 40));
    assert_eq!((phone_frame.width, phone_frame.height), (80, 24));
}

#[tokio::test]
async fn resize_shared_runtime_resizes_background_tabs() {
    let mut server = test_headless_server();
    let mut workspace = crate::workspace::Workspace::test_new("test");
    let background_tab = workspace.test_add_tab(Some("background"));
    let active_pane = workspace.tabs[0].root_pane;
    let background_pane = workspace.tabs[background_tab].root_pane;
    workspace.tabs[0].runtimes.insert(
        active_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );
    workspace.tabs[background_tab].runtimes.insert(
        background_pane,
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );
    server.app.state.workspaces = vec![workspace];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    server.resize_shared_runtime_to_effective_size();

    let terminal_area = server.app.state.view.terminal_area;
    let expected = (terminal_area.height, terminal_area.width.saturating_sub(1));
    assert_eq!(
        server
            .app
            .state
            .runtime_for_pane(&server.app.terminal_runtimes, active_pane)
            .unwrap()
            .current_size(),
        expected
    );
    assert_eq!(
        server
            .app
            .state
            .runtime_for_pane(&server.app.terminal_runtimes, background_pane)
            .unwrap()
            .current_size(),
        expected
    );
}

#[test]
fn terminal_attach_disconnect_restores_app_pane_size() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime");
    let _runtime_guard = rt.enter();
    let mut server = test_headless_server();
    let workspace = crate::workspace::Workspace::test_new("test");
    let pane_id = workspace.tabs[0].root_pane;
    let terminal_id = workspace.terminal_id(pane_id).expect("terminal id").clone();
    let terminal_id_string = terminal_id.to_string();
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    server.app.terminal_runtimes.insert(
        terminal_id.clone(),
        crate::terminal::TerminalRuntime::test_with_screen_bytes(80, 24, b""),
    );
    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            None,
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    server.resize_shared_runtime_to_effective_size();
    let expected_app_size = server
        .app
        .terminal_runtimes
        .get(&terminal_id)
        .expect("runtime")
        .current_size();
    assert_ne!(expected_app_size, (24, 80));

    let (writer, _control_rx, _render_rx) = test_client_writer();
    assert!(server.handle_server_event(ServerEvent::ClientConnected {
        client_id: 2,
        cols: 80,
        rows: 24,
        cell_width_px: 0,
        cell_height_px: 0,
        render_encoding: RenderEncoding::TerminalAnsi,
        keybindings: None,
        direct_attach_requested: true,
        writer,
    }));
    assert!(
        server.handle_server_event(ServerEvent::ClientAttachTerminal {
            client_id: 2,
            terminal_id: terminal_id_string,
            takeover: false,
        })
    );
    assert_eq!(server.foreground_client_id, Some(1));
    assert!(server
        .app
        .state
        .direct_attach_resize_locks
        .contains(&terminal_id));
    assert_eq!(
        server
            .app
            .terminal_runtimes
            .get(&terminal_id)
            .expect("runtime")
            .current_size(),
        (24, 80)
    );

    assert!(server.handle_server_event(ServerEvent::ClientDisconnected { client_id: 2 }));

    assert!(!server
        .app
        .state
        .direct_attach_resize_locks
        .contains(&terminal_id));
    assert_eq!(
        server
            .app
            .terminal_runtimes
            .get(&terminal_id)
            .expect("runtime")
            .current_size(),
        expected_app_size
    );
    drop(server);
    drop(_runtime_guard);
    rt.shutdown_timeout(Duration::from_millis(100));
}

#[test]
fn render_and_stream_sends_terminal_frame_for_terminal_ansi_client() {
    let mut server = test_headless_server();
    let (client_tx, _client_control_rx, client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::TerminalAnsi,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);

    server.render_and_stream();

    match read_server_message(
        client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("terminal frame"),
    ) {
        ServerMessage::Terminal(frame) => {
            assert_eq!(frame.seq, 1);
            assert_eq!((frame.width, frame.height), (80, 24));
            assert!(frame.full);
            assert!(!frame.bytes.is_empty());
        }
        other => panic!("expected terminal frame, got {other:?}"),
    }
    assert_eq!(
        server
            .clients
            .get(&1)
            .unwrap()
            .render_state
            .terminal_seq()
            .unwrap(),
        1
    );
}

#[test]
fn terminal_ansi_input_does_not_reset_blit_baseline() {
    let mut server = test_headless_server();
    let (client_tx, _client_control_rx, client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::TerminalAnsi,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);

    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial terminal frame");
    assert_eq!(
        server
            .clients
            .get(&1)
            .unwrap()
            .render_state
            .terminal_seq()
            .unwrap(),
        1
    );

    assert!(!server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: Vec::new(),
    }));
    server.render_and_stream();

    assert_eq!(
        server
            .clients
            .get(&1)
            .unwrap()
            .render_state
            .terminal_seq()
            .unwrap(),
        1
    );
    assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
}

#[test]
fn outer_focus_gained_forces_terminal_ansi_full_redraw() {
    let mut server = test_headless_server();
    let (client_tx, _client_control_rx, client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::TerminalAnsi,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);

    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial terminal frame");

    assert!(server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b[I".to_vec(),
    }));
    server.render_and_stream();

    match read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()) {
        ServerMessage::Terminal(frame) => {
            assert_eq!(frame.seq, 2);
            assert!(frame.full);
        }
        other => panic!("expected terminal frame, got {other:?}"),
    }
}

#[tokio::test]
async fn outer_focus_gained_client_render_pending_survives_semantic_render_queue_full() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");

    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial semantic frame");

    let queued = HeadlessServer::frame_server_message(&ServerMessage::ReloadSoundConfig)
        .expect("serialize dummy message");
    server
        .clients
        .get(&1)
        .unwrap()
        .writer
        .as_ref()
        .unwrap()
        .render
        .try_send(queued)
        .expect("pre-fill render queue");

    assert!(server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b[I".to_vec(),
    }));
    assert!(server.clients.get(&1).unwrap().render_pending);

    server.render_and_stream();

    assert!(server.clients.get(&1).unwrap().render_pending);
    assert!(matches!(
        read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()),
        ServerMessage::ReloadSoundConfig
    ));

    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(b"\rZ");

    assert!(!server.render_retained_pty_update_and_stream());
    assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());

    assert!(server.handle_server_event(ServerEvent::ClientWriterDrained { client_id: 1 }));
    server.render_and_stream();

    assert!(!server.clients.get(&1).unwrap().render_pending);
    assert!(matches!(
        read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()),
        ServerMessage::Frame(_)
    ));
}

#[test]
fn outer_focus_gained_does_not_force_terminal_ansi_full_redraw_when_disabled() {
    let mut server = test_headless_server();
    server.app.state.redraw_on_focus_gained = false;
    let (client_tx, _client_control_rx, client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::TerminalAnsi,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);

    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial terminal frame");

    server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b[I".to_vec(),
    });
    server.render_and_stream();

    assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
    assert_eq!(server.clients[&1].outer_terminal_focus, Some(true));
    assert_eq!(server.app.state.outer_terminal_focus, Some(true));
    assert_eq!(
        server
            .clients
            .get(&1)
            .unwrap()
            .render_state
            .terminal_seq()
            .unwrap(),
        1
    );
}

#[test]
fn outer_focus_gained_does_not_mark_semantic_render_pending_when_disabled() {
    let mut server = test_headless_server();
    server.app.state.redraw_on_focus_gained = false;
    let (client_tx, _client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);

    assert!(server.handle_server_event(ServerEvent::ClientInput {
        client_id: 1,
        data: b"\x1b[I".to_vec(),
    }));

    assert!(!server.clients.get(&1).unwrap().render_pending);
    assert!(!server.app.full_redraw_pending);
    assert_eq!(server.clients[&1].outer_terminal_focus, Some(true));
    assert_eq!(server.app.state.outer_terminal_focus, Some(true));
}

#[test]
fn full_render_queue_does_not_advance_terminal_ansi_baseline() {
    let mut server = test_headless_server();
    let (client_tx, _client_control_rx, client_rx) = test_client_writer();
    let queued = HeadlessServer::frame_server_message(&ServerMessage::ReloadSoundConfig)
        .expect("serialize dummy message");
    client_tx
        .render
        .try_send(queued)
        .expect("pre-fill render queue");

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::TerminalAnsi,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);

    server.render_and_stream();

    assert_eq!(
        server
            .clients
            .get(&1)
            .unwrap()
            .render_state
            .terminal_seq()
            .unwrap(),
        0
    );
    assert!(matches!(
        read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()),
        ServerMessage::ReloadSoundConfig
    ));
    assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
}

#[test]
fn writer_drained_retries_pending_terminal_ansi_render() {
    let mut server = test_headless_server();
    let (client_tx, _client_control_rx, client_rx) = test_client_writer();
    let queued = HeadlessServer::frame_server_message(&ServerMessage::ReloadSoundConfig)
        .expect("serialize dummy message");
    client_tx
        .render
        .try_send(queued)
        .expect("pre-fill render queue");

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::TerminalAnsi,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);

    server.render_and_stream();
    assert!(server.clients.get(&1).unwrap().render_pending);
    assert!(matches!(
        read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()),
        ServerMessage::ReloadSoundConfig
    ));

    assert!(server.handle_server_event(ServerEvent::ClientWriterDrained { client_id: 1 }));
    server.render_and_stream();

    match read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()) {
        ServerMessage::Terminal(frame) => assert_eq!(frame.seq, 1),
        other => panic!("expected terminal frame, got {other:?}"),
    }
    assert_eq!(
        server
            .clients
            .get(&1)
            .unwrap()
            .render_state
            .terminal_seq()
            .unwrap(),
        1
    );
    assert!(!server.clients.get(&1).unwrap().render_pending);
}

#[test]
fn render_and_stream_skips_identical_frame_sends() {
    let mut server = test_headless_server();
    server.app.state.workspaces = vec![crate::workspace::Workspace::test_new("test")];
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;

    let (client_tx, _client_control_rx, client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();
    server.resize_shared_runtime_to_effective_size();

    server.render_and_stream();
    let first = client_rx.recv_timeout(Duration::from_millis(100));
    assert!(first.is_ok(), "expected first frame to be sent");

    server.render_and_stream();
    assert!(
        client_rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "identical frame should not be sent twice"
    );
}

#[tokio::test]
async fn retained_pty_update_streams_dirty_row_from_last_frame() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
    server.render_and_stream();
    let first = read_server_frame(
        client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial frame"),
    );
    assert!(first.cells.iter().any(|cell| cell.symbol == "a"));

    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(b"\rZ");

    assert!(server.render_retained_pty_update_and_stream());
    let patched = read_server_frame(
        client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("retained frame"),
    );
    assert!(patched.cells.iter().any(|cell| cell.symbol == "Z"));
    assert_eq!((patched.width, patched.height), (80, 24));
}

#[tokio::test]
async fn retained_pty_update_declines_while_toast_is_visible() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
    server.app.state.toast = Some(crate::app::state::ToastNotification {
        kind: crate::app::state::ToastKind::NeedsAttention,
        title: "pi needs attention".to_owned(),
        context: "background · 2".to_owned(),
        position: None,
        target: None,
    });
    server.render_and_stream();
    let initial = read_server_frame(
        client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("initial frame"),
    );
    assert!(
        frame_text(&initial).contains("pi needs attention"),
        "expected initial full frame to include toast text"
    );

    let toast_row = server.app.state.view.toast_hit_area.y;
    let inner_rect = server.app.state.view.pane_infos[0].inner_rect;
    let pane_row = toast_row
        .checked_sub(inner_rect.y)
        .expect("toast should overlap the pane")
        + 1;
    assert!(pane_row <= inner_rect.height);
    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(format!("\x1b[{pane_row};1Hzzzz").as_bytes());

    assert!(!server.render_retained_pty_update_and_stream());
    assert!(
        client_rx.recv_timeout(Duration::from_millis(50)).is_err(),
        "retained path should not stream a frame that can overwrite toast cells"
    );
}

#[tokio::test]
async fn retained_pty_update_matches_full_render_frame() {
    let initial = b"\x1b[6 qleft \xe4\xb8\xad";
    let update = b"\r\x1b[44mZ\x1b[0m";
    let (mut retained_server, retained_rx, retained_pane_id) = retained_test_server(initial);
    let (mut full_server, full_rx, full_pane_id) = retained_test_server(initial);

    retained_server.render_and_stream();
    let _ = retained_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial retained baseline");
    full_server.render_and_stream();
    let _ = full_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial full baseline");

    retained_server
        .app
        .state
        .runtime_for_pane_in_workspace(&retained_server.app.terminal_runtimes, 0, retained_pane_id)
        .expect("retained runtime")
        .test_process_pty_bytes(update);
    full_server
        .app
        .state
        .runtime_for_pane_in_workspace(&full_server.app.terminal_runtimes, 0, full_pane_id)
        .expect("full runtime")
        .test_process_pty_bytes(update);

    assert!(retained_server.render_retained_pty_update_and_stream());
    full_server.render_and_stream();

    let retained_frame = read_server_frame(
        retained_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("retained frame"),
    );
    let full_frame = read_server_frame(
        full_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("full frame"),
    );
    assert_frame_data_eq(&retained_frame, &full_frame);
}

#[tokio::test]
async fn retained_pty_update_streams_cursor_only_change() {
    let initial = b"abcd";
    let update = b"\x1b[D";
    let (mut retained_server, retained_rx, retained_pane_id) = retained_test_server(initial);
    let (mut full_server, full_rx, full_pane_id) = retained_test_server(initial);

    retained_server.render_and_stream();
    let _ = retained_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial retained baseline");
    full_server.render_and_stream();
    let _ = full_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial full baseline");

    retained_server
        .app
        .state
        .runtime_for_pane_in_workspace(&retained_server.app.terminal_runtimes, 0, retained_pane_id)
        .expect("retained runtime")
        .test_process_pty_bytes(update);
    full_server
        .app
        .state
        .runtime_for_pane_in_workspace(&full_server.app.terminal_runtimes, 0, full_pane_id)
        .expect("full runtime")
        .test_process_pty_bytes(update);

    assert!(retained_server.render_retained_pty_update_and_stream());
    full_server.render_and_stream();

    let retained_frame = read_server_frame(
        retained_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("retained cursor frame"),
    );
    let full_frame = read_server_frame(
        full_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("full cursor frame"),
    );
    assert_frame_data_eq(&retained_frame, &full_frame);
}

#[tokio::test]
async fn retained_pty_update_declines_unsafe_mode_without_consuming_dirty_rows() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial frame");

    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(b"\rZ");

    server.app.state.mode = crate::app::Mode::Navigate;
    assert!(!server.render_retained_pty_update_and_stream());
    assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());

    server.app.state.mode = crate::app::Mode::Terminal;
    assert!(server.render_retained_pty_update_and_stream());
    let patched = read_server_frame(
        client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("retained frame after safe mode"),
    );
    assert!(patched.cells.iter().any(|cell| cell.symbol == "Z"));
}

#[tokio::test]
async fn headless_full_render_clears_full_redraw_pending_for_future_retained_updates() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
    server.app.full_redraw_pending = true;

    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("full redraw frame");
    assert!(!server.app.full_redraw_pending);

    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(b"\rZ");

    assert!(server.render_retained_pty_update_and_stream());
}

#[tokio::test]
async fn retained_pty_update_declines_when_patch_would_stale_hyperlinks() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"link");
    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial frame");
    let inner_rect = server.app.state.view.pane_infos[0].inner_rect;
    let client = server.clients.get_mut(&1).unwrap();
    let mut frame = client.render_state.last_frame().unwrap().clone();
    frame.hyperlinks = vec!["https://example.com".to_owned()];
    let hyperlink_idx =
        usize::from(inner_rect.y) * usize::from(frame.width) + usize::from(inner_rect.x);
    frame.cells[hyperlink_idx].hyperlink = Some(0);
    let prepared = client
        .render_state
        .prepare_frame(frame)
        .expect("hyperlink frame differs");
    client.render_state.commit_sent_frame(prepared);

    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(b"\rplain");

    assert!(!server.render_retained_pty_update_and_stream());
    assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());

    server.render_and_stream();
    let full = read_server_frame(
        client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("full frame after hyperlink overwrite"),
    );
    assert!(
        full.cells.iter().all(|cell| cell.hyperlink.is_none()),
        "full render should clear overwritten hyperlink cells"
    );
}

#[tokio::test]
async fn retained_pty_update_allows_kitty_enabled_empty_graphics_cache() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
    server.app.state.kitty_graphics_enabled = true;
    server.clients.get_mut(&1).unwrap().cell_size = crate::kitty_graphics::HostCellSize {
        width_px: 10,
        height_px: 20,
    };

    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial frame");

    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(b"\rZ");

    assert!(server.render_retained_pty_update_and_stream());
    let retained = read_server_frame(
        client_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("retained frame with kitty enabled"),
    );
    assert!(retained.cells.iter().any(|cell| cell.symbol == "Z"));
}

#[tokio::test]
async fn retained_pty_update_declines_when_graphics_cache_has_content() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
    server.app.state.kitty_graphics_enabled = true;
    let client = server.clients.get_mut(&1).unwrap();
    client.cell_size = crate::kitty_graphics::HostCellSize {
        width_px: 10,
        height_px: 20,
    };

    server.render_and_stream();
    let _ = client_rx
        .recv_timeout(Duration::from_millis(100))
        .expect("initial frame");
    server
        .clients
        .get_mut(&1)
        .unwrap()
        .graphics_cache
        .test_mark_non_empty();

    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(b"\rZ");

    assert!(!server.render_retained_pty_update_and_stream());
    assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
}

#[tokio::test]
async fn full_redraw_pending_survives_full_render_queue_full() {
    let (mut server, client_rx, pane_id) = retained_test_server(b"aaaa");
    let queued = HeadlessServer::frame_server_message(&ServerMessage::ReloadSoundConfig)
        .expect("serialize dummy message");
    server
        .clients
        .get(&1)
        .unwrap()
        .writer
        .as_ref()
        .unwrap()
        .render
        .try_send(queued)
        .expect("pre-fill render queue");
    server.app.full_redraw_pending = true;

    server.render_and_stream();

    assert!(server.app.full_redraw_pending);
    assert!(server.clients.get(&1).unwrap().render_pending);
    assert!(matches!(
        read_server_message(client_rx.recv_timeout(Duration::from_millis(100)).unwrap()),
        ServerMessage::ReloadSoundConfig
    ));

    let runtime = server
        .app
        .state
        .runtime_for_pane_in_workspace(&server.app.terminal_runtimes, 0, pane_id)
        .expect("runtime");
    runtime.test_process_pty_bytes(b"\rZ");

    assert!(!server.render_retained_pty_update_and_stream());
    assert!(client_rx.recv_timeout(Duration::from_millis(50)).is_err());
}

#[test]
fn client_config_reload_request_refreshes_attached_clients() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.app.state.request_client_config_reload = true;

    server.drain_client_config_reload_request();

    match read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("client config reload message"),
    ) {
        ServerMessage::ReloadSoundConfig => {}
        other => panic!("expected ReloadSoundConfig, got {other:?}"),
    }
    assert!(!server.app.state.request_client_config_reload);
}

#[test]
fn clipboard_write_targets_foreground_client_only() {
    let mut server = test_headless_server();
    let (background_tx, background_control_rx, _background_rx) = test_client_writer();
    let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(background_tx),
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            2,
            RenderEncoding::SemanticFrame,
            Some(foreground_tx),
        ),
    );
    server.foreground_client_id = Some(2);
    server.sync_foreground_client_state();

    let changed = server.handle_internal_event_with_forwarding(AppEvent::ClipboardWrite {
        content: b"test".to_vec(),
    });

    assert!(changed);
    assert_eq!(
        server
            .app
            .state
            .copy_feedback
            .as_ref()
            .map(|feedback| feedback.message.as_str()),
        Some("copied to clipboard")
    );
    match read_server_message(
        foreground_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("foreground clipboard message"),
    ) {
        ServerMessage::Clipboard { data } => assert_eq!(data, "dGVzdA=="),
        other => panic!("expected clipboard message, got {other:?}"),
    }
    assert!(
        background_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "background client should not receive clipboard writes"
    );
}

#[test]
fn clipboard_write_without_foreground_client_does_not_show_feedback() {
    let mut server = test_headless_server();
    server.foreground_client_id = None;

    let changed = server.handle_internal_event_with_forwarding(AppEvent::ClipboardWrite {
        content: b"test".to_vec(),
    });

    assert!(changed);
    assert!(
        server.app.state.copy_feedback.is_none(),
        "clipboard feedback should only show when a foreground client can receive the write"
    );
}

#[test]
fn clipboard_write_failed_foreground_send_does_not_show_feedback() {
    let mut server = test_headless_server();
    let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();
    drop(foreground_control_rx);

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(foreground_tx),
        ),
    );
    server.foreground_client_id = Some(1);

    let changed = server.handle_internal_event_with_forwarding(AppEvent::ClipboardWrite {
        content: b"test".to_vec(),
    });

    assert!(changed);
    assert!(
        server.app.state.copy_feedback.is_none(),
        "clipboard feedback should only show after the foreground client receives the write"
    );
    assert!(
        !server.clients.contains_key(&1),
        "failed targeted send should remove the broken foreground client"
    );
}

#[test]
fn client_local_notifications_target_foreground_client_only() {
    let mut server = test_headless_server();
    let (background_tx, background_control_rx, _background_rx) = test_client_writer();
    let (foreground_tx, foreground_control_rx, _foreground_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (120, 40),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(background_tx),
        ),
    );
    server.clients.insert(
        2,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            2,
            RenderEncoding::SemanticFrame,
            Some(foreground_tx),
        ),
    );
    server.foreground_client_id = Some(2);
    server.sync_foreground_client_state();

    assert!(server.send_to_foreground_client(ServerMessage::Notify {
        kind: protocol::NotifyKind::Toast,
        message: "pi finished".to_string(),
        body: Some("workspace 1".to_string()),
    }));

    match read_server_message(
        foreground_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("foreground toast message"),
    ) {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::Toast);
            assert_eq!(message, "pi finished");
            assert_eq!(body.as_deref(), Some("workspace 1"));
        }
        other => panic!("expected toast notify, got {other:?}"),
    }
    assert!(
        background_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "background client should not receive client-local notifications"
    );
}

#[test]
fn herdr_toast_delivery_keeps_toast_in_frame_without_client_notify() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;

    let changed = server.handle_internal_event_with_forwarding(AppEvent::UpdateReady {
        version: "9.9.9".to_string(),
        install_command: "bora update".into(),
    });

    assert!(changed);
    assert!(server.app.state.toast.is_some());
    assert!(
        client_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "herdr delivery should render in-frame instead of forwarding a client-local notification"
    );
}

#[test]
fn system_toast_delivery_forwards_system_notify_kind() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::System;

    let changed = server.handle_internal_event_with_forwarding(AppEvent::UpdateReady {
        version: "9.9.9".to_string(),
        install_command: "bora update".into(),
    });

    assert!(changed);
    match read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("system toast message"),
    ) {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::SystemToast);
            assert_eq!(message, "v9.9.9 available");
            assert_eq!(
                body.as_deref(),
                Some("detach, run `bora update`, then follow its restart guidance")
            );
        }
        other => panic!("expected system toast notify, got {other:?}"),
    }
}

#[test]
fn notification_show_api_forwards_system_notification_to_foreground_client() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::System;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed = server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "notify".into(),
            method: api::schema::Method::NotificationShow(api::schema::NotificationShowParams {
                title: "build failed".into(),
                body: Some("api workspace".into()),
                position: Some(crate::config::ToastHerdrPosition::TopLeft),
                sound: api::schema::NotificationShowSound::Request,
            }),
        },
        respond_to,
    });

    assert!(changed);
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: true,
            reason: api::schema::NotificationShowReason::Shown,
        }
    );
    let first = read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("api notification message"),
    );
    let second = read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("api sound message"),
    );

    match first {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::SystemToast);
            assert_eq!(message, "build failed");
            assert_eq!(body.as_deref(), Some("api workspace"));
        }
        other => panic!("expected api notification, got {other:?}"),
    }
    match second {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::Sound);
            assert_eq!(message, "agent attention");
            assert!(body.is_none());
        }
        other => panic!("expected api sound, got {other:?}"),
    }
}

#[test]
fn notification_show_api_preserves_colon_in_forwarded_title() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::System;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed = server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "notify".into(),
            method: api::schema::Method::NotificationShow(api::schema::NotificationShowParams {
                title: "build: failed".into(),
                body: Some("api workspace".into()),
                position: None,
                sound: api::schema::NotificationShowSound::None,
            }),
        },
        respond_to,
    });

    assert!(changed);
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: true,
            reason: api::schema::NotificationShowReason::Shown,
        }
    );
    match read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("api notification message"),
    ) {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::SystemToast);
            assert_eq!(message, "build: failed");
            assert_eq!(body.as_deref(), Some("api workspace"));
        }
        other => panic!("expected api notification, got {other:?}"),
    }
}

#[test]
fn notification_show_api_validates_empty_title_before_disabled_delivery() {
    let mut server = test_headless_server();
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::Off;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed = server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "notify".into(),
            method: api::schema::Method::NotificationShow(api::schema::NotificationShowParams {
                title: "\n\t".into(),
                body: None,
                position: None,
                sound: api::schema::NotificationShowSound::None,
            }),
        },
        respond_to,
    });

    assert!(changed);
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::ErrorResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed.error.code, "invalid_params");
    assert_eq!(parsed.error.message, "notification title is empty");
}

#[test]
fn notification_show_api_reports_no_foreground_client() {
    let mut server = test_headless_server();
    server.foreground_client_id = None;
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::System;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed = server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "notify".into(),
            method: api::schema::Method::NotificationShow(api::schema::NotificationShowParams {
                title: "build failed".into(),
                body: None,
                position: None,
                sound: api::schema::NotificationShowSound::Request,
            }),
        },
        respond_to,
    });

    assert!(changed);
    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: false,
            reason: api::schema::NotificationShowReason::NoForegroundClient,
        }
    );
}

#[test]
fn notification_show_api_herdr_toast_expires_headless() {
    let mut server = test_headless_server();
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    assert!(
        server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
            request: api::schema::Request {
                id: "notify".into(),
                method: api::schema::Method::NotificationShow(
                    api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: None,
                        position: None,
                        sound: api::schema::NotificationShowSound::None,
                    },
                ),
            },
            respond_to,
        })
    );

    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: true,
            reason: api::schema::NotificationShowReason::Shown,
        }
    );
    let deadline = server.app.toast_deadline.expect("api toast deadline");
    assert!(server.handle_scheduled_tasks_headless(deadline, false));
    assert!(server.app.state.toast.is_none());
    assert!(server.app.toast_deadline.is_none());
}

#[test]
fn notification_show_api_forwards_sound_for_herdr_delivery() {
    let mut server = test_headless_server();
    let (client_tx, client_control_rx, _client_rx) = test_client_writer();

    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::Herdr;

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    assert!(
        server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
            request: api::schema::Request {
                id: "notify".into(),
                method: api::schema::Method::NotificationShow(
                    api::schema::NotificationShowParams {
                        title: "build failed".into(),
                        body: None,
                        position: None,
                        sound: api::schema::NotificationShowSound::Done,
                    },
                ),
            },
            respond_to,
        })
    );

    let response = response_rx
        .recv_timeout(Duration::from_millis(100))
        .unwrap();
    let parsed: api::schema::SuccessResponse = serde_json::from_str(&response).unwrap();
    assert_eq!(
        parsed.result,
        api::schema::ResponseResult::NotificationShow {
            shown: true,
            reason: api::schema::NotificationShowReason::Shown,
        }
    );
    match read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("api sound message"),
    ) {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::Sound);
            assert_eq!(message, "agent done");
            assert!(body.is_none());
        }
        other => panic!("expected api sound, got {other:?}"),
    }
}

#[test]
fn delayed_agent_notification_forwards_after_deadline() {
    let mut server = test_headless_server();
    let background = crate::workspace::Workspace::test_new("background");
    let pane_id = background.tabs[0].root_pane;
    let foreground = crate::workspace::Workspace::test_new("foreground");
    server.app.state.workspaces = vec![background, foreground];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(1);
    server.app.state.selected = 1;
    server.app.state.mode = crate::app::Mode::Terminal;
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::System;
    server.app.state.toast_config.delay_seconds = 1;

    let (client_tx, client_control_rx, _client_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();

    let changed = server.handle_internal_event_with_forwarding(AppEvent::StateChanged {
        pane_id,
        agent: Some(crate::detect::Agent::Pi),
        state: crate::detect::AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: Instant::now(),
    });

    assert!(changed);
    assert!(server.app.state.toast.is_none());
    assert!(
        client_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "delayed transition should not notify immediately"
    );

    let deadline = server
        .app
        .state
        .next_pending_agent_notification_deadline()
        .expect("pending notification deadline");
    assert!(server.handle_scheduled_tasks_headless(deadline, false));

    let first = read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("delayed sound message"),
    );
    let second = read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("delayed toast message"),
    );

    assert!(matches!(
        first,
        ServerMessage::Notify {
            kind: protocol::NotifyKind::Sound,
            ..
        }
    ));
    match second {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::SystemToast);
            assert_eq!(message, "pi needs attention");
            assert_eq!(body.as_deref(), Some("background · 1"));
        }
        other => panic!("expected delayed system toast, got {other:?}"),
    }
    assert!(server.app.state.pending_agent_notifications.is_empty());
}

#[test]
fn delayed_active_tab_unfocused_agent_notification_forwards_after_deadline() {
    let mut server = test_headless_server();
    let workspace = crate::workspace::Workspace::test_new("active");
    let pane_id = workspace.tabs[0].root_pane;
    server.app.state.workspaces = vec![workspace];
    server.app.state.ensure_test_terminals();
    server.app.state.active = Some(0);
    server.app.state.selected = 0;
    server.app.state.mode = crate::app::Mode::Terminal;
    server.app.state.toast_config.delivery = crate::config::ToastDelivery::System;
    server.app.state.toast_config.delay_seconds = 1;

    let (client_tx, client_control_rx, _client_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            Some(false),
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();

    assert!(
        server.handle_internal_event_with_forwarding(AppEvent::StateChanged {
            pane_id,
            agent: Some(crate::detect::Agent::Pi),
            state: crate::detect::AgentState::Blocked,
            visible_blocker: false,
            visible_working: false,
            process_exited: false,
            observed_at: Instant::now(),
        })
    );
    assert!(server.app.state.toast.is_none());
    assert!(
        client_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "delayed transition should not notify immediately"
    );

    let deadline = server
        .app
        .state
        .next_pending_agent_notification_deadline()
        .expect("pending notification deadline");
    assert!(server.handle_scheduled_tasks_headless(deadline, false));

    let first = read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("delayed sound message"),
    );
    let second = read_server_message(
        client_control_rx
            .recv_timeout(Duration::from_millis(100))
            .expect("delayed toast message"),
    );

    assert!(matches!(
        first,
        ServerMessage::Notify {
            kind: protocol::NotifyKind::Sound,
            ..
        }
    ));
    match second {
        ServerMessage::Notify {
            kind,
            message,
            body,
        } => {
            assert_eq!(kind, protocol::NotifyKind::SystemToast);
            assert_eq!(message, "pi needs attention");
            assert_eq!(body.as_deref(), Some("active · 1"));
        }
        other => panic!("expected delayed system toast, got {other:?}"),
    }
}

#[test]
fn stale_api_agent_report_does_not_forward_done_sound() {
    let mut server = test_headless_server();
    let background = crate::workspace::Workspace::test_new("background");
    let pane_id = background.tabs[0].root_pane;
    let public_pane_id = format!("{}:p1", background.id);
    let foreground = crate::workspace::Workspace::test_new("foreground");
    server.app.state.workspaces = vec![background, foreground];
    server.app.state.ensure_test_terminals();
    let terminal_id = server.app.state.workspaces[0]
        .pane_state(pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    server
        .app
        .state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_hook_authority(
            "herdr:pi".into(),
            "pi".into(),
            crate::detect::AgentState::Working,
            None,
            Some(20),
        );
    server.app.state.active = Some(1);
    server.app.state.selected = 1;
    server.app.state.mode = crate::app::Mode::Terminal;

    let (client_tx, client_control_rx, _client_rx) = test_client_writer();
    server.clients.insert(
        1,
        ClientConnection::new(
            (80, 24),
            crate::kitty_graphics::HostCellSize::default(),
            crate::terminal_theme::TerminalTheme::default(),
            None,
            1,
            RenderEncoding::SemanticFrame,
            Some(client_tx),
        ),
    );
    server.foreground_client_id = Some(1);
    server.sync_foreground_client_state();

    let (respond_to, response_rx) = std::sync::mpsc::channel();
    let changed = server.handle_api_request_with_shutdown_check(api::ApiRequestMessage {
        request: api::schema::Request {
            id: "stale".into(),
            method: api::schema::Method::PaneReportAgent(api::schema::PaneReportAgentParams {
                pane_id: public_pane_id,
                source: "herdr:pi".into(),
                agent: "pi".into(),
                state: api::schema::PaneAgentState::Idle,
                message: None,
                custom_status: None,
                seq: Some(19),
                agent_session_id: None,
                agent_session_path: None,
            }),
        },
        respond_to,
    });

    assert!(changed);
    assert!(response_rx.recv_timeout(Duration::from_millis(100)).is_ok());
    assert_eq!(
        server.app.state.terminals.get(&terminal_id).unwrap().state,
        crate::detect::AgentState::Working
    );
    assert!(
        client_control_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "stale idle report must not forward a done sound"
    );
}

/// Verify that no direct calls to `self.app.handle_internal_event`
/// exist outside of `handle_internal_event_with_forwarding` in this
/// module. This ensures the forwarding bypass cannot be reintroduced.
///
/// The search pattern looks for `handle_internal_event` calls that
/// are NOT inside the `handle_internal_event_with_forwarding` method.
#[test]
fn no_handle_internal_event_bypass_in_module() {
    let source = include_str!("headless.rs");

    // Find all lines containing handle_internal_event
    let mut bypass_lines: Vec<String> = Vec::new();
    let mut inside_forwarding_method = false;
    let mut forwarding_method_brace_depth = 0u32;

    for (i, line) in source.lines().enumerate() {
        let line_num = i + 1;

        // Track when we're inside handle_internal_event_with_forwarding
        if line.contains("fn handle_internal_event_with_forwarding") {
            inside_forwarding_method = true;
            forwarding_method_brace_depth = 0;
        }

        if inside_forwarding_method {
            // Count braces to track when we exit the method
            for ch in line.chars() {
                match ch {
                    '{' => forwarding_method_brace_depth += 1,
                    '}' => {
                        forwarding_method_brace_depth =
                            forwarding_method_brace_depth.saturating_sub(1);
                        if forwarding_method_brace_depth == 0 {
                            inside_forwarding_method = false;
                        }
                    }
                    _ => {}
                }
            }
        } else if line.contains("self.app.handle_internal_event(")
            && !line.trim().starts_with("///")
            && !line.contains("contains(")
        {
            // Direct call to handle_internal_event outside the forwarding method
            bypass_lines.push(format!("line {}: {}", line_num, line.trim()));
        }
    }

    assert!(
        bypass_lines.is_empty(),
        "Found direct calls to self.app.handle_internal_event outside \
         handle_internal_event_with_forwarding (bypass risk):\n  {}",
        bypass_lines.join("\n  ")
    );
}
