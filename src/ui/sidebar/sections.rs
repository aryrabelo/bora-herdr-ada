//! The Project-view Section model (epic bora-79l, leaf F1).
//!
//! Everything inside a project group is a [`Section`] of one of four
//! [`SectionKind`]s: `branch` (children are the two-line workspace blocks),
//! `comando`/`checks` (children are display items), and `livre` (the empty,
//! mountable slot). Header on/off is the old HIDDEN switch; [`SectionParts`]
//! are the right-click toggles (dots, diff). `id` is the section's own
//! stable identity (plan decision on restore — epic bora-79l, F7): distinct
//! from position in the list, so reordering/adding/removing sections
//! elsewhere in the file never reassigns a workspace to the wrong slot.
//! Model decisions are the owner's and recorded in the epic plan; this
//! module owns the data and its (de)serialization — render wiring is F2/F3,
//! projects.yml persistence is F7.
//!
//! YAML stub shape (serde_yaml_ng, same conventions as
//! `persist::projects`): kind is lowercase, children are internally tagged
//! with `type`, and every field except `kind` has a default so a
//! hand-written stub stays terse — including `id`, which gets a fresh one
//! assigned on load when absent and round-trips verbatim after that:
//!
//! ```yaml
//! - kind: branch
//!   children:
//!     - type: workspace
//!       name: main
//!       checkout: main
//! - kind: checks
//!   children:
//!     - type: item
//!       label: clippy
//!       failing: true
//! - kind: livre
//! ```

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionKind {
    /// Session blocks: children are [`SectionChild::Workspace`] entries.
    Branch,
    /// Declared-command band: children are [`SectionChild::Item`] entries.
    Comando,
    /// CI-check band: children are [`SectionChild::Item`] entries.
    Checks,
    /// The empty, mountable slot — hidden until something is dropped in.
    Livre,
}

/// Right-click toggles per section (plan decision 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionParts {
    #[serde(default = "default_true")]
    pub dots: bool,
    #[serde(default = "default_true")]
    pub diff: bool,
}

impl Default for SectionParts {
    fn default() -> Self {
        Self {
            dots: true,
            diff: true,
        }
    }
}

fn default_true() -> bool {
    true
}

/// Field-level default for `Section::parts`: when the whole `parts` key is
/// absent, serde must use THIS (both toggles ON — the domain default), not
/// a derived all-false struct default. `#[serde(default)]` on the field
/// alone would bypass the inner per-key defaults, which only fire when
/// `parts` is present but a key inside it is not.
fn default_parts() -> SectionParts {
    SectionParts::default()
}

static NEXT_SECTION_ID: AtomicU64 = AtomicU64::new(1);

/// Field-level default for `Section::id`: a hand-written stub (or a
/// section from before pinned ids landed) gets a fresh one assigned on
/// load, exactly once — from then on the owner round-trips the same value
/// back to disk, and F7's restore reconciliation matches "the same
/// section" by this, never by index.
fn generate_section_id() -> String {
    let counter = NEXT_SECTION_ID.fetch_add(1, Ordering::Relaxed);
    format!("sec-{counter}")
}

/// One entry inside a section. `Workspace` carries the identity the
/// two-line block renders (F2); `Item` is a display-only band entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum SectionChild {
    Workspace {
        /// The workspace's own unique display name (random `adjetivo-animal`
        /// at creation, user-editable — plan decision on names).
        name: String,
        /// Which checkout of the repo this workspace is bound to.
        checkout: String,
    },
    Item {
        label: String,
        #[serde(default)]
        failing: bool,
    },
}

/// A mountable section of a project group (plan decisions 1-4, 8-9).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Section {
    /// Stable identity for this section, independent of its position in
    /// the list (see the module doc and [`generate_section_id`]).
    #[serde(default = "generate_section_id")]
    pub id: String,
    pub kind: SectionKind,
    /// The old HIDDEN switch: whether the section's header line renders.
    #[serde(default = "default_true")]
    pub header_on: bool,
    #[serde(default = "default_parts")]
    pub parts: SectionParts,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SectionChild>,
}

