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
    EmptyParams, ProjectCreateParams, ProjectMemberAddParams, ProjectMemberInfo,
    ProjectMemberRemoveParams, ProjectMemberResolution, ProjectSummary, ProjectUpdateParams,
    ResponseResult,
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
    use crate::config::{Config, IsolatedConfigDir};
    use crate::persist::projects::{projects_file_path, WorktreesScope};

    fn test_app() -> App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        )
    }

    fn create(app: &mut App, slug: &str) -> serde_json::Value {
        let response = app.handle_project_create(
            "req".into(),
            ProjectCreateParams {
                slug: slug.into(),
                name: None,
                channel: None,
            },
        );
        serde_json::from_str(&response).unwrap()
    }

    fn list(app: &mut App) -> serde_json::Value {
        let response = app.handle_project_list("req".into(), EmptyParams {});
        serde_json::from_str(&response).unwrap()
    }

    #[test]
    fn create_then_list_round_trips_and_persists_to_disk() {
        let _isolated = IsolatedConfigDir::new("project-create-list");
        let mut app = test_app();

        let created = app.handle_project_create(
            "req".into(),
            ProjectCreateParams {
                slug: "cnb".into(),
                name: Some("CNB".into()),
                channel: Some("#cnb-room".into()),
            },
        );
        let created: serde_json::Value = serde_json::from_str(&created).unwrap();
        assert_eq!(created["result"]["project"]["slug"], "cnb");
        assert_eq!(created["result"]["project"]["name"], "CNB");
        assert_eq!(created["result"]["project"]["channel"], "#cnb-room");

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

    #[test]
    fn create_on_existing_slug_is_an_error_not_a_silent_overwrite() {
        let _isolated = IsolatedConfigDir::new("project-create-duplicate");
        let mut app = test_app();
        create(&mut app, "cnb");

        let duplicate = app.handle_project_create(
            "req2".into(),
            ProjectCreateParams {
                slug: "cnb".into(),
                name: Some("Overwritten".into()),
                channel: None,
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

    #[test]
    fn update_replaces_name_and_channel_and_persists() {
        let _isolated = IsolatedConfigDir::new("project-update");
        let mut app = test_app();
        create(&mut app, "cnb");

        let updated = app.handle_project_update(
            "req".into(),
            ProjectUpdateParams {
                slug: "cnb".into(),
                name: Some("CNB Landing".into()),
                channel: Some("#cnb-eng".into()),
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
        let _isolated = IsolatedConfigDir::new("project-update-missing");
        let mut app = test_app();

        let response = app.handle_project_update(
            "req".into(),
            ProjectUpdateParams {
                slug: "ghost".into(),
                name: Some("nope".into()),
                channel: None,
            },
        );
        let response: serde_json::Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["error"]["code"], "project_not_found");
    }

    #[test]
    fn member_add_then_list_shows_resolved_identity() {
        let _isolated = IsolatedConfigDir::new("project-member-add");
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

    #[test]
    fn member_add_on_existing_dir_updates_in_place_never_appends() {
        let _isolated = IsolatedConfigDir::new("project-member-add-idempotent");
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

    #[test]
    fn member_remove_of_absent_dir_errors_naming_what_was_not_found() {
        let _isolated = IsolatedConfigDir::new("project-member-remove-missing");
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

    #[test]
    fn member_remove_deletes_the_member_and_persists() {
        let _isolated = IsolatedConfigDir::new("project-member-remove");
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

    #[test]
    fn write_never_leaves_a_tmp_file_and_leaves_a_fully_parseable_file() {
        let _isolated = IsolatedConfigDir::new("project-atomic-write");
        let mut app = test_app();
        create(&mut app, "cnb");
        app.handle_project_update(
            "req".into(),
            ProjectUpdateParams {
                slug: "cnb".into(),
                name: Some("CNB".into()),
                channel: None,
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
}
