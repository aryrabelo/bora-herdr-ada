//! Sidebar `#canal` badge support: which workspace has a pane that joined a
//! channel living elsewhere. Refreshed periodically (see
//! `CHANNEL_MEMBERSHIP_REFRESH_INTERVAL`) so the sidebar never touches disk.

use std::time::Instant;

use super::{App, CHANNEL_MEMBERSHIP_REFRESH_INTERVAL};

/// `#`-labelled workspace name a `#`-channel workspace is hosted at, or
/// `None` for anything else (mirrors `app::api::channels::workspace_channel_name`,
/// which is private to that module).
fn channel_home_name(ws: &crate::workspace::Workspace) -> Option<&str> {
    if ws.visual_group.is_some() {
        return None;
    }
    ws.custom_name.as_deref().and_then(|name| name.strip_prefix('#'))
}

impl App {
    /// Refresh `Workspace::cached_channels` for every workspace: the
    /// `#`-channels it has a pane explicitly joined into (not counting a
    /// channel's own home workspace — its name already shows that). Reads a
    /// handful of small `channels/*.members.json` files; cheap enough to run
    /// inline here rather than spawn a worker thread, but still off the
    /// render path.
    pub(crate) fn refresh_channel_membership_if_due(&mut self, now: Instant) {
        if now.saturating_duration_since(self.last_channel_membership_refresh)
            < CHANNEL_MEMBERSHIP_REFRESH_INTERVAL
        {
            return;
        }
        self.last_channel_membership_refresh = now;

        let channel_names: Vec<String> = self
            .state
            .workspaces
            .iter()
            .filter_map(channel_home_name)
            .map(str::to_string)
            .collect();

        let mut membership: std::collections::HashMap<usize, Vec<String>> =
            std::collections::HashMap::new();
        for name in channel_names {
            let members = crate::persist::channels::read_joined_members(&name, |pane| {
                self.parse_pane_id(pane).is_some()
            });
            for pane in members {
                if let Some((owner_ws_idx, _)) = self.parse_pane_id(&pane) {
                    let entry = membership.entry(owner_ws_idx).or_default();
                    if !entry.contains(&name) {
                        entry.push(name.clone());
                    }
                }
            }
        }

        for (ws_idx, ws) in self.state.workspaces.iter_mut().enumerate() {
            let mut channels = membership.remove(&ws_idx).unwrap_or_default();
            channels.sort();
            if ws.cached_channels != channels {
                ws.cached_channels = channels;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Workspace;

    fn test_app() -> App {
        App::new(
            &crate::config::Config::default(),
            true,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        )
    }

    /// Channel roster reads/writes hit `state_dir()`; isolate each test to a
    /// scratch `XDG_STATE_HOME` so it never touches the real one, mirroring
    /// `persist::channels::tests::with_isolated_state_dir`.
    fn with_isolated_state_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
        let _guard = crate::config::test_config_env_lock().lock().unwrap();
        let old_state = std::env::var_os("XDG_STATE_HOME");
        let dir = std::env::temp_dir()
            .join(format!("bora-channel-membership-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_STATE_HOME", &dir);
        let result = f();
        match old_state {
            Some(value) => std::env::set_var("XDG_STATE_HOME", value),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    #[test]
    fn refresh_populates_channel_for_pane_that_joined_elsewhere() {
        with_isolated_state_dir("joined-elsewhere", || {
            let mut app = test_app();
            let mut planning = Workspace::test_new("#planning");
            planning.custom_name = Some("#planning".into());
            let member = Workspace::test_new("member");
            app.state.workspaces.push(planning);
            app.state.workspaces.push(member);
            let member_ws = &app.state.workspaces[1];
            let member_pane = member_ws.tabs[0].root_pane;
            let member_public_id = crate::workspace::public_pane_id_for_number(&member_ws.id, 1);
            assert_eq!(app.parse_pane_id(&member_public_id), Some((1, member_pane)));
            crate::persist::channels::write_joined_members("planning", &[member_public_id])
                .expect("write roster");

            app.refresh_channel_membership_if_due(Instant::now());

            assert_eq!(
                app.state.workspaces[1].cached_channels,
                vec!["planning".to_string()]
            );
            // The channel's own home workspace doesn't badge itself.
            assert!(app.state.workspaces[0].cached_channels.is_empty());
        });
    }

    #[test]
    fn refresh_is_throttled_within_the_interval() {
        with_isolated_state_dir("throttled", || {
            let mut app = test_app();
            app.state.workspaces.push(Workspace::test_new("one"));
            let now = Instant::now();
            app.last_channel_membership_refresh = now;
            app.state.workspaces[0].cached_channels = vec!["stale".into()];

            app.refresh_channel_membership_if_due(now);

            assert_eq!(
                app.state.workspaces[0].cached_channels,
                vec!["stale".to_string()]
            );
        });
    }
}
