//! `bora worktree create` as the whole dispatch chain: repo from the caller's
//! shell, sidebar folder from the caller's pane, agent on the new pane.

use super::harness::*;

use std::os::unix::fs::PermissionsExt;

/// A shell that immediately becomes an agent, the way a pane whose environment
/// launches one behaves: `agent.start` on such a pane answers `agent_pane_busy`.
fn write_agent_shell(base: &Path) -> (PathBuf, PathBuf) {
    let bin = base.join("bin");
    fs::create_dir_all(&bin).unwrap();
    // The server shells out to git; the overridden PATH has to keep it.
    let git = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    std::os::unix::fs::symlink(git.trim(), bin.join("git")).unwrap();
    let prompts = base.join("prompts");
    let fake_pi = bin.join("pi");
    fs::write(
        &fake_pi,
        format!(
            "#!/bin/sh\nexport HERDR_AGENT=pi\n'{0}' pane report-agent \"$HERDR_PANE_ID\" --source custom:fake-pi --agent pi --state idle >/dev/null\nwhile IFS= read -r prompt; do\n  printf '%s\\n' \"$prompt\" >> '{1}'\ndone\n",
            env!("CARGO_BIN_EXE_bora"),
            prompts.display(),
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755)).unwrap();
    (bin, prompts)
}

