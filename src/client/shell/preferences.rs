use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ClientRemoteCollapsedGroups {
    pub(super) profile_id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) collapsed_groups: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(
    from = "ClientChromePreferencesFile",
    into = "ClientChromePreferencesFile"
)]
pub(super) struct ClientChromePreferences {
    pub(super) sidebar_width: Option<u16>,
    pub(super) sidebar_section_split: Option<f32>,
    pub(super) sidebar_collapsed: Option<bool>,
    pub(super) view_mode: Option<crate::config::ViewMode>,
    pub(super) agent_panel_sort: Option<crate::config::AgentPanelSortConfig>,
    /// The saved sort keys exactly as a newer client wrote them, when this
    /// build cannot read the sort. Written back verbatim while
    /// `agent_panel_sort` is `None`, so saving any other preference never
    /// erases a newer client's choice (ceo-bora#354).
    pub(super) unreadable_agent_panel_sort: Option<UnreadableAgentPanelSort>,
    pub(super) collapsed_groups: Vec<String>,
    pub(super) remote_collapsed_groups: Vec<ClientRemoteCollapsedGroups>,
}

#[derive(Clone, Debug)]
pub(super) struct UnreadableAgentPanelSort {
    agent_panel_sort: Option<serde_json::Value>,
    agent_panel_sort_v2: Option<serde_json::Value>,
}

/// The on-disk shape of [`ClientChromePreferences`]. The saved agent sort
/// lives under two keys (ceo-bora#354): clients up to 0.52.7 read
/// `agent_panel_sort` strictly and drop the WHOLE file (sidebar width,
/// collapse state, everything) on a value they do not know, but they
/// ignore unknown keys. So `agent_panel_sort` only ever holds a value those
/// clients know, and `agent_panel_sort_v2` holds the full value. A saved
/// `recent` therefore leaves an older client on its config-default sort
/// with every other preference intact.
#[derive(Default, Deserialize, Serialize)]
struct ClientChromePreferencesFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_width: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_section_split: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sidebar_collapsed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    view_mode: Option<crate::config::ViewMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_panel_sort: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    agent_panel_sort_v2: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    collapsed_groups: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    remote_collapsed_groups: Vec<ClientRemoteCollapsedGroups>,
}

/// `agent_panel_sort_v2` wins over `agent_panel_sort`. A saved sort this
/// build does not know (a newer client wrote it) must not cost the whole
/// file: it reads as "not saved", so the config default applies, one
/// warning names it, and the raw keys are kept for the next store.
impl From<ClientChromePreferencesFile> for ClientChromePreferences {
    fn from(file: ClientChromePreferencesFile) -> Self {
        let parsed = file
            .agent_panel_sort_v2
            .as_ref()
            .or(file.agent_panel_sort.as_ref())
            .map(|saved| {
                crate::config::AgentPanelSortConfig::deserialize(saved)
                    .map_err(|error| (saved.to_string(), error))
            });
        let (agent_panel_sort, unreadable_agent_panel_sort) = match parsed {
            None => (None, None),
            Some(Ok(sort)) => (Some(sort), None),
            Some(Err((saved, error))) => {
                tracing::warn!(
                    %saved,
                    %error,
                    "ignoring unknown saved agent_panel_sort; using the config default"
                );
                (
                    None,
                    Some(UnreadableAgentPanelSort {
                        agent_panel_sort: file.agent_panel_sort,
                        agent_panel_sort_v2: file.agent_panel_sort_v2,
                    }),
                )
            }
        };
        Self {
            sidebar_width: file.sidebar_width,
            sidebar_section_split: file.sidebar_section_split,
            sidebar_collapsed: file.sidebar_collapsed,
            view_mode: file.view_mode,
            agent_panel_sort,
            unreadable_agent_panel_sort,
            collapsed_groups: file.collapsed_groups,
            remote_collapsed_groups: file.remote_collapsed_groups,
        }
    }
}

