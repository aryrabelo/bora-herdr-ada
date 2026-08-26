//! Append-only JSONL transcript store for `#`-channel workspaces, plus the
//! explicit-membership roster, per-pane scope registry, and per-member
//! read-cursor registry that live beside it.
//!
//! Each channel's messages live at `state_dir()/channels/<name>.jsonl` (name
//! without the leading `#`), one JSON object per line. Panes that joined a
//! channel they don't live in are listed at
//! `state_dir()/channels/<name>.members.json` as a JSON array of public pane
//! ids. Declared write/read directories per pane live at
//! `state_dir()/channels/<name>.scope.json` (CANAL-ESCOPO.md Shape 2) — the
//! registry the harness scope gate consults at runtime. Each member's
//! high-water read cursor lives at `state_dir()/channels/<name>.cursors.json`
//! (CANAL-NAO-LIDO's unread primitive) — the seq a `channel tail` /
//! `channel history` read has advanced that member through.

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

pub fn channel_scope_file_path(name: &str) -> PathBuf {
    channels_dir().join(format!("{}.scope.json", normalize_channel_name(name)))
}

pub fn channel_cursors_file_path(name: &str) -> PathBuf {
    channels_dir().join(format!("{}.cursors.json", normalize_channel_name(name)))
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

/// One channel member: the agent that joined, plus the pane it was last
/// seen in. `agent` is the identity — an `AgentId`, minted once and
/// restored verbatim; `pane` is only its *current address*, a pointer
/// refreshed on every write. A public pane id is reallocated on cold
/// restore, so keying membership by it hands the seat to whoever inherits
/// the number.
///
/// `agent: None` marks a legacy entry, written before the roster was
/// rekeyed. Those are still honoured by `pane` and are never silently
/// dropped; they simply cannot survive a pane reallocation, which is the
/// bug they predate. There is no backfill: nothing on disk records which
/// agent owned a bare pane id, and inventing one would be worse than
/// admitting the gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelMember {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub pane: String,
}

impl ChannelMember {
    /// A legacy roster entry: address only, no identity.
    pub fn legacy(pane: String) -> Self {
        Self { agent: None, pane }
    }

    /// Whether this entry is the same member as `other` — by identity when
    /// both carry one, else by address. Two legacy entries can only be
    /// compared by pane; an identified entry never matches a legacy one on
    /// pane alone, or a reallocated pane id would merge two agents.
    pub fn is_same_member(&self, other: &Self) -> bool {
        match (&self.agent, &other.agent) {
            (Some(mine), Some(theirs)) => mine == theirs,
            (None, None) => self.pane == other.pane,
            _ => false,
        }
    }

    /// Whether this stored entry is `other`, *or* a legacy entry sitting at
    /// `other`'s pane.
    ///
    /// The asymmetry is deliberate: only a *stored* entry without an
    /// identity may be matched by pane alone. That is how a pre-rekey entry
    /// gets retired — an agent joining or leaving absorbs the legacy row at
    /// its own seat instead of leaving it behind forever.
    ///
    /// Use this only where absorbing a legacy row is the point (join
    /// dedupe, leave, scope removal). NEVER use it to *grant* anything: a
    /// reallocated pane id must never inherit a stranger's scope or read
    /// cursor, which is what [`Self::is_same_member`] refuses to allow.
    pub fn is_same_member_or_legacy_seat(&self, other: &Self) -> bool {
        self.is_same_member(other) || (self.agent.is_none() && self.pane == other.pane)
    }
}

