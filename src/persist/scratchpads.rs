//! Append-only JSONL store for project-scoped scratchpads — named markdown
//! documents, sectioned by headings, from `.local/prd/sidebar-design.md`
//! §Todos and scratchpads. `channel.note` (`api::schema::channels`) is the
//! near-cousin that lacks sectioning and search; this is a new store with
//! the same shape, not an extension of that one.
//!
//! Layout: `state_dir()/scratchpads/<project-slug>/<doc-name>.jsonl`, one
//! [`ScratchpadSection`] per line in document order, mirroring
//! `src/persist/channels.rs`'s append+cursor+event pattern: appends carry a
//! monotonic per-doc `seq` and flush before returning, [`read_since`]
//! replays sections after a cursor for live followers, and every appended
//! line is self-describing so bora-s3y.2 can emit one event per append
//! without re-reading the file. `write_doc` (create/replace) is the
//! wholesale rewrite — the same tmp+rename atomicity idiom as the channel
//! sidecar files — with fresh seqs continuing from the current tip, so
//! cursors stay monotonic across a replace.

// bora-s3y.1 lands the store; the socket verbs / MCP tools that call it
// are bora-s3y.2, so every public item here is temporarily unreferenced.
#![allow(dead_code)]

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::state_dir;

fn scratchpads_dir(project: &str) -> PathBuf {
    state_dir().join("scratchpads").join(project)
}

pub fn scratchpad_file_path(project: &str, name: &str) -> PathBuf {
    scratchpads_dir(project).join(format!("{name}.jsonl"))
}
/// Names of every scratchpad doc in `project`, sorted — the sidebar NOTES
/// section's list (bora-s3y.3). A missing directory reads as no docs.
pub fn list_docs(project: &str) -> io::Result<Vec<String>> {
    validate_key("project", project)?;
    let entries = match fs::read_dir(scratchpads_dir(project)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut names = Vec::new();
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if let Some(doc) = name.strip_suffix(".jsonl") {
            names.push(doc.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Reject scope keys that could escape the store directory: empty, `.` /
/// `..`, or anything containing a path separator or NUL. The socket verbs
/// landing in bora-s3y.2 take these keys from the wire, so the store is the
/// trust boundary.
fn validate_key(kind: &str, key: &str) -> io::Result<()> {
    let ok = !key.is_empty()
        && key != "."
        && key != ".."
        && !key.chars().any(|c| c == '/' || c == '\\' || c == '\0');
    if ok {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("invalid {kind} key: {key:?}"),
        ))
    }
}

/// One section of a scratchpad doc: a markdown heading (`title`) plus its
/// `body`. Sections are the doc — a doc with no sections is empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScratchpadSection {
    /// Append cursor: monotonic per doc, exactly like channel message
    /// seqs. `write_doc` replacements continue from the current tip rather
    /// than restarting at 1, so a follower's cursor never rewinds.
    pub seq: u64,
    pub title: String,
    pub body: String,
}

/// Section content supplied to [`write_doc`] — a [`ScratchpadSection`]
/// without its store-assigned `seq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScratchpadDraft {
    pub title: String,
    pub body: String,
}

/// Retained-history snapshot returned by [`read_since`] — same contract
/// shape as `persist::channels::ChannelSince`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScratchpadSince {
    /// Retained sections with `seq > after_seq`, in document order.
    pub sections: Vec<ScratchpadSection>,
    /// Seq of the oldest retained (parseable) line; `None` when the doc
    /// has no retained history. `oldest > after_seq + 1` signals a gap
    /// (e.g. a `write_doc` replace dropped the lines in between).
    pub oldest_seq: Option<u64>,
}

/// One [`find`] hit: which doc, which section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScratchpadHit {
    pub doc: String,
    pub section: ScratchpadSection,
}

/// Every parseable line of the doc, in order. A missing file reads as an
/// empty doc; malformed lines are skipped rather than failing the whole
/// read — mirrors `persist::channels::read_tail`.
fn read_doc_lines(project: &str, name: &str) -> io::Result<Vec<ScratchpadSection>> {
    let file = match fs::File::open(scratchpad_file_path(project, name)) {
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
        if let Ok(section) = serde_json::from_str::<ScratchpadSection>(&line) {
            all.push(section);
        }
    }
    Ok(all)
}

