use super::*;
use crate::detect::{Agent, AgentState};
use crate::workspace::Workspace;
use ratatui::layout::Direction;

fn app_with_workspaces(names: &[&str]) -> AppState {
    let mut state = AppState::test_new();
    state.toast_config.delay_seconds = 0;
    for name in names {
        let ws = Workspace::test_new(name);
        state.workspaces.push(ws);
    }
    state.ensure_test_terminals();
    if !state.workspaces.is_empty() {
        state.active = Some(0);
        state.mode = Mode::Terminal;
    }
    state
}

fn mark_linked_worktree(state: &mut AppState, ws_idx: usize) {
    state.workspaces[ws_idx].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
        key: "repo-key".into(),
        label: "herdr".into(),
        repo_root: "/repo/herdr".into(),
        checkout_path: format!("/repo/worktree-{ws_idx}").into(),
        is_linked_worktree: true,
    });
}

fn mark_parent_worktree(state: &mut AppState, ws_idx: usize) {
    state.workspaces[ws_idx].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
        key: "repo-key".into(),
        label: "herdr".into(),
        repo_root: "/repo/herdr".into(),
        checkout_path: "/repo/herdr".into(),
        is_linked_worktree: false,
    });
}

#[test]
fn notification_context_formats_resolved_workspace_label() {
    let state = app_with_workspaces(&["stale"]);
    let root = state.workspaces[0].tabs[0].root_pane;

    assert_eq!(
        notification_context(&state.workspaces[0], "__herdr_projects__", 0, root),
        "__herdr_projects__ · 1"
    );
}

fn selected_word(row: &str, col: u16) -> Option<String> {
    let (start, end) = word_bounds_at_column(row, col)?;
    Some(text_in_cell_range(row, start, end))
}

fn selected_url<'a>(row: &'a str, click: &str) -> Option<&'a str> {
    url_at_column(row, col_of(row, click))
}

fn text_in_cell_range(row: &str, start_col: u16, end_col: u16) -> String {
    text_cells(row)
        .into_iter()
        .filter(|cell| cell.start_col >= start_col && cell.end_col <= end_col)
        .map(|cell| cell.ch)
        .collect()
}

fn col_of(row: &str, needle: &str) -> u16 {
    let byte_idx = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not found in {row:?}"));
    let prefix = &row[..byte_idx];
    prefix
        .chars()
        .map(|ch| UnicodeWidthChar::width(ch).unwrap_or(0) as u16)
        .sum()
}

fn assert_selects(row: &str, click: &str, expected: &str) {
    assert_eq!(
        selected_word(row, col_of(row, click)).as_deref(),
        Some(expected),
        "row={row:?}, click={click:?}"
    );
}

fn assert_selects_nothing(row: &str, click: &str) {
    assert_eq!(
        selected_word(row, col_of(row, click)),
        None,
        "row={row:?}, click={click:?}"
    );
}

#[test]
fn double_click_word_bounds_cover_terminal_text() {
    let cases = [
        (
            "see https://example.com/a-b_c?q=x@y.",
            "example.com",
            "https://example.com/a-b_c?q=x@y",
        ),
        (
            "open \"https://example.com/a,b;c?q=x\";",
            "example.com",
            "https://example.com/a,b;c?q=x",
        ),
        (
            "see https://en.wikipedia.org/wiki/Foo_(bar_(baz)),",
            "wikipedia",
            "https://en.wikipedia.org/wiki/Foo_(bar_(baz))",
        ),
        (
            "see https://example.com/a(b[c{d}e]f),",
            "example.com",
            "https://example.com/a(b[c{d}e]f)",
        ),
        (
            "see (https://example.com/a(b(c)d)))",
            "example.com",
            "https://example.com/a(b(c)d)",
        ),
        (
            "open /tmp/foo-bar/baz_qux/",
            "foo-bar",
            "/tmp/foo-bar/baz_qux/",
        ),
        (
            "open ./src/app/actions.rs:795",
            "actions",
            "./src/app/actions.rs:795",
        ),
        (
            "open ../herdr-worktrees/issue-1",
            "herdr",
            "../herdr-worktrees/issue-1",
        ),
        (
            "edit src/app/actions.rs,then",
            "actions",
            "src/app/actions.rs",
        ),
        (
            "cat \"/tmp/build output/log.txt\"",
            "output",
            "/tmp/build output/log.txt",
        ),
        (
            "cat '/Users/me/Library/Application Support/app/config.json'",
            "Support",
            "/Users/me/Library/Application Support/app/config.json",
        ),
        ("echo 你好-world done", "好", "你好-world"),
        ("先跑 cargo test", "cargo", "cargo"),
        (
            "export PATH=$HOME/.cargo/bin:$PATH",
            "$HOME",
            "PATH=$HOME/.cargo/bin:$PATH",
        ),
        (
            "git checkout feature/foo-bar_baz",
            "foo",
            "feature/foo-bar_baz",
        ),
        ("refs #123 and @owner/name", "#123", "#123"),
        ("refs #123 and @owner/name", "owner", "@owner/name"),
        ("cargo test --package=herdr", "--package", "--package=herdr"),
        (
            "cargo test app::actions::tests",
            "app::",
            "app::actions::tests",
        ),
        (
            "image ghcr.io/org/app:latest",
            "ghcr",
            "ghcr.io/org/app:latest",
        ),
        ("ERROR [worker-1] request_id=abc-123", "worker", "worker-1"),
        (
            "tmux|newhoo|fixhoo|newmoo|notification|window_bell|herdr",
            "newhoo",
            "newhoo",
        ),
        (
            "render_status_line(app, area)",
            "render",
            "render_status_line",
        ),
        ("render_status_line(app, area)", "app", "app"),
        ("render_status_line(app, area)", "area", "area"),
        ("if !enabled {", "enabled", "enabled"),
        ("println!(\"hi\")", "println", "println"),
        ("( master)$", "master", "master"),
        ("regex foo$", "foo", "foo$"),
    ];

    for (row, click, expected) in cases {
        assert_selects(row, click, expected);
    }

    let row = "echo 你好-world done";
    assert_eq!(
        selected_word(row, col_of(row, "好") + 1).as_deref(),
        Some("你好-world")
    );
}

#[test]
fn double_click_word_bounds_ignore_delimiters() {
    for (row, click) in [
        (
            "tmux|newhoo|fixhoo|newmoo|notification|window_bell|herdr",
            "|",
        ),
        ("alpha,beta;gamma", ","),
        ("alpha,beta;gamma", ";"),
        ("render_status_line(app, area)", "("),
        ("render_status_line(app, area)", ")"),
        ("if !enabled {", "!"),
        ("if !enabled {", "{"),
        ("(done).", "("),
        ("(done).", "."),
    ] {
        assert_selects_nothing(row, click);
    }
}

#[test]
fn url_at_column_returns_safe_visible_url_only() {
    assert_eq!(
        selected_url("see https://example.com/a(b)c.", "example"),
        Some("https://example.com/a(b)c")
    );
    assert_eq!(
        selected_url("[docs](https://example.com/docs),", "example"),
        Some("https://example.com/docs")
    );
    assert_eq!(
        selected_url("[docs](https://example.com/docs)", "docs"),
        None
    );
    assert_eq!(selected_url("open file:///tmp/report", "file"), None);
}

