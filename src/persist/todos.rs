//! Append-only JSONL store for project-scoped todos — the swarm's shared
//! plan state from `.local/prd/sidebar-design.md` §Todos and scratchpads
//! ("todos/scratchpads are the swarm's shared memory, which is
//! project-scoped by nature").
//!
//! Layout: `state_dir()/todos/<project-slug>.jsonl`, one [`Todo`] snapshot
//! per line, mirroring `src/persist/channels.rs`'s append+cursor+event
//! pattern: every mutation appends a full snapshot of the todo with a fresh
//! monotonic `seq` and flushes before returning, readers fold by `id`
//! keeping the newest snapshot ([`read_todos`]), and [`read_since`] replays
//! raw appends after a cursor for live followers. Each appended line is
//! self-describing — id, seq, and all five todo fields — so bora-s3y.2 can
//! emit one event per append without re-reading the file.
//!
//! ponytail: no rotation/compaction — a project's todo log is tiny next to
//! a channel transcript. If volume ever matters, compact by folding and
//! rewriting via the tmp+rename idiom (`persist::scratchpads::write_doc`
//! already rewrites wholesale); seqs stay monotonic across such a rewrite,
//! so [`TodosSince::oldest_seq`] already gives followers gap detection.

// bora-s3y.1 lands the store; the socket verbs / MCP tools that call it
// are bora-s3y.2, so every public item here is temporarily unreferenced.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::state_dir;

fn todos_dir() -> PathBuf {
    state_dir().join("todos")
}

pub fn todo_file_path(project: &str) -> PathBuf {
    todos_dir().join(format!("{project}.jsonl"))
}

/// Reject scope keys that could escape the store directory: empty, `.` /
/// `..`, or anything containing a path separator or NUL. The socket verbs
/// landing in bora-s3y.2 take this key from the wire, so the store is the
/// trust boundary.
fn validate_project_key(project: &str) -> io::Result<()> {
    let ok = !project.is_empty()
        && project != "."
        && project != ".."
        && !project.chars().any(|c| c == '/' || c == '\\' || c == '\0');
    if ok {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid project key: {project:?}"),
        ))
    }
}

/// Lifecycle of a todo. Only `open`/`done` — the design's sidebar section
/// needs `n/m = done/total` and the actionable filter needs "blockers all
/// done"; anything richer belongs to the verbs bead, not the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TodoState {
    Open,
    Done,
}

/// One appended todo snapshot. Carries all five contract fields (title,
/// state, blockers, assignee, origin) plus the store's `id`/`seq`, so a
/// single line is everything a later event emitter needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    /// Stable identity across state flips; allocated as max existing id + 1.
    pub id: u64,
    /// Append cursor: monotonic per project log, exactly like channel
    /// message seqs. A state flip re-appends the whole record with a new
    /// seq; the newest seq per id wins on read.
    pub seq: u64,
    pub title: String,
    pub state: TodoState,
    /// Ids of other todos that must reach `done` before this one is
    /// actionable. A blocker id that no live todo has counts as not done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blockers: Vec<u64>,
    /// Agent id this todo is assigned to; `None` = unassigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// Where the todo came from (e.g. `beads:bora-s3y`, `sidebar`,
    /// `channel:#eng`) — free text, interpreted by writers, never the store.
    pub origin: String,
}

/// Retained-history snapshot returned by [`read_since`] — same contract
/// shape as `persist::channels::ChannelSince`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodosSince {
    /// Raw appended snapshots with `seq > after_seq`, in append order.
    pub records: Vec<Todo>,
    /// Seq of the oldest retained (parseable) line; `None` when no history
    /// is retained at all. `oldest > after_seq + 1` signals a gap.
    pub oldest_seq: Option<u64>,
}

/// Every parseable line of `project`'s log, in append order. A missing
/// file reads as empty history; malformed lines are skipped rather than
/// failing the whole read — mirrors `persist::channels::read_tail`.
fn read_log(project: &str) -> io::Result<Vec<Todo>> {
    let file = match fs::File::open(todo_file_path(project)) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut all = Vec::new();
    for line in io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(todo) = serde_json::from_str::<Todo>(&line) {
            all.push(todo);
        }
    }
    Ok(all)
}

/// Append one snapshot, creating the `todos/` directory and the file on
/// first use. Flushes so the write is durable before this call returns —
/// same guarantee as `persist::channels::append_message`.
fn append_snapshot(project: &str, todo: &Todo) -> io::Result<()> {
    fs::create_dir_all(todos_dir())?;
    let path = todo_file_path(project);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let line = serde_json::to_string(todo)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    writeln!(file, "{line}")?;
    file.flush()
}

