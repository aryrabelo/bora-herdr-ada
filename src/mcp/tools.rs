//! Generates MCP tool definitions from `Method`'s derived JSON Schema instead
//! of hand-listing them, and dispatches `tools/call` onto the socket API.
//!
//! The allowlist is the only place that names which `Method` variants are
//! reachable over MCP; everything else (schema, description) is read off the
//! enum via `schemars`, so a new enum variant appears as a tool the moment
//! it's added here — no separate schema to keep in sync.

#[cfg(test)]
use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::api::schema::{Method, Request};
use crate::mcp::McpOptions;

/// `(wire method, tool name override)`. `None` derives the tool name by
/// replacing `.` with `_`. `channel.wait` is exposed as `channel_tail`
/// because that's the verb agents already know from `bora channel tail`.
const ALLOWLIST: &[(&str, Option<&str>)] = &[
    ("channel.list", None),
    ("channel.members", None),
    ("channel.history", None),
    ("channel.wait", Some("channel_tail")),
    ("channel.send", None),
    ("channel.note", None),
    ("channel.ask", None),
    ("channel.join", None),
    ("channel.leave", None),
    ("agent.list", None),
    ("agent.prompt", None),
];

/// Channel-scoped tools: their params carry a bare (no leading `#`) channel
/// `name` field that `--channels` fences before the request reaches the
/// socket. `channel_list` is scoped separately, by filtering its result.
const CHANNEL_NAME_SCOPED_TOOLS: &[&str] = &[
    "channel_members",
    "channel_history",
    "channel_tail",
    "channel_send",
    "channel_note",
    "channel_ask",
    "channel_join",
    "channel_leave",
];

/// Tools whose params struct has a `from_pane` field the CLI would normally
/// fill from `$HERDR_PANE_ID`. MCP callers get the same default.
const FROM_PANE_TOOLS: &[&str] = &[
    "channel_send",
    "channel_note",
    "channel_ask",
    "agent_prompt",
];

struct ToolEntry {
    name: String,
    wire_method: String,
    schema: Value,
}

fn tool_name(wire_method: &str, override_name: Option<&str>) -> String {
    override_name
        .map(str::to_string)
        .unwrap_or_else(|| wire_method.replace('.', "_"))
}

/// Walks `schema_for!(Method)`'s `oneOf` variants, keeps only the allowlisted
/// method names, and resolves each variant's `params` schema into a
/// self-contained (no `$ref`) inputSchema. An allowlisted method absent from
/// the enum (e.g. `channel.note` before it exists) is skipped silently.
fn build_tools(allow_prompt: bool) -> Vec<ToolEntry> {
    let schema: Value = schemars::schema_for!(Method).into();
    let defs = schema
        .get("$defs")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let variants = schema
        .get("oneOf")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut entries = Vec::new();
    for variant in &variants {
        let Some(method_name) = variant
            .pointer("/properties/method/const")
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some((_, override_name)) = ALLOWLIST.iter().find(|(name, _)| *name == method_name)
        else {
            continue;
        };
        let name = tool_name(method_name, *override_name);
        if name == "agent_prompt" && !allow_prompt {
            continue;
        }
        let mut stack = Vec::new();
        let params_schema = variant
            .pointer("/properties/params")
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        let schema = inline_refs(&params_schema, &defs, &mut stack);
        entries.push(ToolEntry {
            name,
            wire_method: method_name.to_string(),
            schema,
        });
    }
    entries
}

/// Recursively replaces `{"$ref": "#/$defs/Name"}` with the referenced
/// definition, so each tool's `inputSchema` stands alone — MCP clients don't
/// share a `$defs` pool across tools. Sibling keys next to a `$ref` (JSON
/// Schema 2020-12 allows them) are preserved on top of the resolved target.
/// A cycle (none exist in the current allowlisted params, but future
/// variants might add one) falls back to leaving the `$ref` in place rather
/// than recursing forever.
fn inline_refs(value: &Value, defs: &Map<String, Value>, stack: &mut Vec<String>) -> Value {
    match value {
        Value::Object(map) => {
            if let Some(name) = map
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/$defs/"))
            {
                if stack.iter().any(|seen| seen == name) {
                    return value.clone();
                }
                let Some(target) = defs.get(name) else {
                    return value.clone();
                };
                stack.push(name.to_string());
                let mut resolved = inline_refs(target, defs, stack);
                stack.pop();
                if let Value::Object(resolved_map) = &mut resolved {
                    for (key, sibling) in map {
                        if key != "$ref" {
                            resolved_map
                                .entry(key.clone())
                                .or_insert_with(|| sibling.clone());
                        }
                    }
                }
                return resolved;
            }
            Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), inline_refs(v, defs, stack)))
                    .collect(),
            )
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| inline_refs(v, defs, stack)).collect())
        }
        other => other.clone(),
    }
}