#[test]
fn navigator_rows_show_tab_nodes_only_for_multi_tab_workspaces() {
    let mut state = app_with_workspaces(&["single", "multi"]);
    state.workspaces[1].test_add_tab(Some("tests"));
    state.ensure_test_terminals();

    state.open_navigator();
    let rows = state.navigator_rows();

    assert!(!rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Tab { ws_idx: 0, .. }
    )));
    assert!(rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Tab {
            ws_idx: 1,
            tab_idx: 0
        }
    )));
    assert!(rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Tab {
            ws_idx: 1,
            tab_idx: 1
        }
    )));
}

#[tokio::test]
async fn navigator_rows_match_live_root_runtime_cwd_workspace_label() {
    let unique = format!(
        "herdr-navigator-runtime-cwd-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    let stale_cwd = root.join("issue-264-nix-support");
    let live_cwd = root.join("herdr");
    std::fs::create_dir_all(stale_cwd.join(".git")).unwrap();
    std::fs::create_dir_all(live_cwd.join(".git")).unwrap();

    let mut state = AppState::test_new();
    let mut workspace = Workspace::test_new("stale-name");
    workspace.custom_name = None;
    workspace.identity_cwd = stale_cwd.clone();
    let pane = workspace.tabs[0].root_pane;
    state.workspaces = vec![workspace];
    state.ensure_test_terminals();
    let terminal_id = state.workspaces[0].terminal_id(pane).cloned().unwrap();
    state.terminals.get_mut(&terminal_id).unwrap().cwd = stale_cwd;

    let (events, _) = tokio::sync::mpsc::channel(4);
    let runtime = crate::terminal::TerminalRuntime::spawn(
        pane,
        24,
        80,
        live_cwd.clone(),
        0,
        crate::terminal_theme::TerminalTheme::default(),
        crate::pane::PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::NonLogin),
        &crate::pane::PaneLaunchEnv::default(),
        events,
        std::sync::Arc::new(tokio::sync::Notify::new()),
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }

    let mut runtime_registry = crate::terminal::TerminalRuntimeRegistry::new();
    runtime_registry.insert(terminal_id, runtime);
    state.open_navigator_from(&runtime_registry);
    state.navigator.query = "herdr".into();
    let rows = state.navigator_rows_from(&runtime_registry);

    for (_, runtime) in runtime_registry.drain() {
        runtime.shutdown();
    }
    let _ = std::fs::remove_dir_all(root);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "herdr (1)");
}

#[test]
fn navigator_rows_include_shell_and_agent_panes() {
    let mut state = app_with_workspaces(&["one"]);
    let shell = state.workspaces[0].tabs[0].root_pane;
    let agent = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();

    let agent_terminal_id = state.workspaces[0].terminal_id(agent).cloned().unwrap();
    let terminal = state.terminals.get_mut(&agent_terminal_id).unwrap();
    terminal.set_detected_state(Some(Agent::Claude), AgentState::Working);

    state.open_navigator();
    let rows = state.navigator_rows();

    assert!(rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == shell
    )));
    assert!(rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == agent
    ) && row.meta.contains("claude")));
}

#[test]
fn opening_navigator_selects_current_pane_and_expands_attention_workspaces() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let blocked = state.workspaces[1].tabs[0].root_pane;
    let blocked_terminal_id = state.workspaces[1].terminal_id(blocked).cloned().unwrap();
    state
        .terminals
        .get_mut(&blocked_terminal_id)
        .unwrap()
        .set_detected_state(Some(Agent::Codex), AgentState::Blocked);

    state.open_navigator();
    let selected = state.navigator_rows()[state.navigator.selected].clone();

    assert!(selected.is_current);
    assert!(state
        .navigator
        .expanded_workspaces
        .contains(&state.workspaces[0].id));
    assert!(state
        .navigator
        .expanded_workspaces
        .contains(&state.workspaces[1].id));
}

#[test]
fn accepting_navigator_pane_switches_workspace_tab_and_focus() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let target = state.workspaces[1].tabs[0].root_pane;
    state.open_navigator();
    state
        .navigator
        .expanded_workspaces
        .insert(state.workspaces[1].id.clone());
    state.navigator.selected = state
        .navigator_rows()
        .iter()
        .position(|row| {
            matches!(
                row.target,
                crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == target
            )
        })
        .unwrap();

    assert!(state.accept_navigator_selection());

    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].focused_pane_id(), Some(target));
    assert_eq!(state.mode, Mode::Terminal);
}

#[test]
fn navigator_idle_search_matches_idle_agents_not_plain_shells() {
    let mut state = app_with_workspaces(&["one"]);
    let shell = state.workspaces[0].tabs[0].root_pane;
    let agent = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();

    let agent_terminal_id = state.workspaces[0].terminal_id(agent).cloned().unwrap();
    state
        .terminals
        .get_mut(&agent_terminal_id)
        .unwrap()
        .set_detected_state(Some(Agent::Claude), AgentState::Idle);

    state.open_navigator();
    state.navigator.query = "idle".into();
    let rows = state.navigator_rows();

    assert!(rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == agent
    )));
    assert!(!rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == shell
    )));
}

#[test]
fn navigator_search_only_matches_visible_row_text() {
    let mut state = app_with_workspaces(&["one"]);
    state.workspaces[0].identity_cwd = "/tmp/herdr-worktrees/issue-work".into();

    state.open_navigator();
    state.navigator.query = "work".into();

    assert!(state.navigator_rows().is_empty());
}

#[test]
fn navigator_state_filter_is_separate_from_text_search() {
    let mut state = app_with_workspaces(&["one"]);
    let shell = state.workspaces[0].tabs[0].root_pane;
    let working = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();

    let shell_terminal_id = state.workspaces[0].terminal_id(shell).cloned().unwrap();
    state
        .terminals
        .get_mut(&shell_terminal_id)
        .unwrap()
        .set_manual_label("wheel notes".into());
    let working_terminal_id = state.workspaces[0].terminal_id(working).cloned().unwrap();
    state
        .terminals
        .get_mut(&working_terminal_id)
        .unwrap()
        .set_detected_state(Some(Agent::Codex), AgentState::Working);

    state.open_navigator();
    state.navigator.state_filter = Some(NavigatorStateFilter::Working);
    let state_rows = state.navigator_rows();

    assert!(state_rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == working
    )));
    assert!(!state_rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == shell
    )));

    state.navigator.state_filter = None;
    state.navigator.query = "w".into();
    let text_rows = state.navigator_rows();

    assert!(text_rows.iter().any(|row| matches!(
        row.target,
        crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == shell
    )));
    assert!(
        text_rows.iter().any(|row| matches!(
            row.target,
            crate::app::state::NavigatorTarget::Pane { pane_id, .. } if pane_id == working
        )),
        "literal one-letter search may still match visible state text"
    );
}