impl From<ClientChromePreferences> for ClientChromePreferencesFile {
    fn from(preferences: ClientChromePreferences) -> Self {
        use crate::config::AgentPanelSortConfig;

        let (agent_panel_sort, agent_panel_sort_v2) = match (
            preferences.agent_panel_sort,
            preferences.unreadable_agent_panel_sort,
        ) {
            (Some(sort), _) => {
                let value = serde_json::to_value(sort).ok();
                let readable_by_older_clients = match sort {
                    AgentPanelSortConfig::Spaces | AgentPanelSortConfig::Priority => true,
                    AgentPanelSortConfig::Recent => false,
                };
                (
                    readable_by_older_clients.then(|| value.clone()).flatten(),
                    value,
                )
            }
            (None, Some(saved)) => (saved.agent_panel_sort, saved.agent_panel_sort_v2),
            (None, None) => (None, None),
        };
        Self {
            sidebar_width: preferences.sidebar_width,
            sidebar_section_split: preferences.sidebar_section_split,
            sidebar_collapsed: preferences.sidebar_collapsed,
            view_mode: preferences.view_mode,
            agent_panel_sort,
            agent_panel_sort_v2,
            collapsed_groups: preferences.collapsed_groups,
            remote_collapsed_groups: preferences.remote_collapsed_groups,
        }
    }
}

pub(super) fn path_for_local_endpoint(socket_path: &Path) -> PathBuf {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in socket_path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    crate::config::state_dir()
        .join("client-shell")
        .join(format!("local-{hash:016x}.json"))
}

/// SSH preferences belong to the target/session, not its per-process bridge socket.
pub(super) fn path_for_remote_endpoint(target: &str, session: &str) -> PathBuf {
    use sha2::{Digest as _, Sha256};

    let mut hash = Sha256::new();
    for part in [target, session] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    crate::config::state_dir()
        .join("client-shell")
        .join(format!("remote-{:x}.json", hash.finalize()))
}

