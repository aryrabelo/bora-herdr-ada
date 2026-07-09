use std::collections::HashMap;

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
}
