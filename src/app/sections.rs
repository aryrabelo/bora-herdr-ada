//! The mutation core for `project.section_create`/`project.section_update`
//! (epic bora-79l, T6 pass 6b — "sections montáveis em runtime"). Pure
//! functions over `&mut Vec<Section>`, no filesystem, no socket, testable
//! with a plain `Vec` literal. The handlers in `app::api::projects` own the
//! read-fresh-modify-write discipline (`persist::projects::
//! update_projects_file`) and the `persist::restore::
//! reconcile_section_layout` pass around these calls; nothing here touches
//! `projects.yml` directly.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::ui::sidebar::sections::{
    generate_section_id, Section, SectionChild, SectionKind, SectionParts,
};

/// Where a `project.section_update` mutation is aimed: an existing
/// section's pinned `id`, or the `checkout` whose Branch section should be
/// found — or, when none of `layout`'s sections name that checkout yet,
/// MATERIALIZED (see [`update_section`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionTarget {
    Id(String),
    Checkout(String),
}

/// The Branch section a project declares for `checkout_key`, if any —
/// `layout` is the persisted source of truth (`persist::projects::
/// Project::layout`, already reloaded by `App::poll_projects_store`), so
/// this reads straight through the store with no in-memory copy of its
/// own. Matching key is `SectionChild::Workspace::checkout`, the same key
/// `persist::restore::reconcile_section_layout` and `ui::sidebar::
/// project_view::section_model_flags` already match sections by.
// ponytail: no production caller in THIS pass — the contract this signature
// comes from (epic bora-79l.10, pass 6b, fatia A) hands it to a sibling
// slice's context-menu work (`app::state`/`app::input`) to decide default
// right-click options for a checkout's declared section. The function is
// real and tested below; the `#[allow]` only covers "not called by
// production code yet", not "not finished". Remove once a menu handler
// calls it, same as `persist::restore::reconcile_section_layout` before
// this pass.
#[allow(dead_code)]
pub fn declared_section_for_checkout<'a>(
    projects: &'a crate::persist::projects::ProjectsStore,
    slug: &str,
    checkout_key: &str,
) -> Option<&'a Section> {
    let project = projects.current().projects.get(slug)?;
    let layout = project.layout.as_ref()?;
    layout.iter().find(|section| {
        section.children.iter().any(|child| {
            matches!(
                child,
                SectionChild::Workspace { checkout, .. } if checkout == checkout_key
            )
        })
    })
}

fn generated_section_name_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

/// Appends a new, empty `kind` section to `layout` and returns its
/// freshly assigned id. `name` is used verbatim when given; when `None`,
/// the header gets a random two-word display name
/// (`crate::worktree::generated_two_word_name`) rather than staying a
/// bare `None` a caller has to special-case — every section created
/// through this verb has a real name from the start.
pub fn create_section(
    layout: &mut Vec<Section>,
    kind: SectionKind,
    name: Option<String>,
) -> String {
    let id = generate_section_id();
    let seed = generated_section_name_seed();
    let name = name.unwrap_or_else(|| crate::worktree::generated_two_word_name(seed));
    layout.push(Section {
        id: id.clone(),
        kind,
        name: Some(name),
        header_on: true,
        parts: SectionParts::default(),
        children: Vec::new(),
    });
    id
}