pub(super) fn load(path: &Path) -> Option<ClientChromePreferences> {
    let content = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

pub(super) fn store(path: &Path, preferences: ClientChromePreferences) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid client shell state path: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed to create client shell state directory: {error}"))?;
    let content = serde_json::to_vec_pretty(&preferences)
        .map_err(|error| format!("failed to encode client shell state: {error}"))?;
    let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
    let mut temp_name = path
        .file_name()
        .ok_or_else(|| format!("invalid client shell state path: {}", path.display()))?
        .to_os_string();
    temp_name.push(format!(".tmp-{}-{sequence}", std::process::id()));
    let temp_path = parent.join(temp_name);
    std::fs::write(&temp_path, content)
        .map_err(|error| format!("failed to write client shell state: {error}"))?;
    std::fs::rename(&temp_path, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp_path);
        format!("failed to replace client shell state: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_paths_are_stable_and_distinct() {
        let first = path_for_local_endpoint(Path::new("/run/herdr/one.sock"));
        let again = path_for_local_endpoint(Path::new("/run/herdr/one.sock"));
        let second = path_for_local_endpoint(Path::new("/run/herdr/two.sock"));
        assert_eq!(first, again);
        assert_ne!(first, second);
    }

    #[test]
    fn remote_preferences_distinguish_targets_sessions_and_component_boundaries() {
        let original = path_for_remote_endpoint("dev@build", "agents");
        assert_eq!(original, path_for_remote_endpoint("dev@build", "agents"));
        assert_ne!(original, path_for_remote_endpoint("other@build", "agents"));
        assert_ne!(original, path_for_remote_endpoint("dev@build", "other"));
        assert_ne!(
            path_for_remote_endpoint("ab", "c"),
            path_for_remote_endpoint("a", "bc")
        );
        assert_ne!(
            original,
            path_for_local_endpoint(Path::new("dev@build/agents"))
        );
    }

    #[test]
    fn legacy_preferences_default_remote_collapses() {
        let preferences: ClientChromePreferences =
            serde_json::from_str(r#"{"collapsed_groups":["/repo"]}"#)
                .expect("legacy client chrome preferences");

        assert_eq!(preferences.collapsed_groups, ["/repo"]);
        assert!(preferences.remote_collapsed_groups.is_empty());
    }

    #[test]
    fn concurrent_stores_leave_complete_preferences() {
        let path = std::env::temp_dir().join(format!(
            "herdr-shell-concurrent-preferences-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let writers = (20..28)
            .map(|width| {
                let path = path.clone();
                std::thread::spawn(move || {
                    store(
                        &path,
                        ClientChromePreferences {
                            sidebar_width: Some(width),
                            ..ClientChromePreferences::default()
                        },
                    )
                })
            })
            .collect::<Vec<_>>();
        for writer in writers {
            writer.join().expect("preference writer").expect("store");
        }
        assert!(load(&path)
            .and_then(|saved| saved.sidebar_width)
            .is_some_and(|width| (20..28).contains(&width)));
        std::fs::remove_file(path).expect("remove preferences");
    }

    #[test]
    fn repeated_store_replaces_existing_preferences() {
        let path = std::env::temp_dir().join(format!(
            "herdr-shell-preferences-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        store(
            &path,
            ClientChromePreferences {
                sidebar_width: Some(24),
                ..ClientChromePreferences::default()
            },
        )
        .expect("first preference store");
        store(
            &path,
            ClientChromePreferences {
                sidebar_width: Some(32),
                ..ClientChromePreferences::default()
            },
        )
        .expect("replacement preference store");
        assert_eq!(load(&path).and_then(|saved| saved.sidebar_width), Some(32));
        std::fs::remove_file(path).expect("remove preferences");
    }

    /// The 0.52.7 client's preferences shape, frozen as shipped: strict on
    /// `agent_panel_sort` (an unknown value fails the whole file), silent on
    /// unknown keys.
    #[derive(Deserialize)]
    struct Shipped0527Preferences {
        #[serde(default)]
        sidebar_width: Option<u16>,
        #[serde(default)]
        agent_panel_sort: Option<Shipped0527Sort>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(rename_all = "lowercase")]
    enum Shipped0527Sort {
        #[serde(alias = "workspaces")]
        Spaces,
        Priority,
    }

    /// Every sort this build saves leaves the file readable by 0.52.7: it
    /// keeps the sidebar width, sees `spaces`/`priority` as saved, and sees
    /// no saved sort for `recent` (its config default applies). This build
    /// reads each value back from the same file.
    #[test]
    fn recent_sort_store_keeps_the_file_readable_by_0_52_7_clients() {
        use crate::config::AgentPanelSortConfig;

        let path = std::env::temp_dir().join(format!(
            "herdr-shell-recent-sort-compat-{}.json",
            std::process::id()
        ));
        for (sort, shipped) in [
            (AgentPanelSortConfig::Spaces, Some(Shipped0527Sort::Spaces)),
            (
                AgentPanelSortConfig::Priority,
                Some(Shipped0527Sort::Priority),
            ),
            (AgentPanelSortConfig::Recent, None),
        ] {
            store(
                &path,
                ClientChromePreferences {
                    sidebar_width: Some(30),
                    agent_panel_sort: Some(sort),
                    ..ClientChromePreferences::default()
                },
            )
            .expect("store preferences");
            let content = std::fs::read_to_string(&path).expect("read preferences");
            let old: Shipped0527Preferences = serde_json::from_str(&content)
                .unwrap_or_else(|error| panic!("0.52.7 rejects {content}: {error}"));
            assert_eq!(
                (old.sidebar_width, old.agent_panel_sort),
                (Some(30), shipped)
            );
            assert_eq!(
                load(&path).and_then(|saved| saved.agent_panel_sort),
                Some(sort)
            );
        }
        std::fs::remove_file(path).expect("remove preferences");
    }
}
