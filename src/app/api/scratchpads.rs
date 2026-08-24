//! Handlers for the `scratchpad.*` socket verbs — `scratchpad.write`,
//! `scratchpad.append_section`, `scratchpad.find` — over the
//! `persist::scratchpads` append-only doc store (bora-s3y.1). Like the
//! `project.*` handlers, every verb answers from (and writes to) the
//! CURRENT on-disk store via the store functions directly, never cached
//! `App` state: a doc written through one agent's socket connection must
//! be visible to another agent's `scratchpad.find` without any in-process
//! sharing — the store is the rendezvous.

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, ScratchpadAppendSectionParams,
    ScratchpadFindParams, ScratchpadHitInfo, ScratchpadSectionInfo, ScratchpadWriteParams,
};
use crate::app::App;
use crate::persist::scratchpads::{self, ScratchpadDraft};

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_scratchpad_write(
        &mut self,
        id: String,
        params: ScratchpadWriteParams,
    ) -> String {
        let sections: Vec<ScratchpadDraft> = params
            .sections
            .into_iter()
            .map(|section| ScratchpadDraft {
                title: section.title,
                body: section.body,
            })
            .collect();
        let tip_seq = match scratchpads::write_doc(&params.project, &params.doc, &sections) {
            Ok(tip_seq) => tip_seq,
            Err(err) => return encode_error(id, "scratchpad_write_failed", err.to_string()),
        };
        self.state.refresh_project_todos_notes(&params.project);
        self.emit_event(EventEnvelope {
            event: EventKind::ScratchpadChanged,
            data: EventData::ScratchpadChanged {
                project: params.project,
                doc: params.doc.clone(),
                seq: tip_seq,
            },
        });
        encode_success(
            id,
            ResponseResult::ScratchpadWritten {
                doc: params.doc,
                tip_seq,
            },
        )
    }

    pub(super) fn handle_scratchpad_append_section(
        &mut self,
        id: String,
        params: ScratchpadAppendSectionParams,
    ) -> String {
        let title = params.title.trim();
        if title.is_empty() {
            return encode_error(
                id,
                "empty_section_title",
                "scratchpad section title must not be empty",
            );
        }
        let section =
            match scratchpads::append_section(&params.project, &params.doc, title, &params.body) {
                Ok(section) => section,
                Err(err) => {
                    return encode_error(id, "scratchpad_append_failed", err.to_string());
                }
            };
        let section = ScratchpadSectionInfo::from(section);
        self.state.refresh_project_todos_notes(&params.project);
        self.emit_event(EventEnvelope {
            event: EventKind::ScratchpadChanged,
            data: EventData::ScratchpadChanged {
                project: params.project,
                doc: params.doc.clone(),
                seq: section.seq,
            },
        });
        encode_success(
            id,
            ResponseResult::ScratchpadSectionAppended {
                doc: params.doc,
                section,
            },
        )
    }

    pub(super) fn handle_scratchpad_find(
        &mut self,
        id: String,
        params: ScratchpadFindParams,
    ) -> String {
        // An empty query matches every section (`""` is a substring of all
        // text) — the store documents that callers wanting to forbid it
        // validate here, at the verb layer.
        if params.query.is_empty() {
            return encode_error(
                id,
                "empty_scratchpad_query",
                "scratchpad find query must not be empty",
            );
        }
        match scratchpads::find(&params.project, &params.query) {
            Ok(hits) => encode_success(
                id,
                ResponseResult::ScratchpadFound {
                    hits: hits.into_iter().map(ScratchpadHitInfo::from).collect(),
                },
            ),
            Err(err) => encode_error(id, "scratchpad_find_failed", err.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::ScratchpadDraftParams;
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

    fn draft(title: &str, body: &str) -> ScratchpadDraftParams {
        ScratchpadDraftParams {
            title: title.into(),
            body: body.into(),
        }
    }

    fn write_doc(
        app: &mut App,
        project: &str,
        doc: &str,
        sections: Vec<ScratchpadDraftParams>,
    ) -> serde_json::Value {
        let response = app.handle_scratchpad_write(
            "req".into(),
            ScratchpadWriteParams {
                project: project.into(),
                doc: doc.into(),
                sections,
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    fn find(app: &mut App, project: &str, query: &str) -> serde_json::Value {
        let response = app.handle_scratchpad_find(
            "req".into(),
            ScratchpadFindParams {
                project: project.into(),
                query: query.into(),
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn scratchpad_write_refreshes_the_sidebar_notes_snapshot() {
        // Epic bora-s3y acceptance: a doc written through the verb appears
        // in the NOTES section's data without a render-path store read.
        let _isolated = IsolatedDirs::new("scratchpad-sidebar-refresh");
        let mut app = test_app();
        write_doc(&mut app, "cnb", "plan", vec![draft("Goals", "g")]);
        write_doc(&mut app, "cnb", "decisions", vec![draft("Store", "JSONL")]);

        let notes = app
            .state
            .project_notes
            .get("cnb")
            .expect("scratchpad.write must refresh the sidebar snapshot");
        assert_eq!(notes, &vec!["decisions".to_string(), "plan".to_string()]);
    }

    #[test]
    fn scratchpad_write_then_find_hits_across_app_instances() {
        let _isolated = IsolatedDirs::new("scratchpad-write-find");
        // Two App instances sharing one isolated state dir: the write goes
        // through one "agent", the find through another — cross-agent
        // visibility flows through the on-disk store.
        let mut writer = test_app();
        let mut reader = test_app();

        let written = write_doc(
            &mut writer,
            "cnb",
            "plan",
            vec![
                draft("Goal", "ship the sidebar"),
                draft("Risks", "layout drift on narrow terminals"),
            ],
        );
        assert_eq!(written["result"]["doc"], "plan");
        assert_eq!(written["result"]["tip_seq"], 2);

        let found = find(&mut reader, "cnb", "sidebar");
        let hits = found["result"]["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["doc"], "plan");
        assert_eq!(hits[0]["section"]["title"], "Goal");

        // Case-insensitive, and a hit in another doc of the same project
        // shows up too.
        write_doc(
            &mut writer,
            "cnb",
            "retro",
            vec![draft("Notes", "SIDEBAR went well")],
        );
        let found = find(&mut reader, "cnb", "sidebar");
        assert_eq!(found["result"]["hits"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn scratchpad_write_replaces_the_doc_and_keeps_seqs_monotonic() {
        let _isolated = IsolatedDirs::new("scratchpad-replace");
        let mut app = test_app();
        write_doc(&mut app, "cnb", "plan", vec![draft("Old", "stale")]);

        let replaced = write_doc(
            &mut app,
            "cnb",
            "plan",
            vec![draft("New", "fresh"), draft("Next", "fresher")],
        );
        assert_eq!(
            replaced["result"]["tip_seq"], 3,
            "replace continues from the old tip instead of restarting at 1"
        );

        let found = find(&mut app, "cnb", "stale");
        assert!(
            found["result"]["hits"].as_array().unwrap().is_empty(),
            "the replaced section is gone"
        );
        let found = find(&mut app, "cnb", "fresh");
        assert_eq!(found["result"]["hits"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn scratchpad_append_section_appends_and_creates_the_doc() {
        let _isolated = IsolatedDirs::new("scratchpad-append");
        let mut app = test_app();

        // Append into a doc that does not exist yet creates it.
        let appended = app.handle_scratchpad_append_section(
            "req".into(),
            ScratchpadAppendSectionParams {
                project: "cnb".into(),
                doc: "log".into(),
                title: "Entry one".into(),
                body: "first".into(),
            },
        );
        let appended: serde_json::Value = serde_json::from_str(&appended).unwrap();
        assert_eq!(appended["result"]["doc"], "log");
        assert_eq!(appended["result"]["section"]["seq"], 1);
        assert_eq!(appended["result"]["section"]["title"], "Entry one");

        let second = app.handle_scratchpad_append_section(
            "req".into(),
            ScratchpadAppendSectionParams {
                project: "cnb".into(),
                doc: "log".into(),
                title: "Entry two".into(),
                body: "second".into(),
            },
        );
        let second: serde_json::Value = serde_json::from_str(&second).unwrap();
        assert_eq!(second["result"]["section"]["seq"], 2);

        let found = find(&mut app, "cnb", "Entry");
        assert_eq!(found["result"]["hits"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn scratchpad_find_rejects_an_empty_query() {
        let _isolated = IsolatedDirs::new("scratchpad-empty-query");
        let mut app = test_app();
        write_doc(&mut app, "cnb", "plan", vec![draft("Goal", "ship")]);
        let found = find(&mut app, "cnb", "");
        assert_eq!(found["error"]["code"], "empty_scratchpad_query");
    }

    #[test]
    fn scratchpad_append_rejects_an_empty_title() {
        let _isolated = IsolatedDirs::new("scratchpad-empty-title");
        let mut app = test_app();
        let response = app.handle_scratchpad_append_section(
            "req".into(),
            ScratchpadAppendSectionParams {
                project: "cnb".into(),
                doc: "log".into(),
                title: "  ".into(),
                body: "body".into(),
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], "empty_section_title");
    }

    #[test]
    fn scratchpad_write_and_append_emit_scratchpad_changed_events() {
        let _isolated = IsolatedDirs::new("scratchpad-event-emission");
        let mut app = test_app();
        write_doc(&mut app, "cnb", "plan", vec![draft("Goal", "ship")]);
        let events = app.event_hub.events_after(0);
        let write_event = events
            .iter()
            .find(|(_, envelope)| envelope.event == EventKind::ScratchpadChanged)
            .expect("scratchpad.write must emit scratchpad.changed");
        match &write_event.1.data {
            EventData::ScratchpadChanged { project, doc, seq } => {
                assert_eq!(project, "cnb");
                assert_eq!(doc, "plan");
                assert_eq!(*seq, 1, "the event carries the doc's new tip");
            }
            other => panic!("expected ScratchpadChanged data, got {other:?}"),
        }

        app.handle_scratchpad_append_section(
            "req".into(),
            ScratchpadAppendSectionParams {
                project: "cnb".into(),
                doc: "plan".into(),
                title: "Next".into(),
                body: "more".into(),
            },
        );
        let events = app.event_hub.events_after(0);
        let changed: Vec<_> = events
            .iter()
            .filter(|(_, envelope)| envelope.event == EventKind::ScratchpadChanged)
            .collect();
        assert_eq!(changed.len(), 2, "write and append each emit one event");
        match &changed[1].1.data {
            EventData::ScratchpadChanged { seq, .. } => {
                assert_eq!(*seq, 2, "the append event carries the section's seq");
            }
            other => panic!("expected ScratchpadChanged data, got {other:?}"),
        }
    }
}