#[test]
fn navigator_search_filters_panes_but_keeps_workspace_context() {
    let mut state = app_with_workspaces(&["one"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    let terminal_id = state.workspaces[0].terminal_id(root).cloned().unwrap();
    state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_manual_label("weekly review".into());
    state.open_navigator();
    state.navigator.query = "weekly".into();

    let rows = state.navigator_rows();

    assert!(rows.iter().any(|row| row.is_workspace));
    assert!(rows
        .iter()
        .any(|row| !row.is_workspace && row.label.contains("weekly")));
}

#[test]
fn apply_workspace_git_statuses_updates_matching_workspace() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let first_id = state.workspaces[0].id.clone();
    let first_cwd = state.workspaces[0].resolved_identity_cwd().unwrap();
    let second_id = state.workspaces[1].id.clone();

    let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
    let changed = state.apply_workspace_git_statuses(
        &terminal_runtimes,
        vec![WorkspaceGitStatus {
            workspace_id: first_id,
            resolved_identity_cwd: first_cwd,
            branch: Some("main".into()),
            ahead_behind: Some((2, 1)),
            space: None,
            change_set: None,
        }],
    );

    assert!(changed);
    assert_eq!(state.workspaces[0].branch().as_deref(), Some("main"));
    assert_eq!(state.workspaces[0].git_ahead_behind(), Some((2, 1)));
    assert_eq!(state.workspaces[1].id, second_id);
    assert_eq!(state.workspaces[1].git_ahead_behind(), None);
}

#[test]
fn apply_workspace_git_statuses_ignores_stale_cwd() {
    let mut state = app_with_workspaces(&["one"]);
    let workspace_id = state.workspaces[0].id.clone();
    state.workspaces[0].cached_git_branch = Some("old".into());
    state.workspaces[0].cached_git_ahead_behind = Some((1, 0));

    let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
    let changed = state.apply_workspace_git_statuses(
        &terminal_runtimes,
        vec![WorkspaceGitStatus {
            workspace_id,
            resolved_identity_cwd: std::path::PathBuf::from("/definitely/not/current"),
            branch: Some("main".into()),
            ahead_behind: Some((0, 1)),
            space: None,
            change_set: None,
        }],
    );

    assert!(!changed);
    assert_eq!(state.workspaces[0].branch().as_deref(), Some("old"));
    assert_eq!(state.workspaces[0].git_ahead_behind(), Some((1, 0)));
}

#[test]
fn apply_workspace_git_statuses_clears_missing_git_status() {
    let mut state = app_with_workspaces(&["one"]);
    let workspace_id = state.workspaces[0].id.clone();
    let cwd = state.workspaces[0].resolved_identity_cwd().unwrap();
    state.workspaces[0].cached_git_branch = Some("main".into());
    state.workspaces[0].cached_git_ahead_behind = Some((1, 2));

    let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
    let changed = state.apply_workspace_git_statuses(
        &terminal_runtimes,
        vec![WorkspaceGitStatus {
            workspace_id,
            resolved_identity_cwd: cwd,
            branch: None,
            ahead_behind: None,
            space: None,
            change_set: None,
        }],
    );

    assert!(changed);
    assert_eq!(state.workspaces[0].branch(), None);
    assert_eq!(state.workspaces[0].git_ahead_behind(), None);
}

#[test]
fn apply_workspace_git_statuses_does_not_change_worktree_membership() {
    let mut state = app_with_workspaces(&["one"]);
    mark_linked_worktree(&mut state, 0);
    let workspace_id = state.workspaces[0].id.clone();
    let cwd = state.workspaces[0].resolved_identity_cwd().unwrap();
    let membership = state.workspaces[0].worktree_space().cloned();

    let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
    let changed = state.apply_workspace_git_statuses(
        &terminal_runtimes,
        vec![WorkspaceGitStatus {
            workspace_id,
            resolved_identity_cwd: cwd,
            branch: Some("scratch".into()),
            ahead_behind: None,
            space: Some(crate::workspace::GitSpaceMetadata {
                key: "other-repo-key".into(),
                checkout_key: "/other/checkout".into(),
                label: "other".into(),
                repo_root: "/other/repo".into(),
                is_linked_worktree: false,
            }),
            change_set: None,
        }],
    );

    assert!(changed);
    assert_eq!(state.workspaces[0].worktree_space().cloned(), membership);
}

fn mark_agent(state: &mut AppState, ws_idx: usize, tab_idx: usize, pane_id: PaneId) {
    set_agent_state(state, ws_idx, tab_idx, pane_id, AgentState::Idle);
}

fn set_agent_state(
    state: &mut AppState,
    ws_idx: usize,
    tab_idx: usize,
    pane_id: PaneId,
    agent_state: AgentState,
) {
    state.ensure_test_terminals();
    let terminal_id = state.workspaces[ws_idx].tabs[tab_idx]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    if let Some(terminal) = state.terminals.get_mut(&terminal_id) {
        terminal.set_detected_state(Some(Agent::Pi), agent_state);
    }
}

fn transition_agent_state(state: &mut AppState, pane_id: PaneId, agent_state: AgentState) {
    state
        .update_terminal_state(pane_id, |terminal| {
            Some(terminal.set_detected_state_with_screen_signals_at(
                Some(Agent::Pi),
                agent_state,
                matches!(agent_state, AgentState::Blocked),
                false,
                false,
                false,
                std::time::Instant::now(),
            ))
        })
        .expect("agent state transition should update pane state");
}

#[test]
fn next_agent_cycles_agent_panel_entries() {
    let mut first = Workspace::test_new("one");
    let first_root = first.tabs[0].root_pane;
    let first_second = first.test_split(Direction::Horizontal);
    first.tabs[0].layout.focus_pane(first_root);
    let second = Workspace::test_new("two");
    let second_root = second.tabs[0].root_pane;

    let mut state = AppState::test_new();
    state.workspaces = vec![first, second];
    state.ensure_test_terminals();
    state.active = Some(0);
    state.selected = 0;
    state.mode = Mode::Terminal;
    mark_agent(&mut state, 0, 0, first_root);
    mark_agent(&mut state, 0, 0, first_second);
    mark_agent(&mut state, 1, 0, second_root);

    state.next_agent();
    assert_eq!(state.active, Some(0));
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(first_second));

    state.next_agent();
    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].focused_pane_id(), Some(second_root));

    state.previous_agent();
    assert_eq!(state.active, Some(0));
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(first_second));
    state.assert_invariants_for_test();
}

#[test]
fn focus_agent_entry_uses_agent_panel_order() {
    let mut first = Workspace::test_new("one");
    let first_root = first.tabs[0].root_pane;
    let first_second = first.test_split(Direction::Horizontal);
    first.tabs[0].layout.focus_pane(first_root);
    let second = Workspace::test_new("two");
    let second_root = second.tabs[0].root_pane;

    let mut state = AppState::test_new();
    state.workspaces = vec![first, second];
    state.active = Some(0);
    state.selected = 0;
    state.mode = Mode::Terminal;
    mark_agent(&mut state, 0, 0, first_root);
    mark_agent(&mut state, 0, 0, first_second);
    mark_agent(&mut state, 1, 0, second_root);

    assert!(state.focus_agent_entry(2));

    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].focused_pane_id(), Some(second_root));
    state.assert_invariants_for_test();
}

#[test]
fn focus_agent_entry_succeeds_for_already_focused_agent() {
    let mut state = app_with_workspaces(&["one"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    mark_agent(&mut state, 0, 0, root);

    assert!(state.focus_agent_entry(0));
    assert_eq!(state.active, Some(0));
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(root));
    state.assert_invariants_for_test();
}

#[test]
fn next_agent_cycles_priority_sorted_agent_panel_entries() {
    let mut first = Workspace::test_new("one");
    let first_root = first.tabs[0].root_pane;
    let first_second = first.test_split(Direction::Horizontal);
    first.tabs[0].layout.focus_pane(first_root);
    let second = Workspace::test_new("two");
    let second_root = second.tabs[0].root_pane;

    let mut state = AppState::test_new();
    state.workspaces = vec![first, second];
    state.ensure_test_terminals();
    state.active = Some(0);
    state.selected = 0;
    state.mode = Mode::Terminal;
    state.agent_panel_sort = crate::app::state::AgentPanelSort::Priority;
    set_agent_state(&mut state, 0, 0, first_root, AgentState::Idle);
    set_agent_state(&mut state, 0, 0, first_second, AgentState::Working);
    set_agent_state(&mut state, 1, 0, second_root, AgentState::Blocked);

    state.next_agent();

    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].focused_pane_id(), Some(second_root));
    state.assert_invariants_for_test();
}

