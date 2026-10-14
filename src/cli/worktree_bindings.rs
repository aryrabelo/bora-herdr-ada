use clap::{Arg, ArgAction, ArgMatches, Command};

use crate::api::schema::worktrees::{
    WorktreeCreateProjection, WorktreeListProjection, WorktreeOpenProjection,
    WorktreeRemoveProjection,
};
use crate::api::schema::{Method, Request, WorktreeCreateParams};

use super::super::projection::{self, Input, Leaf, ParseError};

pub(crate) fn pr() -> Input<Option<u64>> {
    Input::value("pr", "NUMBER", |slot, value| {
        let number = value
            .parse::<u64>()
            .map_err(|_| ParseError::Usage(format!("invalid value for --pr: {value}")))?;
        *slot = Some(number);
        Ok(())
    })
    .configure(|arg| arg.conflicts_with_all(["branch", "base"]))
}

pub(crate) fn path(name: &'static str) -> Input<Option<String>> {
    Input::value(name, "PATH", |slot, value| {
        *slot = Some(super::normalize_path_arg(value)?);
        Ok(())
    })
    .path()
}

const LEAVES: [Leaf; 4] = [
    Leaf {
        name: "list",
        about: "List worktree workspaces",
        id: "cli:worktree:list",
        arguments: WorktreeListProjection::arguments,
        method: |matches| WorktreeListProjection::from_matches(matches).map(Method::WorktreeList),
    },
    Leaf {
        name: "create",
        about: "Create and open a Git worktree, optionally with an agent and a prompt",
        id: "cli:worktree:create",
        arguments: create_arguments,
        method: |matches| {
            WorktreeCreateProjection::from_matches(matches).map(Method::WorktreeCreate)
        },
    },
    Leaf {
        name: "open",
        about: "Open an existing Git worktree",
        id: "cli:worktree:open",
        arguments: WorktreeOpenProjection::arguments,
        method: |matches| WorktreeOpenProjection::from_matches(matches).map(Method::WorktreeOpen),
    },
    Leaf {
        name: "remove",
        about: "Remove a worktree checkout",
        id: "cli:worktree:remove",
        arguments: WorktreeRemoveProjection::arguments,
        method: |matches| {
            WorktreeRemoveProjection::from_matches(matches).map(Method::WorktreeRemove)
        },
    },
];

/// What `worktree create` does after the checkout exists. These flags shape
/// the CLI's follow-up requests (folder, agent, prompt) and never reach the
/// `worktree.create` wire params.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CreateFollowUp {
    pub(crate) group: Option<String>,
    pub(crate) no_group: bool,
    pub(crate) agent_kind: Option<String>,
    pub(crate) agent_name: Option<String>,
    pub(crate) prompt: Option<String>,
    pub(crate) prompt_file: Option<String>,
}

fn follow_up_value(name: &'static str, value_name: &'static str, help: &'static str) -> Arg {
    Arg::new(name)
        .long(name)
        .value_name(value_name)
        .action(ArgAction::Set)
        .allow_hyphen_values(true)
        .help(help)
}

fn follow_up_arguments() -> Vec<Arg> {
    vec![
        follow_up_value(
            "group",
            "NAME",
            "Sidebar folder for the new workspace (default: the caller pane's folder)",
        )
        .conflicts_with("no-group"),
        Arg::new("no-group")
            .long("no-group")
            .action(ArgAction::SetTrue)
            .help("Leave the new workspace ungrouped"),
        follow_up_value(
            "agent",
            "KIND",
            "Agent to run on the new pane (default: [agents] default)",
        ),
        follow_up_value(
            "agent-name",
            "NAME",
            "Agent name (default: the workspace label)",
        ),
        follow_up_value("prompt", "TEXT", "Prompt to send the agent").conflicts_with("prompt-file"),
        follow_up_value(
            "prompt-file",
            "PATH",
            "Spec file the agent is told to read and follow",
        ),
    ]
}

fn create_arguments() -> Vec<Arg> {
    let mut arguments = WorktreeCreateProjection::arguments();
    arguments.extend(follow_up_arguments());
    arguments
}

impl CreateFollowUp {
    fn from_matches(matches: &ArgMatches) -> Self {
        let text = |id: &str| matches.get_one::<String>(id).cloned();
        Self {
            group: text("group"),
            no_group: matches.get_flag("no-group"),
            agent_kind: text("agent"),
            agent_name: text("agent-name"),
            prompt: text("prompt"),
            prompt_file: text("prompt-file"),
        }
    }
}

/// `worktree create` argv, split into the wire params and the follow-up.
pub(crate) fn create_request(
    args: &[String],
) -> Result<(WorktreeCreateParams, CreateFollowUp), ParseError> {
    let create = command()
        .find_subcommand("create")
        .cloned()
        .expect("worktree command declares create");
    projection::parse(create.bin_name("bora worktree create"), args, |matches| {
        Ok((
            WorktreeCreateProjection::from_matches(matches)?,
            CreateFollowUp::from_matches(matches),
        ))
    })
}

pub(crate) fn command() -> Command {
    Command::new("worktree")
        .about("Manage Git worktree-backed workspaces")
        .subcommand_required(true)
        .subcommands(LEAVES.iter().map(Leaf::command))
        .mut_subcommand("open", |open| {
            open.group(
                clap::ArgGroup::new("open_target")
                    .args(["path", "branch"])
                    .required(true),
            )
        })
}