/// Applies `header_on`/`dots`/`diff` (each `None` leaves that field
/// untouched) to the section `target` names, and returns the id the
/// update landed on.
///
/// `SectionTarget::Id` fails closed: `None` when no section in `layout`
/// carries that id, mutating nothing.
///
/// `SectionTarget::Checkout` never fails: it finds the Branch section
/// whose children already name that checkout, or — when none does —
/// MATERIALIZES one (`SectionKind::Branch`, header on, default parts, a
/// single `SectionChild::Workspace` child for the checkout) and appends
/// it to `layout` before applying the requested fields. This is what
/// makes the toggle work against a `projects.yml` with no `layout:` at
/// all (every real project today) — see the gate's design note (epic
/// bora-79l.10, pass 6b).
pub fn update_section(
    layout: &mut Vec<Section>,
    target: &SectionTarget,
    header_on: Option<bool>,
    dots: Option<bool>,
    diff: Option<bool>,
) -> Option<String> {
    let section_id = match target {
        SectionTarget::Id(id) => {
            if layout.iter().any(|section| &section.id == id) {
                id.clone()
            } else {
                return None;
            }
        }
        SectionTarget::Checkout(checkout) => {
            let existing = layout.iter().find(|section| {
                section.children.iter().any(|child| {
                    matches!(
                        child,
                        SectionChild::Workspace { checkout: key, .. } if key == checkout
                    )
                })
            });
            match existing {
                Some(section) => section.id.clone(),
                None => {
                    let id = generate_section_id();
                    layout.push(Section {
                        id: id.clone(),
                        kind: SectionKind::Branch,
                        name: None,
                        header_on: true,
                        parts: SectionParts::default(),
                        children: vec![SectionChild::Workspace {
                            name: checkout.clone(),
                            checkout: checkout.clone(),
                        }],
                    });
                    id
                }
            }
        }
    };

    let section = layout.iter_mut().find(|section| section.id == section_id)?;
    if let Some(header_on) = header_on {
        section.header_on = header_on;
    }
    if let Some(dots) = dots {
        section.parts.dots = dots;
    }
    if let Some(diff) = diff {
        section.parts.diff = diff;
    }
    Some(section_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace_child(checkout: &str) -> SectionChild {
        SectionChild::Workspace {
            name: checkout.to_string(),
            checkout: checkout.to_string(),
        }
    }

    #[test]
    fn create_section_appends_a_named_section_and_returns_its_id() {
        let mut layout = Vec::new();
        let id = create_section(&mut layout, SectionKind::Comando, Some("Dev".to_string()));

        assert_eq!(
            layout.len(),
            1,
            "create_section must append exactly one section"
        );
        let section = &layout[0];
        assert_eq!(section.id, id);
        assert_eq!(section.kind, SectionKind::Comando);
        assert_eq!(section.name.as_deref(), Some("Dev"));
        assert!(section.header_on);
        assert_eq!(section.parts, SectionParts::default());
        assert!(section.children.is_empty());
    }

    #[test]
    fn create_section_without_a_name_gets_a_generated_two_word_name() {
        let mut layout = Vec::new();
        create_section(&mut layout, SectionKind::Livre, None);

        let name = layout[0].name.as_deref().expect("a name must be generated");
        assert!(
            name.contains('-') && name.chars().all(|c| c.is_ascii_lowercase() || c == '-'),
            "generated name must be a bare two-word hyphenated name, got {name:?}"
        );
    }

    #[test]
    fn create_section_ids_are_unique_across_calls() {
        let mut layout = Vec::new();
        let first = create_section(&mut layout, SectionKind::Checks, None);
        let second = create_section(&mut layout, SectionKind::Checks, None);
        assert_ne!(first, second, "each created section must get a distinct id");
    }

    #[test]
    fn update_section_by_id_applies_only_the_fields_given() {
        let mut layout = vec![Section {
            id: "sec-1".to_string(),
            kind: SectionKind::Branch,
            name: None,
            header_on: true,
            parts: SectionParts::default(),
            children: vec![workspace_child("main")],
        }];

        let landed = update_section(
            &mut layout,
            &SectionTarget::Id("sec-1".to_string()),
            Some(false),
            None,
            Some(false),
        );

        assert_eq!(landed, Some("sec-1".to_string()));
        let section = &layout[0];
        assert!(!section.header_on, "header_on: Some(false) must apply");
        assert!(
            section.parts.dots,
            "dots: None must leave the default untouched"
        );
        assert!(!section.parts.diff, "diff: Some(false) must apply");
    }

    #[test]
    fn update_section_by_unknown_id_mutates_nothing_and_returns_none() {
        let mut layout = vec![Section {
            id: "sec-1".to_string(),
            kind: SectionKind::Branch,
            name: None,
            header_on: true,
            parts: SectionParts::default(),
            children: vec![workspace_child("main")],
        }];
        let before = layout.clone();

        let landed = update_section(
            &mut layout,
            &SectionTarget::Id("sec-ghost".to_string()),
            Some(false),
            Some(false),
            Some(false),
        );

        assert_eq!(landed, None, "an unknown id must not land on any section");
        assert_eq!(layout, before, "an unknown id must mutate nothing");
    }

    #[test]
    fn update_section_by_checkout_on_an_empty_layout_materializes_one_branch_section() {
        let mut layout = Vec::new();

        let landed = update_section(
            &mut layout,
            &SectionTarget::Checkout("checkout-a".to_string()),
            Some(false),
            None,
            None,
        );

        assert_eq!(layout.len(), 1, "exactly one section must be materialized");
        let section = &layout[0];
        assert_eq!(landed, Some(section.id.clone()));
        assert_eq!(section.kind, SectionKind::Branch);
        assert!(
            !section.header_on,
            "the requested field must apply to the materialized section"
        );
        assert_eq!(
            section.children,
            vec![SectionChild::Workspace {
                name: "checkout-a".to_string(),
                checkout: "checkout-a".to_string(),
            }],
            "the materialized section must carry the checkout as its one child"
        );
    }

    #[test]
    fn update_section_by_checkout_reuses_the_declaring_section_when_one_exists() {
        let mut layout = vec![
            Section {
                id: "sec-other".to_string(),
                kind: SectionKind::Branch,
                name: None,
                header_on: true,
                parts: SectionParts::default(),
                children: vec![workspace_child("other")],
            },
            Section {
                id: "sec-target".to_string(),
                kind: SectionKind::Branch,
                name: None,
                header_on: true,
                parts: SectionParts::default(),
                children: vec![workspace_child("target")],
            },
        ];

        let landed = update_section(
            &mut layout,
            &SectionTarget::Checkout("target".to_string()),
            Some(false),
            None,
            None,
        );

        assert_eq!(landed, Some("sec-target".to_string()));
        assert_eq!(layout.len(), 2, "no new section must be materialized");
        assert!(!layout[1].header_on);
        assert!(
            layout[0].header_on,
            "the non-matching section must stay untouched"
        );
    }

    #[test]
    fn declared_section_for_checkout_reads_the_projects_store_layout() {
        // Same idiom `ui::sidebar::project_view`'s `store_with` uses: write
        // into an isolated `XDG_CONFIG_HOME`, then load through the real
        // `ProjectsStore::load()` path, so this never touches the
        // operator's own `~/.config/bora/projects.yml` — and exactly ONE
        // `IsolatedDirs` guard (AGENTS.md: two separate isolation guards
        // deadlock).
        let _isolated = crate::config::IsolatedDirs::new("app-sections-declared-for-checkout");
        let mut file = crate::persist::projects::ProjectsFile::default();
        file.projects.insert(
            "cnb".to_string(),
            crate::persist::projects::Project {
                name: None,
                channel: None,
                members: Vec::new(),
                orchestrator: None,
                sections: None,
                layout: Some(vec![Section {
                    id: "sec-main".to_string(),
                    kind: SectionKind::Branch,
                    name: None,
                    header_on: true,
                    parts: SectionParts::default(),
                    children: vec![workspace_child("main")],
                }]),
                auto_join: true,
            },
        );
        crate::persist::projects::write_projects_file(&file).expect("write projects.yml");
        let store = crate::persist::projects::ProjectsStore::load();

        let found = declared_section_for_checkout(&store, "cnb", "main");
        assert_eq!(found.map(|section| section.id.as_str()), Some("sec-main"));

        assert!(
            declared_section_for_checkout(&store, "cnb", "nope").is_none(),
            "an unmatched checkout must yield None"
        );
        assert!(
            declared_section_for_checkout(&store, "ghost", "main").is_none(),
            "an unknown slug must yield None"
        );
    }
}
