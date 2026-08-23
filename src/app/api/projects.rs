//! Handlers for the `project.*` socket verbs — `project.list`, `.create`,
//! `.update`, `.member_add`, `.member_remove` — the write path onto
//! `~/.config/bora/projects.yml` that `persist::projects` (bora-e9i.1)
//! landed as read-only. Every handler here goes through
//! `persist::projects::load_projects_file_fresh` /
//! `persist::projects::update_projects_file` directly, never through
//! `App`'s own cached state, because a socket verb must always answer from
//! (and write on top of) the CURRENT on-disk file — see that module's doc
//! comment for why.

use crate::api::schema::{
    ChannelCreateParams, EmptyParams, ProjectCreateParams, ProjectMemberAddParams,
    ProjectMemberInfo, ProjectMemberRemoveParams, ProjectMemberResolution, ProjectSummary,
    ProjectUpdateParams, ResponseResult,
};
use crate::app::App;
use crate::persist::projects::{self, Member, MemberResolution, Project, ProjectsUpdateError};

use super::responses::{encode_error, encode_success};

/// A verb-specific validation failure from an `update_projects_file`
/// mutation closure: the machine-readable error code alongside a human
/// message, so a handler can report it directly instead of pattern
/// matching on which business rule tripped.
struct MutationFailure {
    code: &'static str,
    message: String,
}

impl MutationFailure {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Converts one parsed [`Project`] plus its slug into the wire
/// [`ProjectSummary`] every `project.*` verb returns, resolving each
/// member's identity along the way (`Member::resolve`) so a caller never
/// has to redo git discovery itself.
fn project_summary(slug: String, project: Project) -> ProjectSummary {
    let channel = project.effective_channel(&slug);
    let auto_join = project.auto_join;
    let members = project
        .members
        .into_iter()
        .map(|member| {
            let resolution = resolution_to_wire(member.resolve());
            ProjectMemberInfo {
                worktrees: member.worktrees,
                dir: member.dir,
                resolution,
            }
        })
        .collect();
    ProjectSummary {
        slug,
        name: project.name,
        channel,
        auto_join,
        members,
    }
}

fn resolution_to_wire(resolution: MemberResolution) -> ProjectMemberResolution {
    match resolution {
        MemberResolution::Resolved(resolved) => ProjectMemberResolution::Resolved {
            repo_identity: resolved.repo_identity,
            checkout_key: resolved.checkout_key,
            subdir: resolved.subdir.display().to_string(),
        },
        MemberResolution::Unresolved { dir, reason } => ProjectMemberResolution::Unresolved {
            dir: dir.display().to_string(),
            reason,
        },
    }
}

impl App {
    pub(super) fn handle_project_list(&mut self, id: String, _params: EmptyParams) -> String {
        let file = match projects::load_projects_file_fresh() {
            Ok(file) => file,
            Err(err) => return encode_error(id, "project_list_failed", err),
        };
        let projects = file
            .projects
            .into_iter()
            .map(|(slug, project)| project_summary(slug, project))
            .collect();
        encode_success(id, ResponseResult::ProjectList { projects })
    }

    pub(super) fn handle_project_create(
        &mut self,
        id: String,
        params: ProjectCreateParams,
    ) -> String {
        let slug = params.slug.trim().to_string();
        if slug.is_empty() {
            return encode_error(id, "project_slug_invalid", "project slug must not be empty");
        }
        let name = params.name;
        let channel = params.channel;
        let auto_join = params.auto_join.unwrap_or(true);
        let insert_slug = slug.clone();
        let result = projects::update_projects_file(move |file| {
            if file.projects.contains_key(&insert_slug) {
                return Err(MutationFailure::new(
                    "project_exists",
                    format!("project {insert_slug:?} already exists"),
                ));
            }
            file.projects.insert(
                insert_slug,
                Project {
                    name,
                    channel,
                    members: Vec::new(),
                    orchestrator: None,
                    sections: None,
                    auto_join,
                },
            );
            Ok(())
        });
        match result {
            Ok(file) => {
                let project = file
                    .projects
                    .get(&slug)
                    .cloned()
                    .expect("just inserted above");
                self.ensure_project_channel(&project.effective_channel(&slug));
                encode_success(
                    id,
                    ResponseResult::ProjectCreated {
                        project: project_summary(slug, project),
                    },
                )
            }
            Err(ProjectsUpdateError::Mutate(failure)) => {
                encode_error(id, failure.code, failure.message)
            }
            Err(ProjectsUpdateError::Load(msg)) => encode_error(id, "project_create_failed", msg),
            Err(ProjectsUpdateError::Save(msg)) => encode_error(id, "project_create_failed", msg),
        }
    }

