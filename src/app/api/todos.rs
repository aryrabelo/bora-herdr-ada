//! Handlers for the `todo.*` socket verbs — `todo.create`, `todo.complete`,
//! `todo.list` — over the `persist::todos` append-only log (bora-s3y.1).
//! Like the `project.*` handlers, every verb answers from (and appends to)
//! the CURRENT on-disk log via the store functions directly, never cached
//! `App` state: a todo written through one agent's socket connection must
//! be visible to another agent's `todo.list` without any in-process
//! sharing — the store is the rendezvous.

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, TodoCompleteParams, TodoCreateParams,
    TodoInfo, TodoListParams,
};
use crate::app::App;
use crate::persist::todos::{self, TodoState};

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_todo_create(&mut self, id: String, params: TodoCreateParams) -> String {
        let title = params.title.trim();
        if title.is_empty() {
            return encode_error(id, "empty_todo_title", "todo title must not be empty");
        }
        let origin = params.origin.trim();
        if origin.is_empty() {
            return encode_error(id, "empty_todo_origin", "todo origin must not be empty");
        }
        let blockers = params.blockers.unwrap_or_default();
        // Blocker ids must name live todos — the store deliberately leaves
        // existence/validity of `blockers` to the verb layer (see
        // `persist::todos::create_todo`).
        match todos::read_todos(&params.project) {
            Ok(todos) => {
                if let Some(missing) = blockers
                    .iter()
                    .find(|blocker| !todos.iter().any(|todo| todo.id == **blocker))
                {
                    return encode_error(
                        id,
                        "todo_blocker_unknown",
                        format!("blocker todo {missing} does not exist"),
                    );
                }
            }
            Err(err) => return encode_error(id, "todo_create_failed", err.to_string()),
        }
        let todo =
            match todos::create_todo(&params.project, title, blockers, params.assignee, origin) {
                Ok(todo) => todo,
                Err(err) => return encode_error(id, "todo_create_failed", err.to_string()),
            };
        let todo = TodoInfo::from(todo);
        self.state.refresh_project_todos_notes(&params.project);
        self.emit_event(EventEnvelope {
            event: EventKind::TodoChanged,
            data: EventData::TodoChanged {
                project: params.project,
                todo: todo.clone(),
            },
        });
        encode_success(id, ResponseResult::TodoCreated { todo })
    }

    pub(super) fn handle_todo_complete(
        &mut self,
        id: String,
        params: TodoCompleteParams,
    ) -> String {
        // Read the current state first so a no-op flip (already done)
        // emits nothing — the store appends nothing for it either, and
        // followers must never see an event for a change that did not
        // happen.
        let was_open = match todos::read_todo(&params.project, params.id) {
            Ok(Some(todo)) => todo.state == TodoState::Open,
            Ok(None) => {
                return encode_error(
                    id,
                    "todo_not_found",
                    format!(
                        "todo {} not found in project {:?}",
                        params.id, params.project
                    ),
                );
            }
            Err(err) => return encode_error(id, "todo_complete_failed", err.to_string()),
        };
        let todo = match todos::set_todo_state(&params.project, params.id, TodoState::Done) {
            Ok(Some(todo)) => todo,
            Ok(None) => {
                return encode_error(
                    id,
                    "todo_not_found",
                    format!(
                        "todo {} not found in project {:?}",
                        params.id, params.project
                    ),
                );
            }
            Err(err) => return encode_error(id, "todo_complete_failed", err.to_string()),
        };
        let todo = TodoInfo::from(todo);
        self.state.refresh_project_todos_notes(&params.project);
        if was_open {
            self.emit_event(EventEnvelope {
                event: EventKind::TodoChanged,
                data: EventData::TodoChanged {
                    project: params.project,
                    todo: todo.clone(),
                },
            });
        }
        encode_success(id, ResponseResult::TodoCompleted { todo })
    }

    pub(super) fn handle_todo_list(&mut self, id: String, params: TodoListParams) -> String {
        let todos = if params.actionable.unwrap_or(false) {
            todos::actionable_todos(&params.project)
        } else {
            todos::read_todos(&params.project)
        };
        match todos {
            Ok(todos) => encode_success(
                id,
                ResponseResult::TodoList {
                    todos: todos.into_iter().map(TodoInfo::from).collect(),
                },
            ),
            Err(err) => encode_error(id, "todo_list_failed", err.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, IsolatedDirs};

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

    fn create(
        app: &mut App,
        project: &str,
        title: &str,
        blockers: Option<Vec<u64>>,
    ) -> serde_json::Value {
        let response = app.handle_todo_create(
            "req".into(),
            TodoCreateParams {
                project: project.into(),
                title: title.into(),
                blockers,
                assignee: None,
                origin: "test".into(),
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    fn list(app: &mut App, project: &str, actionable: Option<bool>) -> serde_json::Value {
        let response = app.handle_todo_list(
            "req".into(),
            TodoListParams {
                project: project.into(),
                actionable,
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn todo_create_refreshes_the_sidebar_todos_snapshot() {
        // Epic bora-s3y acceptance, end to end: a todo created through the
        // verb is visible in the TODOS section's data with the correct n/m
        // without any store read on the render path.
        let _isolated = IsolatedDirs::new("todo-sidebar-refresh");
        let mut app = test_app();
        // Sequential creates in one store allocate ids 1, 2, 3.
        create(&mut app, "cnb", "first", None);
        create(&mut app, "cnb", "second", None);
        create(&mut app, "cnb", "third", Some(vec![1]));

        let summary = app
            .state
            .project_todos
            .get("cnb")
            .expect("todo.create must refresh the sidebar snapshot");
        assert_eq!((summary.done, summary.total), (0, 3));
        assert_eq!(
            summary.actionable,
            vec!["first".to_string(), "second".to_string()],
            "the blocked todo is excluded from the section's actionable rows"
        );
    }

    #[test]
    fn todo_create_then_list_round_trips_across_app_instances() {
        let _isolated = IsolatedDirs::new("todo-create-list");
        // Two App instances sharing one isolated state dir: the create
        // goes through one "agent", the list through another — cross-agent
        // visibility flows through the on-disk log, never in-process
        // state.
        let mut writer = test_app();
        let mut reader = test_app();

        let created = create(&mut writer, "cnb", "land the verbs", None);
        assert_eq!(created["result"]["todo"]["title"], "land the verbs");
        assert_eq!(created["result"]["todo"]["state"], "open");
        assert_eq!(created["result"]["todo"]["origin"], "test");
        let id = created["result"]["todo"]["id"].as_u64().unwrap();
        assert!(id >= 1);

        let listed = list(&mut reader, "cnb", None);
        let todos = listed["result"]["todos"].as_array().unwrap();
        assert_eq!(todos.len(), 1, "reader must see the writer's todo");
        assert_eq!(todos[0]["id"], id);
        assert_eq!(todos[0]["title"], "land the verbs");
    }

    #[test]
    fn todo_complete_flips_state_and_is_a_quiet_no_op_when_already_done() {
        let _isolated = IsolatedDirs::new("todo-complete");
        let mut app = test_app();
        let created = create(&mut app, "cnb", "flip me", None);
        let id = created["result"]["todo"]["id"].as_u64().unwrap();
        let events_before = app.event_hub.current_sequence();

        let completed = app.handle_todo_complete(
            "req".into(),
            TodoCompleteParams {
                project: "cnb".into(),
                id,
            },
        );
        let completed: serde_json::Value = serde_json::from_str(&completed).unwrap();
        assert_eq!(completed["result"]["todo"]["state"], "done");
        assert!(
            completed["result"]["todo"]["seq"].as_u64().unwrap()
                > created["result"]["todo"]["seq"].as_u64().unwrap(),
            "a real flip appends a fresh snapshot"
        );
        let flip_events = app.event_hub.events_after(events_before);
        assert_eq!(
            flip_events.len(),
            1,
            "the flip emits exactly one todo.changed event"
        );

        // Completing again is a clean no-op: same seq, no new event.
        let events_after_flip = app.event_hub.current_sequence();
        let again = app.handle_todo_complete(
            "req".into(),
            TodoCompleteParams {
                project: "cnb".into(),
                id,
            },
        );
        let again: serde_json::Value = serde_json::from_str(&again).unwrap();
        assert_eq!(again["result"]["todo"]["state"], "done");
        assert_eq!(
            again["result"]["todo"]["seq"], completed["result"]["todo"]["seq"],
            "a no-op complete must not append"
        );
        assert!(
            app.event_hub.events_after(events_after_flip).is_empty(),
            "a no-op complete must not emit"
        );

        let listed = list(&mut app, "cnb", None);
        assert_eq!(listed["result"]["todos"][0]["state"], "done");
    }

    #[test]
    fn todo_list_actionable_excludes_blocked_until_blockers_done() {
        let _isolated = IsolatedDirs::new("todo-actionable");
        let mut app = test_app();
        let first = create(&mut app, "cnb", "foundation", None);
        let first_id = first["result"]["todo"]["id"].as_u64().unwrap();
        create(&mut app, "cnb", "roof", Some(vec![first_id]));

        let actionable = list(&mut app, "cnb", Some(true));
        let todos = actionable["result"]["todos"].as_array().unwrap();
        assert_eq!(todos.len(), 1, "the blocked todo stays out");
        assert_eq!(todos[0]["title"], "foundation");

        app.handle_todo_complete(
            "req".into(),
            TodoCompleteParams {
                project: "cnb".into(),
                id: first_id,
            },
        );
        let actionable = list(&mut app, "cnb", Some(true));
        let todos = actionable["result"]["todos"].as_array().unwrap();
        assert_eq!(todos.len(), 1, "only the unblocked todo remains open");
        assert_eq!(todos[0]["title"], "roof");

        // The unfiltered list still reports both.
        let all = list(&mut app, "cnb", None);
        assert_eq!(all["result"]["todos"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn todo_create_rejects_an_unknown_blocker() {
        let _isolated = IsolatedDirs::new("todo-blocker-unknown");
        let mut app = test_app();
        let created = create(&mut app, "cnb", "blocked on a ghost", Some(vec![42]));
        assert_eq!(created["error"]["code"], "todo_blocker_unknown");
        assert!(
            list(&mut app, "cnb", None)["result"]["todos"]
                .as_array()
                .unwrap()
                .is_empty(),
            "a rejected create must not append"
        );
    }

    #[test]
    fn todo_complete_on_an_unknown_id_errors() {
        let _isolated = IsolatedDirs::new("todo-complete-missing");
        let mut app = test_app();
        let response = app.handle_todo_complete(
            "req".into(),
            TodoCompleteParams {
                project: "cnb".into(),
                id: 99,
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], "todo_not_found");
    }

    #[test]
    fn todo_create_rejects_empty_title_and_origin() {
        let _isolated = IsolatedDirs::new("todo-empty-fields");
        let mut app = test_app();
        for (title, origin, code) in [
            ("", "test", "empty_todo_title"),
            ("   ", "test", "empty_todo_title"),
            ("real title", "", "empty_todo_origin"),
        ] {
            let response = app.handle_todo_create(
                "req".into(),
                TodoCreateParams {
                    project: "cnb".into(),
                    title: title.into(),
                    blockers: None,
                    assignee: None,
                    origin: origin.into(),
                },
            );
            let response: serde_json::Value = serde_json::from_str(&response).unwrap();
            assert_eq!(
                response["error"]["code"], code,
                "title={title:?} origin={origin:?}"
            );
        }
    }

    #[test]
    fn todo_create_and_complete_emit_todo_changed_events() {
        let _isolated = IsolatedDirs::new("todo-event-emission");
        let mut app = test_app();
        create(&mut app, "cnb", "eventful", None);
        let events = app.event_hub.events_after(0);
        let created_event = events
            .iter()
            .find(|(_, envelope)| envelope.event == EventKind::TodoChanged)
            .expect("todo.create must emit todo.changed");
        match &created_event.1.data {
            EventData::TodoChanged { project, todo } => {
                assert_eq!(project, "cnb");
                assert_eq!(todo.title, "eventful");
            }
            other => panic!("expected TodoChanged data, got {other:?}"),
        }

        let id = match &created_event.1.data {
            EventData::TodoChanged { todo, .. } => todo.id,
            _ => unreachable!(),
        };
        app.handle_todo_complete(
            "req".into(),
            TodoCompleteParams {
                project: "cnb".into(),
                id,
            },
        );
        let events = app.event_hub.events_after(0);
        assert_eq!(
            events
                .iter()
                .filter(|(_, envelope)| envelope.event == EventKind::TodoChanged)
                .count(),
            2,
            "create and the real flip each emit exactly one event"
        );
    }
}
