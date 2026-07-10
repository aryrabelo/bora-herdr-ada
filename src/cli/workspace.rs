use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::api::schema::{
    Method, Request, WorkspaceCreateParams, WorkspaceRenameParams, WorkspaceReportMetadataParams,
    WorkspaceSetGroupParams,
};

pub(super) fn run_workspace_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(std::string::String::as_str) else {
        print_workspace_help();
        return Ok(2);
    };

    match subcommand {
        "list" => workspace_list(&args[1..]),
        "create" => workspace_create(&args[1..]),
        "get" => workspace_get(&args[1..]),
        "focus" => workspace_focus(&args[1..]),
        "rename" => workspace_rename(&args[1..]),
        "report-metadata" => workspace_report_metadata(&args[1..]),
        "set-group" => workspace_set_group(&args[1..]),
        "close" => workspace_close(&args[1..]),
        "run" => workspace_run(&args[1..]),
        "help" | "--help" | "-h" => {
            print_workspace_help();
            Ok(0)
        }
        _ => {
            print_workspace_help();
            Ok(2)
        }
    }
}

fn workspace_list(args: &[String]) -> std::io::Result<i32> {
    if !args.is_empty() {
        eprintln!("usage: bora workspace list");
        return Ok(2);
    }

    super::runtime::workspace_list()
}

fn workspace_create(args: &[String]) -> std::io::Result<i32> {
    let mut cwd = None;
    let mut focus = false;
    let mut label = None;
    let mut env = HashMap::new();
    let mut group = None;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --cwd");
                    return Ok(2);
                };
                cwd = Some(value.clone());
                index += 2;
            }
            "--label" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --label");
                    return Ok(2);
                };
                label = Some(value.clone());
                index += 2;
            }
            "--group" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --group");
                    return Ok(2);
                };
                group = Some(value.clone());
                index += 2;
            }
            "--focus" => {
                focus = true;
                index += 1;
            }
            "--no-focus" => {
                focus = false;
                index += 1;
            }
            "--env" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --env");
                    return Ok(2);
                };
                let (key, value) = match super::parse_env_assignment(value) {
                    Ok(pair) => pair,
                    Err(err) => {
                        eprintln!("{err}");
                        return Ok(2);
                    }
                };
                env.insert(key, value);
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    super::runtime::workspace_create(WorkspaceCreateParams {
        cwd,
        focus,
        label,
        group,
        env,
    })
}

fn workspace_get(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_workspace_id) = args.first() else {
        eprintln!("usage: bora workspace get <workspace_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: bora workspace get <workspace_id>");
        return Ok(2);
    }

    super::runtime::workspace_get(super::normalize_workspace_id(raw_workspace_id))
}

fn workspace_focus(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_workspace_id) = args.first() else {
        eprintln!("usage: bora workspace focus <workspace_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: bora workspace focus <workspace_id>");
        return Ok(2);
    }

    super::runtime::workspace_focus(super::normalize_workspace_id(raw_workspace_id))
}

fn workspace_rename(args: &[String]) -> std::io::Result<i32> {
    if args.len() < 2 {
        eprintln!("usage: bora workspace rename <workspace_id> <label>");
        return Ok(2);
    }

    super::runtime::workspace_rename(WorkspaceRenameParams {
        workspace_id: super::normalize_workspace_id(&args[0]),
        label: args[1..].join(" "),
    })
}