pub(crate) fn request(args: &[String]) -> Result<Request, ParseError> {
    projection::group_request(command(), &LEAVES, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Request, ParseError> {
        request(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
    }

    fn create(args: &[&str]) -> Result<(WorktreeCreateParams, CreateFollowUp), ParseError> {
        create_request(&args.iter().map(|arg| (*arg).to_owned()).collect::<Vec<_>>())
    }

    #[test]
    fn worktree_create_splits_wire_params_from_follow_up_flags() {
        let (params, follow_up) = create(&[
            "--pr",
            "42",
            "--group",
            "team/docs",
            "--agent",
            "codex",
            "--agent-name",
            "doc-writer",
            "--prompt",
            "--start here",
        ])
        .unwrap();
        assert_eq!(
            serde_json::to_value(params).unwrap(),
            serde_json::json!({"pr": 42, "focus": false})
        );
        assert_eq!(
            follow_up,
            CreateFollowUp {
                group: Some("team/docs".into()),
                no_group: false,
                agent_kind: Some("codex".into()),
                agent_name: Some("doc-writer".into()),
                prompt: Some("--start here".into()),
                prompt_file: None,
            }
        );
    }

    #[test]
    fn worktree_create_rejects_conflicting_follow_up_flags_before_transport() {
        for args in [
            vec!["--pr", "1", "--branch", "topic"],
            vec!["--pr", "1", "--base", "HEAD"],
            vec!["--prompt", "x", "--prompt-file", "spec.md"],
            vec!["--group", "a", "--no-group"],
        ] {
            assert!(
                matches!(create(&args), Err(ParseError::Clap(error)) if error.exit_code() == 2),
                "{args:?}"
            );
        }
        assert!(
            matches!(create(&["--pr", "abc"]), Err(ParseError::Usage(message)) if message == "invalid value for --pr: abc")
        );
    }

    #[test]
    fn worktree_projection_preserves_literal_requests_and_hidden_json() {
        for (args, expected) in [
            (
                vec!["list", "--json"],
                serde_json::json!({"id":"cli:worktree:list","method":"worktree.list","params":{}}),
            ),
            (
                vec!["create", "--json"],
                serde_json::json!({"id":"cli:worktree:create","method":"worktree.create","params":{"focus":false}}),
            ),
            (
                vec![
                    "create",
                    "--workspace",
                    "2",
                    "--branch",
                    "topic",
                    "--base",
                    "HEAD",
                    "--label",
                    "name",
                    "--focus",
                    "--no-focus",
                    "--focus",
                    "--trust-repository",
                    "--json",
                ],
                serde_json::json!({"id":"cli:worktree:create","method":"worktree.create","params":{"workspace_id":"2","branch":"topic","base":"HEAD","label":"name","focus":true,"trust_repository":true}}),
            ),
            (
                vec!["open", "--branch", "topic", "--json"],
                serde_json::json!({"id":"cli:worktree:open","method":"worktree.open","params":{"branch":"topic","focus":false}}),
            ),
            (
                vec!["remove", "--workspace=2", "--json"],
                serde_json::json!({"id":"cli:worktree:remove","method":"worktree.remove","params":{"workspace_id":"2","force":false}}),
            ),
            (
                vec![
                    "remove",
                    "--workspace",
                    "2",
                    "--force",
                    "--trust-repository",
                    "--json",
                ],
                serde_json::json!({"id":"cli:worktree:remove","method":"worktree.remove","params":{"workspace_id":"2","force":true,"trust_repository":true}}),
            ),
        ] {
            assert_eq!(
                serde_json::to_value(parse(&args).unwrap()).unwrap(),
                expected
            );
        }
        for leaf in command().get_subcommands() {
            assert!(leaf
                .get_arguments()
                .any(|arg| arg.get_long() == Some("json") && arg.is_hide_set()));
            let mut leaf = leaf.clone();
            assert!(!leaf.render_long_help().to_string().contains("--json"));
        }
        command().debug_assert();
    }

    #[test]
    fn worktree_projection_normalizes_only_local_paths() {
        let expected = std::env::current_dir()
            .unwrap()
            .join("relative")
            .display()
            .to_string();
        assert_eq!(
            serde_json::to_value(parse(&["open", "--path", "relative"]).unwrap()).unwrap(),
            serde_json::json!({
                "id":"cli:worktree:open", "method":"worktree.open", "params":{"path":expected, "focus":false}
            })
        );
        crate::cli::target::with_test_client(crate::api::client::ApiClient::local(), || {
            assert_eq!(
                serde_json::to_value(
                    parse(&["create", "--cwd", "~/repo", "--path", "/remote/new"]).unwrap()
                )
                .unwrap(),
                serde_json::json!({
                    "id":"cli:worktree:create", "method":"worktree.create", "params":{"cwd":"~/repo", "path":"/remote/new", "focus":false}
                })
            );
            assert!(
                matches!(parse(&["open", "--path", "relative"]), Err(ParseError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidInput && error.to_string() == "remote worktree paths must be absolute or start with ~/")
            );
        });
    }

    #[test]
    fn worktree_projection_keeps_context_and_xor_validations() {
        for args in [
            vec!["list", "--workspace", "2", "--cwd", "."],
            vec!["create", "--workspace", "2", "--cwd", "."],
            vec!["open"],
            vec!["open", "--path", ".", "--branch", "topic"],
        ] {
            assert!(
                matches!(parse(&args), Err(ParseError::Clap(error)) if error.exit_code() == 2),
                "{args:?}"
            );
        }
        assert!(
            matches!(parse(&["remove"]), Err(ParseError::Clap(error)) if error.exit_code() == 2)
        );
        assert!(
            matches!(parse(&["open", "--help"]), Err(ParseError::Clap(error)) if error.exit_code() == 0)
        );
    }
}