/// Next per-project sequence id: last persisted seq + 1 (1 for a project
/// with no readable history). Same seeding rule as
/// `persist::channels::next_seq`.
pub fn next_seq(project: &str) -> u64 {
    read_log(project)
        .ok()
        .and_then(|log| log.last().map(|last| last.seq + 1))
        .unwrap_or(1)
}

/// Current state of every live todo in `project`, folded from the log by
/// id (newest seq wins) and returned in id order. A missing file reads as
/// no todos.
pub fn read_todos(project: &str) -> io::Result<Vec<Todo>> {
    validate_project_key(project)?;
    let mut by_id: BTreeMap<u64, Todo> = BTreeMap::new();
    for todo in read_log(project)? {
        match by_id.get(&todo.id) {
            Some(current) if current.seq >= todo.seq => {}
            _ => {
                by_id.insert(todo.id, todo);
            }
        }
    }
    Ok(by_id.into_values().collect())
}

/// One todo's current snapshot, or `None` if `id` was never created.
pub fn read_todo(project: &str, id: u64) -> io::Result<Option<Todo>> {
    Ok(read_todos(project)?.into_iter().find(|todo| todo.id == id))
}

/// Cursor read, mirroring `persist::channels::read_since`: every retained
/// appended snapshot with `seq > after_seq` (in append order), plus the
/// oldest retained seq so the caller can detect a history gap instead of
/// silently losing records. Replay from cursor 0 yields every record in
/// order; a cursor at the tip yields only newer appends.
pub fn read_since(project: &str, after_seq: u64) -> io::Result<TodosSince> {
    validate_project_key(project)?;
    let mut since = TodosSince::default();
    for todo in read_log(project)? {
        if since.oldest_seq.is_none() {
            since.oldest_seq = Some(todo.seq);
        }
        if todo.seq > after_seq {
            since.records.push(todo);
        }
    }
    Ok(since)
}

/// Create a todo, allocating its id (max existing id + 1) and seq, and
/// append its first snapshot. Returns the persisted record — its `seq` is
/// the cursor a follower replays from. Existence/validity of `blockers`
/// ids is the verb layer's concern (bora-s3y.2), not the store's.
pub fn create_todo(
    project: &str,
    title: &str,
    blockers: Vec<u64>,
    assignee: Option<String>,
    origin: &str,
) -> io::Result<Todo> {
    validate_project_key(project)?;
    let log = read_log(project)?;
    let id = log
        .iter()
        .map(|todo| todo.id)
        .max()
        .map_or(1, |max| max + 1);
    let seq = log.last().map_or(1, |last| last.seq + 1);
    let todo = Todo {
        id,
        seq,
        title: title.to_string(),
        state: TodoState::Open,
        blockers,
        assignee,
        origin: origin.to_string(),
    };
    append_snapshot(project, &todo)?;
    Ok(todo)
}

/// Flip `id`'s state by appending a fresh snapshot (new seq, all other
/// fields carried over). Returns the new snapshot, or `None` when no such
/// todo exists. A flip to the state the todo is already in is a no-op —
/// no spurious append, so followers never see a record that changed
/// nothing.
pub fn set_todo_state(project: &str, id: u64, state: TodoState) -> io::Result<Option<Todo>> {
    validate_project_key(project)?;
    let Some(mut todo) = read_todo(project, id)? else {
        return Ok(None);
    };
    if todo.state == state {
        return Ok(Some(todo));
    }
    todo.state = state;
    todo.seq = next_seq(project);
    append_snapshot(project, &todo)?;
    Ok(Some(todo))
}

/// Open todos whose blockers are all done — the store-level primitive the
/// `todo.list --actionable` verb (bora-s3y.2) and the sidebar's TODOS
/// section will expose. A blocker id with no live todo keeps the todo
/// blocked (never silently actionable).
pub fn actionable_todos(project: &str) -> io::Result<Vec<Todo>> {
    let todos = read_todos(project)?;
    let done: std::collections::BTreeSet<u64> = todos
        .iter()
        .filter(|todo| todo.state == TodoState::Done)
        .map(|todo| todo.id)
        .collect();
    Ok(todos
        .into_iter()
        .filter(|todo| {
            todo.state == TodoState::Open && todo.blockers.iter().all(|id| done.contains(id))
        })
        .collect())
}
/// Display-ready summary of a project's todos for the sidebar TODOS
/// section (bora-s3y.3): the header's `done/total` plus the titles of the
/// open todos whose blockers are all done — the same actionable rule as
/// `actionable_todos`, derived from one already-loaded snapshot so the
/// refresh path reads the log once. Blocked todos get no row.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TodosSummary {
    pub done: usize,
    pub total: usize,
    pub actionable: Vec<String>,
}