#[test]
fn priority_sort_keeps_recently_changed_idle_agent_above_older_idle_agent() {
    let mut workspace = Workspace::test_new("one");
    let first = workspace.tabs[0].root_pane;
    let second = workspace.test_split(Direction::Horizontal);
    workspace.tabs[0].layout.focus_pane(first);

    let mut state = AppState::test_new();
    state.workspaces = vec![workspace];
    state.ensure_test_terminals();
    state.active = Some(0);
    state.selected = 0;
    state.mode = Mode::Terminal;
    state.agent_panel_sort = crate::app::state::AgentPanelSort::Priority;

    transition_agent_state(&mut state, first, AgentState::Idle);
    transition_agent_state(&mut state, second, AgentState::Working);
    assert_eq!(crate::ui::agent_panel_entries(&state)[0].pane_id, second);

    transition_agent_state(&mut state, second, AgentState::Idle);

    assert_eq!(crate::ui::agent_panel_entries(&state)[0].pane_id, second);
    state.assert_invariants_for_test();
}

#[test]
fn previous_agent_keeps_wrapped_target_visible_in_agent_panel() {
    let mut workspace = Workspace::test_new("one");
    let root = workspace.tabs[0].root_pane;
    for idx in 1..8 {
        workspace.test_add_tab(Some(&format!("tab-{idx}")));
    }

    let mut state = AppState::test_new();
    state.workspaces = vec![workspace];
    state.ensure_test_terminals();
    state.active = Some(0);
    state.selected = 0;
    state.mode = Mode::Terminal;
    for tab_idx in 0..state.workspaces[0].tabs.len() {
        let pane_id = state.workspaces[0].tabs[tab_idx].root_pane;
        mark_agent(&mut state, 0, tab_idx, pane_id);
    }
    state.workspaces[0].tabs[0].layout.focus_pane(root);
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

    state.previous_agent();

    let last_idx = state.workspaces[0].tabs.len() - 1;
    assert_eq!(state.workspaces[0].active_tab, last_idx);
    assert!(state.agent_panel_scroll > 0);
    state.assert_invariants_for_test();
}

#[test]
fn switch_workspace_updates_active_and_selected() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    state.switch_workspace(2);
    assert_eq!(state.active, Some(2));
    assert_eq!(state.selected, 2);
}

#[test]
fn last_pane_toggles_to_previous_focus_in_active_tab() {
    let mut state = app_with_workspaces(&["test"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    let right = state.workspaces[0].test_split(Direction::Horizontal);

    state.focus_pane_in_workspace(0, root);
    state.focus_pane_in_workspace(0, right);
    state.last_pane();

    assert_eq!(state.workspaces[0].focused_pane_id(), Some(root));

    state.last_pane();

    assert_eq!(state.workspaces[0].focused_pane_id(), Some(right));
}

#[test]
fn removing_background_pane_preserves_last_pane_history() {
    let mut state = app_with_workspaces(&["test"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    let right = state.workspaces[0].test_split(Direction::Horizontal);
    let background = state.workspaces[0].test_split(Direction::Horizontal);

    state.focus_pane_in_workspace(0, root);
    state.focus_pane_in_workspace(0, right);
    state.workspaces[0].remove_pane(background);
    state.last_pane();

    assert_eq!(state.workspaces[0].focused_pane_id(), Some(root));
}

#[test]
fn last_pane_jumps_across_workspaces_and_tabs() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let first_root = state.workspaces[0].tabs[0].root_pane;
    let second_tab = state.workspaces[1].test_add_tab(Some("logs"));
    let second_tab_root = state.workspaces[1].tabs[second_tab].root_pane;

    state.focus_pane_in_workspace(0, first_root);
    state.focus_pane_in_workspace(1, second_tab_root);
    state.last_pane();

    assert_eq!(state.active, Some(0));
    assert_eq!(state.workspaces[0].active_tab, 0);
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(first_root));

    state.last_pane();

    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].active_tab, second_tab);
    assert_eq!(state.workspaces[1].focused_pane_id(), Some(second_tab_root));
}

#[test]
fn last_pane_tracks_tab_and_workspace_switches() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let first_root = state.workspaces[0].tabs[0].root_pane;
    let first_second_tab = state.workspaces[0].test_add_tab(Some("logs"));
    let first_second_root = state.workspaces[0].tabs[first_second_tab].root_pane;
    let second_root = state.workspaces[1].tabs[0].root_pane;

    state.switch_tab(first_second_tab);
    state.last_pane();

    assert_eq!(state.active, Some(0));
    assert_eq!(state.workspaces[0].active_tab, 0);
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(first_root));

    state.last_pane();

    assert_eq!(state.active, Some(0));
    assert_eq!(state.workspaces[0].active_tab, first_second_tab);
    assert_eq!(
        state.workspaces[0].focused_pane_id(),
        Some(first_second_root)
    );

    state.switch_workspace(1);
    state.last_pane();

    assert_eq!(state.active, Some(0));
    assert_eq!(state.workspaces[0].active_tab, first_second_tab);
    assert_eq!(
        state.workspaces[0].focused_pane_id(),
        Some(first_second_root)
    );

    state.last_pane();

    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].focused_pane_id(), Some(second_root));
}

#[test]
fn last_pane_tracks_cross_workspace_tab_selection() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let first_root = state.workspaces[0].tabs[0].root_pane;
    let second_first_root = state.workspaces[1].tabs[0].root_pane;
    let second_tab = state.workspaces[1].test_add_tab(Some("logs"));
    let second_tab_root = state.workspaces[1].tabs[second_tab].root_pane;

    state.switch_workspace_tab(1, second_tab);
    state.last_pane();

    assert_eq!(state.active, Some(0));
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(first_root));

    state.last_pane();

    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].active_tab, second_tab);
    assert_eq!(state.workspaces[1].focused_pane_id(), Some(second_tab_root));
    assert_ne!(second_first_root, second_tab_root);
}

#[test]
fn switch_workspace_keeps_selected_visible_in_scrolled_sidebar() {
    let mut state = app_with_workspaces(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

    state.switch_workspace(7);
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 80, 14));

    assert!(state
        .view
        .workspace_card_areas
        .iter()
        .any(|card| card.ws_idx == 7));
}

#[test]
fn switch_workspace_marks_panes_seen() {
    let mut state = app_with_workspaces(&["a", "b"]);
    // Mark a pane in workspace 1 as unseen
    let id = *state.workspaces[1].panes.keys().next().unwrap();
    state.workspaces[1].panes.get_mut(&id).unwrap().seen = false;

    state.switch_workspace(1);
    assert!(state.workspaces[1].panes.get(&id).unwrap().seen);
}

#[test]
fn switch_workspace_out_of_bounds_is_noop() {
    let mut state = app_with_workspaces(&["a"]);
    state.switch_workspace(5);
    assert_eq!(state.active, Some(0));
}

