//! Append-only JSONL transcript store for `#`-channel workspaces, plus the
//! explicit-membership roster that lives beside it.
//!
//! Each channel's messages live at `state_dir()/channels/<name>.jsonl` (name
//! without the leading `#`), one JSON object per line. Panes that joined a
//! channel they don't live in are listed at
//! `state_dir()/channels/<name>.members.json` as a JSON array of public pane
//! ids.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::api::schema::ChannelMessage;
use crate::config::state_dir;

/// Strip a leading `#` (and surrounding whitespace) so callers can pass a
/// channel name with or without it.
pub fn normalize_channel_name(name: &str) -> String {
    name.trim().trim_start_matches('#').to_string()
}

fn channels_dir() -> PathBuf {
    state_dir().join("channels")
}

pub fn channel_file_path(name: &str) -> PathBuf {
    channels_dir().join(format!("{}.jsonl", normalize_channel_name(name)))
}

pub fn channel_members_file_path(name: &str) -> PathBuf {
    channels_dir().join(format!("{}.members.json", normalize_channel_name(name)))
}

pub fn channel_protocol_file_path(name: &str) -> PathBuf {
    channels_dir().join(format!("{}.protocol.json", normalize_channel_name(name)))
}

/// Hard cap on JSONL lines kept per channel. Once `append_message` would
/// push a channel's log past this, the file is atomically rewritten to keep
/// only the newest half — a fixed low-water mark avoids rotating on nearly
/// every append while still bounding disk use for long-lived channels.
const MAX_CHANNEL_LOG_LINES: usize = 10_000;

/// Rewrite `path` to keep only its newest `max_lines / 2` lines once it
/// exceeds `max_lines`. No-op below the cap or when the file doesn't exist
/// yet. Writes to a sibling `.tmp` file and renames over the original so
/// concurrent readers never observe a partially-written log.
fn rotate_to_cap(path: &std::path::Path, max_lines: usize) -> io::Result<()> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    let lines: Vec<String> = io::BufReader::new(file)
        .lines()
        .collect::<io::Result<_>>()?;
    if lines.len() <= max_lines {
        return Ok(());
    }
    let keep_from = lines.len() - max_lines / 2;
    let tmp_path = path.with_extension("jsonl.tmp");
    {
        let mut tmp = fs::File::create(&tmp_path)?;
        for line in &lines[keep_from..] {
            writeln!(tmp, "{line}")?;
        }
        tmp.flush()?;
    }
    fs::rename(&tmp_path, path)
}

/// Append one message, creating the `channels/` directory and the file on
/// first use. Flushes so the write is durable before this call returns.
/// Applies the bounded-storage rotation policy (see `MAX_CHANNEL_LOG_LINES`)
/// after the append.
pub fn append_message(name: &str, message: &ChannelMessage) -> io::Result<()> {
    fs::create_dir_all(channels_dir())?;
    let path = channel_file_path(name);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let line = serde_json::to_string(message)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    writeln!(file, "{line}")?;
    file.flush()?;
    drop(file);
    rotate_to_cap(&path, MAX_CHANNEL_LOG_LINES)
}

/// Read the last `limit` messages. A missing file reads as empty history.
/// Malformed lines are skipped rather than failing the whole read.
pub fn read_tail(name: &str, limit: usize) -> io::Result<Vec<ChannelMessage>> {
    let file = match fs::File::open(channel_file_path(name)) {
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
        if let Ok(message) = serde_json::from_str::<ChannelMessage>(&line) {
            all.push(message);
        }
    }
    let start = all.len().saturating_sub(limit);
    Ok(all.split_off(start))
}

/// Next per-channel sequence id: last persisted seq + 1 (1 for a channel
/// with no readable history). Reads the tail rather than counting lines so
/// ids stay monotonic across rotation; pre-seq history lines default to 0,
/// so the first seq'd message following old history is 1.
pub fn next_seq(name: &str) -> u64 {
    read_tail(name, 1)
        .ok()
        .and_then(|mut tail| tail.pop())
        .map_or(1, |last| last.seq + 1)
}

/// Cursor read for `channel.wait`: every retained message with
/// `seq > after_seq` (in append order), plus the oldest retained seq so the
/// caller can detect a rotation gap (`oldest > after_seq + 1` means
/// messages in between were dropped) instead of silently losing them.
/// Pre-seq history lines (seq 0) count as the oldest retained line, so a
/// cursor of 0 never reports a gap against them. A missing file reads as
/// empty history.
pub fn read_since(name: &str, after_seq: u64) -> io::Result<ChannelSince> {
    let file = match fs::File::open(channel_file_path(name)) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok(ChannelSince {
                messages: Vec::new(),
                oldest_seq: None,
            });
        }
        Err(err) => return Err(err),
    };
    let mut since = ChannelSince {
        messages: Vec::new(),
        oldest_seq: None,
    };
    for line in io::BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(message) = serde_json::from_str::<ChannelMessage>(&line) {
            if since.oldest_seq.is_none() {
                since.oldest_seq = Some(message.seq);
            }
            if message.seq > after_seq {
                since.messages.push(message);
            }
        }
    }
    Ok(since)
}