fn workspace_report_metadata(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_workspace_id) = args.first() else {
        eprintln!("usage: herdr workspace report-metadata <workspace_id> --source ID [--token NAME=VALUE] [--clear-token NAME] [--seq N] [--ttl-ms N]");
        return Ok(2);
    };
    let workspace_id = super::normalize_workspace_id(raw_workspace_id);
    let mut source = None;
    let mut tokens = HashMap::new();
    let mut seq = None;
    let mut ttl_ms = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--source" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --source");
                    return Ok(2);
                };
                source = Some(value.clone());
                index += 2;
            }
            "--token" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --token");
                    return Ok(2);
                };
                let (key, value) = match super::parse_token_assignment(value) {
                    Ok(token) => token,
                    Err(message) => {
                        eprintln!("{message}");
                        return Ok(2);
                    }
                };
                tokens.insert(key, value);
                index += 2;
            }
            "--clear-token" => {
                let Some(key) = args.get(index + 1) else {
                    eprintln!("missing value for --clear-token");
                    return Ok(2);
                };
                tokens.insert(key.clone(), None);
                index += 2;
            }
            "--seq" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --seq");
                    return Ok(2);
                };
                seq = Some(super::parse_u64_flag("--seq", value)?);
                index += 2;
            }
            "--ttl-ms" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --ttl-ms");
                    return Ok(2);
                };
                ttl_ms = Some(super::parse_u64_flag("--ttl-ms", value)?);
                index += 2;
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }
    let Some(source) = source.filter(|source| !source.trim().is_empty()) else {
        eprintln!("missing required --source");
        return Ok(2);
    };
    if tokens.is_empty() {
        eprintln!("missing token to set or clear");
        return Ok(2);
    }
    super::send_ok_request(Method::WorkspaceReportMetadata(
        WorkspaceReportMetadataParams {
            workspace_id,
            source,
            tokens,
            seq,
            ttl_ms,
        },
    ))
}

fn workspace_set_group(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_workspace_id) = args.first() else {
        eprintln!("usage: bora workspace set-group <workspace_id> [name]");
        return Ok(2);
    };
    let group = if args.len() > 1 {
        let name = args[1..].join(" ");
        let trimmed = name.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    } else {
        None
    };

    super::print_response(&super::send_request(&Request {
        id: "cli:workspace:set-group".into(),
        method: Method::WorkspaceSetGroup(WorkspaceSetGroupParams {
            workspace_id: super::normalize_workspace_id(raw_workspace_id),
            group,
        }),
    })?)
}

fn workspace_close(args: &[String]) -> std::io::Result<i32> {
    let Some(raw_workspace_id) = args.first() else {
        eprintln!("usage: bora workspace close <workspace_id>");
        return Ok(2);
    };
    if args.len() != 1 {
        eprintln!("usage: bora workspace close <workspace_id>");
        return Ok(2);
    }

    super::runtime::workspace_close(super::normalize_workspace_id(raw_workspace_id))
}

fn print_workspace_help() {
    eprintln!("bora workspace commands:");
    eprintln!("  bora workspace list");
    eprintln!("  bora workspace create [--cwd PATH] [--label TEXT] [--group NAME] [--env KEY=VALUE] [--focus] [--no-focus]");
    eprintln!("  bora workspace get <workspace_id>");
    eprintln!("  bora workspace focus <workspace_id>");
    eprintln!("  bora workspace rename <workspace_id> <label>");
    eprintln!("  bora workspace report-metadata <workspace_id> --source ID [--token NAME=VALUE] [--clear-token NAME] [--seq N] [--ttl-ms N]");
    eprintln!("  bora workspace set-group <workspace_id> [name]   (omit name to ungroup)");
    eprintln!("  bora workspace close <workspace_id>");
    eprintln!("  bora workspace run [--cwd PATH]   (execute .bora/settings.toml run script)");
}