#[test]
fn move_workspace_reorders_without_changing_logical_selection() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    let active_id = state.workspaces[1].id.clone();
    let selected_id = state.workspaces[2].id.clone();
    state.active = Some(1);
    state.selected = 2;

    state.move_workspace(1, 0);

    let names: Vec<_> = state
        .workspaces
        .iter()
        .map(crate::workspace::Workspace::display_name)
        .collect();
    assert_eq!(names, vec!["b", "a", "c"]);
    assert_eq!(state.active, Some(0));
    assert_eq!(state.selected, 2);
    assert_eq!(state.workspaces[state.active.unwrap()].id, active_id);
    assert_eq!(state.workspaces[state.selected].id, selected_id);
}

#[test]
fn move_workspace_accepts_insert_at_end() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);

    state.move_workspace(0, state.workspaces.len());

    let names: Vec<_> = state
        .workspaces
        .iter()
        .map(crate::workspace::Workspace::display_name)
        .collect();
    assert_eq!(names, vec!["b", "c", "a"]);
}

#[test]
fn close_workspace_adjusts_indices() {
    let mut state = app_with_workspaces(&["a", "b", "c"]);
    state.selected = 1;
    state.active = Some(1);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 2);
    assert_eq!(state.selected, 1);
    assert_eq!(state.active, Some(1));
    assert_eq!(state.workspaces[1].custom_name.as_deref(), Some("c"));
}

#[test]
fn close_parent_worktree_workspace_closes_group() {
    let mut state = app_with_workspaces(&["main", "issue", "notes"]);
    state.workspaces[0].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
        key: "repo-key".into(),
        label: "herdr".into(),
        repo_root: "/repo/herdr".into(),
        checkout_path: "/repo/herdr".into(),
        is_linked_worktree: false,
    });
    state.workspaces[1].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
        key: "repo-key".into(),
        label: "herdr".into(),
        repo_root: "/repo/herdr".into(),
        checkout_path: "/repo/herdr-issue".into(),
        is_linked_worktree: true,
    });
    state.selected = 0;
    state.active = Some(0);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].display_name(), "notes");
    assert_eq!(state.active, Some(0));
    assert_eq!(state.selected, 0);
}

#[test]
fn close_last_workspace_clears_active() {
    let mut state = app_with_workspaces(&["only"]);
    state.selected = 0;
    state.close_selected_workspace();

    assert!(state.workspaces.is_empty());
    assert_eq!(state.active, None);
    assert_eq!(state.selected, 0);
}

#[test]
fn close_workspace_at_end_adjusts_selected() {
    let mut state = app_with_workspaces(&["a", "b"]);
    state.selected = 1;
    state.active = Some(1);

    state.close_selected_workspace();

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.selected, 0);
    assert_eq!(state.active, Some(0));
}

#[test]
fn pane_died_last_pane_removes_workspace() {
    let mut state = app_with_workspaces(&["a", "b"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_pane_died(pane_id);

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].custom_name.as_deref(), Some("b"));
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_last_workspace_enters_navigate() {
    let mut state = app_with_workspaces(&["only"]);
    state.mode = Mode::Terminal;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_pane_died(pane_id);

    assert!(state.workspaces.is_empty());
    assert_eq!(state.mode, Mode::Navigate);
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_multi_pane_keeps_workspace() {
    let mut state = app_with_workspaces(&["test"]);
    let second_id = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();

    state.handle_pane_died(second_id);

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].panes.len(), 1);
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_unknown_pane_is_noop() {
    let mut state = app_with_workspaces(&["test"]);
    let fake_id = PaneId::from_raw(9999);

    state.handle_pane_died(fake_id);

    assert_eq!(state.workspaces.len(), 1);
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_unrelated_pane_preserves_selection() {
    // Two workspaces; user is selecting text in workspace 0.
    // A pane in workspace 1 dies — selection must be preserved.
    let mut state = app_with_workspaces(&["active", "bg"]);
    let active_pane = *state.workspaces[0].panes.keys().next().unwrap();
    let bg_pane = *state.workspaces[1].panes.keys().next().unwrap();

    state.selection = Some(crate::selection::Selection::anchor(active_pane, 0, 0, None));
    state.selection_autoscroll = Some(crate::app::state::SelectionAutoscroll {
        direction: crate::app::state::SelectionAutoscrollDirection::Down,
        last_mouse_screen_col: 0,
        last_mouse_screen_row: 23,
        inner_rect: ratatui::layout::Rect::new(0, 0, 80, 24),
    });

    state.handle_pane_died(bg_pane);

    assert!(state.selection.is_some());
    assert!(state.selection_autoscroll.is_some());
    state.assert_invariants_for_test();
}

#[test]
fn pane_died_same_pane_clears_selection() {
    let mut state = app_with_workspaces(&["test"]);
    let first_id = state.workspaces[0].tabs[0].root_pane;
    let second_id = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();

    state.selection = Some(crate::selection::Selection::anchor(second_id, 0, 0, None));
    state.selection_autoscroll = Some(crate::app::state::SelectionAutoscroll {
        direction: crate::app::state::SelectionAutoscrollDirection::Down,
        last_mouse_screen_col: 0,
        last_mouse_screen_row: 23,
        inner_rect: ratatui::layout::Rect::new(0, 0, 80, 24),
    });

    state.handle_pane_died(second_id);

    // first_id still alive, workspace stays, but selection was on the dying pane
    assert!(state.selection.is_none());
    assert!(state.selection_autoscroll.is_none());
    assert_eq!(state.workspaces[0].panes.len(), 1);
    assert_eq!(state.workspaces[0].panes.keys().next().unwrap(), &first_id);
    state.assert_invariants_for_test();
}

#[test]
fn state_changed_updates_pane() {
    let mut state = app_with_workspaces(&["test"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Working,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Working);
    assert_eq!(terminal.detected_agent, Some(Agent::Pi));
}

#[test]
fn state_changed_idle_in_background_marks_unseen() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    // First set it to Working
    let bg_terminal_id = state.workspaces[1]
        .panes
        .get(&bg_pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&bg_terminal_id).unwrap().state = AgentState::Working;

    // Now transition to Idle while in background
    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(!pane.seen);
    assert!(matches!(
        state.toast.as_ref().map(|toast| toast.kind),
        Some(ToastKind::Finished)
    ));
}

#[test]
fn active_tab_completion_marks_pane_seen() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(true);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&terminal_id).unwrap().state = AgentState::Working;
    state.workspaces[0].panes.get_mut(&pane_id).unwrap().seen = false;

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Idle);
    let pane = state.workspaces[0].panes.get(&pane_id).unwrap();
    assert!(pane.seen);
}

#[test]
fn initial_idle_in_background_stays_seen() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(pane.seen);
}

#[test]
fn idle_after_known_unknown_agent_in_background_marks_done() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.active = Some(0);
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Unknown,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Idle,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let pane = state.workspaces[1].panes.get(&bg_pane_id).unwrap();
    assert!(!pane.seen);
}

#[test]
fn waiting_sound_plays_even_in_active_workspace() {
    assert_eq!(
        notification_sound_for_state_change(true, AgentState::Working, AgentState::Blocked),
        Some(crate::sound::Sound::Request)
    );
}

#[test]
fn done_sound_only_plays_in_background() {
    assert_eq!(
        notification_sound_for_state_change(false, AgentState::Working, AgentState::Idle),
        Some(crate::sound::Sound::Done)
    );
    assert_eq!(
        notification_sound_for_state_change(true, AgentState::Working, AgentState::Idle),
        None
    );
    assert_eq!(
        notification_sound_for_state_change(false, AgentState::Unknown, AgentState::Idle),
        None
    );
}