#[test]
fn worktree_create_resolves_the_repo_from_the_callers_own_directory() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let repo = base.join("repo");
    create_committed_repo(&repo);

    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    // The focused workspace is not in this repo — it is not in a repo at all.
    // Without the caller's cwd the server answers about that workspace.
    let elsewhere = base.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    run_cli_json(
        &socket_path,
        &["workspace", "create", "--cwd", elsewhere.to_str().unwrap()],
    );

    let created = run_cli_json_in_dir(
        &socket_path,
        &["worktree", "create", "--branch", "feature/from-cwd"],
        &repo,
    );
    assert_eq!(created["result"]["ok"], true);
    assert_eq!(created["result"]["branch"], "feature/from-cwd");
    assert_eq!(created["result"]["branch_source"], "new");
    // Label defaults to the branch slug so `workspace list` stays searchable
    // by branch, which is the only place a branch shows up there.
    assert_eq!(created["result"]["label"], "feature-from-cwd");
    let first_checkout = created["result"]["path"].as_str().unwrap().to_string();

    // Dispatching from INSIDE a linked worktree resolves the parent repo, not
    // the worktree: `git worktree add` only works from a repo.
    let nested = run_cli_json_in_dir(
        &socket_path,
        &["worktree", "create", "--branch", "feature/from-worktree"],
        Path::new(&first_checkout),
    );
    assert_eq!(nested["result"]["ok"], true);
    assert_eq!(nested["result"]["branch"], "feature/from-worktree");

    let worktrees = run_cli_json(
        &socket_path,
        &["worktree", "list", "--cwd", repo.to_str().unwrap()],
    );
    let branches = worktrees["result"]["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| entry["branch"].as_str())
        .collect::<Vec<_>>();
    assert!(branches.contains(&"feature/from-cwd"));
    assert!(branches.contains(&"feature/from-worktree"));

    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn worktree_create_re_run_reuses_the_workspace_instead_of_duplicating_it() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let repo = base.join("repo");
    create_committed_repo(&repo);

    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let first = run_cli_json_in_dir(
        &socket_path,
        &["worktree", "create", "--branch", "feature/twice"],
        &repo,
    );
    assert_eq!(first["result"]["already_open"], false);

    let second = run_cli_json_in_dir(
        &socket_path,
        &["worktree", "create", "--branch", "feature/twice"],
        &repo,
    );
    assert_eq!(second["result"]["ok"], true);
    assert_eq!(second["result"]["already_open"], true);
    assert_eq!(second["result"]["branch_source"], "existing");
    assert_eq!(
        second["result"]["workspace_id"],
        first["result"]["workspace_id"]
    );

    let workspaces = run_cli_json(&socket_path, &["workspace", "list"]);
    let matching = workspaces["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|workspace| {
            workspace["worktree"]["checkout_path"].as_str() == first["result"]["path"].as_str()
        })
        .count();
    assert_eq!(matching, 1, "a re-run must not open a second workspace");

    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn worktree_create_lands_in_the_calling_panes_sidebar_folder() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let repo = base.join("repo");
    create_committed_repo(&repo);

    let herdr = spawn_herdr(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let caller = run_cli_json(
        &socket_path,
        &[
            "workspace",
            "create",
            "--cwd",
            repo.to_str().unwrap(),
            "--group",
            "fleet",
        ],
    );
    let caller_pane = caller["result"]["root_pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();

    let created = run_cli_json_in_dir_with_env(
        &socket_path,
        &["worktree", "create", "--branch", "feature/grouped"],
        &repo,
        &[("HERDR_PANE_ID", caller_pane.as_str())],
    );
    assert_eq!(created["result"]["group"], "fleet");
    let workspace_id = created["result"]["workspace_id"].as_str().unwrap();
    let workspaces = run_cli_json(&socket_path, &["workspace", "list"]);
    let placed = workspaces["result"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|workspace| workspace["workspace_id"].as_str() == Some(workspace_id))
        .unwrap();
    assert_eq!(placed["visual_group"], "fleet");

    // Opting out is explicit, and an unset pane inherits nothing.
    let ungrouped = run_cli_json_in_dir_with_env(
        &socket_path,
        &[
            "worktree",
            "create",
            "--branch",
            "feature/ungrouped",
            "--no-group",
        ],
        &repo,
        &[("HERDR_PANE_ID", caller_pane.as_str())],
    );
    assert!(ungrouped["result"]["group"].is_null());

    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn worktree_create_renames_the_agent_a_busy_pane_already_runs_and_prompts_it() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let repo = base.join("repo");
    create_committed_repo(&repo);
    let (bin, prompts) = write_agent_shell(&base);
    let config = format!(
        "onboarding = false\n[terminal]\ndefault_shell = {:?}\nshell_mode = \"non_login\"\n",
        bin.join("pi").to_str().unwrap()
    );

    let herdr = spawn_herdr_with_config(
        &config_home,
        &runtime_dir,
        &socket_path,
        Some(&bin),
        &config,
    );
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let spec = base.join("SPEC.md");
    fs::write(&spec, "do the thing\n").unwrap();

    let created = run_cli_json_in_dir(
        &socket_path,
        &[
            "worktree",
            "create",
            "--branch",
            "feature/agent",
            "--agent",
            "pi",
            "--agent-name",
            "dispatched",
            "--prompt-file",
            spec.to_str().unwrap(),
        ],
        &repo,
    );

    assert_eq!(created["result"]["ok"], true);
    assert_eq!(created["result"]["agent"]["name"], "dispatched");
    assert_eq!(
        created["result"]["agent"]["started"], false,
        "a pane that already runs an agent is renamed, never reported as busy"
    );
    assert_eq!(created["result"]["prompt_sent"], true);

    // The name is the handle: the agent is addressable by it afterwards.
    let resolved = run_cli_json(&socket_path, &["agent", "get", "dispatched"]);
    assert_eq!(
        resolved["result"]["agent"]["pane_id"],
        created["result"]["pane_id"]
    );

    // `--prompt-file` delivers a pointer to the absolute path, not the text,
    // so the spec survives the agent compacting its own context.
    let deadline = Instant::now() + Duration::from_secs(5);
    let delivered = loop {
        if let Ok(contents) = fs::read_to_string(&prompts) {
            if contents.contains(spec.to_str().unwrap()) {
                break contents;
            }
        }
        assert!(Instant::now() < deadline, "prompt never reached the agent");
        thread::sleep(Duration::from_millis(100));
    };
    assert!(delivered.contains("Read and follow the spec at"));

    cleanup_spawned_herdr(herdr, base);
}

#[test]
fn worktree_create_starts_an_agent_on_a_pane_that_runs_none() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("herdr.sock");
    let repo = base.join("repo");
    create_committed_repo(&repo);
    let (bin, prompts) = write_agent_shell(&base);
    // Plain shell: nothing claims the pane, so the agent has to be started.
    let config =
        "onboarding = false\n[terminal]\ndefault_shell = \"/bin/sh\"\nshell_mode = \"non_login\"\n";

    let herdr =
        spawn_herdr_with_config(&config_home, &runtime_dir, &socket_path, Some(&bin), config);
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let created = run_cli_json_in_dir(
        &socket_path,
        &[
            "worktree",
            "create",
            "--branch",
            "feature/started",
            "--agent",
            "pi",
            "--agent-name",
            "fresh",
            "--prompt",
            "ship it",
        ],
        &repo,
    );

    assert_eq!(created["result"]["ok"], true);
    assert_eq!(created["result"]["agent"]["name"], "fresh");
    assert_eq!(created["result"]["agent"]["started"], true);
    assert_eq!(created["result"]["prompt_sent"], true);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if fs::read_to_string(&prompts).is_ok_and(|contents| contents.contains("ship it")) {
            break;
        }
        assert!(Instant::now() < deadline, "prompt never reached the agent");
        thread::sleep(Duration::from_millis(100));
    }

    cleanup_spawned_herdr(herdr, base);
}