/// Retained-history snapshot returned by [`read_since`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChannelSince {
    /// Retained messages with `seq > after_seq`, in append order.
    pub messages: Vec<ChannelMessage>,
    /// Seq of the oldest retained (parseable) line; `None` when no history
    /// is retained at all.
    pub oldest_seq: Option<u64>,
}

/// Public pane ids that explicitly joined `name`, keeping only those `live`
/// still resolves. Pruning is read-only: a pane id that stops resolving
/// (closed pane, restarted session) simply stops being a member, and the
/// next `write_joined_members` persists the pruned set. A missing or
/// unparseable roster reads as no joined members — a corrupt roster must not
/// take the channel down with it.
pub fn read_joined_members(name: &str, live: impl Fn(&str) -> bool) -> Vec<String> {
    let raw = match fs::read_to_string(channel_members_file_path(name)) {
        Ok(raw) => raw,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!(channel = %name, error = %err, "channel roster unreadable");
            }
            return Vec::new();
        }
    };
    let stored: Vec<String> = match serde_json::from_str(&raw) {
        Ok(stored) => stored,
        Err(err) => {
            tracing::warn!(channel = %name, error = %err, "channel roster malformed");
            return Vec::new();
        }
    };
    stored.into_iter().filter(|pane_id| live(pane_id)).collect()
}

/// Replace `name`'s joined roster. Writes a sibling `.tmp` and renames over
/// the roster so a concurrent reader never observes a half-written file. An
/// empty roster removes the file rather than leaving `[]` behind.
pub fn write_joined_members(name: &str, members: &[String]) -> io::Result<()> {
    let path = channel_members_file_path(name);
    if members.is_empty() {
        return match fs::remove_file(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    }
    fs::create_dir_all(channels_dir())?;
    let body = serde_json::to_string(members)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let tmp_path = path.with_extension("json.tmp");
    {
        let mut tmp = fs::File::create(&tmp_path)?;
        tmp.write_all(body.as_bytes())?;
        tmp.flush()?;
    }
    fs::rename(&tmp_path, &path)
}

/// One pane's recorded channel-protocol delivery: the pane it was sent to
/// and the protocol version it received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolSent {
    pub pane: String,
    pub version: u32,
}

/// Panes that have already received `name`'s channel protocol block, and at
/// which version. A missing or unparseable record reads as empty — a
/// corrupt record must not block delivery, only cause a harmless resend.
pub fn read_protocol_sent(name: &str) -> Vec<ProtocolSent> {
    let raw = match fs::read_to_string(channel_protocol_file_path(name)) {
        Ok(raw) => raw,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!(channel = %name, error = %err, "channel protocol record unreadable");
            }
            return Vec::new();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(channel = %name, error = %err, "channel protocol record malformed");
            Vec::new()
        }
    }
}