#[test]
fn background_waiting_sets_attention_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2");
}

#[test]
fn delayed_background_waiting_schedules_before_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.toast.is_none());
    assert!(state.pending_agent_notifications.contains_key(&bg_pane_id));

    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    let deliveries = state.drain_due_agent_notifications(deadline);
    assert_eq!(deliveries.len(), 1);

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2");
    assert!(state.pending_agent_notifications.is_empty());
}

#[test]
fn delayed_background_waiting_cancels_when_agent_resumes_working() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    let deadline = state.next_pending_agent_notification_deadline().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Working,
        visible_blocker: false,
        visible_working: true,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.pending_agent_notifications.is_empty());
    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn delayed_background_waiting_is_suppressed_if_pane_becomes_active() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    state.active = Some(1);

    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn delayed_active_tab_unfocused_keeps_client_notification_available() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(false);
    state.toast_config.delivery = crate::config::ToastDelivery::System;
    state.toast_config.delay_seconds = 1;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    let deliveries = state.drain_due_agent_notifications(deadline);

    assert_eq!(deliveries.len(), 1);
    assert!(deliveries[0].toast.is_none());
    assert!(deliveries[0].client_notification.is_some());
    assert!(state.toast.is_none());
}

#[test]
fn delayed_background_waiting_is_cleared_when_pane_dies() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.toast_config.delay_seconds = 1;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    let deadline = state.next_pending_agent_notification_deadline().unwrap();
    state.handle_app_event(AppEvent::PaneDied {
        pane_id: bg_pane_id,
    });

    assert!(state.pending_agent_notifications.is_empty());
    assert!(state.drain_due_agent_notifications(deadline).is_empty());
    assert!(state.toast.is_none());
}

#[test]
fn hook_reported_unknown_agent_sets_toast_title_from_label() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::HookStateReported {
        pane_id: bg_pane_id,
        source: "custom:hermes".into(),
        agent_label: "hermes".into(),
        state: AgentState::Blocked,
        message: None,
        custom_status: None,
        seq: None,
        session_ref: None,
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "hermes needs attention");
    assert_eq!(toast.context, "background · 2");
}

#[test]
fn visible_blocker_overrides_hook_working_and_notifies() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();
    let bg_terminal_id = state.workspaces[1]
        .panes
        .get(&bg_pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Codex),
        state: AgentState::Idle,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    state.handle_app_event(AppEvent::HookStateReported {
        pane_id: bg_pane_id,
        source: "herdr:codex".into(),
        agent_label: "codex".into(),
        state: AgentState::Working,
        message: None,
        custom_status: None,
        seq: Some(1),
        session_ref: None,
    });
    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Codex),
        state: AgentState::Blocked,
        visible_blocker: true,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let terminal = state.terminals.get(&bg_terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Blocked);
    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "codex needs attention");
}

#[test]
fn reserved_native_state_report_does_not_override_screen_state() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Claude),
        state: AgentState::Working,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    state.handle_app_event(AppEvent::HookStateReported {
        pane_id,
        source: "herdr:claude".into(),
        agent_label: "claude".into(),
        state: AgentState::Blocked,
        message: None,
        custom_status: None,
        seq: Some(1),
        session_ref: crate::agent_resume::AgentSessionRef::id("claude-session"),
    });
    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Working);
    assert!(terminal.hook_authority.is_none());
    assert!(terminal.persisted_agent_session.is_some());

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Claude),
        state: AgentState::Idle,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Idle);
    assert!(state.toast.is_none());
}

#[test]
fn reserved_native_release_report_does_not_clear_screen_state() {
    let mut state = app_with_workspaces(&["active"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Claude),
        state: AgentState::Working,
        visible_blocker: false,
        visible_working: true,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    state.handle_app_event(AppEvent::HookAgentReleased {
        pane_id,
        source: "herdr:claude".into(),
        agent_label: "claude".into(),
        known_agent: Some(Agent::Claude),
        seq: Some(1),
    });

    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Working);
    assert_eq!(terminal.detected_agent, Some(Agent::Claude));
}

#[test]
fn devin_state_report_refreshes_session_without_overriding_screen_state() {
    let mut state = app_with_workspaces(&["active"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .panes
        .get(&pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Devin),
        state: AgentState::Idle,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });
    state.handle_app_event(AppEvent::HookStateReported {
        pane_id,
        source: "herdr:devin".into(),
        agent_label: "devin".into(),
        state: AgentState::Working,
        message: None,
        custom_status: None,
        seq: Some(1),
        session_ref: crate::agent_resume::AgentSessionRef::id("devin-session"),
    });

    let terminal = state.terminals.get(&terminal_id).unwrap();
    assert_eq!(terminal.state, AgentState::Idle);
    assert!(terminal.hook_authority.is_none());
    assert!(terminal.persisted_agent_session.is_some());
}

#[test]
fn hidden_custom_session_ref_only_update_marks_session_dirty_without_visible_update() {
    let mut state = app_with_workspaces(&["active"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let test_dir = std::env::current_dir().unwrap();
    let first_session = test_dir.join("one.jsonl").display().to_string();
    let second_session = test_dir.join("two.jsonl").display().to_string();

    let first_updates = state.handle_app_event(AppEvent::HookStateReported {
        pane_id,
        source: "custom:pi".into(),
        agent_label: "pi".into(),
        state: AgentState::Working,
        message: None,
        custom_status: None,
        seq: Some(20),
        session_ref: crate::agent_resume::AgentSessionRef::path(first_session),
    });
    assert_eq!(first_updates.len(), 1);
    state.session_dirty = false;

    let second_updates = state.handle_app_event(AppEvent::HookStateReported {
        pane_id,
        source: "custom:pi".into(),
        agent_label: "pi".into(),
        state: AgentState::Working,
        message: None,
        custom_status: None,
        seq: Some(21),
        session_ref: crate::agent_resume::AgentSessionRef::path(second_session),
    });

    assert!(second_updates.is_empty());
    assert!(state.session_dirty);
}

#[test]
fn terminal_cwd_report_updates_terminal_cwd_and_marks_session_dirty() {
    let mut state = app_with_workspaces(&["active"]);
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();
    let terminal_id = state.workspaces[0]
        .pane_state(pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    let cwd = std::env::temp_dir().join(format!("herdr-cwd-report-test-{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();
    state.session_dirty = false;

    let updates = state.handle_app_event(AppEvent::TerminalCwdReported {
        pane_id,
        cwd: cwd.clone(),
    });

    assert!(updates.is_empty());
    assert_eq!(state.terminals.get(&terminal_id).unwrap().cwd, cwd);
    assert!(state.session_dirty);
    let _ = std::fs::remove_dir_all(cwd);
}

#[test]
fn background_idle_sets_finished_toast() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let bg_pane_id = *state.workspaces[1].panes.keys().next().unwrap();
    let bg_terminal_id = state.workspaces[1]
        .panes
        .get(&bg_pane_id)
        .unwrap()
        .attached_terminal_id
        .clone();
    state.terminals.get_mut(&bg_terminal_id).unwrap().state = AgentState::Working;

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Droid),
        state: AgentState::Idle,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::Finished);
    assert_eq!(toast.title, "droid finished");
    assert_eq!(toast.context, "background · 2");
    let target = toast.target.as_ref().expect("toast target");
    assert_eq!(&target.workspace_id, &state.workspaces[1].id);
    assert_eq!(target.pane_id, bg_pane_id);
}

#[test]
fn background_toast_includes_tab_name_when_workspace_has_multiple_tabs() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.workspaces[1].tabs[0].set_custom_name("main".into());
    let second_tab = state.workspaces[1].test_add_tab(Some("logs"));
    state.ensure_test_terminals();
    let bg_pane_id = state.workspaces[1].tabs[second_tab].root_pane;

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "background · 2 · logs");
}