/// Next per-doc sequence id: last persisted seq + 1 (1 for a doc with no
/// readable history). Same seeding rule as `persist::channels::next_seq`.
pub fn next_seq(project: &str, name: &str) -> u64 {
    read_doc_lines(project, name)
        .ok()
        .and_then(|doc| doc.last().map(|last| last.seq + 1))
        .unwrap_or(1)
}

/// The doc's sections in order. A missing doc reads as empty.
pub fn read_doc(project: &str, name: &str) -> io::Result<Vec<ScratchpadSection>> {
    validate_key("project", project)?;
    validate_key("scratchpad", name)?;
    read_doc_lines(project, name)
}

/// Cursor read, mirroring `persist::channels::read_since`: every retained
/// section with `seq > after_seq` (in document order), plus the oldest
/// retained seq so the caller can detect a replace gap instead of silently
/// losing sections. Replay from cursor 0 yields every retained section in
/// order; a cursor at the tip yields only newer appends.
pub fn read_since(project: &str, name: &str, after_seq: u64) -> io::Result<ScratchpadSince> {
    validate_key("project", project)?;
    validate_key("scratchpad", name)?;
    let mut since = ScratchpadSince::default();
    for section in read_doc_lines(project, name)? {
        if since.oldest_seq.is_none() {
            since.oldest_seq = Some(section.seq);
        }
        if section.seq > after_seq {
            since.sections.push(section);
        }
    }
    Ok(since)
}

/// Append one section to the doc, creating the doc (and the project's
/// scratchpads directory) on first use. Flushes so the write is durable
/// before this call returns. Returns the persisted section — its `seq` is
/// the cursor a follower replays from.
pub fn append_section(
    project: &str,
    name: &str,
    title: &str,
    body: &str,
) -> io::Result<ScratchpadSection> {
    validate_key("project", project)?;
    validate_key("scratchpad", name)?;
    fs::create_dir_all(scratchpads_dir(project))?;
    let section = ScratchpadSection {
        seq: next_seq(project, name),
        title: title.to_string(),
        body: body.to_string(),
    };
    let path = scratchpad_file_path(project, name);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let line = serde_json::to_string(&section)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    writeln!(file, "{line}")?;
    file.flush()?;
    Ok(section)
}

/// Create or replace the doc wholesale: the new sections are the whole
/// doc. Each gets a fresh seq continuing from the current tip, so cursors
/// stay monotonic across the replace. Writes to a sibling `.tmp` and
/// renames over the original — the same idiom as the channel sidecar
/// files — so a concurrent reader never observes a half-written doc.
/// Returns the new tip seq (0 when both doc and `sections` are empty).
pub fn write_doc(project: &str, name: &str, sections: &[ScratchpadDraft]) -> io::Result<u64> {
    validate_key("project", project)?;
    validate_key("scratchpad", name)?;
    fs::create_dir_all(scratchpads_dir(project))?;
    let path = scratchpad_file_path(project, name);
    let mut seq = next_seq(project, name);
    let tmp_path = path.with_extension("jsonl.tmp");
    {
        let mut tmp = fs::File::create(&tmp_path)?;
        for draft in sections {
            let section = ScratchpadSection {
                seq,
                title: draft.title.clone(),
                body: draft.body.clone(),
            };
            let line = serde_json::to_string(&section)
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
            writeln!(tmp, "{line}")?;
            seq += 1;
        }
        tmp.flush()?;
    }
    fs::rename(&tmp_path, &path)?;
    Ok(seq - 1)
}