/// `tools/list` payload: `{name, description?, inputSchema}` per tool.
pub(crate) fn generate_tools(opts: &McpOptions) -> Vec<Value> {
    build_tools(opts.allow_prompt)
        .into_iter()
        .map(|entry| {
            let description = entry
                .schema
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_string);
            let mut tool = Map::new();
            tool.insert("name".into(), Value::String(entry.name));
            if let Some(description) = description {
                tool.insert("description".into(), Value::String(description));
            }
            tool.insert("inputSchema".into(), entry.schema);
            Value::Object(tool)
        })
        .collect()
}

pub(crate) enum DispatchError {
    /// JSON-RPC protocol-level error: the call never reached the socket.
    Protocol(i64, String),
    /// The socket API answered with an error, or the answer couldn't be
    /// rendered; surfaced as an MCP tool error (`isError: true`), not a
    /// protocol error, because the tool itself is real and was invoked.
    Tool(String),
}

fn bare_channel_name(display_name: &str) -> &str {
    display_name.strip_prefix('#').unwrap_or(display_name)
}

/// Strips channels outside `--channels` from a `channel.list` result in
/// place. `ChannelSummary.name` carries a leading `#`; `--channels` entries
/// don't.
fn filter_channel_list_result(result: &mut Value, allowed: &[String]) {
    let Some(channels) = result.get_mut("channels").and_then(Value::as_array_mut) else {
        return;
    };
    channels.retain(|channel| {
        channel
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|name| allowed.iter().any(|c| c == bare_channel_name(name)))
    });
}

/// Fills `from_pane` from `$HERDR_PANE_ID` exactly like the CLI does for
/// `channel send`: only when the caller didn't already supply one.
fn fill_from_pane(arguments: &mut Value) {
    let Some(pane) = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|v| !v.trim().is_empty())
    else {
        return;
    };
    if let Value::Object(map) = arguments {
        map.entry("from_pane")
            .or_insert_with(|| Value::String(pane));
    }
}

static REQUEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_request_id() -> String {
    let n = REQUEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("mcp:{n}")
}

/// Translates one `tools/call` into a socket `Request`, sends it, and maps
/// the reply into MCP `content` text. Never panics or propagates an `Err`
/// that would kill the serve loop — every failure becomes a
/// `DispatchError` the caller renders as either a JSON-RPC or tool error.
pub(crate) fn dispatch(
    name: &str,
    mut arguments: Value,
    opts: &McpOptions,
) -> Result<String, DispatchError> {
    let tools = build_tools(opts.allow_prompt);
    let Some(entry) = tools.iter().find(|t| t.name == name) else {
        return Err(DispatchError::Protocol(
            -32602,
            format!("unknown tool: {name}"),
        ));
    };

    if CHANNEL_NAME_SCOPED_TOOLS.contains(&name) {
        if let Some(channels) = &opts.channels {
            let channel_name = arguments.get("name").and_then(Value::as_str);
            match channel_name {
                Some(requested) if channels.iter().any(|c| c == requested) => {}
                Some(requested) => {
                    return Err(DispatchError::Protocol(
                        -32602,
                        format!(
                            "channel '{requested}' is out of scope; allowed channels: {}",
                            channels.join(", ")
                        ),
                    ));
                }
                None => {
                    return Err(DispatchError::Protocol(
                        -32602,
                        "missing required parameter: name".into(),
                    ));
                }
            }
        }
    }

    if FROM_PANE_TOOLS.contains(&name) {
        fill_from_pane(&mut arguments);
    }

    let envelope = serde_json::json!({
        "id": next_request_id(),
        "method": entry.wire_method,
        "params": arguments,
    });
    let request: Request = serde_json::from_value(envelope)
        .map_err(|err| DispatchError::Tool(format!("invalid arguments: {err}")))?;

    let client = crate::api::client::ApiClient::local();
    let response = client
        .request_value(&request)
        .map_err(|err| DispatchError::Tool(err.to_string()))?;

    if let Some(error) = response.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(DispatchError::Tool(message.to_string()));
    }

    let mut result = response.get("result").cloned().unwrap_or(Value::Null);
    if name == "channel_list" {
        if let Some(channels) = &opts.channels {
            filter_channel_list_result(&mut result, channels);
        }
    }
    serde_json::to_string_pretty(&result).map_err(|err| DispatchError::Tool(err.to_string()))
}