#[test]
fn background_tab_in_active_workspace_still_sets_toast() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.workspaces[0].tabs[0].set_custom_name("main".into());
    let second_tab = state.workspaces[0].test_add_tab(Some("logs"));
    state.ensure_test_terminals();
    let bg_pane_id = state.workspaces[0].tabs[second_tab].root_pane;

    state.handle_app_event(AppEvent::StateChanged {
        pane_id: bg_pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    let toast = state.toast.as_ref().unwrap();
    assert_eq!(toast.kind, ToastKind::NeedsAttention);
    assert_eq!(toast.title, "pi needs attention");
    assert_eq!(toast.context, "active · 1 · logs");
}

#[test]
fn active_workspace_active_tab_does_not_set_toast() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.toast.is_none());
}

#[test]
fn active_workspace_active_tab_keeps_herdr_toast_suppressed_when_outer_terminal_is_unfocused() {
    let mut state = app_with_workspaces(&["active"]);
    state.active = Some(0);
    state.outer_terminal_focus = Some(false);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let pane_id = *state.workspaces[0].panes.keys().next().unwrap();

    state.handle_app_event(AppEvent::StateChanged {
        pane_id,
        agent: Some(Agent::Pi),
        state: AgentState::Blocked,
        visible_blocker: false,
        visible_working: false,
        process_exited: false,
        observed_at: std::time::Instant::now(),
    });

    assert!(state.toast.is_none());
}

#[test]
fn active_tab_suppression_preserves_unknown_focus_behavior() {
    assert!(active_tab_suppresses_notifications(true, None));
    assert!(active_tab_suppresses_notifications(true, Some(true)));
    assert!(!active_tab_suppresses_notifications(true, Some(false)));
    assert!(!active_tab_suppresses_notifications(false, None));
}

#[test]
fn update_ready_sets_manual_update_toast() {
    let mut state = AppState::test_new();
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;

    let updates = state.handle_app_event(AppEvent::UpdateReady {
        version: "0.5.0".into(),
        install_command: "bora update".into(),
    });

    assert!(updates.is_empty());
    assert_eq!(state.update_available.as_deref(), Some("0.5.0"));
    assert!(state.latest_release_notes_available);
    assert!(state.update_dismissed);
    let toast = state.toast.as_ref().expect("update toast");
    assert_eq!(toast.kind, ToastKind::UpdateInstalled);
    assert_eq!(toast.title, "v0.5.0 available");
    assert_eq!(
        toast.context,
        "detach, run `bora update`, then follow its restart guidance"
    );
}

#[test]
fn update_ready_uses_event_install_command_in_toast() {
    let mut state = AppState::test_new();
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;

    state.handle_app_event(AppEvent::UpdateReady {
        version: "0.5.0".into(),
        install_command: "brew update && brew upgrade herdr".into(),
    });

    assert_eq!(
        state.update_install_command,
        "brew update && brew upgrade herdr"
    );
    let toast = state.toast.as_ref().expect("update toast");
    assert_eq!(
        toast.context,
        "detach, run `brew update && brew upgrade herdr`, then restart this Herdr session when ready"
    );
}

#[test]
fn agent_detection_manifest_update_event_updates_status_and_toast() {
    let mut state = AppState::test_new();
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    let status = crate::detect::manifest_update::ManifestUpdateStatus {
        last_result: Some("checked".to_string()),
        ..Default::default()
    };

    let updates = state.handle_app_event(AppEvent::AgentDetectionManifestsUpdated {
        updated: vec![crate::detect::manifest_update::ManifestUpdateCommit {
            agent: Agent::Codex,
            version: crate::detect::manifest_update::ManifestVersion::parse("2026.06.10.1")
                .unwrap(),
        }],
        status,
    });

    assert!(updates.is_empty());
    assert_eq!(
        state.agent_manifest_update_status.last_result.as_deref(),
        Some("checked")
    );
    let toast = state.toast.as_ref().expect("manifest update toast");
    assert_eq!(toast.kind, ToastKind::UpdateInstalled);
    assert_eq!(toast.title, "Agent detection rules updated");
    assert_eq!(toast.context, "codex 2026.06.10.1");
}

#[test]
fn toggle_zoom_works() {
    let mut state = app_with_workspaces(&["test"]);
    state.workspaces[0].test_split(Direction::Horizontal);

    assert!(!state.workspaces[0].zoomed);
    state.toggle_zoom();
    assert!(state.workspaces[0].zoomed);
    state.toggle_zoom();
    assert!(!state.workspaces[0].zoomed);
}

#[test]
fn toggle_zoom_single_pane_noop() {
    let mut state = app_with_workspaces(&["test"]);
    state.toggle_zoom();
    assert!(!state.workspaces[0].zoomed);
}

#[test]
fn navigate_pane_changes_focus_while_zoomed() {
    let mut state = app_with_workspaces(&["test"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    let right = state.workspaces[0].test_split(Direction::Horizontal);
    state.workspaces[0].layout.focus_pane(root);
    state.workspaces[0].zoomed = true;
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

    assert_eq!(state.view.pane_infos.len(), 1);
    assert_eq!(state.view.pane_infos[0].id, root);

    state.navigate_pane(NavDirection::Right);
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

    assert!(state.workspaces[0].zoomed);
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(right));
    assert_eq!(state.view.pane_infos.len(), 1);
    assert_eq!(state.view.pane_infos[0].id, right);
    assert!(state.view.pane_infos[0].inner_rect.x > state.view.pane_infos[0].rect.x);
}

#[test]
fn swap_pane_direction_preserves_focus_and_swaps_layout_cells() {
    let mut state = app_with_workspaces(&["test"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    let right = state.workspaces[0].test_split(Direction::Horizontal);
    state.workspaces[0].layout.focus_pane(root);
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));
    let before_root_rect = state
        .view
        .pane_infos
        .iter()
        .find(|info| info.id == root)
        .unwrap()
        .rect;
    let before_right_rect = state
        .view
        .pane_infos
        .iter()
        .find(|info| info.id == right)
        .unwrap()
        .rect;

    assert!(state.swap_pane(NavDirection::Right));
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

    assert_eq!(state.workspaces[0].focused_pane_id(), Some(root));
    assert_eq!(
        state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == root)
            .unwrap()
            .rect,
        before_right_rect
    );
    assert_eq!(
        state
            .view
            .pane_infos
            .iter()
            .find(|info| info.id == right)
            .unwrap()
            .rect,
        before_root_rect
    );
}

