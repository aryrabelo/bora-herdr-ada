//! Append-only JSONL transcript store for `#`-channel workspaces.
//!
//! Each channel's messages live at `state_dir()/channels/<name>.jsonl` (name
//! without the leading `#`), one JSON object per line.

use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

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
            from_pane: "w1A:p2".into(),
            from_name: "brandos".into(),
            text: text.into(),
        }
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
}