    pub(super) fn handle_project_update(
        &mut self,
        id: String,
        params: ProjectUpdateParams,
    ) -> String {
        let slug = params.slug.trim().to_string();
        if slug.is_empty() {
            return encode_error(id, "project_slug_invalid", "project slug must not be empty");
        }
        let name = params.name;
        let channel = params.channel;
        let auto_join = params.auto_join.unwrap_or(true);
        let target_slug = slug.clone();
        let result = projects::update_projects_file(move |file| {
            let Some(project) = file.projects.get_mut(&target_slug) else {
                return Err(MutationFailure::new(
                    "project_not_found",
                    format!("project {target_slug:?} not found"),
                ));
            };
            project.name = name;
            project.channel = channel;
            project.auto_join = auto_join;
            Ok(())
        });
        match result {
            Ok(file) => {
                let project = file
                    .projects
                    .get(&slug)
                    .cloned()
                    .expect("verified present above");
                // Re-bind, never rename (design doc §"Project = channel —
                // resolved", decision A): the OLD channel this project may
                // have pointed at before is left completely untouched —
                // its transcript, roster, and workspace all stay exactly
                // as they were, just unbound from this project. Only the
                // NEW effective channel gets ensured here.
                self.ensure_project_channel(&project.effective_channel(&slug));
                encode_success(
                    id,
                    ResponseResult::ProjectUpdated {
                        project: project_summary(slug, project),
                    },
                )
            }
            Err(ProjectsUpdateError::Mutate(failure)) => {
                encode_error(id, failure.code, failure.message)
            }
            Err(ProjectsUpdateError::Load(msg)) => encode_error(id, "project_update_failed", msg),
            Err(ProjectsUpdateError::Save(msg)) => encode_error(id, "project_update_failed", msg),
        }
    }

    /// Creates `channel_name`'s channel workspace via the existing
    /// `channel.create` verb when it does not already exist, or leaves an
    /// existing one — transcript, roster, panes — completely untouched.
    /// `handle_channel_create`'s own `channel_exists` response IS the
    /// reuse path, not a separate one this bead needs to build. Called by
    /// both `project.create` (bind on creation) and `project.update`
    /// (re-bind on a `channel:` change).
    ///
    /// Never fails the caller: `projects.yml` has already been written by
    /// the time this runs, so a channel workspace that could not be
    /// created must not roll back or fail an otherwise-successful
    /// `project.create`/`project.update` — it is logged and the project is
    /// left pointing at a channel name with no live workspace yet, exactly
    /// as if an operator had hand-typed a `channel:` no one created.
    fn ensure_project_channel(&mut self, channel_name: &str) {
        let name = crate::persist::channels::normalize_channel_name(channel_name);
        if name.is_empty() {
            return;
        }
        let response = self.handle_channel_create(
            "internal:project-channel-bind".into(),
            ChannelCreateParams { name: name.clone() },
        );
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&response) else {
            return;
        };
        if parsed.get("result").is_some() || parsed["error"]["code"] == "channel_exists" {
            return;
        }
        tracing::warn!(
            channel = %name,
            error = %parsed["error"]["message"].as_str().unwrap_or("unknown"),
            "project channel binding failed; project persisted without a live channel workspace"
        );
    }