impl TodosSummary {
    pub fn from_todos(todos: &[Todo]) -> Self {
        let done_ids: std::collections::BTreeSet<u64> = todos
            .iter()
            .filter(|todo| todo.state == TodoState::Done)
            .map(|todo| todo.id)
            .collect();
        Self {
            done: done_ids.len(),
            total: todos.len(),
            actionable: todos
                .iter()
                .filter(|todo| {
                    todo.state == TodoState::Open
                        && todo.blockers.iter().all(|id| done_ids.contains(id))
                })
                .map(|todo| todo.title.clone())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_isolated_state_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let _guard = crate::config::test_config_env_lock().lock();
        let old_state = std::env::var_os("XDG_STATE_HOME");
        let dir =
            std::env::temp_dir().join(format!("bora-todos-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        std::env::set_var("XDG_STATE_HOME", &dir);
        let result = f();
        match old_state {
            Some(value) => std::env::set_var("XDG_STATE_HOME", value),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        let _ = fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn todo_roundtrip_persists_and_reloads_all_fields() {
        with_isolated_state_dir("roundtrip", || {
            let first = create_todo(
                "bora",
                "land the store",
                Vec::new(),
                Some("TodoStores2".into()),
                "beads:bora-s3y.1",
            )
            .unwrap();
            let second = create_todo(
                "bora",
                "wire the verbs",
                vec![first.id],
                None,
                "sidebar-design:todos",
            )
            .unwrap();
            set_todo_state("bora", first.id, TodoState::Done).unwrap();

            // Reload from disk: a fresh read with nothing cached in-process.
            let reloaded = read_todos("bora").unwrap();
            assert_eq!(reloaded.len(), 2);
            assert_eq!(
                reloaded[0],
                Todo {
                    id: first.id,
                    seq: 3,
                    title: "land the store".into(),
                    state: TodoState::Done,
                    blockers: Vec::new(),
                    assignee: Some("TodoStores2".into()),
                    origin: "beads:bora-s3y.1".into(),
                }
            );
            assert_eq!(
                reloaded[1],
                Todo {
                    id: second.id,
                    seq: 2,
                    title: "wire the verbs".into(),
                    state: TodoState::Open,
                    blockers: vec![first.id],
                    assignee: None,
                    origin: "sidebar-design:todos".into(),
                }
            );
        });
    }

    #[test]
    fn todo_cursor_replay_from_zero_sees_every_record_in_order() {
        with_isolated_state_dir("cursor-replay", || {
            let first = create_todo("p", "one", Vec::new(), None, "test").unwrap();
            let second = create_todo("p", "two", vec![first.id], None, "test").unwrap();
            set_todo_state("p", first.id, TodoState::Done).unwrap();

            let since = read_since("p", 0).unwrap();
            assert_eq!(
                since.records.iter().map(|t| t.seq).collect::<Vec<_>>(),
                vec![1, 2, 3]
            );
            assert_eq!(
                since
                    .records
                    .iter()
                    .map(|t| (t.id, t.state))
                    .collect::<Vec<_>>(),
                vec![
                    (first.id, TodoState::Open),
                    (second.id, TodoState::Open),
                    (first.id, TodoState::Done),
                ]
            );
            assert_eq!(since.oldest_seq, Some(1));
        });
    }

    #[test]
    fn todo_cursor_at_tip_sees_only_new_appends() {
        with_isolated_state_dir("cursor-tip", || {
            let first = create_todo("p", "one", Vec::new(), None, "test").unwrap();
            let tip = read_since("p", first.seq).unwrap();
            assert!(tip.records.is_empty());
            assert_eq!(tip.oldest_seq, Some(1));

            let second = create_todo("p", "two", Vec::new(), None, "test").unwrap();
            let delta = read_since("p", first.seq).unwrap();
            assert_eq!(
                delta.records.iter().map(|t| t.seq).collect::<Vec<_>>(),
                vec![second.seq]
            );
            assert_eq!(delta.records[0].title, "two");
        });
    }

    #[test]
    fn todo_append_advances_cursor_monotonically() {
        with_isolated_state_dir("append-cursor", || {
            assert_eq!(next_seq("p"), 1);
            let first = create_todo("p", "one", Vec::new(), None, "test").unwrap();
            assert_eq!(first.seq, 1);
            let second = create_todo("p", "two", Vec::new(), None, "test").unwrap();
            assert_eq!(second.seq, 2);
            let flipped = set_todo_state("p", first.id, TodoState::Done)
                .unwrap()
                .unwrap();
            assert_eq!(flipped.seq, 3);
            assert_eq!(next_seq("p"), 4);
        });
    }

    #[test]
    fn todo_actionable_listing_respects_blocker_flipping_done() {
        with_isolated_state_dir("blocker-actionable", || {
            let blocker = create_todo("p", "blocker", Vec::new(), None, "test").unwrap();
            let blocked = create_todo("p", "blocked", vec![blocker.id], None, "test").unwrap();
            let free = create_todo("p", "free", Vec::new(), None, "test").unwrap();

            let actionable = actionable_todos("p").unwrap();
            assert_eq!(
                actionable.iter().map(|t| t.id).collect::<Vec<_>>(),
                vec![blocker.id, free.id]
            );
            assert!(!actionable.iter().any(|t| t.id == blocked.id));

            set_todo_state("p", blocker.id, TodoState::Done).unwrap();
            let actionable = actionable_todos("p").unwrap();
            assert_eq!(
                actionable.iter().map(|t| t.id).collect::<Vec<_>>(),
                vec![blocked.id, free.id]
            );
        });
    }

    #[test]
    fn todo_state_fold_keeps_newest_snapshot_per_id() {
        with_isolated_state_dir("state-fold", || {
            let todo = create_todo("p", "one", Vec::new(), None, "test").unwrap();
            set_todo_state("p", todo.id, TodoState::Done).unwrap();
            set_todo_state("p", todo.id, TodoState::Open).unwrap();

            let todos = read_todos("p").unwrap();
            assert_eq!(todos.len(), 1);
            assert_eq!(todos[0].state, TodoState::Open);
            assert_eq!(todos[0].seq, 3);

            // Flipping to the current state is a no-op: no spurious append.
            let unchanged = set_todo_state("p", todo.id, TodoState::Open)
                .unwrap()
                .unwrap();
            assert_eq!(unchanged.seq, 3);
            assert_eq!(read_since("p", 0).unwrap().records.len(), 3);
        });
    }

    #[test]
    fn todos_summary_counts_done_and_lists_only_actionable_titles() {
        with_isolated_state_dir("summary", || {
            let done = create_todo("p", "done task", Vec::new(), None, "test").unwrap();
            let blocker = create_todo("p", "blocker", Vec::new(), None, "test").unwrap();
            let blocked = create_todo("p", "blocked", vec![blocker.id], None, "test").unwrap();
            let unknown_blocked =
                create_todo("p", "unknown-blocked", vec![999], None, "test").unwrap();
            let free = create_todo("p", "free", Vec::new(), None, "test").unwrap();
            set_todo_state("p", done.id, TodoState::Done).unwrap();

            let summary = TodosSummary::from_todos(&read_todos("p").unwrap());
            assert_eq!((summary.done, summary.total), (1, 5));
            assert_eq!(
                summary.actionable,
                vec![blocker.title, free.title],
                "blocked todos (known or unknown blocker) get no actionable row"
            );

            // Flipping the blocker done unblocks the dependent title.
            set_todo_state("p", blocker.id, TodoState::Done).unwrap();
            let summary = TodosSummary::from_todos(&read_todos("p").unwrap());
            assert_eq!((summary.done, summary.total), (2, 5));
            assert!(summary.actionable.contains(&blocked.title));
            assert!(!summary.actionable.contains(&unknown_blocked.title));
        });
    }

    #[test]
    fn todo_reads_on_missing_project_are_empty() {
        with_isolated_state_dir("missing", || {
            assert!(read_todos("nope").unwrap().is_empty());
            assert!(read_todo("nope", 1).unwrap().is_none());
            let since = read_since("nope", 0).unwrap();
            assert!(since.records.is_empty());
            assert_eq!(since.oldest_seq, None);
            assert_eq!(next_seq("nope"), 1);
            assert!(set_todo_state("nope", 42, TodoState::Done)
                .unwrap()
                .is_none());
        });
    }

    #[test]
    fn todo_log_skips_malformed_lines() {
        with_isolated_state_dir("malformed", || {
            create_todo("bad", "good", Vec::new(), None, "test").unwrap();
            let path = todo_file_path("bad");
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(file, "not json").unwrap();
            file.flush().unwrap();
            drop(file);

            let todos = read_todos("bad").unwrap();
            assert_eq!(todos.len(), 1);
            assert_eq!(todos[0].title, "good");
        });
    }

    #[test]
    fn todo_project_key_rejects_path_separators() {
        with_isolated_state_dir("key-validation", || {
            for bad in ["", ".", "..", "a/b", "a\\b"] {
                assert!(create_todo(bad, "x", Vec::new(), None, "test").is_err());
                assert!(read_todos(bad).is_err());
                assert!(read_since(bad, 0).is_err());
            }
        });
    }
}