#[test]
fn swap_pane_direction_stays_zoomed_and_mutates_hidden_layout() {
    let mut state = app_with_workspaces(&["test"]);
    let root = state.workspaces[0].tabs[0].root_pane;
    let right = state.workspaces[0].test_split(Direction::Horizontal);
    state.workspaces[0].layout.focus_pane(root);
    state.workspaces[0].zoomed = true;
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

    assert!(state.swap_pane(NavDirection::Right));
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));

    assert!(state.workspaces[0].zoomed);
    assert_eq!(state.workspaces[0].focused_pane_id(), Some(root));
    assert_eq!(state.view.pane_infos.len(), 1);
    assert_eq!(state.view.pane_infos[0].id, root);

    state.workspaces[0].zoomed = false;
    crate::ui::compute_view(&mut state, ratatui::layout::Rect::new(0, 0, 100, 20));
    let root_rect = state
        .view
        .pane_infos
        .iter()
        .find(|info| info.id == root)
        .unwrap()
        .rect;
    let right_rect = state
        .view
        .pane_infos
        .iter()
        .find(|info| info.id == right)
        .unwrap()
        .rect;

    assert!(root_rect.x > right_rect.x);
}

#[test]
fn close_pane_removes_from_workspace() {
    let mut state = app_with_workspaces(&["test"]);
    let closed = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();
    assert_eq!(state.workspaces[0].panes.len(), 2);
    state.plugin_panes.insert(
        closed,
        crate::app::state::PluginPaneRecord {
            plugin_id: "example.pane".into(),
            entrypoint: "board".into(),
        },
    );

    state.close_pane();
    assert_eq!(state.workspaces[0].panes.len(), 1);
    assert!(!state.plugin_panes.contains_key(&closed));
    state.assert_invariants_for_test();
}

#[test]
fn pane_process_exit_publish_marks_agent_idle_before_pane_removal() {
    let mut state = app_with_workspaces(&["active", "background"]);
    state.toast_config.delivery = crate::config::ToastDelivery::Herdr;
    state.active = Some(1);
    state.ensure_test_terminals();
    let pane_id = state.workspaces[0].tabs[0].root_pane;
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();
    state
        .terminals
        .get_mut(&terminal_id)
        .unwrap()
        .set_detected_state(Some(Agent::Pi), AgentState::Working);
    assert_eq!(
        state.terminals.get(&terminal_id).unwrap().state,
        AgentState::Working
    );

    let update = state
        .publish_pane_process_exit_if_agent(pane_id)
        .expect("process exit update");

    assert!(!state.pane_is_in_active_tab(update.ws_idx, pane_id));
    assert_eq!(update.previous_state, AgentState::Working);
    assert_eq!(update.state, AgentState::Idle);
    assert_eq!(update.agent_label.as_deref(), Some("pi"));
    assert_eq!(update.known_agent, Some(Agent::Pi));
    assert!(matches!(
        state.toast.as_ref().map(|toast| toast.kind),
        Some(ToastKind::Finished)
    ));
}

#[test]
fn close_pane_removes_unattached_terminal_state() {
    let mut state = app_with_workspaces(&["test"]);
    let pane_id = state.workspaces[0].test_split(Direction::Horizontal);
    state.ensure_test_terminals();
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();

    state.close_pane();

    assert!(!state.terminals.contains_key(&terminal_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_tab_removes_unattached_terminal_states() {
    let mut state = app_with_workspaces(&["test"]);
    let tab_idx = state.workspaces[0].test_add_tab(Some("logs"));
    state.ensure_test_terminals();
    state.workspaces[0].switch_tab(tab_idx);
    let pane_id = state.workspaces[0].tabs[tab_idx].root_pane;
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();
    state.plugin_panes.insert(
        pane_id,
        crate::app::state::PluginPaneRecord {
            plugin_id: "example.pane".into(),
            entrypoint: "board".into(),
        },
    );

    state.close_tab();

    assert!(!state.terminals.contains_key(&terminal_id));
    assert!(!state.plugin_panes.contains_key(&pane_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_workspace_removes_unattached_terminal_states() {
    let mut state = app_with_workspaces(&["one", "two"]);
    let pane_id = state.workspaces[0].tabs[0].root_pane;
    let terminal_id = state.terminal_id_for_pane(0, pane_id).unwrap();
    state.plugin_panes.insert(
        pane_id,
        crate::app::state::PluginPaneRecord {
            plugin_id: "example.pane".into(),
            entrypoint: "board".into(),
        },
    );

    state.close_selected_workspace();

    assert!(!state.terminals.contains_key(&terminal_id));
    assert!(!state.plugin_panes.contains_key(&pane_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_tab_closes_active_workspace_not_selected_workspace() {
    let mut state = app_with_workspaces(&["selected", "active"]);
    let active_terminal_id = state
        .terminal_id_for_pane(1, state.workspaces[1].tabs[0].root_pane)
        .unwrap();
    state.active = Some(1);
    state.selected = 0;

    state.close_tab();

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].display_name(), "selected");
    assert!(!state.terminals.contains_key(&active_terminal_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_pane_last_pane_closes_active_workspace_not_selected_workspace() {
    let mut state = app_with_workspaces(&["selected", "active"]);
    let active_terminal_id = state
        .terminal_id_for_pane(1, state.workspaces[1].tabs[0].root_pane)
        .unwrap();
    state.active = Some(1);
    state.selected = 0;

    state.close_pane();

    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].display_name(), "selected");
    assert!(!state.terminals.contains_key(&active_terminal_id));
    state.assert_invariants_for_test();
}

#[test]
fn close_pane_last_pane_in_parent_worktree_group_prompts() {
    let mut state = app_with_workspaces(&["parent", "child"]);
    mark_parent_worktree(&mut state, 0);
    mark_linked_worktree(&mut state, 1);
    state.active = Some(0);
    state.selected = 1;

    let deferred = state.close_pane();

    assert!(deferred);
    assert_eq!(state.mode, Mode::ConfirmClose);
    assert_eq!(state.selected, 0);
    assert_eq!(state.workspaces.len(), 2);
}

#[test]
fn close_tab_in_linked_worktree_closes_workspace_only() {
    let mut state = app_with_workspaces(&["selected", "active"]);
    mark_linked_worktree(&mut state, 1);
    state.active = Some(1);
    state.selected = 0;

    state.close_tab();

    assert_eq!(state.request_remove_linked_worktree, None);
    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].display_name(), "selected");
}

#[test]
fn close_tab_last_tab_in_parent_worktree_group_prompts() {
    let mut state = app_with_workspaces(&["parent", "child"]);
    mark_parent_worktree(&mut state, 0);
    mark_linked_worktree(&mut state, 1);
    state.active = Some(0);
    state.selected = 1;

    let deferred = state.close_tab();

    assert!(deferred);
    assert_eq!(state.mode, Mode::ConfirmClose);
    assert_eq!(state.selected, 0);
    assert_eq!(state.workspaces.len(), 2);
}

#[test]
fn close_pane_last_pane_in_linked_worktree_closes_workspace_only() {
    let mut state = app_with_workspaces(&["selected", "active"]);
    mark_linked_worktree(&mut state, 1);
    state.active = Some(1);
    state.selected = 0;

    state.close_pane();

    assert_eq!(state.request_remove_linked_worktree, None);
    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].display_name(), "selected");
}

#[test]
fn close_pane_last_pane_in_parent_worktree_group_closes_when_confirmation_disabled() {
    let mut state = app_with_workspaces(&["parent", "child", "notes"]);
    mark_parent_worktree(&mut state, 0);
    mark_linked_worktree(&mut state, 1);
    state.confirm_close = false;
    state.active = Some(0);
    state.selected = 0;

    let deferred = state.close_pane();

    assert!(!deferred);
    assert_eq!(state.workspaces.len(), 1);
    assert_eq!(state.workspaces[0].display_name(), "notes");
}