    pub(super) fn handle_project_member_add(
        &mut self,
        id: String,
        params: ProjectMemberAddParams,
    ) -> String {
        let slug = params.slug.trim().to_string();
        if slug.is_empty() {
            return encode_error(id, "project_slug_invalid", "project slug must not be empty");
        }
        let dir = params.dir.trim().to_string();
        if dir.is_empty() {
            return encode_error(
                id,
                "project_member_dir_invalid",
                "member dir must not be empty",
            );
        }
        let worktrees = params.worktrees;
        let target_slug = slug.clone();
        let target_dir = dir;
        let result = projects::update_projects_file(move |file| {
            let Some(project) = file.projects.get_mut(&target_slug) else {
                return Err(MutationFailure::new(
                    "project_not_found",
                    format!("project {target_slug:?} not found"),
                ));
            };
            match project.members.iter_mut().find(|m| m.dir == target_dir) {
                Some(existing) => existing.worktrees = worktrees,
                None => project.members.push(Member {
                    dir: target_dir,
                    worktrees,
                    template: None,
                }),
            }
            Ok(())
        });
        match result {
            Ok(file) => match file.projects.get(&slug).cloned() {
                Some(project) => encode_success(
                    id,
                    ResponseResult::ProjectMemberAdded {
                        project: project_summary(slug, project),
                    },
                ),
                // Unreachable: the closure above errors out when the slug is
                // absent, so a successful write implies it is present. Encoded
                // as an error rather than `expect`ed because this runs on the
                // server's request path, where a panic takes the session down.
                None => encode_error(
                    id,
                    "project_member_add_failed",
                    format!("project {slug:?} vanished between write and read"),
                ),
            },
            Err(ProjectsUpdateError::Mutate(failure)) => {
                encode_error(id, failure.code, failure.message)
            }
            Err(ProjectsUpdateError::Load(msg)) => {
                encode_error(id, "project_member_add_failed", msg)
            }
            Err(ProjectsUpdateError::Save(msg)) => {
                encode_error(id, "project_member_add_failed", msg)
            }
        }
    }