/// Exposed for tests that need the raw `(tool -> wire method)` mapping
/// without going through a live socket.
#[cfg(test)]
pub(crate) fn tool_index(allow_prompt: bool) -> HashMap<String, String> {
    build_tools(allow_prompt)
        .into_iter()
        .map(|e| (e.name, e.wire_method))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(channels: Option<Vec<&str>>, allow_prompt: bool) -> McpOptions {
        McpOptions {
            channels: channels.map(|cs| cs.into_iter().map(str::to_string).collect()),
            nick: None,
            allow_prompt,
        }
    }

    #[test]
    fn generates_a_tool_per_present_allowlisted_variant() {
        let index = tool_index(true);
        // channel.create is deliberately not in the allowlist.
        assert!(!index.contains_key("channel_create"));

        for (tool, wire) in [
            ("channel_list", "channel.list"),
            ("channel_members", "channel.members"),
            ("channel_history", "channel.history"),
            ("channel_tail", "channel.wait"),
            ("channel_send", "channel.send"),
            ("channel_note", "channel.note"),
            ("channel_ask", "channel.ask"),
            ("channel_join", "channel.join"),
            ("channel_leave", "channel.leave"),
            ("agent_list", "agent.list"),
            ("agent_prompt", "agent.prompt"),
        ] {
            assert_eq!(
                index.get(tool).map(String::as_str),
                Some(wire),
                "tool {tool}"
            );
        }
    }

    #[test]
    fn every_tool_schema_is_self_contained() {
        let tools = generate_tools(&opts(None, true));
        assert!(!tools.is_empty());
        for tool in &tools {
            let schema = &tool["inputSchema"];
            assert!(
                schema.is_object(),
                "{} inputSchema not an object",
                tool["name"]
            );
            assert!(
                !contains_ref(schema),
                "{} inputSchema still has a $ref",
                tool["name"]
            );
        }
    }

    fn contains_ref(value: &Value) -> bool {
        match value {
            Value::Object(map) => map.contains_key("$ref") || map.values().any(contains_ref),
            Value::Array(items) => items.iter().any(contains_ref),
            _ => false,
        }
    }

    #[test]
    fn allow_prompt_gates_agent_prompt_from_the_list() {
        let with_prompt = generate_tools(&opts(None, true));
        let without_prompt = generate_tools(&opts(None, false));
        assert!(with_prompt.iter().any(|t| t["name"] == "agent_prompt"));
        assert!(!without_prompt.iter().any(|t| t["name"] == "agent_prompt"));
    }

    #[test]
    fn dispatch_rejects_agent_prompt_when_not_allowed() {
        let err = dispatch("agent_prompt", serde_json::json!({}), &opts(None, false));
        assert!(matches!(err, Err(DispatchError::Protocol(-32602, _))));
    }

    #[test]
    fn dispatch_rejects_unknown_tool() {
        let err = dispatch("does_not_exist", serde_json::json!({}), &opts(None, true));
        assert!(matches!(err, Err(DispatchError::Protocol(-32602, _))));
    }

    #[test]
    fn dispatch_rejects_out_of_scope_channel_before_the_socket() {
        let err = dispatch(
            "channel_send",
            serde_json::json!({"name": "other", "text": "hi"}),
            &opts(Some(vec!["eng"]), true),
        );
        match err {
            Err(DispatchError::Protocol(-32602, message)) => {
                assert!(
                    message.contains("eng"),
                    "message should name allowed channels: {message}"
                );
            }
            _ => panic!("expected a protocol error rejecting the out-of-scope channel"),
        }
    }

    #[test]
    fn dispatch_allows_in_scope_channel_past_the_fence() {
        // No live server in unit tests: this must fail as a Tool error from
        // the socket call, never as the Protocol fence rejection.
        let err = dispatch(
            "channel_send",
            serde_json::json!({"name": "eng", "text": "hi"}),
            &opts(Some(vec!["eng"]), true),
        );
        assert!(matches!(err, Err(DispatchError::Tool(_))));
    }

    #[test]
    fn filter_channel_list_result_strips_out_of_scope_channels() {
        let mut result = serde_json::json!({
            "type": "channel_list",
            "channels": [
                {"name": "#eng", "pane_count": 1, "agent_count": 1, "member_status_counts": {}},
                {"name": "#other", "pane_count": 2, "agent_count": 2, "member_status_counts": {}},
            ],
        });
        filter_channel_list_result(&mut result, &["eng".to_string()]);
        let channels = result["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0]["name"], "#eng");
    }

    #[test]
    fn fill_from_pane_only_fills_when_absent() {
        std::env::set_var("HERDR_PANE_ID", "w1:p1");
        let mut with_existing = serde_json::json!({"name": "eng", "from_pane": "w2:p2"});
        fill_from_pane(&mut with_existing);
        assert_eq!(with_existing["from_pane"], "w2:p2");

        let mut without = serde_json::json!({"name": "eng"});
        fill_from_pane(&mut without);
        assert_eq!(without["from_pane"], "w1:p1");
        std::env::remove_var("HERDR_PANE_ID");
    }
}
