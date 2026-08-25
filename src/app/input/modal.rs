use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(test)]
use ratatui::layout::Direction;
use ratatui::layout::Rect;

use crate::{
    app::{
        state::{
            AppState, ContextMenuKind, ContextMenuState, MenuListState, Mode, NavigatorStateFilter,
        },
        App,
    },
    input::TerminalKey,
    layout::NavDirection,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModalAction {
    Continue,
    Save,
    Clear,
    Cancel,
    Confirm,
    Apply,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ModalKeyBinding {
    Enter,
    Esc,
    CtrlC,
}

impl ModalKeyBinding {
    fn matches(self, key: &KeyEvent) -> bool {
        match self {
            Self::Enter => key.code == KeyCode::Enter,
            Self::Esc => key.code == KeyCode::Esc,
            Self::CtrlC => {
                key.code == KeyCode::Char('c')
                    && key.modifiers == crossterm::event::KeyModifiers::CONTROL
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ModalActionSpec<A> {
    pub action: A,
    pub bindings: &'static [ModalKeyBinding],
}

pub(super) fn modal_action_from_key<A: Copy>(
    key: &KeyEvent,
    specs: &[ModalActionSpec<A>],
) -> Option<A> {
    specs
        .iter()
        .find(|spec| spec.bindings.iter().any(|binding| binding.matches(key)))
        .map(|spec| spec.action)
}

pub(super) fn modal_action_from_buttons<A: Copy>(
    col: u16,
    row: u16,
    buttons: &[(Rect, A)],
) -> Option<A> {
    buttons.iter().find_map(|(rect, action)| {
        (col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height)
            .then_some(*action)
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlobalMenuAction {
    Detach,
    WhatsNew,
    Keybinds,
    ReloadConfig,
    Settings,
    Chat,
}

pub(super) fn global_menu_actions(state: &AppState) -> Vec<GlobalMenuAction> {
    let mut actions = vec![GlobalMenuAction::Settings];
    if state.chat_view {
        actions.push(GlobalMenuAction::Chat);
    }
    actions.push(GlobalMenuAction::Keybinds);
    actions.push(GlobalMenuAction::ReloadConfig);
    if state.update_available.is_some() || state.latest_release_notes_available {
        actions.push(GlobalMenuAction::WhatsNew);
    }
    actions.push(GlobalMenuAction::Detach);
    actions
}

pub(super) fn open_global_menu(state: &mut AppState) {
    state.global_menu = MenuListState::new(0);
    state.mode = Mode::GlobalMenu;
}

pub(super) fn open_keybind_help(state: &mut AppState) {
    state.keybind_help.scroll = 0;
    state.keybind_help.query.clear();
    state.keybind_help.search_focused = false;
    state.mode = Mode::KeybindHelp;
}

fn open_update_release_notes(state: &mut AppState) {
    let Some(notes) = crate::release_notes::load_latest() else {
        return;
    };

    state.release_notes = Some(crate::app::state::ReleaseNotesState {
        version: notes.version,
        body: notes.body,
        scroll: 0,
        preview: notes.preview,
    });
    state.mode = Mode::ReleaseNotes;
}

pub(super) fn request_detach(state: &mut AppState) {
    if state.detach_exits {
        state.should_quit = true;
    } else {
        state.detach_requested = true;
    }
}

pub(super) fn apply_global_menu_action(state: &mut AppState, action: GlobalMenuAction) {
    match action {
        GlobalMenuAction::Detach => {
            leave_modal(state);
            request_detach(state);
        }
        GlobalMenuAction::WhatsNew => open_update_release_notes(state),
        GlobalMenuAction::Keybinds => open_keybind_help(state),
        GlobalMenuAction::ReloadConfig => {
            state.request_reload_config = true;
            leave_modal(state);
        }
        GlobalMenuAction::Settings => super::settings::open_settings(state),
        GlobalMenuAction::Chat => {
            state.request_open_chat = true;
            leave_modal(state);
        }
    }
}

pub(crate) fn handle_global_menu_key(state: &mut AppState, key: KeyEvent) {
    let actions = global_menu_actions(state);
    match key.code {
        KeyCode::Esc => leave_modal(state),
        KeyCode::Up | KeyCode::Char('k') => state.global_menu.move_prev(),
        KeyCode::Down | KeyCode::Char('j') => state.global_menu.move_next(actions.len()),
        KeyCode::Enter => {
            if let Some(action) = actions.get(state.global_menu.highlighted).copied() {
                apply_global_menu_action(state, action);
            }
        }
        _ => {}
    }
}

pub(crate) fn handle_navigator_key(
    state: &mut AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    key: KeyEvent,
) {
    if state.navigator.search_focused {
        match key.code {
            KeyCode::Esc => {
                state.navigator.search_focused = false;
            }
            KeyCode::Enter => {
                state.accept_navigator_selection_from(terminal_runtimes);
            }
            KeyCode::Backspace => {
                state.navigator.state_filter = None;
                state.navigator.query.pop();
                state.select_first_navigator_match_from(terminal_runtimes);
            }
            KeyCode::Up => state.move_navigator_selection_from(terminal_runtimes, -1),
            KeyCode::Down => state.move_navigator_selection_from(terminal_runtimes, 1),
            KeyCode::Char('n') if key.modifiers == KeyModifiers::CONTROL => {
                state.move_navigator_selection_from(terminal_runtimes, 1)
            }
            KeyCode::Char('p') if key.modifiers == KeyModifiers::CONTROL => {
                state.move_navigator_selection_from(terminal_runtimes, -1)
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                state.navigator.query.clear();
                state.navigator.state_filter = None;
                state.clamp_navigator_selection_from(terminal_runtimes);
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                insert_navigator_search_text(state, terminal_runtimes, &c.to_string());
            }
            _ => {}
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            leave_modal(state);
        }
        KeyCode::Enter => {
            state.accept_navigator_selection_from(terminal_runtimes);
        }
        KeyCode::Char('/') => {
            state.navigator.state_filter = None;
            state.navigator.search_focused = true;
            state.clamp_navigator_selection_from(terminal_runtimes);
        }
        KeyCode::Backspace if state.navigator.state_filter.is_some() => {
            state.navigator.state_filter = None;
            state.clamp_navigator_selection_from(terminal_runtimes);
        }
        KeyCode::Char('a') if key.modifiers.is_empty() => {
            state.navigator.query.clear();
            state.navigator.state_filter = None;
            state.clamp_navigator_selection_from(terminal_runtimes);
        }
        KeyCode::Char('b') if key.modifiers.is_empty() => {
            state.navigator.query.clear();
            state.navigator.state_filter = Some(NavigatorStateFilter::Blocked);
            state.select_first_navigator_match_from(terminal_runtimes);
        }
        KeyCode::Char('w') if key.modifiers.is_empty() => {
            state.navigator.query.clear();
            state.navigator.state_filter = Some(NavigatorStateFilter::Working);
            state.select_first_navigator_match_from(terminal_runtimes);
        }
        KeyCode::Char('i') if key.modifiers.is_empty() => {
            state.navigator.query.clear();
            state.navigator.state_filter = Some(NavigatorStateFilter::Idle);
            state.select_first_navigator_match_from(terminal_runtimes);
        }
        KeyCode::Char('d') if key.modifiers.is_empty() => {
            state.navigator.query.clear();
            state.navigator.state_filter = Some(NavigatorStateFilter::Done);
            state.select_first_navigator_match_from(terminal_runtimes);
        }
        KeyCode::Char('j') | KeyCode::Down if key.modifiers.is_empty() => {
            state.move_navigator_selection_from(terminal_runtimes, 1)
        }
        KeyCode::Char('k') | KeyCode::Up if key.modifiers.is_empty() => {
            state.move_navigator_selection_from(terminal_runtimes, -1)
        }
        KeyCode::Char('d') if key.modifiers == KeyModifiers::CONTROL => state
            .move_navigator_selection_by_lines_from(
                terminal_runtimes,
                (state.navigator_body_rect().height / 2).max(1) as isize,
            ),
        KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => state
            .move_navigator_selection_by_lines_from(
                terminal_runtimes,
                -((state.navigator_body_rect().height / 2).max(1) as isize),
            ),
        KeyCode::Char(' ') => state.toggle_selected_navigator_workspace_from(terminal_runtimes),
        KeyCode::Home => {
            state.navigator.selected = 0;
            state.ensure_navigator_selection_visible_from(terminal_runtimes);
        }
        KeyCode::End | KeyCode::Char('G') => {
            state.navigator.selected = state
                .navigator_rows_from(terminal_runtimes)
                .len()
                .saturating_sub(1);
            state.ensure_navigator_selection_visible_from(terminal_runtimes);
        }
        _ => {}
    }
}

pub(crate) fn insert_navigator_search_text(
    state: &mut AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    text: &str,
) {
    if !state.navigator.search_focused {
        return;
    }
    state.navigator.state_filter = None;
    state.navigator.query.push_str(text);
    state.select_first_navigator_match_from(terminal_runtimes);
}

pub(crate) fn insert_keybind_help_query_text(state: &mut AppState, text: &str) {
    if !state.keybind_help.search_focused {
        return;
    }
    state
        .keybind_help
        .query
        .extend(text.chars().filter(|ch| !ch.is_control()));
    state.keybind_help.scroll = 0;
}

pub(super) fn keybind_help_back(state: &mut AppState) {
    if state.keybind_help.search_focused {
        state.keybind_help.query.clear();
        state.keybind_help.search_focused = false;
        state.keybind_help.scroll = 0;
    } else {
        leave_modal(state);
    }
}

pub(crate) fn handle_keybind_help_key(state: &mut AppState, key: TerminalKey) {
    if state.keybind_help.search_focused {
        let text_char = keybind_help_text_char(key.clone());
        match key.code {
            KeyCode::Up => state.scroll_keybind_help(-1),
            KeyCode::Down => state.scroll_keybind_help(1),
            KeyCode::PageUp => state.scroll_keybind_help(-8),
            KeyCode::PageDown => state.scroll_keybind_help(8),
            KeyCode::Home => state.keybind_help.scroll = 0,
            KeyCode::End => state.keybind_help.scroll = state.keybind_help_max_scroll(),
            KeyCode::Backspace => {
                state.keybind_help.query.pop();
                state.keybind_help.scroll = 0;
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                state.keybind_help.query.clear();
                state.keybind_help.scroll = 0;
            }
            KeyCode::Esc => keybind_help_back(state),
            KeyCode::Enter => leave_modal(state),
            _ => {
                if let Some(character) = text_char {
                    insert_keybind_help_query_text(state, &character.to_string());
                }
            }
        }
        return;
    }

    match key.code {
        KeyCode::Up | KeyCode::Char('k') => state.scroll_keybind_help(-1),
        KeyCode::Down | KeyCode::Char('j') => state.scroll_keybind_help(1),
        KeyCode::PageUp => state.scroll_keybind_help(-8),
        KeyCode::PageDown => state.scroll_keybind_help(8),
        KeyCode::Home => state.keybind_help.scroll = 0,
        KeyCode::End => state.keybind_help.scroll = state.keybind_help_max_scroll(),
        _ if keybind_help_text_char(key.clone()) == Some('/') => {
            state.keybind_help.search_focused = true;
            state.keybind_help.scroll = 0;
        }
        KeyCode::Esc => keybind_help_back(state),
        KeyCode::Enter => leave_modal(state),
        _ if keybind_help_text_char(key) == Some('?') => leave_modal(state),
        _ => {}
    }
}

fn keybind_help_text_char(key: TerminalKey) -> Option<char> {
    if !key.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
        return None;
    }
    if let Some(character) = key.shifted_codepoint.and_then(char::from_u32) {
        return Some(character);
    }
    let KeyCode::Char(character) = key.code else {
        return None;
    };
    Some(character)
}

pub(super) fn open_rename_workspace(
    state: &mut AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    ws_idx: usize,
) {
    state.pending_workspace_create_cwd = None;
    state.selected = ws_idx;
    state.rename_pane_target = None;
    state.name_input =
        state.workspaces[ws_idx].display_name_from(&state.terminals, terminal_runtimes);
    state.name_input_replace_on_type = false;
    state.mode = Mode::RenameWorkspace;
}

pub(super) fn open_set_workspace_group(state: &mut AppState, ws_idx: usize) {
    state.selected = ws_idx;
    state.rename_pane_target = None;
    // Pre-populate with the current group name, if any.
    state.name_input = state
        .workspaces
        .get(ws_idx)
        .and_then(|ws| ws.visual_group.clone())
        .unwrap_or_default();
    state.name_input_replace_on_type = false;
    state.mode = Mode::SetWorkspaceGroup;
}

pub(crate) fn open_new_workspace_dialog(state: &mut AppState, cwd: std::path::PathBuf) {
    let suggested_name = crate::workspace::derive_label_from_cwd(&cwd);
    state.creating_new_tab = false;
    state.requested_new_tab_name = None;
    state.pending_workspace_create_cwd = Some(cwd);
    state.rename_pane_target = None;
    state.name_input = suggested_name;
    state.name_input_replace_on_type = true;
    state.mode = Mode::RenameWorkspace;
}

pub(super) fn open_rename_active_tab(state: &mut AppState, replace_on_type: bool) {
    state.creating_new_tab = false;
    state.requested_new_tab_name = None;
    state.pending_workspace_create_cwd = None;
    state.rename_pane_target = None;
    if let Some(ws) = state.active.and_then(|i| state.workspaces.get(i)) {
        if let Some(name) = ws.active_tab_display_name() {
            state.name_input = name;
            state.name_input_replace_on_type = replace_on_type;
            state.mode = Mode::RenameTab;
        }
    }
}

pub(super) fn open_rename_pane(state: &mut AppState, pane_id: crate::layout::PaneId) {
    let Some(ws) = state.active.and_then(|i| state.workspaces.get(i)) else {
        return;
    };
    let Some(pane) = ws.pane_state(pane_id) else {
        return;
    };
    let terminal = state.terminals.get(&pane.attached_terminal_id);
    state.creating_new_tab = false;
    state.requested_new_tab_name = None;
    state.pending_workspace_create_cwd = None;
    state.rename_pane_target = Some(pane_id);
    state.name_input = terminal
        .and_then(|t| t.manual_label.clone())
        .unwrap_or_default();
    state.name_input_replace_on_type = terminal.and_then(|t| t.manual_label.as_ref()).is_none();
    state.mode = Mode::RenamePane;
}

fn workspace_create_label(input: &str, suggested_name: &str) -> Option<String> {
    let name = input.trim();
    (!name.is_empty() && name != suggested_name).then(|| name.to_string())
}

fn next_new_tab_default_name(state: &AppState) -> String {
    state
        .active
        .and_then(|i| state.workspaces.get(i))
        .map(|ws| (ws.tabs.len() + 1).to_string())
        .unwrap_or_else(|| "1".to_string())
}

pub(super) fn open_new_tab_dialog(state: &mut AppState) {
    state.creating_new_tab = true;
    state.requested_new_tab_name = None;
    state.pending_workspace_create_cwd = None;
    state.rename_pane_target = None;
    state.name_input = next_new_tab_default_name(state);
    state.name_input_replace_on_type = true;
    state.mode = Mode::RenameTab;
}

pub(super) fn leave_modal(state: &mut AppState) {
    if state.active.is_some() {
        state.mode = Mode::Terminal;
    } else {
        state.mode = Mode::Navigate;
    }
}

/// Minutes for a "Hide Nm" context-menu label.
fn hide_minutes(label: &str) -> u64 {
    match label {
        "Hide 5m" => 5,
        "Hide 10m" => 10,
        "Hide 15m" => 15,
        _ => 30,
    }
}

pub(super) const ONBOARDING_WELCOME_ACTIONS: &[ModalActionSpec<ModalAction>] = &[ModalActionSpec {
    action: ModalAction::Continue,
    bindings: &[ModalKeyBinding::Enter],
}];

pub(super) const RELEASE_NOTES_ACTIONS: &[ModalActionSpec<ModalAction>] = &[ModalActionSpec {
    action: ModalAction::Close,
    bindings: &[ModalKeyBinding::Enter, ModalKeyBinding::Esc],
}];

pub(super) const RENAME_ACTIONS: &[ModalActionSpec<ModalAction>] = &[
    ModalActionSpec {
        action: ModalAction::Save,
        bindings: &[ModalKeyBinding::Enter],
    },
    ModalActionSpec {
        action: ModalAction::Clear,
        bindings: &[ModalKeyBinding::CtrlC],
    },
    ModalActionSpec {
        action: ModalAction::Cancel,
        bindings: &[ModalKeyBinding::Esc],
    },
];

pub(super) const CONFIRM_CLOSE_ACTIONS: &[ModalActionSpec<ModalAction>] = &[
    ModalActionSpec {
        action: ModalAction::Confirm,
        bindings: &[ModalKeyBinding::Enter],
    },
    ModalActionSpec {
        action: ModalAction::Cancel,
        bindings: &[ModalKeyBinding::Esc],
    },
];

pub(super) const SETTINGS_ACTIONS: &[ModalActionSpec<ModalAction>] = &[
    ModalActionSpec {
        action: ModalAction::Apply,
        bindings: &[ModalKeyBinding::Enter],
    },
    ModalActionSpec {
        action: ModalAction::Close,
        bindings: &[ModalKeyBinding::Esc],
    },
];

#[cfg(test)]
pub(super) fn apply_rename_action(state: &mut AppState, action: ModalAction) {
    match action {
        ModalAction::Save => {
            let new_name = if state.name_input.trim().is_empty() {
                state.name_input.clone()
            } else {
                state.name_input.trim().to_string()
            };
            match state.mode {
                Mode::RenameWorkspace
                    if state.pending_workspace_create_cwd.is_none()
                        && !state.workspaces.is_empty()
                        && !new_name.is_empty() =>
                {
                    let workspace_id = state.workspaces[state.selected].id.clone();
                    state.workspaces[state.selected].set_custom_name(new_name);
                    crate::logging::workspace_renamed(&workspace_id);
                    state.mark_session_dirty();
                }
                Mode::RenameTab if state.creating_new_tab => {
                    state.request_new_tab = true;
                    let default_name = next_new_tab_default_name(state);
                    state.requested_new_tab_name =
                        if new_name.is_empty() || new_name == default_name {
                            None
                        } else {
                            Some(new_name)
                        };
                }
                Mode::RenameTab => {
                    if let Some(ws_idx) = state.active {
                        if let Some(ws) = state.workspaces.get_mut(ws_idx) {
                            let workspace_id = ws.id.clone();
                            let active_tab = ws.active_tab;
                            let keep_auto_name = ws
                                .tabs
                                .get(active_tab)
                                .is_some_and(crate::workspace::Tab::is_auto_named)
                                && ws
                                    .tab_display_name(active_tab)
                                    .is_some_and(|name| new_name == name);
                            if let Some(tab) = ws.active_tab_mut() {
                                if !new_name.is_empty() && !keep_auto_name {
                                    tab.set_custom_name(new_name);
                                    let tab_id = ws
                                        .public_tab_number(active_tab)
                                        .map(|number| {
                                            crate::workspace::public_tab_id_for_number(
                                                &workspace_id,
                                                number,
                                            )
                                        })
                                        .unwrap_or_else(|| workspace_id.clone());
                                    crate::logging::tab_renamed(&workspace_id, &tab_id);
                                    state.mark_session_dirty();
                                }
                            }
                        }
                    }
                }
                Mode::RenamePane => {
                    if let (Some(ws_idx), Some(pane_id)) = (state.active, state.rename_pane_target)
                    {
                        if let Some(ws) = state.workspaces.get(ws_idx) {
                            if let Some(pane) = ws.pane_state(pane_id) {
                                let terminal_id = pane.attached_terminal_id.clone();
                                if let Some(terminal) = state.terminals.get_mut(&terminal_id) {
                                    terminal.set_manual_label(new_name);
                                    state.mark_session_dirty();
                                }
                            }
                        }
                    }
                }
                Mode::SetWorkspaceGroup if !state.workspaces.is_empty() => {
                    // Empty input clears the group (same as "Remove from group").
                    if new_name.is_empty() {
                        state.workspaces[state.selected].visual_group = None;
                    } else {
                        state.workspaces[state.selected].visual_group = Some(new_name);
                    }
                    state.mark_session_dirty();
                }
                Mode::ProjectNameInput => {
                    if let Some(target) = state.project_name_target.take() {
                        let name = new_name.trim();
                        if !name.is_empty() {
                            match target {
                                crate::app::state::ProjectNameTarget::Rename { slug } => {
                                    if let Err(err) = rename_project(&slug, name) {
                                        tracing::warn!(err = ?err, "project rename failed");
                                    }
                                }
                                crate::app::state::ProjectNameTarget::New { member_dir } => {
                                    if let Err(err) =
                                        create_project_with_optional_member(name, member_dir)
                                    {
                                        tracing::warn!(err = ?err, "project create failed");
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            state.creating_new_tab = false;
            state.pending_workspace_create_cwd = None;
            state.rename_pane_target = None;
            state.project_name_target = None;
            state.name_input.clear();
            state.name_input_replace_on_type = false;
            leave_modal(state);
        }
        ModalAction::Clear => {
            state.name_input.clear();
            state.name_input_replace_on_type = false;
        }
        ModalAction::Cancel => {
            state.creating_new_tab = false;
            state.requested_new_tab_name = None;
            state.pending_workspace_create_cwd = None;
            state.rename_pane_target = None;
            state.project_name_target = None;
            state.name_input.clear();
            state.name_input_replace_on_type = false;
            leave_modal(state);
        }
        _ => {}
    }
}

fn clear_rename_input(state: &mut AppState) {
    state.name_input.clear();
    state.name_input_replace_on_type = false;
}

pub(crate) fn insert_rename_input_text(state: &mut AppState, text: &str) {
    if state.name_input_replace_on_type {
        clear_rename_input(state);
    }
    state.name_input.push_str(text);
}

fn delete_rename_input_char(state: &mut AppState) {
    if state.name_input_replace_on_type {
        clear_rename_input(state);
    } else {
        state.name_input.pop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RenameWordDeleteClass {
    Word,
    Separator,
}

fn rename_word_delete_class(ch: char) -> RenameWordDeleteClass {
    if ch.is_alphanumeric() || ch == '_' {
        RenameWordDeleteClass::Word
    } else {
        RenameWordDeleteClass::Separator
    }
}

fn delete_rename_input_word(state: &mut AppState) {
    if state.name_input_replace_on_type {
        clear_rename_input(state);
        return;
    }

    while state
        .name_input
        .chars()
        .last()
        .is_some_and(char::is_whitespace)
    {
        state.name_input.pop();
    }

    let Some(class) = state
        .name_input
        .chars()
        .last()
        .map(rename_word_delete_class)
    else {
        return;
    };

    while state
        .name_input
        .chars()
        .last()
        .is_some_and(|ch| !ch.is_whitespace() && rename_word_delete_class(ch) == class)
    {
        state.name_input.pop();
    }
}

fn handle_rename_edit_key(state: &mut AppState, key: KeyEvent) {
    match key.code {
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            clear_rename_input(state);
        }
        KeyCode::Backspace if key.modifiers.contains(KeyModifiers::SUPER) => {
            clear_rename_input(state);
        }
        KeyCode::Backspace
            if key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::ALT) =>
        {
            delete_rename_input_word(state);
        }
        KeyCode::Char('h' | 'w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            delete_rename_input_word(state);
        }
        KeyCode::Backspace => delete_rename_input_char(state),
        KeyCode::Char(c) if key.modifiers.difference(KeyModifiers::SHIFT).is_empty() => {
            insert_rename_input_text(state, &c.to_string());
        }
        _ => {}
    }
}

#[cfg(test)]
pub(crate) fn handle_rename_key(state: &mut AppState, key: KeyEvent) {
    if let Some(action) = modal_action_from_key(&key, RENAME_ACTIONS) {
        apply_rename_action(state, action);
        return;
    }

    handle_rename_edit_key(state, key);
}

#[cfg(test)]
pub(crate) fn handle_resize_key(state: &mut AppState, raw_key: TerminalKey) {
    let key = raw_key.as_key_event();
    if key.code == KeyCode::Esc
        || key.code == KeyCode::Enter
        || state.keybinds.resize_mode.matches_prefix_key(&raw_key)
        || state.keybinds.resize_mode.matches_direct_key(&raw_key)
    {
        if state.active.is_some() {
            state.mode = Mode::Terminal;
        } else {
            state.mode = Mode::Navigate;
        }
        return;
    }

    match key.code {
        KeyCode::Char('h') | KeyCode::Left => state.resize_pane(NavDirection::Left),
        KeyCode::Char('l') | KeyCode::Right => state.resize_pane(NavDirection::Right),
        KeyCode::Char('j') | KeyCode::Down => state.resize_pane(NavDirection::Down),
        KeyCode::Char('k') | KeyCode::Up => state.resize_pane(NavDirection::Up),
        _ => {}
    }
}

pub(super) fn open_confirm_close(state: &mut AppState) {
    state.mode = Mode::ConfirmClose;
}

#[cfg(test)]
pub(super) fn confirm_close_accept(state: &mut AppState) {
    state.close_selected_workspace();
    if state.workspaces.is_empty() {
        state.mode = Mode::Navigate;
    } else {
        state.mode = Mode::Terminal;
    }
}

pub(super) fn confirm_close_cancel(state: &mut AppState) {
    state.mode = Mode::Navigate;
}

#[cfg(test)]
pub(crate) fn handle_confirm_close_key(state: &mut AppState, key: KeyEvent) {
    match modal_action_from_key(&key, CONFIRM_CLOSE_ACTIONS) {
        Some(ModalAction::Confirm) => confirm_close_accept(state),
        Some(ModalAction::Cancel) => confirm_close_cancel(state),
        _ => {}
    }
}

#[cfg(test)]
pub(super) fn apply_context_menu_action(
    state: &mut AppState,
    terminal_runtimes: &mut crate::terminal::TerminalRuntimeRegistry,
    menu: ContextMenuState,
    idx: usize,
) {
    let item_owned = menu.items.get(idx).cloned();
    let item = item_owned.as_deref();
    let bora_commands = menu.bora_commands;
    let bora_port = menu.bora_port;
    match (menu.kind, item) {
        (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("New worktree")) => {
            state.request_new_linked_worktree = Some(ws_idx);
            leave_modal(state);
        }
        (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Delete worktree\u{2026}")) => {
            state.request_remove_linked_worktree = Some(ws_idx);
            leave_modal(state);
        }
        (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Open worktree\u{2026}")) => {
            state.request_open_existing_worktree = Some(ws_idx);
            leave_modal(state);
        }
        (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Merge to main")) => {
            state.request_merge_worktree_to_main = Some(ws_idx);
            leave_modal(state);
        }
        (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Open PR")) => {
            state.request_open_worktree_pr = Some(ws_idx);
            leave_modal(state);
        }
        (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Sync")) => {
            state.request_sync_workspace_git = Some(ws_idx);
            leave_modal(state);
        }
        (
            ContextMenuKind::GitWorkspace {
                ws_idx, collapsed, ..
            },
            Some("Collapse" | "Expand"),
        ) => {
            if let Some(key) = state
                .workspaces
                .get(ws_idx)
                .and_then(|ws| ws.worktree_space())
                .map(|space| space.key.clone())
            {
                if collapsed {
                    state.collapsed_space_keys.remove(&key);
                } else {
                    state.collapsed_space_keys.insert(key);
                }
                state.mark_session_dirty();
            }
            leave_modal(state);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some("Copy path"),
        ) => {
            if let Some(ws) = state.workspaces.get(ws_idx) {
                let path = ws.identity_cwd.display().to_string();
                state.request_clipboard_write = Some(path.into_bytes());
            }
            leave_modal(state);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some("Rename"),
        ) => {
            open_rename_workspace(state, terminal_runtimes, ws_idx);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some("New group\u{2026}"),
        ) => {
            open_set_workspace_group(state, ws_idx);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some(item_str),
        ) if item_str.starts_with("\u{2192} ") => {
            let group_name = item_str["\u{2192} ".len()..].to_string();
            if let Some(ws) = state.workspaces.get_mut(ws_idx) {
                ws.visual_group = Some(group_name);
                state.mark_session_dirty();
            }
            leave_modal(state);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some("Remove from group"),
        ) => {
            if let Some(ws) = state.workspaces.get_mut(ws_idx) {
                ws.visual_group = None;
                state.mark_session_dirty();
            }
            leave_modal(state);
        }
        // ── Project assembly (bora-uqv) ─────────────────────────────
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some(item_str),
        ) if item_str.starts_with("Add to ") => {
            let slug = item_str["Add to ".len()..].to_string();
            if let Some(dir) = state
                .workspaces
                .get(ws_idx)
                .map(crate::workspace::Workspace::project_member_dir)
            {
                if let Err(err) = add_member(&slug, &dir) {
                    tracing::warn!(err = ?err, "project member_add failed");
                }
            }
            leave_modal(state);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some("New project\u{2026}"),
        ) => {
            if let Some(dir) = state
                .workspaces
                .get(ws_idx)
                .map(crate::workspace::Workspace::project_member_dir)
            {
                open_new_project_prompt(state, Some(dir));
            } else {
                leave_modal(state);
            }
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some("Remove"),
        ) => {
            if let Some(dir) = state
                .workspaces
                .get(ws_idx)
                .map(crate::workspace::Workspace::project_member_dir)
            {
                remove_membership_direct(&dir);
            }
            leave_modal(state);
        }
        (ContextMenuKind::ProjectHeader { slug, .. }, Some("Add workspaces\u{2026}")) => {
            let orphans = orphan_member_dirs(state);
            if orphans.is_empty() {
                leave_modal(state);
            } else {
                let items = orphans
                    .iter()
                    .map(|dir| format!("\u{ff0b} {dir}"))
                    .collect();
                follow_up_menu(
                    state,
                    ContextMenuKind::ProjectOrphanPicker { slug, orphans },
                    items,
                    menu.x,
                    menu.y,
                );
            }
        }
        (ContextMenuKind::ProjectHeader { .. }, Some("New project\u{2026}")) => {
            open_new_project_prompt(state, None);
        }
        (
            ContextMenuKind::ProjectHeader {
                slug: Some(slug), ..
            },
            Some("Rename project\u{2026}"),
        ) => {
            let prefill = projects::load_projects_file_fresh()
                .ok()
                .and_then(|file| {
                    file.projects
                        .get(&slug)
                        .map(|project| project.name.clone().unwrap_or_else(|| slug.clone()))
                })
                .unwrap_or_else(|| slug.clone());
            open_project_name_input(
                state,
                crate::app::state::ProjectNameTarget::Rename { slug },
                prefill,
                false,
            );
        }
        (ContextMenuKind::ProjectOrphanPicker { slug, orphans }, Some(_)) => {
            match (slug, orphans.get(idx).cloned()) {
                (Some(slug), Some(dir)) => {
                    if let Err(err) = add_member(&slug, &dir) {
                        tracing::warn!(err = ?err, "project member_add failed");
                    }
                    leave_modal(state);
                }
                (None, Some(dir)) => {
                    let items = assembly_items_for_dir(&dir);
                    follow_up_menu(
                        state,
                        ContextMenuKind::ProjectMemberTargets { member_dir: dir },
                        items,
                        menu.x,
                        menu.y,
                    );
                }
                _ => leave_modal(state),
            }
        }
        (ContextMenuKind::ProjectMemberTargets { member_dir }, Some(item_str))
            if item_str.starts_with("Add to ") =>
        {
            let slug = item_str["Add to ".len()..].to_string();
            if let Err(err) = add_member(&slug, &member_dir) {
                tracing::warn!(err = ?err, "project member_add failed");
            }
            leave_modal(state);
        }
        (ContextMenuKind::ProjectMemberTargets { member_dir }, Some("New project\u{2026}")) => {
            open_new_project_prompt(state, Some(member_dir));
        }
        (ContextMenuKind::ProjectMemberTargets { member_dir }, Some("Remove")) => {
            remove_membership_direct(&member_dir);
            leave_modal(state);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some("Close" | "Close workspace"),
        ) => {
            state.selected = ws_idx;
            if state.confirm_close {
                open_confirm_close(state);
            } else {
                state.close_selected_workspace();
                state.mode = Mode::Navigate;
            }
        }
        (ContextMenuKind::Tab { ws_idx, tab_idx }, Some("New tab")) => {
            state.selected = ws_idx;
            state.active = Some(ws_idx);
            state.switch_tab(tab_idx);
            open_new_tab_dialog(state);
        }
        (ContextMenuKind::Tab { ws_idx, tab_idx }, Some("Rename")) => {
            state.selected = ws_idx;
            state.active = Some(ws_idx);
            state.switch_tab(tab_idx);
            open_rename_active_tab(state, false);
        }
        (ContextMenuKind::Tab { ws_idx, tab_idx }, Some("Close")) => {
            state.selected = ws_idx;
            state.active = Some(ws_idx);
            state.switch_tab(tab_idx);
            if !state.close_tab() {
                state.mode = if state.active.is_some() {
                    Mode::Terminal
                } else {
                    Mode::Navigate
                };
            }
        }
        (ContextMenuKind::Pane { pane_id, .. }, Some("Rename pane")) => {
            open_rename_pane(state, pane_id);
        }
        (
            ContextMenuKind::Pane {
                ws_idx, pane_id, ..
            },
            Some("Clear pane name"),
        ) => {
            if let Some(ws) = state.workspaces.get(ws_idx) {
                if let Some(pane) = ws.pane_state(pane_id) {
                    let terminal_id = pane.attached_terminal_id.clone();
                    if let Some(terminal) = state.terminals.get_mut(&terminal_id) {
                        terminal.clear_manual_label();
                        state.mark_session_dirty();
                    }
                }
            }
            state.mode = Mode::Terminal;
        }
        (
            ContextMenuKind::Pane {
                ws_idx,
                tab_idx,
                pane_id,
                source_pane_id,
                ..
            },
            Some("Swap with focused pane"),
        ) => {
            if let Some(source_pane_id) = source_pane_id {
                state.selected = ws_idx;
                state.active = Some(ws_idx);
                state.switch_tab(tab_idx);
                if let Some(tab) = state
                    .workspaces
                    .get_mut(ws_idx)
                    .and_then(|ws| ws.tabs.get_mut(tab_idx))
                {
                    if tab.layout.swap_panes(source_pane_id, pane_id) {
                        tab.layout.focus_pane(source_pane_id);
                        state.mark_session_dirty();
                    }
                }
            }
            state.mode = Mode::Terminal;
        }
        (
            ContextMenuKind::Pane {
                ws_idx,
                tab_idx,
                pane_id,
                ..
            },
            Some("Split right"),
        ) => {
            state.selected = ws_idx;
            state.active = Some(ws_idx);
            state.switch_tab(tab_idx);
            state.focus_pane_in_workspace(ws_idx, pane_id);
            state.split_pane(terminal_runtimes, Direction::Horizontal);
            state.mode = Mode::Terminal;
        }
        (
            ContextMenuKind::Pane {
                ws_idx,
                tab_idx,
                pane_id,
                ..
            },
            Some("Split down"),
        ) => {
            state.selected = ws_idx;
            state.active = Some(ws_idx);
            state.switch_tab(tab_idx);
            state.focus_pane_in_workspace(ws_idx, pane_id);
            state.split_pane(terminal_runtimes, Direction::Vertical);
            state.mode = Mode::Terminal;
        }
        (
            ContextMenuKind::Pane {
                ws_idx,
                tab_idx,
                pane_id,
                ..
            },
            Some("Zoom"),
        ) => {
            state.selected = ws_idx;
            state.active = Some(ws_idx);
            state.switch_tab(tab_idx);
            state.focus_pane_in_workspace(ws_idx, pane_id);
            state.toggle_zoom();
            state.mode = Mode::Terminal;
        }
        (
            ContextMenuKind::Pane {
                ws_idx,
                tab_idx,
                pane_id,
                ..
            },
            Some("Close pane"),
        ) => {
            state.selected = ws_idx;
            state.active = Some(ws_idx);
            state.switch_tab(tab_idx);
            state.focus_pane_in_workspace(ws_idx, pane_id);
            if !state.close_pane() {
                state.mode = if state.active.is_some() {
                    Mode::Terminal
                } else {
                    Mode::Navigate
                };
            }
        }
        (ContextMenuKind::RepoPr { ws_idx, number, .. }, Some("Open in worktree")) => {
            state.request_open_pr_worktree = Some((ws_idx, number));
            leave_modal(state);
        }
        (ContextMenuKind::RepoPr { url, .. }, Some("Open in browser")) => {
            state.request_open_url = Some(url);
            leave_modal(state);
        }
        (ContextMenuKind::RepoPr { url, .. }, Some("Copy URL")) => {
            state.request_clipboard_write = Some(url.into_bytes());
            leave_modal(state);
        }
        (ContextMenuKind::RepoIssue { number, url, .. }, Some("Run with bora-flow")) => {
            state.request_flow_run = Some(crate::app::state::FlowRunRequest { number, url });
            leave_modal(state);
        }
        (ContextMenuKind::RepoIssue { url, .. }, Some("Open in browser")) => {
            state.request_open_url = Some(url);
            leave_modal(state);
        }
        (ContextMenuKind::RepoIssue { url, .. }, Some("Copy URL")) => {
            state.request_clipboard_write = Some(url.into_bytes());
            leave_modal(state);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some(label @ ("Hide 5m" | "Hide 10m" | "Hide 15m" | "Hide 30m")),
        ) => {
            if let Some(ws) = state.workspaces.get(ws_idx) {
                let minutes = hide_minutes(label);
                let key = format!("ws:{}", ws.id);
                state.hidden_space_keys.insert(
                    key,
                    std::time::Instant::now() + std::time::Duration::from_secs(minutes * 60),
                );
            }
            leave_modal(state);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some("Unhide"),
        ) => {
            let keys: Vec<String> = if let Some(ws) = state.workspaces.get(ws_idx) {
                let mut k = vec![format!("ws:{}", ws.id)];
                if let Some(group) = &ws.visual_group {
                    k.push(format!("vg:{group}"));
                }
                if let Some(space) = ws.git_space() {
                    k.push(space.repo_identity.clone());
                }
                k
            } else {
                Vec::new()
            };
            for key in keys {
                state.hidden_space_keys.remove(&key);
            }
            leave_modal(state);
        }
        (
            ContextMenuKind::GroupHeader { collapse_key, .. },
            Some(label @ ("Hide 5m" | "Hide 10m" | "Hide 15m" | "Hide 30m")),
        ) => {
            let minutes = hide_minutes(label);
            state.hidden_space_keys.insert(
                collapse_key,
                std::time::Instant::now() + std::time::Duration::from_secs(minutes * 60),
            );
            leave_modal(state);
        }
        (ContextMenuKind::GroupHeader { collapse_key, .. }, Some("Unhide")) => {
            state.hidden_space_keys.remove(&collapse_key);
            leave_modal(state);
        }
        (
            ContextMenuKind::Workspace { ws_idx, .. }
            | ContextMenuKind::GitWorkspace { ws_idx, .. },
            Some(label),
        ) if bora_commands.iter().any(|c| c.label == label) => {
            let cmd = bora_commands
                .iter()
                .find(|c| c.label == label)
                .expect("guard guarantees match");
            state.pending_bora_command = Some(crate::app::state::PendingBoraCommand {
                ws_idx,
                command: cmd.command.clone(),
                mode: cmd.mode.clone(),
                label: Some(cmd.label.clone()),
                port: bora_port,
            });
            leave_modal(state);
        }
        (kind, Some(label))
            if crate::app::state::plugin_menu_action_id(&kind, label, &state.installed_plugins)
                .is_some() =>
        {
            let action_id =
                crate::app::state::plugin_menu_action_id(&kind, label, &state.installed_plugins)
                    .expect("guard guarantees match");
            // bora-1e9: deferred like every App-owned action — invoking a
            // plugin command cannot run inside state-only code. Generalizes
            // the old dagr-only deferred flag: any label the menu built via
            // `plugin_menu_titles` resolves back to its `plugin_id.action_id`
            // here, which the App loop hands straight to
            // `find_plugin_action`/`invoke_plugin_action_from_ui`.
            state.request_plugin_action = Some(action_id);
            leave_modal(state);
        }
        _ => leave_modal(state),
    }
}

#[cfg(test)]
pub(crate) fn handle_context_menu_key(
    state: &mut AppState,
    terminal_runtimes: &mut crate::terminal::TerminalRuntimeRegistry,
    key: KeyEvent,
) {
    match key.code {
        KeyCode::Esc => {
            state.context_menu = None;
            leave_modal(state);
        }
        KeyCode::Up => {
            if let Some(menu) = &mut state.context_menu {
                loop {
                    menu.list.move_prev();
                    if menu.items.get(menu.list.highlighted).map(String::as_str)
                        != Some(crate::app::state::CONTEXT_MENU_SEPARATOR)
                    {
                        break;
                    }
                    if menu.list.highlighted == 0 {
                        break;
                    }
                }
            }
        }
        KeyCode::Down => {
            if let Some(menu) = &mut state.context_menu {
                let len = menu.items.len();
                loop {
                    menu.list.move_next(len);
                    if menu.items.get(menu.list.highlighted).map(String::as_str)
                        != Some(crate::app::state::CONTEXT_MENU_SEPARATOR)
                    {
                        break;
                    }
                    if menu.list.highlighted >= len.saturating_sub(1) {
                        break;
                    }
                }
            }
        }
        KeyCode::Enter => {
            if let Some(menu) = state.context_menu.take() {
                let idx = menu.list.highlighted;
                apply_context_menu_action(state, terminal_runtimes, menu, idx);
            }
        }
        _ => {}
    }
}

impl App {
    pub(crate) fn handle_rename_key_via_api(&mut self, key: KeyEvent) {
        if let Some(action) = modal_action_from_key(&key, RENAME_ACTIONS) {
            self.apply_rename_mouse_action_via_api(action);
            return;
        }

        handle_rename_edit_key(&mut self.state, key);
    }

    fn save_rename_modal_via_api(&mut self) {
        let new_name = if self.state.name_input.trim().is_empty() {
            self.state.name_input.clone()
        } else {
            self.state.name_input.trim().to_string()
        };

        match self.state.mode {
            Mode::RenameWorkspace => {
                if let Some(cwd) = self.state.pending_workspace_create_cwd.take() {
                    let suggested_name = crate::workspace::derive_label_from_cwd(&cwd);
                    let label = workspace_create_label(&new_name, &suggested_name);
                    self.runtime_workspace_create(
                        "tui.workspace.create_named",
                        crate::api::schema::WorkspaceCreateParams {
                            cwd: Some(cwd.display().to_string()),
                            focus: true,
                            label,
                            group: None,
                            env: Default::default(),
                        },
                    );
                } else if !self.state.workspaces.is_empty() && !new_name.is_empty() {
                    let workspace_id = self.public_workspace_id(self.state.selected);
                    self.runtime_workspace_rename(
                        "tui.workspace.rename",
                        crate::api::schema::WorkspaceRenameParams {
                            workspace_id,
                            label: new_name,
                        },
                    );
                }
            }
            Mode::RenameTab if self.state.creating_new_tab => {
                let default_name = next_new_tab_default_name(&self.state);
                let label = if new_name.is_empty() || new_name == default_name {
                    None
                } else {
                    Some(new_name)
                };
                self.runtime_tab_create(
                    "tui.tab.create_named",
                    crate::api::schema::TabCreateParams {
                        workspace_id: None,
                        cwd: None,
                        focus: true,
                        label,
                        env: Default::default(),
                    },
                );
            }
            Mode::RenameTab if !new_name.is_empty() => {
                let Some(ws_idx) = self.state.active else {
                    cancel_rename_modal(&mut self.state);
                    return;
                };
                let tab_idx = self.state.workspaces[ws_idx].active_tab;
                let keep_auto_name = self.state.workspaces[ws_idx]
                    .tabs
                    .get(tab_idx)
                    .is_some_and(crate::workspace::Tab::is_auto_named)
                    && self.state.workspaces[ws_idx]
                        .tab_display_name(tab_idx)
                        .is_some_and(|name| new_name == name);
                if !keep_auto_name {
                    if let Some(tab_id) = self.public_tab_id(ws_idx, tab_idx) {
                        self.runtime_tab_rename(
                            "tui.tab.rename",
                            crate::api::schema::TabRenameParams {
                                tab_id,
                                label: new_name,
                            },
                        );
                    }
                }
            }
            Mode::RenamePane => {
                if let (Some(ws_idx), Some(pane_id)) =
                    (self.state.active, self.state.rename_pane_target)
                {
                    if let Some(pane_id) = self.public_pane_id(ws_idx, pane_id) {
                        self.runtime_pane_rename(
                            "tui.pane.rename",
                            crate::api::schema::PaneRenameParams {
                                pane_id,
                                label: Some(new_name),
                            },
                        );
                    }
                }
            }
            Mode::SetWorkspaceGroup if !self.state.workspaces.is_empty() => {
                // visual_group is TUI presentation state, not a runtime
                // mutation. Empty input clears the group (same as "Remove
                // from group").
                let selected = self.state.selected;
                if new_name.trim().is_empty() {
                    self.state.workspaces[selected].visual_group = None;
                } else {
                    self.state.workspaces[selected].visual_group = Some(new_name);
                }
                self.state.mark_session_dirty();
            }
            Mode::ProjectNameInput => {
                if let Some(target) = self.state.project_name_target.take() {
                    let name = new_name.trim().to_string();
                    if !name.is_empty() {
                        match target {
                            crate::app::state::ProjectNameTarget::Rename { slug } => {
                                self.runtime_project_update(
                                    "tui.project.update",
                                    crate::api::schema::ProjectUpdateParams {
                                        slug,
                                        name: Some(name),
                                        channel: None,
                                        auto_join: None,
                                    },
                                );
                            }
                            crate::app::state::ProjectNameTarget::New { member_dir } => {
                                let slug = projects::load_projects_file_fresh()
                                    .map(|file| unique_project_slug(&file, &slug_from_name(&name)))
                                    .unwrap_or_else(|_| slug_from_name(&name));
                                self.runtime_project_create(
                                    "tui.project.create",
                                    crate::api::schema::ProjectCreateParams {
                                        slug: slug.clone(),
                                        name: Some(name),
                                        channel: None,
                                        auto_join: None,
                                    },
                                );
                                if let Some(dir) = member_dir {
                                    self.runtime_project_member_add(
                                        "tui.project.member_add",
                                        crate::api::schema::ProjectMemberAddParams {
                                            slug,
                                            dir,
                                            worktrees: WorktreesScope::All,
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        cancel_rename_modal(&mut self.state);
    }

    pub(super) fn apply_rename_mouse_action_via_api(&mut self, action: ModalAction) {
        match action {
            ModalAction::Save => self.save_rename_modal_via_api(),
            ModalAction::Clear => {
                self.state.name_input.clear();
                self.state.name_input_replace_on_type = false;
            }
            ModalAction::Cancel => cancel_rename_modal(&mut self.state),
            _ => {}
        }
    }

    pub(super) fn confirm_close_accept_via_api(&mut self) {
        let ws_idx = self.state.selected;
        if ws_idx < self.state.workspaces.len() {
            self.close_workspace_idx_via_api(ws_idx);
        }
        self.state.mode = if self.state.active.is_some() {
            Mode::Terminal
        } else {
            Mode::Navigate
        };
    }

    pub(crate) fn handle_resize_key_via_api(&mut self, raw_key: TerminalKey) {
        let key = raw_key.as_key_event();
        if key.code == KeyCode::Esc
            || key.code == KeyCode::Enter
            || self.state.keybinds.resize_mode.matches_prefix_key(&raw_key)
            || self.state.keybinds.resize_mode.matches_direct_key(&raw_key)
        {
            self.state.mode = if self.state.active.is_some() {
                Mode::Terminal
            } else {
                Mode::Navigate
            };
            return;
        }

        let direction = match key.code {
            KeyCode::Char('h') | KeyCode::Left => Some(NavDirection::Left),
            KeyCode::Char('l') | KeyCode::Right => Some(NavDirection::Right),
            KeyCode::Char('j') | KeyCode::Down => Some(NavDirection::Down),
            KeyCode::Char('k') | KeyCode::Up => Some(NavDirection::Up),
            _ => None,
        };
        if let Some(direction) = direction {
            self.runtime_pane_resize(
                "tui.pane.resize",
                crate::api::schema::PaneResizeParams {
                    pane_id: None,
                    direction: super::navigate::api_pane_direction(direction),
                    amount: None,
                },
            );
        }
    }

    pub(crate) fn handle_confirm_close_key_via_api(&mut self, key: KeyEvent) {
        match modal_action_from_key(&key, CONFIRM_CLOSE_ACTIONS) {
            Some(ModalAction::Confirm) => {
                self.confirm_close_accept_via_api();
            }
            Some(ModalAction::Cancel) => confirm_close_cancel(&mut self.state),
            _ => {}
        }
    }

    pub(crate) fn handle_context_menu_key_via_api(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.context_menu = None;
                leave_modal(&mut self.state);
            }
            KeyCode::Up => {
                if let Some(menu) = &mut self.state.context_menu {
                    menu.list.move_prev();
                }
            }
            KeyCode::Down => {
                if let Some(menu) = &mut self.state.context_menu {
                    menu.list.move_next(menu.items().len());
                }
            }
            KeyCode::Enter => {
                if let Some(menu) = self.state.context_menu.take() {
                    let idx = menu.list.highlighted;
                    self.apply_context_menu_action_via_api(menu, idx);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn apply_context_menu_action_via_api(&mut self, menu: ContextMenuState, idx: usize) {
        let item_owned = menu.items.get(idx).cloned();
        let item = item_owned.as_deref();
        let bora_commands = menu.bora_commands;
        let bora_port = menu.bora_port;
        match (menu.kind, item) {
            (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("New worktree")) => {
                self.state.request_new_linked_worktree = Some(ws_idx);
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Delete worktree\u{2026}")) => {
                self.state.request_remove_linked_worktree = Some(ws_idx);
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Open worktree\u{2026}")) => {
                self.state.request_open_existing_worktree = Some(ws_idx);
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Merge to main")) => {
                self.state.request_merge_worktree_to_main = Some(ws_idx);
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Open PR")) => {
                self.state.request_open_worktree_pr = Some(ws_idx);
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::GitWorkspace { ws_idx, .. }, Some("Sync")) => {
                self.state.request_sync_workspace_git = Some(ws_idx);
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("Refresh status"),
            ) => {
                // Force a fresh agent detection probe on every pane and make
                // sure idle terminals have an idle timestamp (lost across
                // restore/handoff). Also kick the git/PR/check refetch for the
                // workspace under the cursor.
                self.reset_all_agent_detection_runtimes();
                let now = std::time::Instant::now();
                for terminal in self.state.terminals.values_mut() {
                    if terminal.state == crate::detect::AgentState::Idle
                        && terminal.idle_since.is_none()
                    {
                        terminal.idle_since = Some(now);
                    }
                }
                self.mark_git_status_refresh_due(now);
                self.start_git_status_refresh_if_due(now);
                if let Some(workspace_id) =
                    self.state.workspaces.get(ws_idx).map(|ws| ws.id.clone())
                {
                    self.start_checks_fetch(&workspace_id);
                }
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::GitWorkspace {
                    ws_idx, collapsed, ..
                },
                Some("Collapse" | "Expand"),
            ) => {
                if let Some(key) = self
                    .state
                    .workspaces
                    .get(ws_idx)
                    .and_then(|ws| ws.worktree_space())
                    .map(|space| space.key.clone())
                {
                    if collapsed {
                        self.state.collapsed_space_keys.remove(&key);
                    } else {
                        self.state.collapsed_space_keys.insert(key);
                    }
                    self.state.mark_session_dirty();
                }
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("Rename"),
            ) => open_rename_workspace(&mut self.state, &self.terminal_runtimes, ws_idx),
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("Copy path"),
            ) => {
                if let Some(ws) = self.state.workspaces.get(ws_idx) {
                    let path = ws.identity_cwd.display().to_string();
                    self.state.request_clipboard_write = Some(path.into_bytes());
                }
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("New group\u{2026}"),
            ) => {
                open_set_workspace_group(&mut self.state, ws_idx);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some(item_str),
            ) if item_str.starts_with("\u{2192} ") => {
                // visual_group is TUI presentation state, not a runtime mutation.
                let group_name = item_str["\u{2192} ".len()..].to_string();
                if let Some(ws) = self.state.workspaces.get_mut(ws_idx) {
                    ws.visual_group = Some(group_name);
                    self.state.mark_session_dirty();
                }
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("Remove from group"),
            ) => {
                if let Some(ws) = self.state.workspaces.get_mut(ws_idx) {
                    ws.visual_group = None;
                    self.state.mark_session_dirty();
                }
                leave_modal(&mut self.state);
            }
            // ── Project assembly (bora-uqv) ─────────────────────────
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some(item_str),
            ) if item_str.starts_with("Add to ") => {
                let slug = item_str["Add to ".len()..].to_string();
                if let Some(dir) = self
                    .state
                    .workspaces
                    .get(ws_idx)
                    .map(crate::workspace::Workspace::project_member_dir)
                {
                    self.runtime_project_member_add(
                        "tui.project.member_add",
                        crate::api::schema::ProjectMemberAddParams {
                            slug,
                            dir,
                            worktrees: WorktreesScope::All,
                        },
                    );
                }
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("New project\u{2026}"),
            ) => {
                if let Some(dir) = self
                    .state
                    .workspaces
                    .get(ws_idx)
                    .map(crate::workspace::Workspace::project_member_dir)
                {
                    open_new_project_prompt(&mut self.state, Some(dir));
                } else {
                    leave_modal(&mut self.state);
                }
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("Remove"),
            ) => {
                if let Some(dir) = self
                    .state
                    .workspaces
                    .get(ws_idx)
                    .map(crate::workspace::Workspace::project_member_dir)
                {
                    if let Ok(file) = projects::load_projects_file_fresh() {
                        let ctx = ProjectAssemblyContext::for_dir(&file, &dir);
                        if let Some(slug) = ctx.current_project_slug {
                            self.runtime_project_member_remove(
                                "tui.project.member_remove",
                                crate::api::schema::ProjectMemberRemoveParams { slug, dir },
                            );
                        }
                    }
                }
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::ProjectHeader { slug, .. }, Some("Add workspaces\u{2026}")) => {
                let orphans = orphan_member_dirs(&self.state);
                if orphans.is_empty() {
                    leave_modal(&mut self.state);
                } else {
                    let items = orphans
                        .iter()
                        .map(|dir| format!("\u{ff0b} {dir}"))
                        .collect();
                    follow_up_menu(
                        &mut self.state,
                        ContextMenuKind::ProjectOrphanPicker { slug, orphans },
                        items,
                        menu.x,
                        menu.y,
                    );
                }
            }
            (ContextMenuKind::ProjectHeader { .. }, Some("New project\u{2026}")) => {
                open_new_project_prompt(&mut self.state, None);
            }
            (
                ContextMenuKind::ProjectHeader {
                    slug: Some(slug), ..
                },
                Some("Rename project\u{2026}"),
            ) => {
                let prefill = projects::load_projects_file_fresh()
                    .ok()
                    .and_then(|file| {
                        file.projects
                            .get(&slug)
                            .map(|project| project.name.clone().unwrap_or_else(|| slug.clone()))
                    })
                    .unwrap_or_else(|| slug.clone());
                open_project_name_input(
                    &mut self.state,
                    crate::app::state::ProjectNameTarget::Rename { slug },
                    prefill,
                    false,
                );
            }
            (ContextMenuKind::ProjectOrphanPicker { slug, orphans }, Some(_)) => {
                match (slug, orphans.get(idx).cloned()) {
                    (Some(slug), Some(dir)) => {
                        self.runtime_project_member_add(
                            "tui.project.member_add",
                            crate::api::schema::ProjectMemberAddParams {
                                slug,
                                dir,
                                worktrees: WorktreesScope::All,
                            },
                        );
                        leave_modal(&mut self.state);
                    }
                    (None, Some(dir)) => {
                        let items = assembly_items_for_dir(&dir);
                        follow_up_menu(
                            &mut self.state,
                            ContextMenuKind::ProjectMemberTargets { member_dir: dir },
                            items,
                            menu.x,
                            menu.y,
                        );
                    }
                    _ => leave_modal(&mut self.state),
                }
            }
            (ContextMenuKind::ProjectMemberTargets { member_dir }, Some(item_str))
                if item_str.starts_with("Add to ") =>
            {
                let slug = item_str["Add to ".len()..].to_string();
                self.runtime_project_member_add(
                    "tui.project.member_add",
                    crate::api::schema::ProjectMemberAddParams {
                        slug,
                        dir: member_dir,
                        worktrees: WorktreesScope::All,
                    },
                );
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::ProjectMemberTargets { member_dir }, Some("New project\u{2026}")) => {
                open_new_project_prompt(&mut self.state, Some(member_dir));
            }
            (ContextMenuKind::ProjectMemberTargets { member_dir }, Some("Remove")) => {
                if let Ok(file) = projects::load_projects_file_fresh() {
                    let ctx = ProjectAssemblyContext::for_dir(&file, &member_dir);
                    if let Some(slug) = ctx.current_project_slug {
                        self.runtime_project_member_remove(
                            "tui.project.member_remove",
                            crate::api::schema::ProjectMemberRemoveParams {
                                slug,
                                dir: member_dir,
                            },
                        );
                    }
                }
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("Close" | "Close workspace"),
            ) => {
                self.state.selected = ws_idx;
                if self.state.confirm_close {
                    open_confirm_close(&mut self.state);
                } else {
                    self.close_workspace_idx_via_api(ws_idx);
                    self.state.mode = Mode::Navigate;
                }
            }
            (ContextMenuKind::Tab { ws_idx, tab_idx }, Some("New tab")) => {
                self.focus_workspace_idx_via_api(ws_idx);
                self.focus_tab_idx_via_api(tab_idx);
                open_new_tab_dialog(&mut self.state);
            }
            (ContextMenuKind::Tab { ws_idx, tab_idx }, Some("Rename")) => {
                self.focus_workspace_idx_via_api(ws_idx);
                self.focus_tab_idx_via_api(tab_idx);
                open_rename_active_tab(&mut self.state, false);
            }
            (ContextMenuKind::Tab { ws_idx, tab_idx }, Some("Close")) => {
                self.focus_workspace_idx_via_api(ws_idx);
                self.focus_tab_idx_via_api(tab_idx);
                if !self.close_active_tab_via_api_requires_confirmation() {
                    leave_modal(&mut self.state);
                }
            }
            (ContextMenuKind::Pane { pane_id, .. }, Some("Rename pane")) => {
                open_rename_pane(&mut self.state, pane_id);
            }
            (
                ContextMenuKind::Pane {
                    ws_idx, pane_id, ..
                },
                Some("Clear pane name"),
            ) => {
                if let Some(pane_id) = self.public_pane_id(ws_idx, pane_id) {
                    self.runtime_pane_rename(
                        "tui.pane.clear_name",
                        crate::api::schema::PaneRenameParams {
                            pane_id,
                            label: None,
                        },
                    );
                }
                self.state.mode = Mode::Terminal;
            }
            (
                ContextMenuKind::Pane {
                    ws_idx, pane_id, ..
                },
                Some(action @ ("Send right-clicks to pane" | "Use Herdr right-click menu")),
            ) => {
                if let Some(pane_id) = self.public_pane_id(ws_idx, pane_id) {
                    self.runtime_pane_input_set(
                        "tui.pane.input.set",
                        crate::api::schema::PaneInputSetParams {
                            pane_id,
                            right_click: if action == "Send right-clicks to pane" {
                                crate::api::schema::PaneRightClickTarget::Pane
                            } else {
                                crate::api::schema::PaneRightClickTarget::Herdr
                            },
                        },
                    );
                }
                self.state.mode = Mode::Terminal;
            }
            (
                ContextMenuKind::Pane {
                    ws_idx,
                    pane_id,
                    source_pane_id: Some(source_pane_id),
                    ..
                },
                Some("Swap with focused pane"),
            ) => {
                let source_public_id = self.public_pane_id(ws_idx, source_pane_id);
                let target_public_id = self.public_pane_id(ws_idx, pane_id);
                if let (Some(source_public_id), Some(target_public_id)) =
                    (source_public_id, target_public_id)
                {
                    self.runtime_pane_swap(
                        "tui.pane.swap_exact",
                        crate::api::schema::PaneSwapParams {
                            pane_id: None,
                            direction: None,
                            source_pane_id: Some(source_public_id),
                            target_pane_id: Some(target_public_id),
                        },
                    );
                    self.focus_pane_internal_via_api(ws_idx, source_pane_id);
                }
                self.state.mode = Mode::Terminal;
            }
            (
                ContextMenuKind::Pane {
                    ws_idx, pane_id, ..
                },
                Some("Split right"),
            ) => {
                self.focus_pane_internal_via_api(ws_idx, pane_id);
                self.split_focused_pane_via_api(crate::api::schema::SplitDirection::Right);
                self.state.mode = Mode::Terminal;
            }
            (
                ContextMenuKind::Pane {
                    ws_idx, pane_id, ..
                },
                Some("Split down"),
            ) => {
                self.focus_pane_internal_via_api(ws_idx, pane_id);
                self.split_focused_pane_via_api(crate::api::schema::SplitDirection::Down);
                self.state.mode = Mode::Terminal;
            }
            (
                ContextMenuKind::Pane {
                    ws_idx, pane_id, ..
                },
                Some("Zoom"),
            ) => {
                self.focus_pane_internal_via_api(ws_idx, pane_id);
                self.zoom_focused_pane_via_api();
                self.state.mode = Mode::Terminal;
            }
            (
                ContextMenuKind::Pane {
                    ws_idx, pane_id, ..
                },
                Some("Close pane"),
            ) => {
                self.focus_pane_internal_via_api(ws_idx, pane_id);
                if !self.close_focused_pane_via_api_requires_confirmation() {
                    self.state.mode = if self.state.active.is_some() {
                        Mode::Terminal
                    } else {
                        Mode::Navigate
                    };
                }
            }
            (ContextMenuKind::RepoPr { ws_idx, number, .. }, Some("Open in worktree")) => {
                self.state.request_open_pr_worktree = Some((ws_idx, number));
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::RepoPr { url, .. }, Some("Open in browser")) => {
                self.state.request_open_url = Some(url);
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::RepoPr { url, .. }, Some("Copy URL")) => {
                self.state.request_clipboard_write = Some(url.into_bytes());
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::RepoIssue { number, url, .. }, Some("Run with bora-flow")) => {
                self.state.request_flow_run =
                    Some(crate::app::state::FlowRunRequest { number, url });
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::RepoIssue { url, .. }, Some("Open in browser")) => {
                self.state.request_open_url = Some(url);
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::RepoIssue { url, .. }, Some("Copy URL")) => {
                self.state.request_clipboard_write = Some(url.into_bytes());
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some(label @ ("Hide 5m" | "Hide 10m" | "Hide 15m" | "Hide 30m")),
            ) => {
                if let Some(ws) = self.state.workspaces.get(ws_idx) {
                    let minutes = hide_minutes(label);
                    let key = format!("ws:{}", ws.id);
                    self.state.hidden_space_keys.insert(
                        key,
                        std::time::Instant::now() + std::time::Duration::from_secs(minutes * 60),
                    );
                }
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some("Unhide"),
            ) => {
                let keys: Vec<String> = if let Some(ws) = self.state.workspaces.get(ws_idx) {
                    let mut k = vec![format!("ws:{}", ws.id)];
                    if let Some(group) = &ws.visual_group {
                        k.push(format!("vg:{group}"));
                    }
                    if let Some(space) = ws.git_space() {
                        k.push(space.repo_identity.clone());
                    }
                    k
                } else {
                    Vec::new()
                };
                for key in keys {
                    self.state.hidden_space_keys.remove(&key);
                }
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::GroupHeader { collapse_key, .. },
                Some(label @ ("Hide 5m" | "Hide 10m" | "Hide 15m" | "Hide 30m")),
            ) => {
                let minutes = hide_minutes(label);
                self.state.hidden_space_keys.insert(
                    collapse_key,
                    std::time::Instant::now() + std::time::Duration::from_secs(minutes * 60),
                );
                leave_modal(&mut self.state);
            }
            (ContextMenuKind::GroupHeader { collapse_key, .. }, Some("Unhide")) => {
                self.state.hidden_space_keys.remove(&collapse_key);
                leave_modal(&mut self.state);
            }
            (
                ContextMenuKind::Workspace { ws_idx, .. }
                | ContextMenuKind::GitWorkspace { ws_idx, .. },
                Some(label),
            ) if bora_commands.iter().any(|c| c.label == label) => {
                let cmd = bora_commands
                    .iter()
                    .find(|c| c.label == label)
                    .expect("guard guarantees match");
                self.state.pending_bora_command = Some(crate::app::state::PendingBoraCommand {
                    ws_idx,
                    command: cmd.command.clone(),
                    mode: cmd.mode.clone(),
                    label: Some(cmd.label.clone()),
                    port: bora_port,
                });
                leave_modal(&mut self.state);
            }
            (kind, Some(label))
                if crate::app::state::plugin_menu_action_id(
                    &kind,
                    label,
                    &self.state.installed_plugins,
                )
                .is_some() =>
            {
                let action_id = crate::app::state::plugin_menu_action_id(
                    &kind,
                    label,
                    &self.state.installed_plugins,
                )
                .expect("guard guarantees match");
                // bora-1e9: same deferred-flag path as the state-only twin
                // — the App loop performs the invoke so the headless
                // server and the TUI share one behavior.
                self.state.request_plugin_action = Some(action_id);
                leave_modal(&mut self.state);
            }
            _ => leave_modal(&mut self.state),
        }
    }
}

fn cancel_rename_modal(state: &mut AppState) {
    state.creating_new_tab = false;
    state.requested_new_tab_name = None;
    state.pending_workspace_create_cwd = None;
    state.rename_pane_target = None;
    state.project_name_target = None;
    state.name_input.clear();
    state.name_input_replace_on_type = false;
    leave_modal(state);
}

impl AppState {
    pub(super) fn global_menu_item_at(&self, col: u16, row: u16) -> Option<GlobalMenuAction> {
        let rect = self.global_menu_rect();
        if col <= rect.x
            || col >= rect.x + rect.width.saturating_sub(1)
            || row <= rect.y
            || row >= rect.y + rect.height.saturating_sub(1)
        {
            return None;
        }
        let idx = (row - rect.y - 1) as usize;
        global_menu_actions(self).get(idx).copied()
    }
}

// ── Project assembly menu (bora-49p.5, wired by bora-uqv) ───────────────
//
// The right-click menu for a Project-view row that edits `projects.yml`
// membership. bora-49p.5 built this decision logic and left it deliberately
// unwired (state.rs belonged to a sibling bead); bora-uqv wired it into
// `ContextMenuKind` — `ProjectHeader` (group header), `ProjectOrphanPicker`
// ("Add workspaces…"), `ProjectMemberTargets` (checkout row / follow-up),
// and the assembly section spliced into Workspace/GitWorkspace menus in
// Project view. What each item writes still lives here, in two flavors
// mirroring every other menu: the direct path (cfg(test) — file writes
// through `update_projects_file`) and the live path (`project.*` verbs via
// `dispatch_runtime_mutation`). Membership is always resolved against the
// file read FRESH (`load_projects_file_fresh`), never a cached
// `ProjectsStore` value; the store's tick poll (`runtime.rs`
// `reload_if_changed`) picks the write up for the next frame.

use crate::persist::projects::{self, WorktreesScope};
#[cfg(test)]
use crate::persist::projects::{Member, Project};

/// One Project-view row's membership, resolved at the moment the assembly
/// menu would open. `member_dir` is the exact string stored as `Member.dir`
/// in `projects.yml` — the same representation `ProjectRowTarget::OpenWorktree`
/// already carries as `checkout_key`, compared by plain string equality just
/// like `app::api::projects::handle_project_member_add`/`_remove` do; never
/// re-resolved through git discovery here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectAssemblyContext {
    pub member_dir: String,
    /// `Some(slug)` when `member_dir` is already a declared member of that
    /// project; `None` for a workspace/worktree with no project yet.
    pub current_project_slug: Option<String>,
}

impl ProjectAssemblyContext {
    /// Resolves `dir`'s membership against the CURRENT `projects.yml`
    /// content (`file`) — caller's responsibility to have read it fresh via
    /// `projects::load_projects_file_fresh`, never a cached `ProjectsStore`
    /// value, matching every other `project.*` writer's own rule.
    pub(crate) fn for_dir(file: &projects::ProjectsFile, dir: &str) -> Self {
        let current_project_slug = file
            .projects
            .iter()
            .find(|(_, project)| project.members.iter().any(|member| member.dir == dir))
            .map(|(slug, _)| slug.clone());
        Self {
            member_dir: dir.to_string(),
            current_project_slug,
        }
    }
}

/// Assembly-menu item labels for `ctx`. `known_project_slugs` is every slug
/// currently in `projects.yml`, in the order `"Add to <slug>"` should offer
/// them. Membership is the only thing gating item presence: `Remove` only
/// when `ctx.current_project_slug` is `Some` (there is a project to remove
/// from); `"Add to <slug>"` / `"New project…"` only when it is `None`.
/// "New project…" never writes from here — every dispatch path routes it to
/// the `ProjectNameInput` prompt instead.
pub(crate) fn project_assembly_menu_items(
    ctx: &ProjectAssemblyContext,
    known_project_slugs: &[String],
) -> Vec<String> {
    let mut items = Vec::new();
    if ctx.current_project_slug.is_none() {
        for slug in known_project_slugs {
            items.push(format!("Add to {slug}"));
        }
        items.push("New project\u{2026}".to_string());
    } else {
        items.push("Remove".to_string());
    }
    items
}

/// The assembly items a Project-view workspace row splices into its context
/// menu: membership resolved against `projects.yml` read FRESH, never the
/// cached `ProjectsStore`.
pub(crate) fn workspace_assembly_items(
    workspaces: &[crate::workspace::Workspace],
    ws_idx: usize,
) -> Vec<String> {
    let Some(ws) = workspaces.get(ws_idx) else {
        return Vec::new();
    };
    assembly_items_for_dir(&ws.project_member_dir())
}

/// `project_assembly_menu_items` for one dir, resolving membership fresh.
/// An unreadable file degrades to just "New project…" — the one item that
/// needs nothing from disk.
pub(crate) fn assembly_items_for_dir(member_dir: &str) -> Vec<String> {
    let Ok(file) = projects::load_projects_file_fresh() else {
        return vec!["New project\u{2026}".to_string()];
    };
    let ctx = ProjectAssemblyContext::for_dir(&file, member_dir);
    let slugs: Vec<String> = file.projects.keys().cloned().collect();
    project_assembly_menu_items(&ctx, &slugs)
}

/// Why a file-level assembly write could not be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum ProjectAssemblyError {
    /// `update_projects_file`'s own mutation rule rejected the write (e.g.
    /// unknown project) — the file was NOT touched.
    Rejected(String),
    /// Reading or writing `projects.yml` itself failed.
    Io(String),
}

#[allow(dead_code)] // see the module note above
#[cfg(test)]
impl From<projects::ProjectsUpdateError<String>> for ProjectAssemblyError {
    fn from(err: projects::ProjectsUpdateError<String>) -> Self {
        match err {
            projects::ProjectsUpdateError::Mutate(message) => Self::Rejected(message),
            projects::ProjectsUpdateError::Load(message)
            | projects::ProjectsUpdateError::Save(message) => Self::Io(message),
        }
    }
}

#[cfg(test)]
pub(crate) fn add_member(slug: &str, dir: &str) -> Result<(), ProjectAssemblyError> {
    let slug = slug.to_string();
    let dir = dir.to_string();
    projects::update_projects_file(move |file| {
        let Some(project) = file.projects.get_mut(&slug) else {
            return Err(format!("project {slug:?} not found"));
        };
        match project.members.iter_mut().find(|member| member.dir == dir) {
            Some(existing) => existing.worktrees = WorktreesScope::All,
            None => project.members.push(Member {
                dir: dir.clone(),
                worktrees: WorktreesScope::All,
                template: None,
            }),
        }
        Ok(())
    })
    .map(|_| ())
    .map_err(ProjectAssemblyError::from)
}

#[cfg(test)]
pub(crate) fn remove_member(slug: &str, dir: &str) -> Result<(), ProjectAssemblyError> {
    let slug = slug.to_string();
    let dir = dir.to_string();
    projects::update_projects_file(move |file| {
        let Some(project) = file.projects.get_mut(&slug) else {
            return Err(format!("project {slug:?} not found"));
        };
        let before = project.members.len();
        project.members.retain(|member| member.dir != dir);
        if project.members.len() == before {
            return Err(format!("project {slug:?} has no member dir {dir:?}"));
        }
        Ok(())
    })
    .map(|_| ())
    .map_err(ProjectAssemblyError::from)
}

/// Slugifies a typed project name exactly as `slug_from_dir` slugifies a
/// basename.
pub(crate) fn slug_from_name(name: &str) -> String {
    let slug: String = name
        .trim()
        .to_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
        .collect();
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug
    }
}

/// `base`, or `base-2`, `base-3`, … — the first slug not already taken in
/// `file`. The slug is the internal key; the typed name stays the display.
pub(crate) fn unique_project_slug(file: &projects::ProjectsFile, base: &str) -> String {
    if !file.projects.contains_key(base) {
        return base.to_string();
    }
    for n in 2.. {
        let candidate = format!("{base}-{n}");
        if !file.projects.contains_key(&candidate) {
            return candidate;
        }
    }
    unreachable!("u32 slug counter cannot wrap")
}

/// Creates a project named `name`, with `member_dir` (when present) as its
/// first member — the direct-path (cfg(test)) half of the
/// `ProjectNameInput` confirm; the live path sends `project.create` +
/// `project.member_add` verbs instead. The slug is `slug_from_name(name)`
/// made unique against the CURRENT file.
#[cfg(test)]
pub(crate) fn create_project_with_optional_member(
    name: &str,
    member_dir: Option<String>,
) -> Result<String, ProjectAssemblyError> {
    let name = name.to_string();
    let mut slug_out = String::new();
    projects::update_projects_file(|file| {
        let slug = unique_project_slug(file, &slug_from_name(&name));
        file.projects.insert(
            slug.clone(),
            Project {
                name: Some(name.clone()),
                channel: None,
                members: member_dir
                    .iter()
                    .map(|dir| Member {
                        dir: dir.clone(),
                        worktrees: WorktreesScope::All,
                        template: None,
                    })
                    .collect(),
                orchestrator: None,
                sections: None,
                auto_join: true,
            },
        );
        slug_out = slug;
        Ok::<(), String>(())
    })
    .map_err(ProjectAssemblyError::from)?;
    Ok(slug_out)
}

/// Renames a project's display name (`name:` in `projects.yml`) — the
/// direct-path half of the `Rename` confirm.
#[cfg(test)]
pub(crate) fn rename_project(slug: &str, name: &str) -> Result<(), ProjectAssemblyError> {
    let slug = slug.to_string();
    let name = name.to_string();
    projects::update_projects_file(move |file| {
        let Some(project) = file.projects.get_mut(&slug) else {
            return Err(format!("project {slug:?} not found"));
        };
        project.name = Some(name.clone());
        Ok(())
    })
    .map(|_| ())
    .map_err(ProjectAssemblyError::from)
}

/// Opens the project name prompt (create or rename, per `target`) with
/// `prefill` in the input. `replace_on_type` selects the suggestion so
/// typing overwrites it — the same convention as the new-workspace dialog.
pub(crate) fn open_project_name_input(
    state: &mut AppState,
    target: crate::app::state::ProjectNameTarget,
    prefill: String,
    replace_on_type: bool,
) {
    state.pending_workspace_create_cwd = None;
    state.rename_pane_target = None;
    state.name_input = prefill;
    state.name_input_replace_on_type = replace_on_type;
    state.project_name_target = Some(target);
    state.mode = Mode::ProjectNameInput;
}

/// Member dirs of workspaces no declared project claims (exact `Member.dir`
/// equality, the same representation the menu itself writes). Candidates
/// for the "Add workspaces…" picker, in sidebar order.
pub(crate) fn orphan_member_dirs(state: &AppState) -> Vec<String> {
    let Ok(file) = projects::load_projects_file_fresh() else {
        return Vec::new();
    };
    state
        .workspaces
        .iter()
        .map(crate::workspace::Workspace::project_member_dir)
        .filter(|dir| {
            !file
                .projects
                .values()
                .any(|project| project.members.iter().any(|member| &member.dir == dir))
        })
        .collect()
}
/// Opens the new-project prompt, prefilled with `member_dir`'s basename
/// (empty input when the menu came from a group header, where there is no
/// dir to suggest one). Shared by every dispatch path — opening the prompt
/// is local state, never a runtime mutation.
pub(crate) fn open_new_project_prompt(state: &mut AppState, member_dir: Option<String>) {
    let prefill = member_dir
        .as_deref()
        .and_then(|dir| std::path::Path::new(dir).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    open_project_name_input(
        state,
        crate::app::state::ProjectNameTarget::New { member_dir },
        prefill,
        true,
    );
}

/// The follow-up menu state after a pick that opens another menu (the
/// orphan picker, or the project-target menu after it): same position,
/// fresh items, cursor back at the top.
pub(crate) fn follow_up_menu(
    state: &mut AppState,
    kind: ContextMenuKind,
    items: Vec<String>,
    x: u16,
    y: u16,
) {
    state.context_menu = Some(ContextMenuState {
        items,
        kind,
        x,
        y,
        list: crate::app::state::MenuListState::new(0),
        bora_commands: vec![],
        bora_port: None,
    });
}

/// Direct-path (cfg(test)) "Remove": resolves the dir's current project
/// against `projects.yml` read fresh and removes it. A dir with no project
/// is a no-op — the menu only shows "Remove" to members.
#[cfg(test)]
pub(super) fn remove_membership_direct(member_dir: &str) {
    if let Ok(file) = projects::load_projects_file_fresh() {
        let ctx = ProjectAssemblyContext::for_dir(&file, member_dir);
        if let Some(slug) = ctx.current_project_slug {
            if let Err(err) = remove_member(&slug, member_dir) {
                tracing::warn!(err = ?err, "project member_remove failed");
            }
        }
    }
}

#[cfg(test)]
mod project_assembly_menu_tests {
    use super::*;
    use crate::config::IsolatedDirs;

    fn seed_project(slug: &str, members: &[&str]) {
        let slug = slug.to_string();
        let members: Vec<String> = members.iter().map(ToString::to_string).collect();
        projects::update_projects_file::<String>(move |file| {
            file.projects.insert(
                slug.clone(),
                Project {
                    name: None,
                    channel: None,
                    members: members
                        .iter()
                        .map(|dir| Member {
                            dir: dir.clone(),
                            worktrees: WorktreesScope::All,
                            template: None,
                        })
                        .collect(),
                    orchestrator: None,
                    sections: None,
                    auto_join: true,
                },
            );
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn context_for_a_declared_member_carries_its_project_slug() {
        let _isolated = IsolatedDirs::new("assembly-menu-context-member");
        seed_project("cnb", &["/repo/cnb"]);

        let file = projects::load_projects_file_fresh().unwrap();
        let ctx = ProjectAssemblyContext::for_dir(&file, "/repo/cnb");

        assert_eq!(ctx.current_project_slug.as_deref(), Some("cnb"));
    }

    #[test]
    fn context_for_an_unmatched_workspace_has_no_project() {
        let _isolated = IsolatedDirs::new("assembly-menu-context-unmatched");
        seed_project("cnb", &["/repo/cnb"]);

        let file = projects::load_projects_file_fresh().unwrap();
        let ctx = ProjectAssemblyContext::for_dir(&file, "/repo/other");

        assert_eq!(ctx.current_project_slug, None);
    }

    #[test]
    fn menu_items_are_gated_on_membership_never_shown_in_both_cases() {
        let member = ProjectAssemblyContext {
            member_dir: "/repo/cnb".into(),
            current_project_slug: Some("cnb".into()),
        };
        let unmatched = ProjectAssemblyContext {
            member_dir: "/repo/other".into(),
            current_project_slug: None,
        };
        let slugs = vec!["cnb".to_string()];

        let member_items = project_assembly_menu_items(&member, &slugs);
        let unmatched_items = project_assembly_menu_items(&unmatched, &slugs);

        assert!(member_items.contains(&"Remove".to_string()));
        assert!(!unmatched_items.contains(&"Remove".to_string()));

        assert!(unmatched_items.contains(&"Add to cnb".to_string()));
        assert!(!member_items.contains(&"Add to cnb".to_string()));
    }

    #[test]
    fn add_to_project_action_persists_a_new_member_to_projects_yml() {
        let _isolated = IsolatedDirs::new("assembly-menu-add-to-project");
        seed_project("cnb", &[]);
        let ctx = ProjectAssemblyContext {
            member_dir: "/repo/cnb-worktree".into(),
            current_project_slug: None,
        };

        add_member("cnb", &ctx.member_dir).unwrap();

        let file = projects::load_projects_file_fresh().unwrap();
        let project = file.projects.get("cnb").expect("project still present");
        assert!(project
            .members
            .iter()
            .any(|m| m.dir == "/repo/cnb-worktree"));
    }

    #[test]
    fn remove_action_deletes_the_member_and_persists() {
        let _isolated = IsolatedDirs::new("assembly-menu-remove");
        seed_project("cnb", &["/repo/cnb"]);
        let ctx = ProjectAssemblyContext {
            member_dir: "/repo/cnb".into(),
            current_project_slug: Some("cnb".into()),
        };

        remove_member("cnb", &ctx.member_dir).unwrap();

        let file = projects::load_projects_file_fresh().unwrap();
        assert!(file.projects.get("cnb").unwrap().members.is_empty());
    }

    #[test]
    fn remove_without_a_project_in_context_is_rejected_before_any_write() {
        let _isolated = IsolatedDirs::new("assembly-menu-remove-guard");
        // The dispatch arms guard on `current_project_slug` before calling
        // `remove_member`; at the file level, removing a dir no project
        // claims is a mutation rejection and touches nothing.
        seed_project("cnb", &["/repo/cnb"]);
        let result = remove_member("cnb", "/repo/absent");

        assert!(matches!(result, Err(ProjectAssemblyError::Rejected(_))));
        let file = projects::load_projects_file_fresh().unwrap();
        assert_eq!(file.projects["cnb"].members.len(), 1);
    }

    #[test]
    fn new_project_action_creates_a_project_slugified_from_the_member_dir() {
        let _isolated = IsolatedDirs::new("assembly-menu-new-project");
        let ctx = ProjectAssemblyContext {
            member_dir: "/repo/arycast".into(),
            current_project_slug: None,
        };

        let slug = create_project_with_optional_member("Arycast", Some(ctx.member_dir)).unwrap();

        let file = projects::load_projects_file_fresh().unwrap();
        let project = file.projects.get(&slug).expect("project created");
        assert_eq!(slug, "arycast");
        assert_eq!(project.name.as_deref(), Some("Arycast"));
        assert_eq!(project.members[0].dir, "/repo/arycast");
    }

    #[test]
    fn rename_project_writes_the_display_name() {
        let _isolated = IsolatedDirs::new("assembly-menu-rename");
        seed_project("cnb", &["/repo/cnb"]);

        rename_project("cnb", "CNB Team").unwrap();

        let file = projects::load_projects_file_fresh().unwrap();
        assert_eq!(file.projects["cnb"].name.as_deref(), Some("CNB Team"));
    }

    #[test]
    fn unique_slug_falls_back_to_a_dash_suffix() {
        let _isolated = IsolatedDirs::new("assembly-menu-unique-slug");
        seed_project("arycast", &[]);

        let file = projects::load_projects_file_fresh().unwrap();
        assert_eq!(unique_project_slug(&file, "arycast"), "arycast-2");
        assert_eq!(unique_project_slug(&file, "fresh"), "fresh");
    }

    fn menu_fixture(kind: ContextMenuKind, items: &[&str]) -> ContextMenuState {
        ContextMenuState {
            kind,
            x: 0,
            y: 0,
            list: crate::app::state::MenuListState::new(0),
            items: items.iter().map(ToString::to_string).collect(),
            bora_commands: vec![],
            bora_port: None,
        }
    }

    fn cloned_menu(menu: &ContextMenuState) -> ContextMenuState {
        menu_fixture(
            menu.kind.clone(),
            &menu.items.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn project_view_row_menu_splices_assembly_and_drops_visual_group_items() {
        let kind = ContextMenuKind::Workspace {
            ws_idx: 0,
            hidden: false,
        };
        let project_items = crate::app::state::build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Project,
            &["Add to alpha".to_string()],
            &[],
            &Default::default(),
        );
        assert!(project_items.iter().any(|item| item == "Add to alpha"));
        assert!(!project_items.iter().any(|item| item == "New group\u{2026}"));

        let repo_items = crate::app::state::build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Repo,
            &[],
            &[],
            &Default::default(),
        );
        assert!(repo_items.iter().any(|item| item == "New group\u{2026}"));
        assert!(!repo_items.iter().any(|item| item == "Add to alpha"));
    }

    #[test]
    fn add_to_from_a_row_menu_persists_membership_and_the_store_picks_it_up() {
        let _isolated = IsolatedDirs::new("assembly-dispatch-add-to");
        let mut state = super::super::state_with_workspaces(&["b"]);
        let dir = state.workspaces[0].project_member_dir();
        seed_project("alpha", &[]);
        let mut runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        // A store on the REAL path (test_new's `ProjectsStore::empty()` is
        // deliberately inert): this is the store the tick poll would refresh.
        let mut store = projects::ProjectsStore::load();

        apply_context_menu_action(
            &mut state,
            &mut runtimes,
            menu_fixture(
                ContextMenuKind::Workspace {
                    ws_idx: 0,
                    hidden: false,
                },
                &["Add to alpha"],
            ),
            0,
        );

        let file = projects::load_projects_file_fresh().unwrap();
        assert!(file.projects["alpha"].members.iter().any(|m| m.dir == dir));
        // The tick poll is how the live sidebar learns about the write —
        // prove it observes the file change without a restart.
        assert_eq!(store.reload_if_changed(), Ok(true));
    }

    #[test]
    fn new_project_from_a_header_opens_the_prompt_and_confirm_creates() {
        let _isolated = IsolatedDirs::new("assembly-dispatch-new-project");
        let mut state = super::super::state_with_workspaces(&["b"]);
        let mut runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        apply_context_menu_action(
            &mut state,
            &mut runtimes,
            menu_fixture(
                ContextMenuKind::ProjectHeader {
                    slug: None,
                    collapse_key: "proj:__orphans__".into(),
                    hidden: false,
                },
                &["New project\u{2026}"],
            ),
            0,
        );
        assert_eq!(state.mode, Mode::ProjectNameInput);

        state.name_input = "My Group".to_string();
        apply_rename_action(&mut state, ModalAction::Save);

        let file = projects::load_projects_file_fresh().unwrap();
        let project = file.projects.get("my-group").expect("slugified from name");
        assert_eq!(project.name.as_deref(), Some("My Group"));
        assert!(project.members.is_empty());
    }

    #[test]
    fn rename_project_from_a_header_writes_the_display_name() {
        let _isolated = IsolatedDirs::new("assembly-dispatch-rename");
        let mut state = super::super::state_with_workspaces(&["b"]);
        seed_project("alpha", &[]);
        let mut runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        apply_context_menu_action(
            &mut state,
            &mut runtimes,
            menu_fixture(
                ContextMenuKind::ProjectHeader {
                    slug: Some("alpha".into()),
                    collapse_key: "proj:alpha".into(),
                    hidden: false,
                },
                &["Rename project\u{2026}"],
            ),
            0,
        );
        assert_eq!(state.mode, Mode::ProjectNameInput);
        // A project with no `name:` prefills with the slug, so an untouched
        // confirm is an identity rename, never a wipe.
        assert_eq!(state.name_input, "alpha");

        state.name_input = "Alpha Team".to_string();
        apply_rename_action(&mut state, ModalAction::Save);

        let file = projects::load_projects_file_fresh().unwrap();
        assert_eq!(file.projects["alpha"].name.as_deref(), Some("Alpha Team"));
    }

    #[test]
    fn orphan_picker_lists_only_orphans_and_files_the_pick() {
        let _isolated = IsolatedDirs::new("assembly-dispatch-orphan-picker");
        let mut state = super::super::state_with_workspaces(&["a", "b"]);
        // test_new shares one identity_cwd across workspaces; distinct dirs
        // are the whole point of this fixture.
        state.workspaces[0].identity_cwd = "/tmp/assembly-member".into();
        state.workspaces[1].identity_cwd = "/tmp/assembly-orphan".into();
        let member_dir = state.workspaces[0].project_member_dir();
        let orphan_dir = state.workspaces[1].project_member_dir();
        seed_project("alpha", &[&member_dir]);
        let mut runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        apply_context_menu_action(
            &mut state,
            &mut runtimes,
            menu_fixture(
                ContextMenuKind::ProjectHeader {
                    slug: None,
                    collapse_key: "proj:__orphans__".into(),
                    hidden: false,
                },
                &["Add workspaces\u{2026}"],
            ),
            0,
        );
        let picker = state.context_menu.take().expect("orphan picker open");
        let ContextMenuKind::ProjectOrphanPicker {
            slug: None,
            orphans,
        } = &picker.kind
        else {
            panic!("expected the orphan picker, got {:?}", picker.kind);
        };
        assert_eq!(orphans, &vec![orphan_dir.clone()]);

        apply_context_menu_action(&mut state, &mut runtimes, cloned_menu(&picker), 0);
        let targets = state.context_menu.take().expect("targets menu open");
        assert!(
            matches!(&targets.kind, ContextMenuKind::ProjectMemberTargets { member_dir } if *member_dir == orphan_dir)
        );
        assert!(targets.items.iter().any(|item| item == "Add to alpha"));

        let apply = cloned_menu(&targets);
        let idx = apply
            .items
            .iter()
            .position(|item| item == "Add to alpha")
            .unwrap();
        apply_context_menu_action(&mut state, &mut runtimes, apply, idx);
        let file = projects::load_projects_file_fresh().unwrap();
        assert!(file.projects["alpha"]
            .members
            .iter()
            .any(|m| m.dir == orphan_dir));
    }

    #[test]
    fn remove_from_a_member_row_deletes_membership() {
        let _isolated = IsolatedDirs::new("assembly-dispatch-remove");
        let mut state = super::super::state_with_workspaces(&["a"]);
        let dir = state.workspaces[0].project_member_dir();
        seed_project("alpha", &[&dir]);
        let mut runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        apply_context_menu_action(
            &mut state,
            &mut runtimes,
            menu_fixture(
                ContextMenuKind::Workspace {
                    ws_idx: 0,
                    hidden: false,
                },
                &["Remove"],
            ),
            0,
        );

        let file = projects::load_projects_file_fresh().unwrap();
        assert!(file.projects["alpha"].members.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    use super::super::{capture_snapshot, state_with_workspaces};
    use super::*;
    use crate::app::state::build_context_menu_items;
    use crate::workspace::Workspace;

    fn config_env_lock() -> &'static parking_lot::Mutex<()> {
        crate::config::test_config_env_lock()
    }

    fn temp_config_path(name: &str) -> std::path::PathBuf {
        let unique = format!(
            "herdr-modal-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        std::env::temp_dir().join(unique).join("config.toml")
    }

    fn app_with_test_workspaces(names: &[&str]) -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.workspaces = names.iter().map(|name| Workspace::test_new(name)).collect();
        app.state.ensure_test_terminals();
        app.state.active = (!app.state.workspaces.is_empty()).then_some(0);
        app.state.selected = 0;
        app
    }

    #[test]
    fn workspace_create_label_preserves_auto_name_for_suggestion_or_blank() {
        assert_eq!(workspace_create_label("project", "project"), None);
        assert_eq!(workspace_create_label("", "project"), None);
        assert_eq!(workspace_create_label("   ", "project"), None);
        assert_eq!(
            workspace_create_label("  logs  ", "project").as_deref(),
            Some("logs")
        );
    }

    fn mark_worktree_space_member(state: &mut AppState, ws_idx: usize, key: &str) {
        state.workspaces[ws_idx].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: key.into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: format!("/repo/worktree-{ws_idx}").into(),
            is_linked_worktree: ws_idx != 0,
        });
    }

    #[test]
    fn custom_resize_key_exits_resize_mode() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::Resize;
        state.keybinds.resize_mode = crate::config::ActionKeybinds::prefix("g");

        handle_resize_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('g'), KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn direct_resize_key_exits_resize_mode() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::Resize;
        state.keybinds.resize_mode = crate::config::ActionKeybinds::direct("ctrl+alt+r");

        handle_resize_key(
            &mut state,
            TerminalKey::new(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            ),
        );

        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn resize_key_exit_matches_enhanced_shifted_punctuation() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::Resize;
        state.keybinds.resize_mode = crate::config::ActionKeybinds::prefix("?");

        handle_resize_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('/'), KeyModifiers::SHIFT)
                .with_shifted_codepoint('?' as u32),
        );

        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn detach_requests_client_detach_in_persistence_mode() {
        let mut state = state_with_workspaces(&["test"]);
        state.detach_exits = false;

        request_detach(&mut state);

        assert!(state.detach_requested);
        assert!(!state.should_quit);
    }

    #[test]
    fn detach_exits_in_no_session_mode() {
        let mut state = state_with_workspaces(&["test"]);
        state.detach_exits = true;

        request_detach(&mut state);

        assert!(state.should_quit);
        assert!(!state.detach_requested);
    }

    #[test]
    fn global_menu_whats_new_opens_saved_release_notes() {
        let _guard = config_env_lock().lock();
        let path = temp_config_path("whats-new-saved-release-notes");
        std::env::set_var(crate::config::CONFIG_PATH_ENV_VAR, &path);
        crate::release_notes::save_pending(env!("CARGO_PKG_VERSION"), "### Changed\n- Menu")
            .unwrap();

        let mut state = state_with_workspaces(&["test"]);
        state.latest_release_notes_available = true;

        assert!(global_menu_actions(&state).contains(&GlobalMenuAction::WhatsNew));

        apply_global_menu_action(&mut state, GlobalMenuAction::WhatsNew);

        assert_eq!(state.mode, Mode::ReleaseNotes);
        assert_eq!(
            state
                .release_notes
                .as_ref()
                .map(|notes| notes.body.as_str()),
            Some("### Changed\n- Menu")
        );

        std::env::remove_var(crate::config::CONFIG_PATH_ENV_VAR);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn rename_modal_keyboard_and_mouse_share_actions() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::RenameWorkspace;
        state.name_input = "hello".into();

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        );
        assert!(state.name_input.is_empty());

        state.name_input = "renamed".into();
        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );
        assert_eq!(state.mode, Mode::Terminal);
        assert_eq!(state.workspaces[0].display_name(), "renamed");
        let snapshot = capture_snapshot(&state);
        assert_eq!(
            snapshot.workspaces[0].custom_name.as_deref(),
            Some("renamed")
        );

        state.view.sidebar_rect = Rect::new(0, 0, 26, 20);
        state.view.terminal_area = Rect::new(26, 0, 80, 20);
        state.mode = Mode::RenameWorkspace;
        state.name_input = "mouse".into();
        let inner = state.rename_modal_inner().unwrap();
        let (save, _, _) = crate::ui::rename_button_rects(inner);
        let action = modal_action_from_buttons(save.x, save.y, &[(save, ModalAction::Save)]);
        assert_eq!(action, Some(ModalAction::Save));
    }

    #[test]
    fn tab_rename_updates_captured_snapshot() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::RenameTab;
        state.name_input = "logs".into();

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        let snapshot = capture_snapshot(&state);
        assert_eq!(
            snapshot.workspaces[0].tabs[0].custom_name.as_deref(),
            Some("logs")
        );
    }

    #[test]
    fn rename_cancel_returns_to_terminal_when_workspace_is_active() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::RenameTab;
        state.name_input = "test".into();

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Terminal);
        assert!(state.name_input.is_empty());
    }

    #[test]
    fn rename_modal_replaces_prefilled_text_on_first_type() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::RenameTab;
        state.name_input = "2".into();
        state.name_input_replace_on_type = true;

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('n'), KeyModifiers::empty()),
        );
        assert_eq!(state.name_input, "n");
        assert!(!state.name_input_replace_on_type);

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::empty()),
        );
        assert_eq!(state.name_input, "ne");
    }

    #[test]
    fn rename_modal_replaces_prefilled_text_on_paste() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::RenameTab;
        state.name_input = "2".into();
        state.name_input_replace_on_type = true;

        insert_rename_input_text(&mut state, "feature/logs");

        assert_eq!(state.name_input, "feature/logs");
        assert!(!state.name_input_replace_on_type);

        insert_rename_input_text(&mut state, "-copy");

        assert_eq!(state.name_input, "feature/logs-copy");
    }

    #[test]
    fn rename_modal_handles_line_editing_shortcuts() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::RenameWorkspace;
        state.name_input = "website zero".into();

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty()),
        );
        assert_eq!(state.name_input, "website zer");

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::CONTROL),
        );
        assert_eq!(state.name_input, "website ");

        state.name_input = "website-zero".into();
        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::ALT),
        );
        assert_eq!(state.name_input, "website-");

        state.name_input = "website-zero".into();
        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL),
        );
        assert_eq!(state.name_input, "website-");

        state.name_input = "website-zero".into();
        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL),
        );
        assert_eq!(state.name_input, "website-");

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Backspace, KeyModifiers::SUPER),
        );
        assert!(state.name_input.is_empty());

        state.name_input = "website zero".into();
        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert!(state.name_input.is_empty());
    }

    #[test]
    fn rename_modal_does_not_insert_modified_shortcut_chars() {
        let mut state = state_with_workspaces(&["test"]);
        state.mode = Mode::RenameWorkspace;
        state.name_input = "website".into();

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
        );
        assert_eq!(state.name_input, "website");

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::SHIFT),
        );
        assert_eq!(state.name_input, "websiteZ");
    }

    #[test]
    fn keybind_help_slash_focuses_filter_and_preserves_vim_scroll() {
        let mut state = state_with_workspaces(&["test"]);
        state.keybind_help.query = "stale".into();
        state.keybind_help.search_focused = true;
        state.view.terminal_area = Rect::new(0, 0, 100, 30);

        open_keybind_help(&mut state);
        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('j'), KeyModifiers::empty()),
        );
        assert_eq!(state.keybind_help.scroll, 1);
        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('k'), KeyModifiers::empty()),
        );
        assert_eq!(state.keybind_help.scroll, 0);

        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('w'), KeyModifiers::empty()),
        );
        assert!(state.keybind_help.query.is_empty());

        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('/'), KeyModifiers::empty()),
        );
        for character in "work".chars() {
            state.keybind_help.scroll = 2;
            handle_keybind_help_key(
                &mut state,
                TerminalKey::new(KeyCode::Char(character), KeyModifiers::empty()),
            );
        }

        assert!(state.keybind_help.search_focused);
        assert_eq!(state.keybind_help.query, "work");
        assert_eq!(state.keybind_help.scroll, 0);
    }

    #[test]
    fn keybind_help_query_supports_backspace_clear_and_sanitized_paste() {
        let mut state = state_with_workspaces(&["test"]);
        open_keybind_help(&mut state);
        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('/'), KeyModifiers::empty()),
        );

        insert_keybind_help_query_text(&mut state, "work\nspace");
        assert_eq!(state.keybind_help.query, "workspace");

        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Backspace, KeyModifiers::empty()),
        );
        assert_eq!(state.keybind_help.query, "workspac");

        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('u'), KeyModifiers::CONTROL),
        );
        assert!(state.keybind_help.query.is_empty());
    }

    #[test]
    fn keybind_help_escape_leaves_search_before_closing() {
        let mut state = state_with_workspaces(&["test"]);
        open_keybind_help(&mut state);
        state.keybind_help.search_focused = true;
        state.keybind_help.query = "work".into();

        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Esc, KeyModifiers::empty()),
        );
        assert_eq!(state.mode, Mode::KeybindHelp);
        assert!(!state.keybind_help.search_focused);
        assert!(state.keybind_help.query.is_empty());

        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Esc, KeyModifiers::empty()),
        );
        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn enhanced_shifted_slash_focuses_keybind_help_filter() {
        let mut state = state_with_workspaces(&["test"]);
        open_keybind_help(&mut state);

        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('7'), KeyModifiers::SHIFT)
                .with_shifted_codepoint('/' as u32),
        );

        assert!(state.keybind_help.search_focused);
    }

    #[test]
    fn enhanced_shifted_question_mark_closes_keybind_help_when_not_searching() {
        let mut state = state_with_workspaces(&["test"]);
        open_keybind_help(&mut state);

        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('/'), KeyModifiers::SHIFT)
                .with_shifted_codepoint('?' as u32),
        );

        assert_eq!(state.mode, Mode::Terminal);

        open_keybind_help(&mut state);
        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('/'), KeyModifiers::empty()),
        );
        handle_keybind_help_key(
            &mut state,
            TerminalKey::new(KeyCode::Char('/'), KeyModifiers::SHIFT)
                .with_shifted_codepoint('?' as u32),
        );

        assert_eq!(state.keybind_help.query, "?");
    }

    #[test]
    fn navigator_search_accepts_pasted_text_when_focused() {
        let mut state = state_with_workspaces(&["alpha", "beta"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.mode = Mode::Navigator;
        state.navigator.search_focused = true;
        state.navigator.state_filter = Some(NavigatorStateFilter::Working);

        insert_navigator_search_text(&mut state, &terminal_runtimes, "beta");

        assert_eq!(state.navigator.query, "beta");
        assert_eq!(state.navigator.state_filter, None);
    }

    #[test]
    fn navigator_search_ignores_paste_when_search_is_not_focused() {
        let mut state = state_with_workspaces(&["alpha", "beta"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.mode = Mode::Navigator;
        state.navigator.search_focused = false;

        insert_navigator_search_text(&mut state, &terminal_runtimes, "beta");

        assert!(state.navigator.query.is_empty());
    }

    #[test]
    fn navigator_empty_search_escape_returns_to_commands() {
        let mut state = state_with_workspaces(&["alpha", "beta"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.mode = Mode::Navigator;
        state.navigator.search_focused = true;

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Navigator);
        assert!(!state.navigator.search_focused);
        assert!(state.navigator.query.is_empty());

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('w'), KeyModifiers::empty()),
        );

        assert_eq!(
            state.navigator.state_filter,
            Some(NavigatorStateFilter::Working)
        );
        assert!(state.navigator.query.is_empty());

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn navigator_search_escape_blurs_then_next_escape_closes() {
        let mut state = state_with_workspaces(&["alpha", "beta"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.mode = Mode::Navigator;
        state.navigator.search_focused = true;
        state.navigator.query = "a".into();

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Navigator);
        assert!(!state.navigator.search_focused);
        assert_eq!(state.navigator.query, "a");

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty()),
        );

        assert_eq!(state.navigator.selected, 1);
        assert_eq!(state.navigator.query, "a");

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('/'), KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Navigator);
        assert!(state.navigator.search_focused);
        assert_eq!(state.navigator.query, "a");

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::empty()),
        );

        assert_eq!(state.navigator.query, "al");

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Navigator);
        assert!(!state.navigator.search_focused);

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn navigator_ignores_modified_j_and_k() {
        let mut state = state_with_workspaces(&["alpha", "beta"]);
        let terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        state.mode = Mode::Navigator;
        state.navigator.selected = 1;

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        );

        assert_eq!(state.navigator.selected, 1);

        handle_navigator_key(
            &mut state,
            &terminal_runtimes,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        );

        assert_eq!(state.navigator.selected, 1);
    }

    #[test]
    fn open_rename_active_tab_can_prefill_default_new_tab_name() {
        let mut state = state_with_workspaces(&["test"]);
        state.workspaces[0].test_add_tab(None);
        state.workspaces[0].switch_tab(1);

        open_rename_active_tab(&mut state, true);

        assert_eq!(state.mode, Mode::RenameTab);
        assert_eq!(state.name_input, "2");
        assert!(state.name_input_replace_on_type);
    }

    #[test]
    fn cancel_new_tab_dialog_leaves_workspace_unchanged() {
        let mut state = state_with_workspaces(&["test"]);
        open_new_tab_dialog(&mut state);

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Terminal);
        assert!(!state.creating_new_tab);
        assert!(!state.request_new_tab);
        assert!(state.requested_new_tab_name.is_none());
        assert_eq!(state.workspaces[0].tabs.len(), 1);
    }

    #[test]
    fn saving_new_tab_dialog_requests_creation_with_name() {
        let mut state = state_with_workspaces(&["test"]);
        open_new_tab_dialog(&mut state);
        state.name_input = "logs".into();
        state.name_input_replace_on_type = false;

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Terminal);
        assert!(!state.creating_new_tab);
        assert!(state.request_new_tab);
        assert_eq!(state.requested_new_tab_name.as_deref(), Some("logs"));
    }

    #[test]
    fn saving_new_tab_dialog_with_default_name_keeps_tab_auto_named() {
        let mut state = state_with_workspaces(&["test"]);
        open_new_tab_dialog(&mut state);

        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Terminal);
        assert!(!state.creating_new_tab);
        assert!(state.request_new_tab);
        assert!(state.requested_new_tab_name.is_none());
    }

    #[test]
    fn closing_first_auto_tab_compacts_remaining_auto_tab_label_and_next_prompt() {
        let mut state = state_with_workspaces(&["test"]);
        open_new_tab_dialog(&mut state);
        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        state.workspaces[0].test_add_tab(state.requested_new_tab_name.as_deref());
        state.request_new_tab = false;
        state.requested_new_tab_name = None;

        state.workspaces[0].close_tab(0);
        state.workspaces[0].switch_tab(0);

        assert_eq!(
            state.workspaces[0].tab_display_name(0).as_deref(),
            Some("1")
        );
        assert!(state.workspaces[0].tabs[0].custom_name.is_none());

        open_new_tab_dialog(&mut state);
        assert_eq!(state.name_input, "2");
    }

    #[test]
    fn renaming_auto_tab_to_its_default_number_keeps_it_auto_named() {
        let mut state = state_with_workspaces(&["test"]);
        state.workspaces[0].test_add_tab(None);
        state.workspaces[0].switch_tab(1);

        open_rename_active_tab(&mut state, false);
        handle_rename_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(state.mode, Mode::Terminal);
        assert!(state.workspaces[0].tabs[1].custom_name.is_none());
        assert_eq!(
            state.workspaces[0].tab_display_name(1).as_deref(),
            Some("2")
        );
    }

    #[test]
    fn confirm_close_keyboard_actions_are_direct_not_focused() {
        let mut state = state_with_workspaces(&["a", "b"]);
        state.mode = Mode::ConfirmClose;
        state.selected = 1;

        handle_confirm_close_key(
            &mut state,
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
        );
        assert_eq!(state.mode, Mode::Navigate);
        assert_eq!(state.workspaces.len(), 2);

        state.mode = Mode::ConfirmClose;
        handle_confirm_close_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );
        assert_eq!(state.workspaces.len(), 1);
    }

    #[test]
    fn confirm_close_for_linked_worktree_closes_workspace_only() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        state.mode = Mode::ConfirmClose;
        state.selected = 1;
        state.workspaces[1].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr-issue".into(),
            is_linked_worktree: true,
        });

        handle_confirm_close_key(
            &mut state,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
        );

        assert_eq!(state.request_remove_linked_worktree, None);
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].display_name(), "main");
        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn context_menu_close_parent_workspace_confirms_then_closes_only_it() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        state.active = Some(0);
        state.selected = 1;
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
        let kind = ContextMenuKind::GitWorkspace {
            ws_idx: 0,
            is_linked_worktree: false,
            has_worktree_children: true,
            collapsed: false,
            hidden: false,
        };
        let items = build_context_menu_items(
            &kind,
            &[],
            crate::config::ViewMode::Repo,
            &[],
            &[],
            &Default::default(),
        );
        let close_idx = items
            .iter()
            .position(|i| i == "Close workspace")
            .expect("close item");
        let menu = ContextMenuState {
            items,
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, close_idx);

        assert_eq!(state.selected, 0);
        assert_eq!(state.mode, Mode::ConfirmClose);

        confirm_close_accept(&mut state);

        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].display_name(), "issue");
        assert_eq!(state.mode, Mode::Terminal);
    }

    #[test]
    fn context_menu_close_last_pane_of_parent_closes_only_it() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        state.active = Some(0);
        state.selected = 1;
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
        let pane_id = state.workspaces[0].tabs[0].root_pane;
        let kind = ContextMenuKind::Pane {
            ws_idx: 0,
            tab_idx: 0,
            pane_id,
            source_pane_id: None,
            has_manual_label: false,
            right_click_passthrough: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        let idx = menu
            .items()
            .iter()
            .position(|item| item.as_str() == "Close pane")
            .expect("close pane item");
        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, idx);

        assert_eq!(state.selected, 0);
        assert_ne!(state.mode, Mode::ConfirmClose);
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].display_name(), "issue");
    }

    #[test]
    fn context_menu_toggles_pane_right_click_passthrough() {
        let mut app = app_with_test_workspaces(&["main"]);
        app.state.active = Some(0);
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let kind = ContextMenuKind::Pane {
            ws_idx: 0,
            tab_idx: 0,
            pane_id,
            source_pane_id: None,
            has_manual_label: false,
            right_click_passthrough: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        let idx = menu
            .items()
            .iter()
            .position(|item| *item == "Send right-clicks to pane")
            .unwrap();
        app.apply_context_menu_action_via_api(menu, idx);

        assert!(
            app.state.workspaces[0]
                .pane_state(pane_id)
                .unwrap()
                .right_click_passthrough
        );
    }

    #[test]
    fn context_menu_close_pane_last_parent_group_pane_keeps_confirmation_mode() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        state.active = Some(0);
        state.selected = 1;
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
        let pane_id = state.workspaces[0].tabs[0].root_pane;
        let kind = ContextMenuKind::Pane {
            ws_idx: 0,
            tab_idx: 0,
            pane_id,
            source_pane_id: None,
            has_manual_label: false,
            right_click_passthrough: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        let idx = menu
            .items()
            .iter()
            .position(|item| item.as_str() == "Close pane")
            .expect("close pane item");
        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, idx);

        assert_eq!(state.selected, 0);
        assert_ne!(state.mode, Mode::ConfirmClose);
        assert_eq!(state.workspaces.len(), 1);
        assert_eq!(state.workspaces[0].display_name(), "issue");
    }
    #[test]
    fn api_context_menu_close_last_tab_of_parent_closes_only_it() {
        let mut app = app_with_test_workspaces(&["main", "issue"]);
        mark_worktree_space_member(&mut app.state, 0, "repo-key");
        mark_worktree_space_member(&mut app.state, 1, "repo-key");
        app.state.active = Some(0);
        app.state.selected = 1;
        app.state.mode = Mode::ContextMenu;
        let kind = ContextMenuKind::Tab {
            ws_idx: 0,
            tab_idx: 0,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        let idx = menu
            .items()
            .iter()
            .position(|item| *item == "Close")
            .expect("close tab item");

        app.apply_context_menu_action_via_api(menu, idx);
        assert_eq!(app.state.selected, 0);
        assert_ne!(app.state.mode, Mode::ConfirmClose);
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].display_name(), "issue");
    }

    #[test]
    fn api_context_menu_enter_close_last_pane_of_parent_closes_only_it() {
        let mut app = app_with_test_workspaces(&["main", "issue"]);
        mark_worktree_space_member(&mut app.state, 0, "repo-key");
        mark_worktree_space_member(&mut app.state, 1, "repo-key");
        app.state.active = Some(0);
        app.state.selected = 1;
        app.state.mode = Mode::ContextMenu;
        let pane_id = app.state.workspaces[0].tabs[0].root_pane;
        let kind = ContextMenuKind::Pane {
            ws_idx: 0,
            tab_idx: 0,
            pane_id,
            source_pane_id: None,
            has_manual_label: false,
            right_click_passthrough: false,
        };
        let mut menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        let close_idx = menu
            .items()
            .iter()
            .position(|item| *item == "Close pane")
            .expect("close pane item");
        menu.list.highlighted = close_idx;
        app.state.context_menu = Some(menu);

        app.handle_context_menu_key_via_api(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));

        assert_eq!(app.state.selected, 0);
        assert_ne!(app.state.mode, Mode::ConfirmClose);
        assert_eq!(app.state.workspaces.len(), 1);
        assert_eq!(app.state.workspaces[0].display_name(), "issue");
        assert!(app.state.context_menu.is_none());
    }

    #[test]
    fn context_menu_linked_worktree_offers_merge_to_main() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        state.active = Some(0);
        state.selected = 1;
        state.workspaces[1].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr-issue".into(),
            is_linked_worktree: true,
        });
        let kind = ContextMenuKind::GitWorkspace {
            ws_idx: 1,
            is_linked_worktree: true,
            has_worktree_children: false,
            collapsed: false,
            hidden: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        assert!(menu.items().iter().any(|i| i.as_str() == "Merge to main"));
        let merge_idx = menu
            .items()
            .iter()
            .position(|item| item.as_str() == "Merge to main")
            .expect("merge to main item");
        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, merge_idx);
        assert_eq!(state.request_merge_worktree_to_main, Some(1));
        assert_ne!(state.mode, Mode::ContextMenu);
    }

    #[test]
    fn context_menu_linked_worktree_offers_open_pr() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        state.active = Some(0);
        state.selected = 1;
        state.workspaces[1].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr-issue".into(),
            is_linked_worktree: true,
        });
        let kind = ContextMenuKind::GitWorkspace {
            ws_idx: 1,
            is_linked_worktree: true,
            has_worktree_children: false,
            collapsed: false,
            hidden: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        assert!(menu.items().iter().any(|i| i.as_str() == "Open PR"));
        let pr_idx = menu
            .items()
            .iter()
            .position(|item| item.as_str() == "Open PR")
            .expect("open pr item");
        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, pr_idx);
        assert_eq!(state.request_open_worktree_pr, Some(1));
    }

    #[test]
    fn context_menu_linked_worktree_offers_sync() {
        let mut state = state_with_workspaces(&["main", "issue"]);
        state.active = Some(0);
        state.selected = 1;
        state.workspaces[1].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr-issue".into(),
            is_linked_worktree: true,
        });
        let kind = ContextMenuKind::GitWorkspace {
            ws_idx: 1,
            is_linked_worktree: true,
            has_worktree_children: false,
            collapsed: false,
            hidden: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        assert!(menu.items().iter().any(|i| i.as_str() == "Sync"));
        let sync_idx = menu
            .items()
            .iter()
            .position(|item| item.as_str() == "Sync")
            .expect("sync item");
        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, sync_idx);
        assert_eq!(state.request_sync_workspace_git, Some(1));
    }

    #[test]
    fn context_menu_non_worktree_git_workspace_offers_sync() {
        let mut state = state_with_workspaces(&["main"]);
        state.active = Some(0);
        state.selected = 0;
        state.workspaces[0].worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
            key: "repo-key".into(),
            label: "herdr".into(),
            repo_root: "/repo/herdr".into(),
            checkout_path: "/repo/herdr".into(),
            is_linked_worktree: false,
        });
        let kind = ContextMenuKind::GitWorkspace {
            ws_idx: 0,
            is_linked_worktree: false,
            has_worktree_children: false,
            collapsed: false,
            hidden: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        assert!(menu.items().iter().any(|i| i.as_str() == "Sync"));
        let sync_idx = menu
            .items()
            .iter()
            .position(|item| item.as_str() == "Sync")
            .expect("sync item");
        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, sync_idx);
        assert_eq!(state.request_sync_workspace_git, Some(0));
    }

    fn repo_pr_menu() -> ContextMenuState {
        let kind = ContextMenuKind::RepoPr {
            ws_idx: 0,
            number: 42,
            url: "https://github.com/owner/proj/pull/42".into(),
            head_ref: "fix/focus".into(),
        };
        ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        }
    }

    #[test]
    fn repo_pr_context_menu_actions_set_request_fields() {
        let menu = repo_pr_menu();
        assert_eq!(
            menu.items,
            [
                "Open in worktree",
                crate::app::state::CONTEXT_MENU_SEPARATOR,
                "Open in browser",
                "Copy URL",
            ]
        );

        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        let mut state = state_with_workspaces(&["proj"]);
        let idx = |label: &str| {
            repo_pr_menu()
                .items
                .iter()
                .position(|item| item == label)
                .expect("menu item")
        };

        apply_context_menu_action(
            &mut state,
            &mut terminal_runtimes,
            repo_pr_menu(),
            idx("Open in worktree"),
        );
        assert_eq!(state.request_open_pr_worktree, Some((0, 42)));
        assert_ne!(state.mode, Mode::ContextMenu, "menu closed after action");

        let mut state = state_with_workspaces(&["proj"]);
        apply_context_menu_action(
            &mut state,
            &mut terminal_runtimes,
            repo_pr_menu(),
            idx("Open in browser"),
        );
        assert_eq!(
            state.request_open_url.as_deref(),
            Some("https://github.com/owner/proj/pull/42")
        );

        let mut state = state_with_workspaces(&["proj"]);
        apply_context_menu_action(
            &mut state,
            &mut terminal_runtimes,
            repo_pr_menu(),
            idx("Copy URL"),
        );
        assert_eq!(
            state.request_clipboard_write.as_deref(),
            Some("https://github.com/owner/proj/pull/42".as_bytes())
        );
    }

    fn repo_issue_menu_with_flow(flow_available: bool) -> ContextMenuState {
        let kind = ContextMenuKind::RepoIssue {
            number: 12,
            url: "https://github.com/owner/proj/issues/12".into(),
            flow_available,
        };
        ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &Default::default(),
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        }
    }

    fn repo_issue_menu() -> ContextMenuState {
        repo_issue_menu_with_flow(false)
    }

    #[test]
    fn repo_issue_context_menu_actions_set_request_fields() {
        let menu = repo_issue_menu();
        assert_eq!(menu.items, ["Open in browser", "Copy URL"]);

        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let idx = |label: &str| {
            repo_issue_menu()
                .items
                .iter()
                .position(|item| item == label)
                .expect("menu item")
        };

        let mut state = state_with_workspaces(&["proj"]);
        apply_context_menu_action(
            &mut state,
            &mut terminal_runtimes,
            repo_issue_menu(),
            idx("Open in browser"),
        );
        assert_eq!(
            state.request_open_url.as_deref(),
            Some("https://github.com/owner/proj/issues/12")
        );
        assert_ne!(state.mode, Mode::ContextMenu, "menu closed after action");

        let mut state = state_with_workspaces(&["proj"]);
        apply_context_menu_action(
            &mut state,
            &mut terminal_runtimes,
            repo_issue_menu(),
            idx("Copy URL"),
        );
        assert_eq!(
            state.request_clipboard_write.as_deref(),
            Some("https://github.com/owner/proj/issues/12".as_bytes())
        );
        assert_ne!(state.mode, Mode::ContextMenu, "menu closed after action");
    }

    #[test]
    fn repo_issue_context_menu_offers_flow_run_only_when_template_resolves() {
        assert_eq!(
            repo_issue_menu_with_flow(true).items,
            [
                "Run with bora-flow",
                crate::app::state::CONTEXT_MENU_SEPARATOR,
                "Open in browser",
                "Copy URL",
            ]
        );
        assert_eq!(
            repo_issue_menu_with_flow(false).items,
            ["Open in browser", "Copy URL"]
        );

        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        let mut state = state_with_workspaces(&["proj"]);
        let menu = repo_issue_menu_with_flow(true);
        let idx = menu
            .items
            .iter()
            .position(|item| item == "Run with bora-flow")
            .expect("menu item");
        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, idx);
        assert_eq!(
            state.request_flow_run,
            Some(crate::app::state::FlowRunRequest {
                number: 12,
                url: "https://github.com/owner/proj/issues/12".into(),
            })
        );
        assert_ne!(state.mode, Mode::ContextMenu, "menu closed after action");
    }

    #[test]
    fn plugin_action_context_selection_sets_request_plugin_action() {
        // bora-1e9: selecting a plugin-contributed menu item must resolve
        // to the exact qualified id (`plugin_id.action_id`) the menu built
        // it from — the same id `find_plugin_action` consumes at invoke
        // time (mod.rs/headless.rs both hand this straight to
        // `invoke_plugin_action_from_ui`).
        let mut state = state_with_workspaces(&["proj"]);
        let mut plugins = crate::app::state::InstalledPluginRegistry::new();
        plugins.insert(
            "example.tool".to_string(),
            crate::api::schema::InstalledPluginInfo {
                plugin_id: "example.tool".into(),
                name: "Example Tool".into(),
                version: "0.1.0".into(),
                min_herdr_version: String::new(),
                description: None,
                manifest_path: "/nonexistent".into(),
                plugin_root: "/nonexistent".into(),
                enabled: true,
                platforms: None,
                build: vec![],
                startup: vec![],
                actions: vec![crate::api::schema::PluginManifestAction {
                    id: "run".into(),
                    title: "Run tool".into(),
                    description: None,
                    contexts: vec![crate::api::schema::PluginActionContext::Workspace],
                    platforms: None,
                    command: vec!["true".into()],
                }],
                events: vec![],
                panes: vec![],
                link_handlers: vec![],
                source: crate::api::schema::PluginSourceInfo::default(),
                warnings: vec![],
            },
        );
        state.installed_plugins = plugins.clone();

        let kind = ContextMenuKind::Workspace {
            ws_idx: 0,
            hidden: false,
        };
        let menu = ContextMenuState {
            items: build_context_menu_items(
                &kind,
                &[],
                crate::config::ViewMode::Repo,
                &[],
                &[],
                &plugins,
            ),
            kind,
            x: 0,
            y: 0,
            list: MenuListState::new(0),
            bora_commands: vec![],
            bora_port: None,
        };
        let idx = menu
            .items()
            .iter()
            .position(|item| item == "Run tool")
            .expect("plugin action item must be present");
        let mut terminal_runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        apply_context_menu_action(&mut state, &mut terminal_runtimes, menu, idx);

        assert_eq!(
            state.request_plugin_action.as_deref(),
            Some("example.tool.run"),
            "selection must resolve to the qualified id find_plugin_action consumes"
        );
        assert_ne!(state.mode, Mode::ContextMenu, "menu closed after action");
    }
}