/// Members that explicitly joined `name`, keeping only those `resolve`
/// still places somewhere. `resolve` returns the member's *current* public
/// pane id — `None` prunes it (closed pane, restarted session), `Some`
/// refreshes the stored pointer, so the next `write_joined_members`
/// persists both the pruned set and the new addresses.
///
/// A missing or unparseable roster reads as no joined members — a corrupt
/// roster must not take the channel down with it. Entries that are bare
/// JSON strings are legacy pane ids and parse as [`ChannelMember::legacy`].
pub fn read_joined_members(
    name: &str,
    resolve: impl Fn(&ChannelMember) -> Option<String>,
) -> Vec<ChannelMember> {
    let raw = match fs::read_to_string(channel_members_file_path(name)) {
        Ok(raw) => raw,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!(channel = %name, error = %err, "channel roster unreadable");
            }
            return Vec::new();
        }
    };
    let stored: Vec<serde_json::Value> = match serde_json::from_str(&raw) {
        Ok(stored) => stored,
        Err(err) => {
            tracing::warn!(channel = %name, error = %err, "channel roster malformed");
            return Vec::new();
        }
    };
    stored
        .into_iter()
        .filter_map(|value| match value {
            serde_json::Value::String(pane) => Some(ChannelMember::legacy(pane)),
            other => match serde_json::from_value::<ChannelMember>(other) {
                Ok(member) => Some(member),
                Err(err) => {
                    // One unreadable entry must not evict the rest of the
                    // roster, so it is dropped alone and loudly.
                    tracing::warn!(channel = %name, error = %err, "channel roster entry malformed");
                    None
                }
            },
        })
        .filter_map(|member| {
            let pane = resolve(&member)?;
            Some(ChannelMember {
                agent: member.agent,
                pane,
            })
        })
        .collect()
}

/// Replace `name`'s joined roster. Writes a sibling `.tmp` and renames over
/// the roster so a concurrent reader never observes a half-written file. An
/// empty roster removes the file rather than leaving `[]` behind.
pub fn write_joined_members(name: &str, members: &[ChannelMember]) -> io::Result<()> {
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

/// One agent's declared write/read scope within a channel — CANAL-ESCOPO.md
/// Shape 2's registry, the data the harness scope gate consults at runtime.
///
/// Keyed by `agent` (an `AgentId`), never by `pane` and never by
/// `terminal_id`: a public pane id is reallocated on cold restore, so a
/// scope keyed by it would silently grant the previous occupant's write
/// permissions to whoever inherits the number — the one failure mode a
/// scope gate must not have. `pane` remains as the current address, and
/// `nick` rides along only for a readable error message, never as an
/// address.
///
/// `agent: None` is a legacy entry, matched by `pane` alone. It predates
/// the rekey and is honoured, not dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelScopeEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub pane: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nick: Option<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub read: Vec<String>,
}

impl ChannelScopeEntry {
    /// Whether this entry describes the same member as `other`, by identity
    /// when both carry one, else by address. Mirrors
    /// [`ChannelMember::is_same_member`] so a rekeyed roster and a rekeyed
    /// scope registry agree on who is who.
    pub fn is_same_member(&self, other: &Self) -> bool {
        match (&self.agent, &other.agent) {
            (Some(mine), Some(theirs)) => mine == theirs,
            (None, None) => self.pane == other.pane,
            _ => false,
        }
    }

    /// Whether this stored entry is `other`, *or* a legacy entry sitting at
    /// `other`'s pane — the same asymmetric rule as
    /// [`ChannelMember::is_same_member_or_legacy_seat`], and used for the
    /// same two operations: replacing an entry on re-join and dropping one
    /// on leave. A departed pane's declared directories must not outlive
    /// its membership just because the row predates the rekey.
    ///
    /// NEVER use this to decide what a pane is *allowed* to touch. Granting
    /// goes through [`Self::is_same_member`], which refuses to let a
    /// reallocated pane id inherit the previous occupant's write scope.
    pub fn is_same_member_or_legacy_seat(&self, other: &Self) -> bool {
        self.is_same_member(other) || (self.agent.is_none() && self.pane == other.pane)
    }
}

/// Every pane's declared scope for `name`. A missing or unparseable record
/// reads as empty — a corrupt scope file must not take the channel down,
/// only make the gate see no declared scope for anyone.
pub fn read_channel_scope(name: &str) -> Vec<ChannelScopeEntry> {
    let raw = match fs::read_to_string(channel_scope_file_path(name)) {
        Ok(raw) => raw,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!(channel = %name, error = %err, "channel scope record unreadable");
            }
            return Vec::new();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(channel = %name, error = %err, "channel scope record malformed");
            Vec::new()
        }
    }
}