/// Names of every doc in the project (file stems, sorted). A missing
/// project directory reads as no docs.
fn doc_names(project: &str) -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(scratchpads_dir(project)) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if let Some(name) = file_name.strip_suffix(".jsonl") {
            names.push(name.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Section hits across every doc in the project: case-insensitive
/// substring match on section title or body, ordered by doc name then seq.
/// An empty query matches every section (`""` is a substring of all text);
/// callers that want to forbid that validate at the verb layer.
pub fn find(project: &str, query: &str) -> io::Result<Vec<ScratchpadHit>> {
    validate_key("project", project)?;
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    for doc in doc_names(project)? {
        for section in read_doc_lines(project, &doc)? {
            if section.title.to_lowercase().contains(&needle)
                || section.body.to_lowercase().contains(&needle)
            {
                hits.push(ScratchpadHit {
                    doc: doc.clone(),
                    section,
                });
            }
        }
    }
    Ok(hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_isolated_state_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let _guard = crate::config::test_config_env_lock().lock();
        let old_state = std::env::var_os("XDG_STATE_HOME");
        let dir = std::env::temp_dir().join(format!(
            "bora-scratchpads-test-{name}-{}",
            std::process::id()
        ));
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

    fn draft(title: &str, body: &str) -> ScratchpadDraft {
        ScratchpadDraft {
            title: title.into(),
            body: body.into(),
        }
    }

    #[test]
    fn scratchpad_list_docs_returns_sorted_names_and_empty_for_missing_project() {
        with_isolated_state_dir("list-docs", || {
            write_doc("bora", "plan", &[draft("Goals", "g")]).unwrap();
            write_doc("bora", "decisions", &[draft("Store shape", "JSONL")]).unwrap();
            append_section("bora", "archive", "Old", "o").unwrap();

            assert_eq!(
                list_docs("bora").unwrap(),
                vec![
                    "archive".to_string(),
                    "decisions".to_string(),
                    "plan".to_string()
                ]
            );
            assert!(list_docs("nope").unwrap().is_empty());
        });
    }

    #[test]
    fn scratchpad_roundtrip_persists_and_reloads_named_docs() {
        with_isolated_state_dir("roundtrip", || {
            let tip = write_doc(
                "bora",
                "plan",
                &[
                    draft("Goals", "- land the stores\n- wire the verbs"),
                    draft("Risks", "none so far"),
                ],
            )
            .unwrap();
            assert_eq!(tip, 2);
            write_doc("bora", "decisions", &[draft("Store shape", "JSONL")]).unwrap();

            // Reload from disk: a fresh read with nothing cached in-process.
            let plan = read_doc("bora", "plan").unwrap();
            assert_eq!(
                plan,
                vec![
                    ScratchpadSection {
                        seq: 1,
                        title: "Goals".into(),
                        body: "- land the stores\n- wire the verbs".into(),
                    },
                    ScratchpadSection {
                        seq: 2,
                        title: "Risks".into(),
                        body: "none so far".into(),
                    },
                ]
            );
            // Sibling docs are independent: separate files, separate seqs.
            let decisions = read_doc("bora", "decisions").unwrap();
            assert_eq!(decisions.len(), 1);
            assert_eq!(decisions[0].seq, 1);
        });
    }

    #[test]
    fn scratchpad_append_section_adds_to_doc() {
        with_isolated_state_dir("append-section", || {
            write_doc("p", "notes", &[draft("One", "first")]).unwrap();
            let appended = append_section("p", "notes", "Two", "second").unwrap();
            assert_eq!(appended.seq, 2);

            let doc = read_doc("p", "notes").unwrap();
            assert_eq!(
                doc.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
                vec!["One", "Two"]
            );
            assert_eq!(doc[1].body, "second");

            // Append also creates a doc that doesn't exist yet.
            let first = append_section("p", "fresh", "Intro", "hello").unwrap();
            assert_eq!(first.seq, 1);
            assert_eq!(read_doc("p", "fresh").unwrap().len(), 1);
        });
    }

    #[test]
    fn scratchpad_find_returns_section_hits_on_title_and_body() {
        with_isolated_state_dir("find", || {
            write_doc(
                "p",
                "alpha",
                &[
                    draft("Dispatch plan", "workers fan out"),
                    draft("Glossary", "a bead is a task"),
                ],
            )
            .unwrap();
            write_doc("p", "beta", &[draft("Notes", "the dispatch loop")]).unwrap();

            // Title hit, case-insensitive.
            let title_hits = find("p", "DISPATCH").unwrap();
            assert_eq!(title_hits.len(), 2);
            assert_eq!(title_hits[0].doc, "alpha");
            assert_eq!(title_hits[0].section.title, "Dispatch plan");
            assert_eq!(title_hits[1].doc, "beta");
            assert_eq!(title_hits[1].section.title, "Notes");

            // Body-only hit.
            let body_hits = find("p", "bead").unwrap();
            assert_eq!(body_hits.len(), 1);
            assert_eq!(body_hits[0].section.title, "Glossary");

            // No match.
            assert!(find("p", "zzz-absent").unwrap().is_empty());
        });
    }

    #[test]
    fn scratchpad_cursor_replay_from_zero_sees_every_section_in_order() {
        with_isolated_state_dir("cursor-replay", || {
            write_doc("p", "doc", &[draft("One", "1"), draft("Two", "2")]).unwrap();
            append_section("p", "doc", "Three", "3").unwrap();

            let since = read_since("p", "doc", 0).unwrap();
            assert_eq!(
                since.sections.iter().map(|s| s.seq).collect::<Vec<_>>(),
                vec![1, 2, 3]
            );
            assert_eq!(
                since
                    .sections
                    .iter()
                    .map(|s| s.title.as_str())
                    .collect::<Vec<_>>(),
                vec!["One", "Two", "Three"]
            );
            assert_eq!(since.oldest_seq, Some(1));
        });
    }

    #[test]
    fn scratchpad_cursor_at_tip_sees_only_new_appends() {
        with_isolated_state_dir("cursor-tip", || {
            let tip = write_doc("p", "doc", &[draft("One", "1")]).unwrap();
            let at_tip = read_since("p", "doc", tip).unwrap();
            assert!(at_tip.sections.is_empty());
            assert_eq!(at_tip.oldest_seq, Some(1));

            let appended = append_section("p", "doc", "Two", "2").unwrap();
            let delta = read_since("p", "doc", tip).unwrap();
            assert_eq!(
                delta.sections.iter().map(|s| s.seq).collect::<Vec<_>>(),
                vec![appended.seq]
            );
            assert_eq!(delta.sections[0].title, "Two");
        });
    }

    #[test]
    fn scratchpad_write_replaces_doc_and_cursor_stays_monotonic() {
        with_isolated_state_dir("replace", || {
            write_doc(
                "p",
                "doc",
                &[draft("Old", "stale"), draft("Older", "staler")],
            )
            .unwrap();
            let tip = write_doc("p", "doc", &[draft("New", "fresh")]).unwrap();
            assert_eq!(tip, 3);

            let doc = read_doc("p", "doc").unwrap();
            assert_eq!(
                doc,
                vec![ScratchpadSection {
                    seq: 3,
                    title: "New".into(),
                    body: "fresh".into(),
                }]
            );

            // The replace drops retained lines: a cursor at 0 replays only
            // the replacement, and oldest_seq signals the gap.
            let since = read_since("p", "doc", 0).unwrap();
            assert_eq!(
                since.sections.iter().map(|s| s.seq).collect::<Vec<_>>(),
                vec![3]
            );
            assert_eq!(since.oldest_seq, Some(3));

            // Appends after a replace continue from the new tip.
            let appended = append_section("p", "doc", "Next", "n").unwrap();
            assert_eq!(appended.seq, 4);
        });
    }

    #[test]
    fn scratchpad_missing_doc_reads_empty() {
        with_isolated_state_dir("missing", || {
            assert!(read_doc("nope", "doc").unwrap().is_empty());
            let since = read_since("nope", "doc", 0).unwrap();
            assert!(since.sections.is_empty());
            assert_eq!(since.oldest_seq, None);
            assert_eq!(next_seq("nope", "doc"), 1);
            assert!(find("nope", "anything").unwrap().is_empty());
        });
    }

    #[test]
    fn scratchpad_keys_reject_path_separators() {
        with_isolated_state_dir("key-validation", || {
            for bad in ["", ".", "..", "a/b", "a\\b"] {
                assert!(append_section(bad, "doc", "t", "b").is_err());
                assert!(append_section("p", bad, "t", "b").is_err());
                assert!(write_doc("p", bad, &[]).is_err());
                assert!(read_doc("p", bad).is_err());
                assert!(read_since("p", bad, 0).is_err());
            }
            assert!(find("", "q").is_err());
        });
    }
}
