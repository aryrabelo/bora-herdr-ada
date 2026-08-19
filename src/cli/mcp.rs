use crate::mcp::McpOptions;

pub(super) fn run_mcp_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        print_mcp_help();
        return Ok(2);
    };

    match subcommand {
        "serve" => mcp_serve(&args[1..]),
        _ => {
            print_mcp_help();
            Ok(2)
        }
    }
}

fn mcp_serve(args: &[String]) -> std::io::Result<i32> {
    let mut channels = None;
    let mut nick = None;
    let mut allow_prompt = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--channels" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --channels");
                    return Ok(2);
                };
                channels = Some(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|c| !c.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>(),
                );
                index += 2;
            }
            "--nick" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --nick");
                    return Ok(2);
                };
                nick = Some(value.clone());
                index += 2;
            }
            "--allow-prompt" => {
                allow_prompt = true;
                index += 1;
            }
            option => {
                eprintln!("unknown option: {option}");
                print_mcp_serve_help();
                return Ok(2);
            }
        }
    }

    crate::mcp::run(McpOptions {
        channels,
        nick,
        allow_prompt,
    })
}

fn print_mcp_help() {
    eprintln!("usage: bora mcp serve [--channels a,b] [--nick NAME] [--allow-prompt]");
}

fn print_mcp_serve_help() {
    print_mcp_help();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_subcommand_prints_help_and_exits_2() {
        assert_eq!(run_mcp_command(&["bogus".to_string()]).unwrap(), 2);
    }

    #[test]
    fn missing_subcommand_prints_help_and_exits_2() {
        assert_eq!(run_mcp_command(&[]).unwrap(), 2);
    }

    #[test]
    fn unknown_serve_flag_exits_2() {
        assert_eq!(
            run_mcp_command(&["serve".to_string(), "--bogus".to_string()]).unwrap(),
            2
        );
    }
}
