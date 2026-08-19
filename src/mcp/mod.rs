//! `bora mcp serve`: an MCP server over stdio (newline-delimited JSON-RPC
//! 2.0), exposing a fenced slice of the socket API as tools. See
//! `CANAL-ESCOPO.md` §MCP: the registration (`--channels`/`--allow-prompt`)
//! *is* the capability — there is no separate ACL system in bora. MCP is
//! client-initiated (the OMP harness talks to us, we never talk first), so
//! this loop only ever answers a line it just read; it never wakes an idle
//! agent (that stays `bora agent prompt --when-idle`'s job).

mod tools;

use std::io::{BufRead, Write};

use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2025-06-18";

pub(crate) struct McpOptions {
    /// `--channels a,b`: bare channel names (no leading `#`) this server may
    /// see or touch. `None` means unrestricted.
    pub channels: Option<Vec<String>>,
    /// `--nick`: informational, echoed in `serverInfo`. `from_pane` fill-in
    /// comes from `$HERDR_PANE_ID` alone and does not use this value.
    pub nick: Option<String>,
    /// `--allow-prompt`: gates the `agent_prompt` tool out of existence
    /// (absent from `tools/list`, rejected by `tools/call`) when false.
    pub allow_prompt: bool,
}

/// `bora mcp serve [--channels a,b] [--nick NAME] [--allow-prompt]`.
pub fn run(options: McpOptions) -> std::io::Result<i32> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    serve(stdin.lock(), &mut stdout, &options)?;
    Ok(0)
}

/// The read/response loop, generic over `BufRead`/`Write` so tests can drive
/// it over an in-memory pair instead of real stdio. Never returns `Err` for
/// a malformed line or a dead socket API connection — those become JSON-RPC
/// or tool errors on the wire; only a genuine I/O failure on the transport
/// itself propagates.
pub(crate) fn serve<R: BufRead, W: Write>(
    mut reader: R,
    writer: &mut W,
    options: &McpOptions,
) -> std::io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(response) = handle_line(trimmed, options) {
            writeln!(writer, "{}", serde_json::to_string(&response)?)?;
            writer.flush()?;
        }
    }
}

fn handle_line(line: &str, options: &McpOptions) -> Option<Value> {
    let message: Value = match serde_json::from_str(line) {
        Ok(value) => value,
        Err(err) => return Some(error_response(Value::Null, -32700, &format!("parse error: {err}"))),
    };
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned();

    match method {
        "initialize" => Some(ok_response(id.unwrap_or(Value::Null), initialize_result(options))),
        "notifications/initialized" => None,
        "ping" => id.map(|id| ok_response(id, json!({}))),
        "tools/list" => {
            id.map(|id| ok_response(id, json!({ "tools": tools::generate_tools(options) })))
        }
        "tools/call" => id.map(|id| handle_tools_call(id, params, options)),
        _ => id.map(|id| error_response(id, -32601, &format!("method not found: {method}"))),
    }
}

fn initialize_result(options: &McpOptions) -> Value {
    let mut server_info = serde_json::Map::new();
    server_info.insert("name".into(), Value::String("bora".into()));
    server_info.insert(
        "version".into(),
        Value::String(env!("CARGO_PKG_VERSION").into()),
    );
    if let Some(nick) = &options.nick {
        server_info.insert("nick".into(), Value::String(nick.clone()));
    }
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": Value::Object(server_info),
    })
}

fn handle_tools_call(id: Value, params: Option<Value>, options: &McpOptions) -> Value {
    let params = params.unwrap_or(Value::Null);
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return error_response(id, -32602, "missing required parameter: name");
    };
    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match tools::dispatch(name, arguments, options) {
        Ok(text) => ok_response(
            id,
            json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            }),
        ),
        Err(tools::DispatchError::Protocol(code, message)) => error_response(id, code, &message),
        Err(tools::DispatchError::Tool(message)) => ok_response(
            id,
            json!({
                "content": [{ "type": "text", "text": message }],
                "isError": true,
            }),
        ),
    }
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> McpOptions {
        McpOptions {
            channels: None,
            nick: None,
            allow_prompt: false,
        }
    }

    fn roundtrip(input: &str, options: &McpOptions) -> Vec<Value> {
        let mut out = Vec::new();
        serve(input.as_bytes(), &mut out, options).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn initialize_reports_protocol_version_and_server_info() {
        let input = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let responses = roundtrip(input, &opts());
        assert_eq!(responses.len(), 1);
        let result = &responses[0]["result"];
        assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(result["serverInfo"]["name"], "bora");
        assert_eq!(result["capabilities"]["tools"], json!({}));
    }

    #[test]
    fn notifications_initialized_produces_no_response() {
        let input = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n\
                      {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}";
        let responses = roundtrip(input, &opts());
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["id"], 2);
        assert_eq!(responses[0]["result"], json!({}));
    }

    #[test]
    fn tools_list_then_tools_call_round_trips() {
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tools/list\"}\n\
                      {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"unknown_tool\",\"arguments\":{}}}";
        let responses = roundtrip(input, &opts());
        assert_eq!(responses.len(), 2);
        let list = responses[0]["result"]["tools"].as_array().unwrap();
        assert!(list.iter().any(|t| t["name"] == "channel_list"));
        assert!(
            !list.iter().any(|t| t["name"] == "agent_prompt"),
            "agent_prompt must stay out of tools/list without --allow-prompt"
        );
        assert!(responses[1]["error"].is_object());
    }

    #[test]
    fn unknown_method_returns_method_not_found() {
        let input = r#"{"jsonrpc":"2.0","id":9,"method":"resources/list"}"#;
        let responses = roundtrip(input, &opts());
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0]["error"]["code"], -32601);
    }

    #[test]
    fn malformed_json_line_keeps_the_loop_alive_and_reports_parse_error() {
        let input = "not json at all\n{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}";
        let responses = roundtrip(input, &opts());
        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0]["error"]["code"], -32700);
        assert_eq!(responses[1]["id"], 3);
        assert_eq!(responses[1]["result"], json!({}));
    }

    #[test]
    fn notification_without_id_gets_no_response_even_for_unknown_methods() {
        let input = r#"{"jsonrpc":"2.0","method":"some/notification"}"#;
        let responses = roundtrip(input, &opts());
        assert!(responses.is_empty());
    }
}
