//! Durable store for `when_idle` agent prompts deferred while their target
//! was `Working`.
//!
//! The queue lives at `state_dir()/pending-prompts.json` as a flat JSON
//! array in delivery order. `App::pending_agent_prompts` is the live copy;
//! this is its mirror, rewritten whole on every mutation. Before it existed
//! the queue was memory-only, so a server restart silently dropped every
//! message an agent had been told was `deferred` — the receipt promised
//! eventual delivery and nothing kept that promise across a restart.
//!
//! Rewritten whole rather than appended because the queue is bounded
//! (`PENDING_AGENT_PROMPT_CAP` per target, and targets are panes), so the
//! file stays small and a full rewrite avoids needing compaction or tombstones
//! for the far more frequent *removal* (drain, evict, pane close).
//!
//! The live map is keyed by public pane id, which is correct in memory: a
//! pane id is valid for as long as the process that minted it. The *record*
//! carries the target's durable `agent_id` as well, because a pane id is
//! reallocated on restore — an entry keyed only by pane could be delivered
//! to whoever inherited the number, which is worse than dropping it. On
//! load the target is resolved through the identity, and only a record
//! written before this field existed (`agent: None`) still falls back to
//! its stored pane id.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::api::schema::AgentPromptParams;
use crate::config::state_dir;

pub fn pending_prompts_file_path() -> PathBuf {
    state_dir().join("pending-prompts.json")
}

/// One deferred prompt, flattened: the live map is
/// `target -> VecDeque<PendingAgentPrompt>`, and this is one of those entries
/// with its target inlined so the record is a plain ordered array. Order in
/// the file is the delivery order within each target.
///
/// `enqueued_at` is deliberately absent: it is an `Instant`, meaningless
/// across processes. A reloaded entry is stamped with the loading process's
/// own `Instant::now()`, which is the honest reading — the only thing that
/// timestamp gates is the drain settle window, and that window should start
/// when this process began watching the target, not before the restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingPromptRecord {
    /// Durable identity of the delivery target. `None` marks a record
    /// written before the queue carried identities; it is honoured through
    /// `target` alone and never given an invented one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Public pane id the target occupied when the prompt was deferred.
    /// Only an address: resolve `agent` first when it is present.
    pub target: String,
    pub queue_id: u64,
    pub params: AgentPromptParams,
}

/// Every deferred prompt on disk, in order. A missing file reads as empty; an
/// unparseable one reads as empty with a warning, because a corrupt queue must
/// not stop the server from starting — losing deferred prompts is bad, failing
/// to boot is worse.
pub fn read_pending_prompts() -> Vec<PendingPromptRecord> {
    let path = pending_prompts_file_path();
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!(path = %path.display(), error = %err, "pending prompt queue unreadable");
            }
            return Vec::new();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(records) => records,
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "pending prompt queue malformed");
            Vec::new()
        }
    }
}

/// Replace the stored queue with `records`. Writes a sibling `.tmp` and
/// renames over the record, so a reader never observes a half-written file.
/// An empty slice removes the file rather than leaving `[]` behind, so a
/// drained queue leaves no trace to misread later.
pub fn write_pending_prompts(records: &[PendingPromptRecord]) -> io::Result<()> {
    let path = pending_prompts_file_path();
    if records.is_empty() {
        return match fs::remove_file(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string(records)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let tmp_path = path.with_extension("json.tmp");
    {
        let mut tmp = fs::File::create(&tmp_path)?;
        tmp.write_all(body.as_bytes())?;
        tmp.flush()?;
    }
    fs::rename(&tmp_path, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IsolatedDirs;

    fn record(target: &str, queue_id: u64, text: &str) -> PendingPromptRecord {
        PendingPromptRecord {
            agent: None,
            target: target.to_string(),
            queue_id,
            params: AgentPromptParams {
                target: target.to_string(),
                text: text.to_string(),
                wait: None,
                from_pane: None,
                when_idle: Some(true),
                when_idle_timeout_ms: None,
                peer_pid: None,
                origin_channel: None,
            },
        }
    }

    #[test]
    fn round_trips_in_order() {
        let _isolated = IsolatedDirs::new("pending-prompts-roundtrip");
        let records = vec![
            record("w1:p1", 1, "first"),
            record("w1:p1", 2, "second"),
            record("w2:p1", 3, "other target"),
        ];
        write_pending_prompts(&records).unwrap();
        assert_eq!(read_pending_prompts(), records);
    }

    /// A record written before the queue carried identities must keep
    /// working, with `agent` reading as `None` rather than failing the whole
    /// parse — a corrupt-looking queue would drop every deferred prompt.
    #[test]
    fn legacy_record_without_an_agent_still_parses() {
        let _isolated = IsolatedDirs::new("pending-prompts-legacy");
        fs::create_dir_all(state_dir()).unwrap();
        fs::write(
            pending_prompts_file_path(),
            br#"[{"target":"w1:p1","queue_id":4,"params":{"target":"w1:p1","text":"legado"}}]"#,
        )
        .unwrap();

        let records = read_pending_prompts();
        assert_eq!(records.len(), 1, "got: {records:?}");
        assert!(
            records[0].agent.is_none(),
            "a legacy record must not be given an invented identity"
        );
        assert_eq!(records[0].target, "w1:p1");
        assert_eq!(records[0].queue_id, 4);
    }

    /// The identity is what survives a restart, so it has to round-trip
    /// verbatim: this is the field the loader resolves the target through.
    #[test]
    fn agent_identity_round_trips_verbatim() {
        let _isolated = IsolatedDirs::new("pending-prompts-identity");
        let records = vec![PendingPromptRecord {
            agent: Some("agent_a1b2".into()),
            ..record("w1:p1", 1, "first")
        }];
        write_pending_prompts(&records).unwrap();
        assert_eq!(read_pending_prompts(), records);
    }

    #[test]
    fn missing_and_malformed_read_as_empty() {
        let _isolated = IsolatedDirs::new("pending-prompts-absent");
        assert!(read_pending_prompts().is_empty());

        fs::create_dir_all(state_dir()).unwrap();
        fs::write(pending_prompts_file_path(), b"{ not an array").unwrap();
        assert!(
            read_pending_prompts().is_empty(),
            "a corrupt queue must read as empty, never panic the boot path"
        );
    }

    #[test]
    fn empty_write_removes_the_file() {
        let _isolated = IsolatedDirs::new("pending-prompts-empty");
        write_pending_prompts(&[record("w1:p1", 1, "queued")]).unwrap();
        assert!(pending_prompts_file_path().exists());

        write_pending_prompts(&[]).unwrap();
        assert!(
            !pending_prompts_file_path().exists(),
            "a drained queue must leave no file, not an empty array"
        );
        // Removing an already-absent file is not an error.
        write_pending_prompts(&[]).unwrap();
    }
}