/// Replace `name`'s whole scope record. Writes a sibling `.tmp` and renames
/// over it, mirroring every other channel sidecar file. An empty list
/// removes the file rather than leaving `[]` behind.
fn write_channel_scope(name: &str, entries: &[ChannelScopeEntry]) -> io::Result<()> {
    let path = channel_scope_file_path(name);
    if entries.is_empty() {
        return match fs::remove_file(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        };
    }
    fs::create_dir_all(channels_dir())?;
    let body = serde_json::to_string(entries)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let tmp_path = path.with_extension("json.tmp");
    {
        let mut tmp = fs::File::create(&tmp_path)?;
        tmp.write_all(body.as_bytes())?;
        tmp.flush()?;
    }
    fs::rename(&tmp_path, &path)
}

/// Replace `entry`'s scope for `name`, dropping any prior one for the same
/// member — a re-join with a new scope replaces wholesale rather than
/// merging (CANAL-ESCOPO.md Shape 2: "re-join REPLACES that pane's
/// entry"). Identity decides who "the same member" is, so an agent that
/// came back in a reallocated pane replaces its own entry instead of
/// growing a second one.
pub fn upsert_channel_scope(name: &str, entry: ChannelScopeEntry) -> io::Result<()> {
    let mut entries = read_channel_scope(name);
    entries.retain(|existing| !existing.is_same_member_or_legacy_seat(&entry));
    entries.push(entry);
    write_channel_scope(name, &entries)
}

/// Drop a member's scope entry for `name`, if any. Returns whether an entry
/// was actually removed. Called from `channel.leave` so a declared scope
/// does not outlive the membership it belongs to.
///
/// `agent` is the identity to drop; `pane` only matches legacy entries that
/// carry no identity. Passing `None` for `agent` therefore removes exactly
/// the legacy entry at `pane`, never an identified agent that happens to
/// occupy that pane id today.
pub fn remove_channel_scope_entry(
    name: &str,
    agent: Option<&str>,
    pane: &str,
) -> io::Result<bool> {
    let target = ChannelScopeEntry {
        agent: agent.map(str::to_string),
        pane: pane.to_string(),
        nick: None,
        write: Vec::new(),
        read: Vec::new(),
    };
    let mut entries = read_channel_scope(name);
    let before = entries.len();
    entries.retain(|entry| !entry.is_same_member_or_legacy_seat(&target));
    let removed = entries.len() != before;
    if removed {
        write_channel_scope(name, &entries)?;
    }
    Ok(removed)
}

/// One member's read cursor for a channel: the highest message `seq` they
/// have read via `channel tail` / `channel history`.
///
/// Keyed by `agent`, the same identity the roster reports, so a cursor
/// survives the pane reallocation that a cold restore performs. Were it
/// keyed by pane id, a restored agent would inherit a stranger's read
/// position and silently skip its own unread backlog. `pane` stays as the
/// current address. `agent: None` is a legacy entry, matched by pane.
/// A member with no entry has never read: everything is unread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelReadCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub pane: String,
    pub seq: u64,
}

impl ChannelReadCursor {
    /// Same identity-first comparison the roster and scope registries use.
    fn matches(&self, agent: Option<&str>, pane: &str) -> bool {
        match (self.agent.as_deref(), agent) {
            (Some(mine), Some(theirs)) => mine == theirs,
            (None, None) => self.pane == pane,
            _ => false,
        }
    }
}

/// Every member's read cursor for `name`. A missing or unparseable record
/// reads as empty — a corrupt cursor file must not take the channel down,
/// only make every member look like they've never read (full-unread is the
/// safe default here, never an error).
pub fn read_channel_cursors(name: &str) -> Vec<ChannelReadCursor> {
    let raw = match fs::read_to_string(channel_cursors_file_path(name)) {
        Ok(raw) => raw,
        Err(err) => {
            if err.kind() != io::ErrorKind::NotFound {
                tracing::warn!(channel = %name, error = %err, "channel read cursor record unreadable");
            }
            return Vec::new();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(channel = %name, error = %err, "channel read cursor record malformed");
            Vec::new()
        }
    }
}