/// Record that `pane` received `version` of `name`'s channel protocol,
/// replacing any prior entry for that pane. Writes a sibling `.tmp` and
/// renames over the record so a concurrent reader never observes a
/// half-written file.
pub fn mark_protocol_sent(name: &str, pane: &str, version: u32) -> io::Result<()> {
    let mut entries = read_protocol_sent(name);
    entries.retain(|entry| entry.pane != pane);
    entries.push(ProtocolSent {
        pane: pane.to_string(),
        version,
    });
    fs::create_dir_all(channels_dir())?;
    let path = channel_protocol_file_path(name);
    let body = serde_json::to_string(&entries)
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
    use crate::api::schema::ChannelSenderKind;

    fn with_isolated_state_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let old_state = std::env::var_os("XDG_STATE_HOME");
        let dir =
            std::env::temp_dir().join(format!("bora-channels-test-{name}-{}", std::process::id()));
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

    fn message(text: &str) -> ChannelMessage {
        ChannelMessage {
            ts: "2026-08-15T00:00:00Z".into(),
            seq: 0,
            from_pane: "w1A:p2".into(),
            from_name: "brandos".into(),
            from_kind: ChannelSenderKind::Agent,
            text: text.into(),
            in_reply_to: None,
            to_pane: None,
            to_human: false,
        }
    }

    #[test]
    fn human_fields_roundtrip_and_old_lines_default_to_agent() {
        with_isolated_state_dir("human-roundtrip", || {
            let mut human = message("from the human");
            human.from_pane = String::new();
            human.from_name = "arya".into();
            human.from_kind = ChannelSenderKind::Human;
            human.to_human = true;
            append_message("eng", &human).unwrap();

            // Pre-human-era line: no from_kind / to_human keys at all.
            let path = channel_file_path("eng");
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(
                file,
                "{}",
                serde_json::json!({
                    "ts": "2026-08-15T00:00:01Z",
                    "seq": 1,
                    "from_pane": "w1A:p2",
                    "from_name": "brandos",
                    "text": "old line",
                })
            )
            .unwrap();

            let tail = read_tail("eng", 10).unwrap();
            assert_eq!(tail.len(), 2);
            assert_eq!(tail[0].from_kind, ChannelSenderKind::Human);
            assert!(tail[0].to_human);
            assert_eq!(tail[0].from_pane, "");
            assert_eq!(tail[0].from_name, "arya");
            assert_eq!(tail[1].from_kind, ChannelSenderKind::Agent);
            assert!(!tail[1].to_human);
            assert_eq!(tail[1].from_pane, "w1A:p2");
        });
    }

    #[test]
    fn name_normalization_strips_leading_hash() {
        assert_eq!(normalize_channel_name("#eng"), "eng");
        assert_eq!(normalize_channel_name("eng"), "eng");
        assert_eq!(normalize_channel_name("  #eng  "), "eng");
    }

    #[test]
    fn append_and_read_tail_roundtrip() {
        with_isolated_state_dir("roundtrip", || {
            for i in 0..5 {
                append_message("#eng", &message(&format!("msg{i}"))).unwrap();
            }
            let tail = read_tail("eng", 3).unwrap();
            assert_eq!(
                tail.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
                vec!["msg2", "msg3", "msg4"]
            );
        });
    }

    #[test]
    fn read_tail_on_missing_channel_is_empty() {
        with_isolated_state_dir("missing", || {
            assert!(read_tail("nope", 50).unwrap().is_empty());
        });
    }

    #[test]
    fn read_tail_skips_malformed_lines() {
        with_isolated_state_dir("malformed", || {
            append_message("bad", &message("good-one")).unwrap();
            let path = channel_file_path("bad");
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(file, "not json").unwrap();
            append_message("bad", &message("good-two")).unwrap();
            let tail = read_tail("bad", 50).unwrap();
            assert_eq!(
                tail.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
                vec!["good-one", "good-two"]
            );
        });
    }

    #[test]
    fn rotate_to_cap_is_noop_under_cap() {
        with_isolated_state_dir("rotate-under", || {
            for i in 0..5 {
                append_message("small", &message(&format!("msg{i}"))).unwrap();
            }
            let path = channel_file_path("small");
            rotate_to_cap(&path, 100).unwrap();
            let tail = read_tail("small", 100).unwrap();
            assert_eq!(tail.len(), 5);
        });
    }

    #[test]
    fn rotate_to_cap_keeps_newest_half_over_cap() {
        with_isolated_state_dir("rotate-over", || {
            for i in 0..10 {
                append_message("big", &message(&format!("msg{i}"))).unwrap();
            }
            let path = channel_file_path("big");
            rotate_to_cap(&path, 4).unwrap();
            let tail = read_tail("big", 100).unwrap();
            // max_lines=4 -> keep newest 4/2=2 lines.
            assert_eq!(
                tail.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
                vec!["msg8", "msg9"]
            );
        });
    }

    #[test]
    fn rotate_to_cap_on_missing_file_is_noop() {
        with_isolated_state_dir("rotate-missing", || {
            let path = channel_file_path("nope");
            assert!(rotate_to_cap(&path, 4).is_ok());
            assert!(!path.exists());
        });
    }

    #[test]
    fn append_message_calls_rotation_after_append() {
        with_isolated_state_dir("append-rotates", || {
            // append_message always calls rotate_to_cap with the real
            // MAX_CHANNEL_LOG_LINES cap; a handful of appends stays well
            // under it, so this just confirms the call doesn't corrupt or
            // truncate a log that hasn't hit the cap yet.
            for i in 0..5 {
                append_message("normal", &message(&format!("msg{i}"))).unwrap();
            }
            let tail = read_tail("normal", 100).unwrap();
            assert_eq!(tail.len(), 5);
        });
    }

    fn append_with_seq(name: &str, text: &str) -> u64 {
        let seq = next_seq(name);
        let mut message = message(text);
        message.seq = seq;
        append_message(name, &message).unwrap();
        seq
    }

    #[test]
    fn next_seq_stays_monotonic_across_rotation() {
        with_isolated_state_dir("seq-rotation", || {
            for i in 0..10 {
                append_with_seq("rotate", &format!("msg{i}"));
            }
            let path = channel_file_path("rotate");
            // 10 lines > cap 4 -> keep newest 2 (seq 9, 10).
            rotate_to_cap(&path, 4).unwrap();
            // The next seq must continue from the last persisted seq, never
            // restart from the rotated line count.
            assert_eq!(next_seq("rotate"), 11);
            let since = read_since("rotate", 0).unwrap();
            assert_eq!(
                since.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
                vec![9, 10]
            );
            assert_eq!(since.oldest_seq, Some(9));
        });
    }

    #[test]
    fn next_seq_seeds_from_last_persisted_line_after_restart() {
        with_isolated_state_dir("seq-reseed", || {
            // A log written by a previous process: last line seq 7.
            fs::create_dir_all(channels_dir()).unwrap();
            let path = channel_file_path("reseed");
            let mut file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            for seq in [5u64, 7] {
                let mut message = message(&format!("msg{seq}"));
                message.seq = seq;
                writeln!(file, "{}", serde_json::to_string(&message).unwrap()).unwrap();
            }
            file.flush().unwrap();
            drop(file);
            assert_eq!(next_seq("reseed"), 8);
        });
    }

    #[test]
    fn read_since_slices_by_cursor_and_reports_oldest() {
        with_isolated_state_dir("read-since", || {
            for i in 0..5 {
                append_with_seq("cursor", &format!("msg{i}"));
            }
            let all = read_since("cursor", 0).unwrap();
            assert_eq!(
                all.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
                vec![1, 2, 3, 4, 5]
            );
            assert_eq!(all.oldest_seq, Some(1));

            let tail = read_since("cursor", 3).unwrap();
            assert_eq!(
                tail.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
                vec![4, 5]
            );

            let none = read_since("cursor", 5).unwrap();
            assert!(none.messages.is_empty());
            assert_eq!(none.oldest_seq, Some(1));
        });
    }

    #[test]
    fn old_jsonl_lines_default_to_pre_seq_values() {
        with_isolated_state_dir("pre-seq", || {
            fs::create_dir_all(channels_dir()).unwrap();
            let path = channel_file_path("legacy");
            fs::write(
                &path,
                "{\"ts\":\"2026-08-15T00:00:00Z\",\"from_pane\":\"w1A:p2\",\"from_name\":\"brandos\",\"text\":\"legacy\"}\n",
            )
            .unwrap();
            let tail = read_tail("legacy", 10).unwrap();
            let legacy = tail.first().expect("legacy line parses");
            assert_eq!(legacy.seq, 0);
            assert_eq!(legacy.in_reply_to, None);
            assert_eq!(legacy.to_pane, None);
            // First seq'd message after legacy history is 1, and a 0
            // cursor skips the pre-seq line.
            assert_eq!(next_seq("legacy"), 1);
            let since = read_since("legacy", 0).unwrap();
            assert!(since.messages.is_empty());
            assert_eq!(since.oldest_seq, Some(0));
        });
    }

    #[test]
    fn joined_members_roundtrip_and_prune_dead_panes() {
        with_isolated_state_dir("roster-roundtrip", || {
            write_joined_members("#eng", &["w1A:p2".into(), "w3B:p1".into()]).unwrap();
            assert_eq!(
                read_joined_members("eng", |_| true),
                vec!["w1A:p2".to_string(), "w3B:p1".to_string()]
            );
            // A pane that no longer resolves is simply not a member.
            assert_eq!(
                read_joined_members("eng", |pane| pane == "w1A:p2"),
                vec!["w1A:p2".to_string()]
            );
        });
    }

    #[test]
    fn joined_members_missing_or_malformed_roster_is_empty() {
        with_isolated_state_dir("roster-absent", || {
            assert!(read_joined_members("nope", |_| true).is_empty());
            fs::create_dir_all(channels_dir()).unwrap();
            fs::write(channel_members_file_path("broken"), "{not json").unwrap();
            assert!(read_joined_members("broken", |_| true).is_empty());
        });
    }

    #[test]
    fn writing_roster_is_atomic_and_empty_removes_it() {
        with_isolated_state_dir("roster-atomic", || {
            write_joined_members("eng", &["w1A:p2".into()]).unwrap();
            let path = channel_members_file_path("eng");
            assert!(path.exists());
            assert!(!path.with_extension("json.tmp").exists());

            write_joined_members("eng", &[]).unwrap();
            assert!(!path.exists());
            assert!(read_joined_members("eng", |_| true).is_empty());
            // Removing an already-absent roster is not an error.
            write_joined_members("eng", &[]).unwrap();
        });
    }
}