    pub(super) fn handle_project_member_remove(
        &mut self,
        id: String,
        params: ProjectMemberRemoveParams,
    ) -> String {
        let slug = params.slug.trim().to_string();
        if slug.is_empty() {
            return encode_error(id, "project_slug_invalid", "project slug must not be empty");
        }
        let dir = params.dir.trim().to_string();
        if dir.is_empty() {
            return encode_error(
                id,
                "project_member_dir_invalid",
                "member dir must not be empty",
            );
        }
        let target_slug = slug.clone();
        let target_dir = dir;
        let result = projects::update_projects_file(move |file| {
            let Some(project) = file.projects.get_mut(&target_slug) else {
                return Err(MutationFailure::new(
                    "project_not_found",
                    format!("project {target_slug:?} not found"),
                ));
            };
            let before = project.members.len();
            project.members.retain(|m| m.dir != target_dir);
            if project.members.len() == before {
                return Err(MutationFailure::new(
                    "project_member_not_found",
                    format!("project {target_slug:?} has no member dir {target_dir:?}"),
                ));
            }
            Ok(())
        });
        match result {
            Ok(file) => {
                let project = file
                    .projects
                    .get(&slug)
                    .cloned()
                    .expect("verified present above");
                encode_success(
                    id,
                    ResponseResult::ProjectMemberRemoved {
                        project: project_summary(slug, project),
                    },
                )
            }
            Err(ProjectsUpdateError::Mutate(failure)) => {
                encode_error(id, failure.code, failure.message)
            }
            Err(ProjectsUpdateError::Load(msg)) => {
                encode_error(id, "project_member_remove_failed", msg)
            }
            Err(ProjectsUpdateError::Save(msg)) => {
                encode_error(id, "project_member_remove_failed", msg)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{ChannelJoinParams, ChannelMessage, ChannelSenderKind};
    use crate::config::{Config, IsolatedDirs};
    use crate::persist::projects::{projects_file_path, WorktreesScope};

    /// `NonLogin` is not cosmetic here: `project.create` binds a channel,
    /// which spawns a real channel workspace, and a login shell sources the
    /// developer's full profile and never exits — every test in this module
    /// hung until this matched `app::api::channels`'s helper.
    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        app.state.default_shell = super::super::test_support::exiting_test_command().into();
        app.state.shell_mode = crate::config::ShellModeConfig::NonLogin;
        app
    }

    fn create(app: &mut App, slug: &str) -> serde_json::Value {
        let response = app.handle_project_create(
            "req".into(),
            ProjectCreateParams {
                slug: slug.into(),
                name: None,
                channel: None,
                auto_join: None,
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    fn list(app: &mut App) -> serde_json::Value {
        let response = app.handle_project_list("req".into(), EmptyParams {});
        serde_json::from_str(&response).unwrap()
    }

    /// A pane living outside any channel workspace, via a real (but
    /// instant-exit) spawned workspace — enough to exercise
    /// `channel.join`'s explicit-membership path against a channel
    /// `project.create`/`.update` bound.
    fn outside_pane(app: &mut App) -> String {
        let idx = app
            .create_workspace_with_launch_env(std::env::temp_dir(), false, Vec::new())
            .expect("outside workspace must spawn");
        let pane = app.state.workspaces[idx].tabs[0].root_pane;
        app.public_pane_id(idx, pane).unwrap()
    }

    fn seeded_message(text: &str) -> ChannelMessage {
        ChannelMessage {
            ts: "2026-01-01T00:00:00Z".into(),
            seq: 1,
            from_pane: "system".into(),
            from_name: "bora".into(),
            from_kind: ChannelSenderKind::Agent,
            text: text.into(),
            in_reply_to: None,
            to_pane: None,
            to_human: false,
        }
    }

    #[tokio::test]
    async fn create_then_list_round_trips_and_persists_to_disk() {
        let _isolated = IsolatedDirs::new("project-create-list");
        let mut app = test_app();

        let created = app.handle_project_create(
            "req".into(),
            ProjectCreateParams {
                slug: "cnb".into(),
                name: Some("CNB".into()),
                channel: Some("#cnb-room".into()),
                auto_join: None,
            },
        );
        let created: serde_json::Value = serde_json::from_str(&created).unwrap();
        assert_eq!(created["result"]["project"]["slug"], "cnb");
        assert_eq!(created["result"]["project"]["name"], "CNB");
        assert_eq!(created["result"]["project"]["channel"], "#cnb-room");
        assert_eq!(created["result"]["project"]["auto_join"], true);

        let listed = list(&mut app);
        let projects = listed["result"]["projects"].as_array().unwrap();
        assert_eq!(projects.len(), 1, "list must report what create wrote");
        assert_eq!(projects[0]["slug"], "cnb");
        assert_eq!(projects[0]["name"], "CNB");

        let on_disk = crate::persist::projects::parse_projects_yaml(
            &std::fs::read_to_string(projects_file_path()).unwrap(),
        )
        .expect("written file must parse");
        let disk_project = on_disk.projects.get("cnb").expect("project on disk");
        assert_eq!(disk_project.name.as_deref(), Some("CNB"));
        assert_eq!(disk_project.channel.as_deref(), Some("#cnb-room"));
    }

    #[tokio::test]
    async fn create_on_existing_slug_is_an_error_not_a_silent_overwrite() {
        let _isolated = IsolatedDirs::new("project-create-duplicate");
        let mut app = test_app();
        create(&mut app, "cnb");

        let duplicate = app.handle_project_create(
            "req2".into(),
            ProjectCreateParams {
                slug: "cnb".into(),
                name: Some("Overwritten".into()),
                channel: None,
                auto_join: None,
            },
        );
        let duplicate: serde_json::Value = serde_json::from_str(&duplicate).unwrap();
        assert_eq!(duplicate["error"]["code"], "project_exists");

        let listed = list(&mut app);
        let projects = listed["result"]["projects"].as_array().unwrap();
        assert_eq!(
            projects.len(),
            1,
            "the duplicate create must not have inserted a second project"
        );
        assert!(
            projects[0]["name"].is_null(),
            "the original project must be untouched, not overwritten by the rejected duplicate"
        );
    }

    #[tokio::test]
    async fn update_replaces_name_and_channel_and_persists() {
        let _isolated = IsolatedDirs::new("project-update");
        let mut app = test_app();
        create(&mut app, "cnb");

        let updated = app.handle_project_update(
            "req".into(),
            ProjectUpdateParams {
                slug: "cnb".into(),
                name: Some("CNB Landing".into()),
                channel: Some("#cnb-eng".into()),
                auto_join: None,
            },
        );
        let updated: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(updated["result"]["project"]["name"], "CNB Landing");
        assert_eq!(updated["result"]["project"]["channel"], "#cnb-eng");

        let on_disk = crate::persist::projects::parse_projects_yaml(
            &std::fs::read_to_string(projects_file_path()).unwrap(),
        )
        .unwrap();
        assert_eq!(
            on_disk.projects.get("cnb").unwrap().name.as_deref(),
            Some("CNB Landing")
        );
    }

    #[test]
    fn update_on_unknown_slug_errors_project_not_found() {
        let _isolated = IsolatedDirs::new("project-update-missing");
        let mut app = test_app();

        let response = app.handle_project_update(
            "req".into(),
            ProjectUpdateParams {
                slug: "ghost".into(),
                name: Some("nope".into()),
                channel: None,
                auto_join: None,
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], "project_not_found");
    }

    #[tokio::test]
    async fn member_add_then_list_shows_resolved_identity() {
        let _isolated = IsolatedDirs::new("project-member-add");
        let repo = std::env::temp_dir().join(format!(
            "bora-project-member-add-repo-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        let mut app = test_app();
        create(&mut app, "cnb");

        let added = app.handle_project_member_add(
            "req".into(),
            ProjectMemberAddParams {
                slug: "cnb".into(),
                dir: repo.display().to_string(),
                worktrees: WorktreesScope::All,
            },
        );
        let added: serde_json::Value = serde_json::from_str(&added).unwrap();
        let members = added["result"]["project"]["members"].as_array().unwrap();
        assert_eq!(members.len(), 1);
        assert_eq!(members[0]["resolution"]["status"], "resolved");
        assert!(
            members[0]["resolution"]["repo_identity"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "member_add must report a resolved repo identity, not raw dir echo"
        );

        let listed = list(&mut app);
        let listed_members = listed["result"]["projects"][0]["members"]
            .as_array()
            .unwrap();
        assert_eq!(listed_members.len(), 1, "list must show the added member");
        assert_eq!(listed_members[0]["resolution"]["status"], "resolved");

        let on_disk = crate::persist::projects::parse_projects_yaml(
            &std::fs::read_to_string(projects_file_path()).unwrap(),
        )
        .unwrap();
        assert_eq!(on_disk.projects.get("cnb").unwrap().members.len(), 1);

        std::fs::remove_dir_all(&repo).ok();
    }

    #[tokio::test]
    async fn member_add_on_existing_dir_updates_in_place_never_appends() {
        let _isolated = IsolatedDirs::new("project-member-add-idempotent");
        let mut app = test_app();
        create(&mut app, "cnb");

        // The two calls differ in `worktrees` on purpose. Adding the same dir
        // twice with the SAME scope cannot tell "updates the existing row"
        // apart from "does nothing", so a re-add that silently ignored a
        // changed scope would pass such a test while leaving the caller's
        // update on the floor.
        for (scope, expected) in [(WorktreesScope::All, "all"), (WorktreesScope::This, "this")] {
            let response = app.handle_project_member_add(
                "req".into(),
                ProjectMemberAddParams {
                    slug: "cnb".into(),
                    dir: "/tmp/does-not-exist-cnb".into(),
                    worktrees: scope,
                },
            );
            let response: serde_json::Value = serde_json::from_str(&response).unwrap();
            let members = response["result"]["project"]["members"].as_array().unwrap();
            assert_eq!(
                members.len(),
                1,
                "adding the same dir twice must not append a second row"
            );
            assert_eq!(
                members[0]["worktrees"], expected,
                "a re-add must apply the new worktrees scope, not ignore it"
            );
        }
    }

    #[tokio::test]
    async fn member_remove_of_absent_dir_errors_naming_what_was_not_found() {
        let _isolated = IsolatedDirs::new("project-member-remove-missing");
        let mut app = test_app();
        create(&mut app, "cnb");

        let response = app.handle_project_member_remove(
            "req".into(),
            ProjectMemberRemoveParams {
                slug: "cnb".into(),
                dir: "/tmp/never-added".into(),
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], "project_member_not_found");
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("/tmp/never-added"),
            "the error must name the dir that was not found"
        );
    }

    #[tokio::test]
    async fn member_remove_deletes_the_member_and_persists() {
        let _isolated = IsolatedDirs::new("project-member-remove");
        let mut app = test_app();
        create(&mut app, "cnb");
        app.handle_project_member_add(
            "req".into(),
            ProjectMemberAddParams {
                slug: "cnb".into(),
                dir: "/tmp/cnb-member".into(),
                worktrees: WorktreesScope::All,
            },
        );

        let removed = app.handle_project_member_remove(
            "req2".into(),
            ProjectMemberRemoveParams {
                slug: "cnb".into(),
                dir: "/tmp/cnb-member".into(),
            },
        );
        let removed: serde_json::Value = serde_json::from_str(&removed).unwrap();
        let members = removed["result"]["project"]["members"].as_array().unwrap();
        assert!(members.is_empty());

        let on_disk = crate::persist::projects::parse_projects_yaml(
            &std::fs::read_to_string(projects_file_path()).unwrap(),
        )
        .unwrap();
        assert!(on_disk.projects.get("cnb").unwrap().members.is_empty());
    }

    #[tokio::test]
    async fn write_never_leaves_a_tmp_file_and_leaves_a_fully_parseable_file() {
        let _isolated = IsolatedDirs::new("project-atomic-write");
        let mut app = test_app();
        create(&mut app, "cnb");
        app.handle_project_update(
            "req".into(),
            ProjectUpdateParams {
                slug: "cnb".into(),
                name: Some("CNB".into()),
                channel: None,
                auto_join: None,
            },
        );

        let tmp_path = projects_file_path().with_extension("yml.tmp");
        assert!(
            !tmp_path.exists(),
            "a completed write must not leave its .tmp sibling behind"
        );
        let raw = std::fs::read_to_string(projects_file_path()).unwrap();
        let parsed = crate::persist::projects::parse_projects_yaml(&raw)
            .expect("a completed write must leave a fully parseable file");
        assert_eq!(
            parsed.projects.get("cnb").unwrap().name.as_deref(),
            Some("CNB"),
            "the file on disk must reflect the update, proving the rename actually \
             landed the new content rather than abandoning it in the .tmp sibling"
        );
    }

    #[tokio::test]
    async fn create_binds_a_channel_that_join_accepts() {
        let _isolated = IsolatedDirs::new("project-create-channel-bind");
        let mut app = test_app();
        create(&mut app, "cnb");

        let pane_id = outside_pane(&mut app);
        let joined = app.handle_channel_join(
            "req".into(),
            ChannelJoinParams {
                name: "#cnb".into(),
                pane: pane_id.clone(),
                scope_write: None,
                scope_read: None,
            },
        );
        let joined: serde_json::Value = serde_json::from_str(&joined).unwrap();
        assert_eq!(
            joined["result"]["pane_id"],
            serde_json::json!(pane_id),
            "project.create must bind a channel workspace channel.join can find, got: {joined}"
        );
        assert_eq!(joined["result"]["source"], serde_json::json!("joined"));
    }

    #[tokio::test]
    async fn create_reuses_an_existing_channel_and_leaves_its_transcript_intact() {
        let _isolated = IsolatedDirs::new("project-create-channel-reuse");
        let mut app = test_app();

        // The channel exists BEFORE any project references it — e.g. an
        // orchestrator that assembled its own project at runtime (design
        // doc, "Single store, many writers"). `project.create` must find
        // and reuse this, not spin up a second workspace or wipe it.
        app.handle_channel_create("req".into(), ChannelCreateParams { name: "cnb".into() });
        assert_eq!(app.state.workspaces.len(), 1);
        crate::persist::channels::append_message("cnb", &seeded_message("pre-existing line"))
            .unwrap();

        create(&mut app, "cnb");

        assert_eq!(
            app.state.workspaces.len(),
            1,
            "reusing an existing channel must not spin up a second channel workspace"
        );
        let tail = crate::persist::channels::read_tail("cnb", 10).unwrap();
        assert!(
            tail.iter().any(|m| m.text == "pre-existing line"),
            "reusing an existing channel must leave its transcript intact, got: {tail:?}"
        );
    }

    #[tokio::test]
    async fn update_rebinds_the_channel_without_touching_the_old_one() {
        let _isolated = IsolatedDirs::new("project-update-channel-rebind");
        let mut app = test_app();
        app.handle_project_create(
            "req".into(),
            ProjectCreateParams {
                slug: "cnb".into(),
                name: None,
                channel: Some("#cnb-old".into()),
                auto_join: None,
            },
        );
        crate::persist::channels::append_message("cnb-old", &seeded_message("old line")).unwrap();

        app.handle_project_update(
            "req2".into(),
            ProjectUpdateParams {
                slug: "cnb".into(),
                name: None,
                channel: Some("#cnb-new".into()),
                auto_join: None,
            },
        );

        let old_tail = crate::persist::channels::read_tail("cnb-old", 10).unwrap();
        assert!(
            old_tail.iter().any(|m| m.text == "old line"),
            "re-binding to a new channel must never touch the old one's transcript"
        );
        assert_eq!(
            app.state.workspaces.len(),
            2,
            "re-bind creates the new channel workspace alongside the untouched old one"
        );

        let pane_id = outside_pane(&mut app);
        let joined = app.handle_channel_join(
            "req3".into(),
            ChannelJoinParams {
                name: "#cnb-new".into(),
                pane: pane_id,
                scope_write: None,
                scope_read: None,
            },
        );
        let joined: serde_json::Value = serde_json::from_str(&joined).unwrap();
        assert!(
            joined.get("error").is_none(),
            "the newly re-bound channel must be joinable, got: {joined}"
        );
    }

    #[tokio::test]
    async fn auto_join_flag_defaults_true_and_is_persisted_when_set_false() {
        let _isolated = IsolatedDirs::new("project-auto-join-flag");
        let mut app = test_app();

        let created = create(&mut app, "cnb");
        assert_eq!(
            created["result"]["project"]["auto_join"], true,
            "auto_join must default to true and be reported on create"
        );

        let opted_out = app.handle_project_create(
            "req2".into(),
            ProjectCreateParams {
                slug: "quiet".into(),
                name: None,
                channel: None,
                auto_join: Some(false),
            },
        );
        let opted_out: serde_json::Value = serde_json::from_str(&opted_out).unwrap();
        assert_eq!(opted_out["result"]["project"]["auto_join"], false);

        let on_disk = crate::persist::projects::parse_projects_yaml(
            &std::fs::read_to_string(projects_file_path()).unwrap(),
        )
        .unwrap();
        assert!(
            !on_disk.projects.get("quiet").unwrap().auto_join,
            "auto_join: false must persist to disk"
        );

        let updated = app.handle_project_update(
            "req3".into(),
            ProjectUpdateParams {
                slug: "cnb".into(),
                name: None,
                channel: None,
                auto_join: Some(false),
            },
        );
        let updated: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(
            updated["result"]["project"]["auto_join"], false,
            "project.update must be able to flip auto_join off"
        );
    }
}