fn workspace_run(args: &[String]) -> std::io::Result<i32> {
    let mut cwd_arg: Option<String> = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--cwd" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --cwd");
                    return Ok(2);
                };
                cwd_arg = Some(value.clone());
                index += 2;
            }
            "help" | "--help" | "-h" => {
                eprintln!("usage: bora workspace run [--cwd PATH]");
                return Ok(0);
            }
            other => {
                eprintln!("unknown option: {other}");
                return Ok(2);
            }
        }
    }

    let cwd = cwd_arg.map_or_else(
        || std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        PathBuf::from,
    );

    let Some(checkout_str) = git_output(&cwd, &["rev-parse", "--show-toplevel"]) else {
        eprintln!("not a git repository (or git failed): {}", cwd.display());
        return Ok(1);
    };
    let checkout = PathBuf::from(checkout_str);

    let Some(common_dir_str) = git_output(
        &cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) else {
        eprintln!("failed to resolve git common dir");
        return Ok(1);
    };
    let common_dir = PathBuf::from(common_dir_str);
    let main_root = main_root_from_common_dir(&common_dir);

    let Some(branch) = git_output(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"]) else {
        eprintln!("failed to resolve branch");
        return Ok(1);
    };

    let Some(settings) = crate::bora_settings::load_bora_settings(&main_root) else {
        eprintln!("no run script configured in .bora/settings.toml");
        return Ok(1);
    };
    let Some(run_script) = settings.scripts.run.clone() else {
        eprintln!("no run script configured in .bora/settings.toml");
        return Ok(1);
    };

    let env = crate::bora_settings::workspace_env(
        &main_root,
        &checkout,
        Some(branch.as_str()),
        &settings,
    );

    let pidfile = if settings.scripts.run_mode == crate::bora_settings::BoraRunMode::Exclusive {
        Some(common_dir.join("info").join("bora-run.pid"))
    } else {
        None
    };

    // Exclusive mode: stop any previously-recorded run before starting ours.
    if let Some(pidfile) = pidfile.as_deref() {
        stop_previous_run(pidfile);
    }

    let mut proc = Command::new("/bin/sh");
    proc.arg("-lc")
        .arg(&run_script)
        .current_dir(&checkout)
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    for (key, value) in &env {
        proc.env(key, value);
    }

    let mut child = proc.spawn()?;

    if let Some(pidfile) = pidfile.as_deref() {
        if let Some(parent) = pidfile.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(err) = std::fs::write(pidfile, child.id().to_string()) {
            tracing::warn!(pidfile = %pidfile.display(), %err, "failed to write bora-run pidfile");
        }
    }

    let status = child.wait()?;

    if let Some(pidfile) = pidfile.as_deref() {
        let _ = std::fs::remove_file(pidfile);
    }

    Ok(status.code().unwrap_or(1))
}

fn git_output(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Derive the main repo checkout from an absolute git common-dir path.
///
/// The common dir is `<main_root>/.git` for both the primary checkout and any
/// linked worktree, so its parent is the main root. A common dir that does not
/// end in `.git` (bare repo or unusual layout) is returned unchanged.
fn main_root_from_common_dir(common_dir: &Path) -> PathBuf {
    if common_dir.file_name().and_then(std::ffi::OsStr::to_str) == Some(".git") {
        common_dir
            .parent()
            .map_or_else(|| common_dir.to_path_buf(), Path::to_path_buf)
    } else {
        common_dir.to_path_buf()
    }
}

/// Exclusive run_mode: SIGTERM a previously-recorded live run, then clear the pidfile.
fn stop_previous_run(pidfile: &Path) {
    let Ok(contents) = std::fs::read_to_string(pidfile) else {
        return;
    };
    let Ok(pid) = contents.trim().parse::<i32>() else {
        let _ = std::fs::remove_file(pidfile);
        return;
    };
    if !pid_is_alive(pid) {
        let _ = std::fs::remove_file(pidfile);
        return;
    }
    let _ = Command::new("kill").arg(pid.to_string()).status();
    eprintln!("stopped previous exclusive run (pid {pid})");
    let _ = std::fs::remove_file(pidfile);
}

#[cfg(unix)]
fn pid_is_alive(pid: i32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn pid_is_alive(_pid: i32) -> bool {
    // No portable liveness check off unix; assume alive and attempt SIGTERM.
    true
}

#[cfg(test)]
mod tests {
    use super::main_root_from_common_dir;
    use std::path::{Path, PathBuf};

    #[test]
    fn workspace_run_main_root_strips_git_leaf() {
        assert_eq!(
            main_root_from_common_dir(Path::new("/repo/.git")),
            PathBuf::from("/repo")
        );
    }

    #[test]
    fn workspace_run_main_root_from_linked_worktree_common_dir() {
        // A linked worktree's common dir points at the MAIN repo's .git.
        assert_eq!(
            main_root_from_common_dir(Path::new("/main/.git")),
            PathBuf::from("/main")
        );
    }

    #[test]
    fn workspace_run_main_root_without_git_leaf_is_identity() {
        assert_eq!(
            main_root_from_common_dir(Path::new("/bare/repo")),
            PathBuf::from("/bare/repo")
        );
    }
}
