//! Wire shapes for the `scratchpad.*` verbs (bora-s3y.2) over the
//! `persist::scratchpads` append-only doc store (bora-s3y.1): params, plus
//! wire mirrors of the store's record types. The doc key is named `doc`
//! (never `name`) so it can never be mistaken for a channel name by the
//! MCP `--channels` fence — a scratchpad document is not channel traffic.

use serde::{Deserialize, Serialize};

/// One section supplied to `scratchpad.write` — a markdown heading
/// (`title`) plus its `body`. The store assigns each section its `seq`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScratchpadDraftParams {
    pub title: String,
    pub body: String,
}

/// `scratchpad.write`: create or replace the doc wholesale — `sections`
/// becomes the whole doc. Followers' cursors stay valid: the replacement's
/// seqs continue from the current tip rather than restarting at 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScratchpadWriteParams {
    pub project: String,
    pub doc: String,
    pub sections: Vec<ScratchpadDraftParams>,
}

/// `scratchpad.append_section`: append one section to the doc, creating
/// the doc on first use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScratchpadAppendSectionParams {
    pub project: String,
    pub doc: String,
    pub title: String,
    pub body: String,
}

/// `scratchpad.find`: case-insensitive substring search over section
/// titles and bodies across every doc in the project. An empty query is
/// rejected at the verb layer (it would match every section).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScratchpadFindParams {
    pub project: String,
    pub query: String,
}

/// Wire mirror of `persist::scratchpads::ScratchpadSection`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScratchpadSectionInfo {
    pub seq: u64,
    pub title: String,
    pub body: String,
}

impl From<crate::persist::scratchpads::ScratchpadSection> for ScratchpadSectionInfo {
    fn from(section: crate::persist::scratchpads::ScratchpadSection) -> Self {
        Self {
            seq: section.seq,
            title: section.title,
            body: section.body,
        }
    }
}

/// Wire mirror of `persist::scratchpads::ScratchpadHit`: which doc, which
/// section.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScratchpadHitInfo {
    pub doc: String,
    pub section: ScratchpadSectionInfo,
}

impl From<crate::persist::scratchpads::ScratchpadHit> for ScratchpadHitInfo {
    fn from(hit: crate::persist::scratchpads::ScratchpadHit) -> Self {
        Self {
            doc: hit.doc,
            section: hit.section.into(),
        }
    }
}