/// A member's stored read cursor for `name`, or `None` if it has never read
/// (including when the whole record is unreadable/corrupt — see
/// [`read_channel_cursors`]). Matched by `agent` when given, else by the
/// legacy `pane` key.
pub fn read_channel_cursor(name: &str, agent: Option<&str>, pane: &str) -> Option<u64> {
    read_channel_cursors(name)
        .into_iter()
        .find(|entry| entry.matches(agent, pane))
        .map(|entry| entry.seq)
}

/// Replace `name`'s whole cursor record. Writes a sibling `.tmp` and
/// renames over it, mirroring every other channel sidecar file.
fn write_channel_cursors(name: &str, entries: &[ChannelReadCursor]) -> io::Result<()> {
    fs::create_dir_all(channels_dir())?;
    let path = channel_cursors_file_path(name);
    let body = serde_json::to_string(entries)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    let tmp_path = path.with_extension("json.tmp");
    {
        let mut tmp = fs::File::create(&tmp_path)?;
        tmp.write_all(body.as_bytes())?;
        tmp.flush()?;
    }
    fs::rename(&tmp_path, &path)
}

/// Records that a member has read through `seq` in `name`, replacing any
/// prior cursor for that member — but only forward: a `seq` at or below the
/// stored cursor (or a `seq` of 0 with no stored cursor at all) is a
/// silent no-op, so re-reading old history, or a caller passing seq 0,
/// can never rewind or invent a member's cursor.
///
/// The stored `pane` is refreshed on every advance, so an agent that moved
/// to a new pane keeps one cursor rather than accreting one per pane it has
/// ever occupied.
pub fn advance_channel_cursor(
    name: &str,
    agent: Option<&str>,
    pane: &str,
    seq: u64,
) -> io::Result<()> {
    let mut entries = read_channel_cursors(name);
    match entries.iter_mut().find(|entry| entry.matches(agent, pane)) {
        Some(entry) if seq <= entry.seq => return Ok(()),
        Some(entry) => {
            entry.seq = seq;
            entry.pane = pane.to_string();
        }
        None if seq == 0 => return Ok(()),
        None => entries.push(ChannelReadCursor {
            agent: agent.map(str::to_string),
            pane: pane.to_string(),
            seq,
        }),
    }
    write_channel_cursors(name, &entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::ChannelSenderKind;

    /// Roster entries reduced to their pane ids, for tests that assert on
    /// addresses rather than identities.
    fn panes(members: Vec<ChannelMember>) -> Vec<String> {
        members.into_iter().map(|member| member.pane).collect()
    }

    fn with_isolated_state_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let _guard = crate::config::test_config_env_lock().lock();
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
            write_joined_members(
                "#eng",
                &[
                    ChannelMember::legacy("w1A:p2".into()),
                    ChannelMember::legacy("w3B:p1".into()),
                ],
            ).unwrap();
            assert_eq!(
                panes(read_joined_members("eng", |member| Some(member.pane.clone()))),
                vec!["w1A:p2".to_string(), "w3B:p1".to_string()]
            );
            // A pane that no longer resolves is simply not a member.
            assert_eq!(
                panes(read_joined_members("eng", |member| {
                    (member.pane == "w1A:p2").then(|| member.pane.clone())
                })),
                vec!["w1A:p2".to_string()]
            );
        });
    }

    #[test]
    fn joined_members_missing_or_malformed_roster_is_empty() {
        with_isolated_state_dir("roster-absent", || {
            assert!(read_joined_members("nope", |member| Some(member.pane.clone())).is_empty());
            fs::create_dir_all(channels_dir()).unwrap();
            fs::write(channel_members_file_path("broken"), "{not json").unwrap();
            assert!(read_joined_members("broken", |member| Some(member.pane.clone())).is_empty());
        });
    }

    /// The bug M7 closes. A pane id is reallocated on cold restore, so a
    /// roster keyed by it hands the seat to whoever inherits the number.
    /// Keyed by identity, the member follows its agent to the new pane.
    #[test]
    fn roster_member_follows_its_agent_to_a_reallocated_pane() {
        with_isolated_state_dir("roster-rekey", || {
            write_joined_members(
                "eng",
                &[ChannelMember {
                    agent: Some("agent_a1".into()),
                    pane: "w1A:p2".into(),
                }],
            )
            .unwrap();

            // After a restore the agent lives in a different pane, and
            // `w1A:p2` now belongs to somebody else entirely.
            let members = read_joined_members("eng", |member| {
                assert_eq!(member.agent.as_deref(), Some("agent_a1"));
                Some("w1A:p7".to_string())
            });
            assert_eq!(
                members,
                vec![ChannelMember {
                    agent: Some("agent_a1".into()),
                    pane: "w1A:p7".into(),
                }],
                "the member must survive with its pointer refreshed, not be pruned"
            );

            // The refreshed pointer is what gets persisted, so the stale
            // pane id does not come back on the next read.
            write_joined_members("eng", &members).unwrap();
            assert_eq!(
                panes(read_joined_members("eng", |member| Some(member.pane.clone()))),
                vec!["w1A:p7".to_string()]
            );
        });
    }

    /// A roster written before the rekey is a JSON array of bare pane id
    /// strings. It must keep working — there is no backfill, because
    /// nothing on disk records which agent owned a bare pane id.
    #[test]
    fn legacy_roster_of_bare_pane_ids_still_reads() {
        with_isolated_state_dir("roster-legacy", || {
            fs::create_dir_all(channels_dir()).unwrap();
            fs::write(
                channel_members_file_path("eng"),
                r#"["w1A:p2","w3B:p1"]"#,
            )
            .unwrap();

            let members = read_joined_members("eng", |member| Some(member.pane.clone()));
            assert!(
                members.iter().all(|member| member.agent.is_none()),
                "a legacy entry must not be given an invented identity"
            );
            assert_eq!(
                panes(members),
                vec!["w1A:p2".to_string(), "w3B:p1".to_string()]
            );
        });
    }

    /// A cursor must not be readable through a pane id once an identity
    /// owns it, or a reallocated pane would inherit a stranger's read
    /// position and silently skip its own backlog.
    #[test]
    fn cursor_keyed_by_identity_is_not_reachable_through_a_stale_pane() {
        with_isolated_state_dir("cursor-rekey", || {
            advance_channel_cursor("eng", Some("agent_a1"), "w1A:p2", 7).unwrap();

            assert_eq!(
                read_channel_cursor("eng", Some("agent_a1"), "w1A:p9"),
                Some(7),
                "the cursor follows the identity, not the pane"
            );
            assert_eq!(
                read_channel_cursor("eng", Some("agent_other"), "w1A:p2"),
                None,
                "a different agent inheriting the pane must not inherit the cursor"
            );

            // Advancing refreshes the stored address instead of accreting a
            // second cursor for the same agent.
            advance_channel_cursor("eng", Some("agent_a1"), "w1A:p9", 9).unwrap();
            let cursors = read_channel_cursors("eng");
            assert_eq!(cursors.len(), 1, "got: {cursors:?}");
            assert_eq!(cursors[0].pane, "w1A:p9");
            assert_eq!(cursors[0].seq, 9);
        });
    }

    /// A legacy scope row must be *absorbed* by the agent that occupies its
    /// seat — replaced, never duplicated and never left behind — because a
    /// row with no identity is exactly the row a reallocated pane could
    /// later inherit. Absorption is the migration: after it, the scope is
    /// keyed by an identity and can no longer be inherited by pane.
    #[test]
    fn identified_agent_absorbs_the_legacy_scope_row_at_its_seat() {
        with_isolated_state_dir("scope-rekey", || {
            upsert_channel_scope("eng", scope_entry("w1A:p2", &["/repo/legacy"], &[])).unwrap();
            assert_eq!(read_channel_scope("eng").len(), 1);
            assert!(read_channel_scope("eng")[0].agent.is_none());

            upsert_channel_scope(
                "eng",
                ChannelScopeEntry {
                    agent: Some("agent_a1".into()),
                    pane: "w1A:p2".into(),
                    nick: None,
                    write: vec!["/repo/mine".into()],
                    read: Vec::new(),
                },
            )
            .unwrap();

            let entries = read_channel_scope("eng");
            assert_eq!(entries.len(), 1, "the legacy row must be replaced: {entries:?}");
            assert_eq!(entries[0].agent.as_deref(), Some("agent_a1"));
            assert_eq!(entries[0].write, vec!["/repo/mine".to_string()]);
        });
    }

    /// Granting is the one place the pane fallback must NOT apply: an agent
    /// that inherits a pane id must not inherit the scope declared by
    /// whoever sat there before it.
    #[test]
    fn a_reallocated_pane_does_not_inherit_the_previous_occupants_scope() {
        let legacy = scope_entry("w1A:p2", &["/repo/legacy"], &[]);
        let newcomer = ChannelScopeEntry {
            agent: Some("agent_new".into()),
            pane: "w1A:p2".into(),
            nick: None,
            write: Vec::new(),
            read: Vec::new(),
        };
        assert!(
            !legacy.is_same_member(&newcomer),
            "the grant path must never match a legacy row by pane alone"
        );
        let previous = ChannelScopeEntry {
            agent: Some("agent_old".into()),
            ..legacy
        };
        assert!(
            !previous.is_same_member(&newcomer)
                && !previous.is_same_member_or_legacy_seat(&newcomer),
            "an identified row is never matched by pane, on either path"
        );
    }

    /// Leaving retires the legacy row at the departing seat: a departed
    /// pane's declared directories must not outlive its membership just
    /// because the row predates the rekey.
    #[test]
    fn leaving_retires_the_legacy_scope_row_at_that_seat() {
        with_isolated_state_dir("scope-legacy-leave", || {
            upsert_channel_scope("eng", scope_entry("w1A:p2", &["/repo/legacy"], &[])).unwrap();
            assert!(remove_channel_scope_entry("eng", Some("agent_a1"), "w1A:p2").unwrap());
            assert!(read_channel_scope("eng").is_empty());
        });
    }

    #[test]
    fn writing_roster_is_atomic_and_empty_removes_it() {
        with_isolated_state_dir("roster-atomic", || {
            write_joined_members("eng", &[ChannelMember::legacy("w1A:p2".into())]).unwrap();
            let path = channel_members_file_path("eng");
            assert!(path.exists());
            assert!(!path.with_extension("json.tmp").exists());

            write_joined_members("eng", &[]).unwrap();
            assert!(!path.exists());
            assert!(read_joined_members("eng", |member| Some(member.pane.clone())).is_empty());
            // Removing an already-absent roster is not an error.
            write_joined_members("eng", &[]).unwrap();
        });
    }

    fn scope_entry(pane: &str, write: &[&str], read: &[&str]) -> ChannelScopeEntry {
        ChannelScopeEntry {
            agent: None,
            pane: pane.to_string(),
            nick: Some(format!("{pane}-nick")),
            write: write.iter().map(std::string::ToString::to_string).collect(),
            read: read.iter().map(std::string::ToString::to_string).collect(),
        }
    }

    #[test]
    fn channel_scope_roundtrips() {
        with_isolated_state_dir("scope-roundtrip", || {
            upsert_channel_scope("#eng", scope_entry("w1A:p2", &["/repo/a"], &["/repo/b"]))
                .unwrap();
            let entries = read_channel_scope("eng");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].pane, "w1A:p2");
            assert_eq!(entries[0].nick.as_deref(), Some("w1A:p2-nick"));
            assert_eq!(entries[0].write, vec!["/repo/a".to_string()]);
            assert_eq!(entries[0].read, vec!["/repo/b".to_string()]);
        });
    }

    #[test]
    fn channel_scope_missing_or_malformed_record_is_empty() {
        with_isolated_state_dir("scope-absent", || {
            assert!(read_channel_scope("nope").is_empty());
            fs::create_dir_all(channels_dir()).unwrap();
            fs::write(channel_scope_file_path("broken"), "{not json").unwrap();
            assert!(read_channel_scope("broken").is_empty());
        });
    }

    #[test]
    fn upsert_channel_scope_replaces_not_appends() {
        with_isolated_state_dir("scope-replace", || {
            upsert_channel_scope("eng", scope_entry("w1A:p2", &["/repo/a"], &[])).unwrap();
            upsert_channel_scope("eng", scope_entry("w3B:p1", &[], &["/repo/c"])).unwrap();
            upsert_channel_scope("eng", scope_entry("w1A:p2", &["/repo/z"], &["/repo/y"])).unwrap();

            let entries = read_channel_scope("eng");
            assert_eq!(entries.len(), 2, "re-join replaces, never duplicates");
            let replaced = entries
                .iter()
                .find(|entry| entry.pane == "w1A:p2")
                .expect("replaced entry still present");
            assert_eq!(replaced.write, vec!["/repo/z".to_string()]);
            assert_eq!(replaced.read, vec!["/repo/y".to_string()]);
        });
    }

    #[test]
    fn remove_channel_scope_entry_drops_pane_and_reports_removal() {
        with_isolated_state_dir("scope-remove", || {
            upsert_channel_scope("eng", scope_entry("w1A:p2", &["/repo/a"], &[])).unwrap();
            upsert_channel_scope("eng", scope_entry("w3B:p1", &["/repo/b"], &[])).unwrap();

            assert!(remove_channel_scope_entry("eng", None, "w1A:p2").unwrap());
            let entries = read_channel_scope("eng");
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0].pane, "w3B:p1");

            // Removing an already-absent pane is a no-op, not an error.
            assert!(!remove_channel_scope_entry("eng", None, "w1A:p2").unwrap());
        });
    }

    #[test]
    fn writing_scope_is_atomic_and_empty_removes_it() {
        with_isolated_state_dir("scope-atomic", || {
            upsert_channel_scope("eng", scope_entry("w1A:p2", &["/repo/a"], &[])).unwrap();
            let path = channel_scope_file_path("eng");
            assert!(path.exists());
            assert!(!path.with_extension("json.tmp").exists());

            assert!(remove_channel_scope_entry("eng", None, "w1A:p2").unwrap());
            assert!(!path.exists(), "removing the last entry deletes the file");
        });
    }

    #[test]
    fn cursor_has_no_entry_until_advanced() {
        with_isolated_state_dir("cursor-missing", || {
            assert_eq!(read_channel_cursor("eng", None, "w1A:p2"), None);
        });
    }

    #[test]
    fn cursor_advances_and_never_regresses() {
        with_isolated_state_dir("cursor-advance", || {
            advance_channel_cursor("eng", None, "w1A:p2", 5).unwrap();
            assert_eq!(read_channel_cursor("eng", None, "w1A:p2"), Some(5));

            // A lower or equal seq is a silent no-op — re-reading old
            // history can never rewind the stored cursor.
            advance_channel_cursor("eng", None, "w1A:p2", 3).unwrap();
            assert_eq!(read_channel_cursor("eng", None, "w1A:p2"), Some(5));
            advance_channel_cursor("eng", None, "w1A:p2", 5).unwrap();
            assert_eq!(read_channel_cursor("eng", None, "w1A:p2"), Some(5));

            advance_channel_cursor("eng", None, "w1A:p2", 9).unwrap();
            assert_eq!(read_channel_cursor("eng", None, "w1A:p2"), Some(9));

            // A second pane's cursor is independent.
            assert_eq!(read_channel_cursor("eng", None, "w1A:p9"), None);
        });
    }

    #[test]
    fn corrupt_cursor_file_reads_as_no_cursor_not_an_error() {
        with_isolated_state_dir("cursor-corrupt", || {
            fs::create_dir_all(channels_dir()).unwrap();
            fs::write(channel_cursors_file_path("eng"), b"not json").unwrap();
            assert_eq!(read_channel_cursor("eng", None, "w1A:p2"), None);
            assert!(read_channel_cursors("eng").is_empty());
        });
    }
}