pub fn parse_sections_yaml(raw: &str) -> Result<Vec<Section>, String> {
    serde_yaml_ng::from_str(raw).map_err(|err| err.to_string())
}

pub fn sections_to_yaml(sections: &[Section]) -> Result<String, String> {
    serde_yaml_ng::to_string(sections).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alvo-shaped tree (what the contract capture describes) must
    /// survive serialize → parse byte-for-byte in structure: this is the
    /// round-trip the leaf's acceptance names.
    #[test]
    fn sections_model_round_trip() {
        let sections = vec![
            Section {
                id: "sec-branch-main".to_string(),
                kind: SectionKind::Branch,
                header_on: true,
                parts: SectionParts {
                    dots: true,
                    diff: true,
                },
                children: vec![
                    SectionChild::Workspace {
                        name: "main".to_string(),
                        checkout: "main".to_string(),
                    },
                    SectionChild::Workspace {
                        name: "main-review".to_string(),
                        checkout: "main-review".to_string(),
                    },
                ],
            },
            Section {
                id: "sec-branch-feature-x".to_string(),
                kind: SectionKind::Branch,
                header_on: false,
                parts: SectionParts {
                    dots: true,
                    diff: false,
                },
                children: vec![SectionChild::Workspace {
                    name: "feature-x".to_string(),
                    checkout: "feature-x".to_string(),
                }],
            },
            Section {
                id: "sec-comando".to_string(),
                kind: SectionKind::Comando,
                header_on: true,
                parts: SectionParts::default(),
                children: vec![SectionChild::Item {
                    label: "dev".to_string(),
                    failing: false,
                }],
            },
            Section {
                id: "sec-checks".to_string(),
                kind: SectionKind::Checks,
                header_on: true,
                parts: SectionParts::default(),
                children: vec![SectionChild::Item {
                    label: "clippy".to_string(),
                    failing: true,
                }],
            },
            Section {
                id: "sec-livre".to_string(),
                kind: SectionKind::Livre,
                header_on: true,
                parts: SectionParts::default(),
                children: vec![],
            },
        ];
        let yaml = sections_to_yaml(&sections).expect("serialize sections");
        let parsed = parse_sections_yaml(&yaml).expect("parse sections back");
        assert_eq!(parsed, sections, "round-trip must preserve the tree");
    }

    /// A hand-written stub may omit everything but `kind`: header ON, both
    /// parts ON, no children, and a freshly assigned `id`. Terse defaults
    /// are what make the file hand-editable, so they are contract, not
    /// convenience. This is also the test that caught the all-false
    /// field-default bug: an omitted `parts` key must not fall through to
    /// a derived all-false default.
    #[test]
    fn sections_model_defaults_fill_optional_fields() {
        let parsed = parse_sections_yaml("- kind: livre\n").expect("minimal stub must parse");
        assert_eq!(parsed.len(), 1, "one stub line, one section");
        let section = &parsed[0];
        assert!(
            !section.id.is_empty(),
            "a missing id must default to a freshly generated one, never an empty string"
        );
        assert_eq!(
            (
                section.kind,
                section.header_on,
                section.parts,
                section.children.as_slice(),
            ),
            (
                SectionKind::Livre,
                true,
                SectionParts {
                    dots: true,
                    diff: true
                },
                &[][..]
            ),
        );
    }

    /// Unknown fields must fail loudly (same stance as
    /// `persist::projects`' `deny_unknown_fields`): a typoed key in a
    /// hand-edited file is a parse error, never silently-dropped state.
    #[test]
    fn sections_model_rejects_unknown_fields() {
        let err = parse_sections_yaml("- kind: livre\n  collapsed: true\n").unwrap_err();
        assert!(
            err.contains("unknown"),
            "unknown fields must be rejected, got: {err}"
        );
    }
}
