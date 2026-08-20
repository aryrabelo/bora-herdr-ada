use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use regex::Regex;

use crate::api::schema::{
    ErrorBody, ErrorResponse, EventData, EventEnvelope, EventKind, EventMatch, Method, Request,
    ResponseResult, Subscription, SubscriptionEventData, SubscriptionEventEnvelope,
    SuccessResponse,
};
use crate::api::server::{
    dispatch_to_app_with_timeout, should_stop_connection, APP_RESPONSE_TIMEOUT,
    CONNECTION_POLL_INTERVAL,
};
use crate::api::subscriptions::ActiveSubscription;
use crate::api::subscriptions::{match_output, output_match_read_source};
use crate::api::{ApiRequestSender, EventHub};
use crate::ipc::LocalStream;

const AGENT_PROMPT_EFFECT_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_WHEN_IDLE_TIMEOUT_MS: u64 = 60_000;
/// `channel.ask` default/cap for `timeout_ms`: 5 minutes default, 10
/// minutes hard cap — long enough for a human to notice and reply, bounded
/// so a forgotten ask can't hold a connection thread open indefinitely.
const DEFAULT_ASK_TIMEOUT_MS: u64 = 300_000;
const MAX_ASK_TIMEOUT_MS: u64 = 600_000;

pub(super) fn wait_for_output(
    request_id: String,
    params: crate::api::schema::PaneWaitForOutputParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    crate::logging::api_wait_started(&request_id, &params.pane_id, params.timeout_ms);
    let deadline = params
        .timeout_ms
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

    let regex = match &params.r#match {
        crate::api::schema::OutputMatch::Regex { value } => match Regex::new(value) {
            Ok(regex) => Some(regex),
            Err(err) => {
                return Ok(Some(
                    serde_json::to_string(&ErrorResponse {
                        id: request_id,
                        error: ErrorBody {
                            code: "invalid_regex".into(),
                            message: err.to_string(),
                        },
                    })
                    .unwrap(),
                ));
            }
        },
        crate::api::schema::OutputMatch::Substring { .. } => None,
    };

    loop {
        if should_stop_connection(stream, running)? {
            crate::logging::api_wait_completed(&request_id, &params.pane_id, "client_disconnected");
            return Ok(None);
        }

        let read_request = Request {
            id: format!("{request_id}:read"),
            method: Method::PaneRead(crate::api::schema::PaneReadParams {
                pane_id: params.pane_id.clone(),
                source: output_match_read_source(&params.source),
                lines: params.lines,
                format: crate::api::schema::ReadFormat::Text,
                strip_ansi: params.strip_ansi,
                intent: crate::api::schema::ReadIntent::Passive,
            }),
        };
        let response =
            dispatch_to_app_with_timeout(read_request, api_tx, Some(APP_RESPONSE_TIMEOUT));
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&response) else {
            return Ok(Some(response));
        };
        if value.get("error").is_some() {
            let mut value = value;
            value["id"] = serde_json::Value::String(request_id);
            return Ok(Some(serde_json::to_string(&value).unwrap()));
        }

        let read_value = value["result"]["read"].clone();
        let Ok(read) = serde_json::from_value::<crate::api::schema::PaneReadResult>(read_value)
        else {
            return Ok(Some(
                serde_json::to_string(&ErrorResponse {
                    id: request_id,
                    error: ErrorBody {
                        code: "internal_error".into(),
                        message: "failed to decode pane read result".into(),
                    },
                })
                .unwrap(),
            ));
        };

        let matched_line = match_output(&read.text, &params.r#match, regex.as_ref());
        if matched_line.is_some() {
            let revision = read.revision;
            crate::logging::api_wait_completed(&request_id, &params.pane_id, "matched");
            return Ok(Some(
                serde_json::to_string(&SuccessResponse {
                    id: request_id,
                    result: ResponseResult::OutputMatched {
                        pane_id: read.pane_id.clone(),
                        revision,
                        matched_line,
                        read,
                    },
                })
                .unwrap(),
            ));
        }

        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            crate::logging::api_wait_timed_out(&request_id, &params.pane_id);
            return Ok(Some(
                serde_json::to_string(&ErrorResponse {
                    id: request_id,
                    error: ErrorBody {
                        code: "timeout".into(),
                        message: "timed out waiting for output match".into(),
                    },
                })
                .unwrap(),
            ));
        }

        std::thread::sleep(CONNECTION_POLL_INTERVAL);
    }
}

pub(super) fn wait_for_agent(
    request_id: String,
    params: crate::api::schema::AgentWaitParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    let last_event_sequence = event_hub.current_sequence();
    let initial = match agent_get(&request_id, &params.target, api_tx) {
        Ok(agent) => agent,
        Err(response) => {
            return serde_json::to_string(&response)
                .map(Some)
                .map_err(std::io::Error::other);
        }
    };
    let until = agent_wait_statuses(params.until);
    if agent_wait_matches(&initial, &until, None) {
        return agent_wait_success(request_id, initial).map(Some);
    }

    match wait_for_resolved_agent(
        request_id.clone(),
        ResolvedAgentWait {
            target: params.target,
            until,
            timeout_ms: params.timeout_ms,
            initial,
            last_event_sequence,
            after_state_change_seq: None,
            accept_transient_status: true,
            timeout_kind: AgentWaitTimeoutKind::Status,
        },
        stream,
        api_tx,
        event_hub,
        running,
    )? {
        Some(AgentWaitOutcome::Matched(agent)) => agent_wait_success(request_id, *agent).map(Some),
        Some(AgentWaitOutcome::Response(response)) => Ok(Some(response)),
        None => Ok(None),
    }
}

pub(super) fn prompt_agent(
    request_id: String,
    params: crate::api::schema::AgentPromptParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    if params.when_idle == Some(true) {
        let idle_last_event_sequence = event_hub.current_sequence();
        let before_idle = match agent_get(&request_id, &params.target, api_tx) {
            Ok(agent) => agent,
            Err(response) => {
                return serde_json::to_string(&response)
                    .map(Some)
                    .map_err(std::io::Error::other);
            }
        };
        if before_idle.agent_status == crate::api::schema::AgentStatus::Working {
            let timeout_ms = params
                .when_idle_timeout_ms
                .unwrap_or(DEFAULT_WHEN_IDLE_TIMEOUT_MS);
            match wait_for_pane_idle(
                request_id.clone(),
                params.target.clone(),
                timeout_ms,
                before_idle,
                idle_last_event_sequence,
                stream,
                api_tx,
                event_hub,
                running,
            )? {
                Some(IdleWaitOutcome::Idle) => {}
                Some(IdleWaitOutcome::TimedOut) => {
                    // The target never left `working` within the budget. Fall
                    // through to dispatch anyway: `handle_agent_prompt`'s own
                    // busy check will enqueue the prompt and return a
                    // `deferred` receipt instead of losing it to a
                    // client-facing timeout error.
                }
                Some(IdleWaitOutcome::Response(response)) => return Ok(Some(response)),
                None => return Ok(None),
            }
        }
    }

    let Some(wait) = params.wait.clone() else {
        return Ok(Some(dispatch_to_app_with_timeout(
            Request {
                id: request_id,
                method: Method::AgentPrompt(params),
            },
            api_tx,
            None,
        )));
    };

    let last_event_sequence = event_hub.current_sequence();
    let before_prompt = match agent_get(&request_id, &params.target, api_tx) {
        Ok(agent) => agent,
        Err(response) => {
            return serde_json::to_string(&response)
                .map(Some)
                .map_err(std::io::Error::other);
        }
    };
    let target = params.target.clone();
    let prompt_response = dispatch_to_app_with_timeout(
        Request {
            id: request_id.clone(),
            method: Method::AgentPrompt(params),
        },
        api_tx,
        None,
    );
    let prompted = match agent_prompt_dispatch_outcome(&request_id, &prompt_response) {
        Ok(PromptDispatchOutcome::Injected(agent)) => *agent,
        // Nothing was actually injected — a `deferred` receipt has no prompt
        // effect to wait on, so return it as-is rather than treating it like
        // a normal injected prompt.
        Ok(PromptDispatchOutcome::Deferred) | Err(_) => return Ok(Some(prompt_response)),
    };
    if !agent_wait_identity_matches(
        &prompted,
        &before_prompt.terminal_id,
        before_prompt.name.as_deref().filter(|name| *name == target),
        before_prompt.agent.as_deref(),
    ) {
        return agent_wait_not_running(request_id).map(Some);
    }

    let wait_started = std::time::Instant::now();
    let prompt_state_change_seq = prompted.state_change_seq;
    let until = agent_wait_statuses(wait.until);
    let mut initial = prompted;
    let mut after_state_change_seq = Some(prompt_state_change_seq);

    if initial.agent_status != crate::api::schema::AgentStatus::Working {
        let effect_timeout_ms = wait
            .timeout_ms
            .map_or(AGENT_PROMPT_EFFECT_TIMEOUT_MS, |timeout_ms| {
                timeout_ms.min(AGENT_PROMPT_EFFECT_TIMEOUT_MS)
            });
        let timeout_kind = if wait
            .timeout_ms
            .is_some_and(|timeout_ms| timeout_ms <= AGENT_PROMPT_EFFECT_TIMEOUT_MS)
        {
            AgentWaitTimeoutKind::Status
        } else {
            AgentWaitTimeoutKind::PromptStalled {
                baseline: prompt_state_change_seq,
                timeout_ms: effect_timeout_ms,
            }
        };
        let Some(outcome) = wait_for_resolved_agent(
            request_id.clone(),
            ResolvedAgentWait {
                target: target.clone(),
                until: all_agent_statuses(),
                timeout_ms: Some(effect_timeout_ms),
                initial,
                last_event_sequence,
                after_state_change_seq,
                accept_transient_status: false,
                timeout_kind,
            },
            stream,
            api_tx,
            event_hub,
            running,
        )?
        else {
            return Ok(None);
        };
        initial = match outcome {
            AgentWaitOutcome::Matched(agent) => *agent,
            AgentWaitOutcome::Response(response) => return Ok(Some(response)),
        };
        after_state_change_seq = None;
        if agent_wait_matches(&initial, &until, None) {
            return agent_prompt_success(request_id, initial).map(Some);
        }
    }

    let Some(outcome) = wait_for_resolved_agent(
        request_id.clone(),
        ResolvedAgentWait {
            target,
            until,
            timeout_ms: remaining_timeout_ms(wait.timeout_ms, wait_started),
            initial,
            // Replay from before submission so terminal lifecycle events consumed by
            // the activity gate still terminate this settled-state wait.
            last_event_sequence,
            after_state_change_seq,
            accept_transient_status: false,
            timeout_kind: AgentWaitTimeoutKind::Status,
        },
        stream,
        api_tx,
        event_hub,
        running,
    )?
    else {
        return Ok(None);
    };
    let agent = match outcome {
        AgentWaitOutcome::Matched(agent) => *agent,
        AgentWaitOutcome::Response(response) => return Ok(Some(response)),
    };
    agent_prompt_success(request_id, agent).map(Some)
}

fn remaining_timeout_ms(total_ms: Option<u64>, started: std::time::Instant) -> Option<u64> {
    total_ms.map(|total_ms| {
        let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        total_ms.saturating_sub(elapsed_ms)
    })
}

fn agent_prompt_success(
    request_id: String,
    agent: crate::api::schema::AgentInfo,
) -> std::io::Result<String> {
    serde_json::to_string(&SuccessResponse {
        id: request_id,
        result: ResponseResult::AgentPrompted {
            agent,
            outcome: crate::api::schema::AgentPromptOutcome::Injected,
            queue_position: None,
            queue_id: None,
        },
    })
    .map_err(std::io::Error::other)
}

struct ResolvedAgentWait {
    target: String,
    until: Vec<crate::api::schema::AgentStatus>,
    timeout_ms: Option<u64>,
    initial: crate::api::schema::AgentInfo,
    last_event_sequence: u64,
    after_state_change_seq: Option<u64>,
    accept_transient_status: bool,
    timeout_kind: AgentWaitTimeoutKind,
}

#[derive(Clone, Copy)]
enum AgentWaitTimeoutKind {
    Status,
    PromptStalled { baseline: u64, timeout_ms: u64 },
    Idle { timeout_ms: u64 },
}

enum AgentWaitOutcome {
    Matched(Box<crate::api::schema::AgentInfo>),
    Response(String),
}

fn wait_for_resolved_agent(
    request_id: String,
    wait: ResolvedAgentWait,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<AgentWaitOutcome>> {
    let deadline = wait
        .timeout_ms
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
    let expected_terminal_id = wait.initial.terminal_id.clone();
    let expected_name = wait
        .initial
        .name
        .as_ref()
        .filter(|name| name.as_str() == wait.target)
        .cloned();
    let expected_agent = wait.initial.agent.clone();
    let pane_id = wait.initial.pane_id.clone();
    let mut last_event_sequence = wait.last_event_sequence;

    loop {
        if should_stop_connection(stream, running)? {
            return Ok(None);
        }

        let mut should_probe = false;
        let mut matched_event_status = None;
        for (sequence, event) in event_hub.events_after(last_event_sequence) {
            last_event_sequence = sequence;
            match event.data {
                EventData::PaneAgentDetected {
                    pane_id: event_pane,
                    agent,
                    released,
                    final_status,
                    ..
                } if event_pane == pane_id => {
                    if released {
                        if let Some(status) = final_status
                            .filter(|status| wait.until.contains(status))
                            .or(matched_event_status)
                        {
                            let mut matched = wait.initial;
                            matched.agent_status = status;
                            return Ok(Some(AgentWaitOutcome::Matched(Box::new(matched))));
                        }
                        return agent_wait_not_running(request_id)
                            .map(AgentWaitOutcome::Response)
                            .map(Some);
                    }
                    if agent.is_some() && expected_agent.is_some() && agent != expected_agent {
                        return agent_wait_not_running(request_id)
                            .map(AgentWaitOutcome::Response)
                            .map(Some);
                    }
                    should_probe = true;
                }
                EventData::PaneAgentStatusChanged {
                    pane_id: event_pane,
                    agent_status,
                    ..
                } if event_pane == pane_id => {
                    if wait.accept_transient_status && wait.until.contains(&agent_status) {
                        matched_event_status = Some(agent_status);
                    }
                    should_probe = true;
                }
                EventData::PaneUpdated { pane } if pane.pane_id == pane_id => should_probe = true,
                EventData::PaneMoved {
                    previous_pane_id, ..
                } if previous_pane_id == pane_id => {
                    return agent_wait_not_running(request_id)
                        .map(AgentWaitOutcome::Response)
                        .map(Some);
                }
                EventData::PaneClosed {
                    pane_id: event_pane,
                    ..
                }
                | EventData::PaneExited {
                    pane_id: event_pane,
                    ..
                } if event_pane == pane_id => {
                    return agent_wait_not_running(request_id)
                        .map(AgentWaitOutcome::Response)
                        .map(Some);
                }
                _ => {}
            }
        }

        if should_probe {
            let current = match agent_get(&request_id, &wait.target, api_tx) {
                Ok(agent) => agent,
                Err(response) => {
                    return agent_wait_probe_error(response)
                        .map(AgentWaitOutcome::Response)
                        .map(Some);
                }
            };
            if !agent_wait_identity_matches(
                &current,
                &expected_terminal_id,
                expected_name.as_deref(),
                expected_agent.as_deref(),
            ) {
                return agent_wait_not_running(request_id)
                    .map(AgentWaitOutcome::Response)
                    .map(Some);
            }
            if let Some(status) = matched_event_status {
                let mut matched = current;
                matched.agent_status = status;
                return Ok(Some(AgentWaitOutcome::Matched(Box::new(matched))));
            }
            if agent_wait_matches(&current, &wait.until, wait.after_state_change_seq) {
                return Ok(Some(AgentWaitOutcome::Matched(Box::new(current))));
            }
        }

        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            let current = match agent_get(&request_id, &wait.target, api_tx) {
                Ok(agent) => agent,
                Err(response) => {
                    return agent_wait_probe_error(response)
                        .map(AgentWaitOutcome::Response)
                        .map(Some);
                }
            };
            if !agent_wait_identity_matches(
                &current,
                &expected_terminal_id,
                expected_name.as_deref(),
                expected_agent.as_deref(),
            ) {
                return agent_wait_not_running(request_id)
                    .map(AgentWaitOutcome::Response)
                    .map(Some);
            }
            if agent_wait_matches(&current, &wait.until, wait.after_state_change_seq) {
                return Ok(Some(AgentWaitOutcome::Matched(Box::new(current))));
            }
            return agent_wait_timeout(request_id, wait.timeout_kind, &current)
                .map(AgentWaitOutcome::Response)
                .map(Some);
        }
        std::thread::sleep(CONNECTION_POLL_INTERVAL);
    }
}

fn all_agent_statuses() -> Vec<crate::api::schema::AgentStatus> {
    // Keep this exhaustive: every status is evidence that the sequence advanced.
    vec![
        crate::api::schema::AgentStatus::Idle,
        crate::api::schema::AgentStatus::Working,
        crate::api::schema::AgentStatus::Blocked,
        crate::api::schema::AgentStatus::Done,
        crate::api::schema::AgentStatus::Unknown,
    ]
}

fn agent_wait_statuses(
    until: Vec<crate::api::schema::AgentStatus>,
) -> Vec<crate::api::schema::AgentStatus> {
    if until.is_empty() {
        vec![
            crate::api::schema::AgentStatus::Idle,
            crate::api::schema::AgentStatus::Done,
            crate::api::schema::AgentStatus::Blocked,
        ]
    } else {
        until
    }
}

fn not_working_statuses() -> Vec<crate::api::schema::AgentStatus> {
    vec![
        crate::api::schema::AgentStatus::Idle,
        crate::api::schema::AgentStatus::Blocked,
        crate::api::schema::AgentStatus::Done,
        crate::api::schema::AgentStatus::Unknown,
    ]
}

enum IdleWaitOutcome {
    Idle,
    /// The target never left `working` within the budget. The caller falls
    /// through to dispatch anyway so the busy prompt is enqueued and
    /// reported as a `deferred` receipt instead of a lost timeout error.
    TimedOut,
    Response(String),
}

/// Best-effort extraction of `error.code` from a JSON-encoded response, for
/// internal control-flow decisions that never leak the raw value outward.
fn response_error_code(response: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(response)
        .ok()?
        .get("error")?
        .get("code")?
        .as_str()
        .map(str::to_string)
}

/// Pre-send idle gate for `AgentPromptParams.when_idle`: blocks until the target leaves
/// `Working`, reusing the same EventHub poll loop and identity re-verification as
/// `wait_for_resolved_agent`, bounded by `timeout_ms`.
fn wait_for_pane_idle(
    request_id: String,
    target: String,
    timeout_ms: u64,
    initial: crate::api::schema::AgentInfo,
    last_event_sequence: u64,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<IdleWaitOutcome>> {
    let Some(outcome) = wait_for_resolved_agent(
        request_id,
        ResolvedAgentWait {
            target,
            until: not_working_statuses(),
            timeout_ms: Some(timeout_ms),
            initial,
            last_event_sequence,
            after_state_change_seq: None,
            accept_transient_status: true,
            timeout_kind: AgentWaitTimeoutKind::Idle { timeout_ms },
        },
        stream,
        api_tx,
        event_hub,
        running,
    )?
    else {
        return Ok(None);
    };
    Ok(Some(match outcome {
        AgentWaitOutcome::Matched(_) => IdleWaitOutcome::Idle,
        // `AgentWaitTimeoutKind::Idle` is only ever constructed here, and its
        // "agent_busy_timeout" code is only ever produced by the genuine
        // still-working deadline branch of `wait_for_resolved_agent` — every
        // other failure in this call (pane gone, identity lost, probe error)
        // uses a different code and is returned to the caller unchanged.
        AgentWaitOutcome::Response(response)
            if response_error_code(&response).as_deref() == Some("agent_busy_timeout") =>
        {
            IdleWaitOutcome::TimedOut
        }
        AgentWaitOutcome::Response(response) => IdleWaitOutcome::Response(response),
    }))
}

fn agent_wait_identity_matches(
    agent: &crate::api::schema::AgentInfo,
    expected_terminal_id: &str,
    expected_name: Option<&str>,
    expected_agent: Option<&str>,
) -> bool {
    agent.terminal_id == expected_terminal_id
        && expected_name.is_none_or(|name| agent.name.as_deref() == Some(name))
        && match (expected_agent, agent.agent.as_deref()) {
            (Some(expected), Some(current)) => expected == current,
            (Some(_), None) => agent.name.is_some(),
            (None, _) => true,
        }
}

fn agent_wait_matches(
    agent: &crate::api::schema::AgentInfo,
    until: &[crate::api::schema::AgentStatus],
    after_state_change_seq: Option<u64>,
) -> bool {
    until.contains(&agent.agent_status)
        && after_state_change_seq.is_none_or(|baseline| agent.state_change_seq > baseline)
}

fn agent_get(
    request_id: &str,
    target: &str,
    api_tx: &ApiRequestSender,
) -> Result<crate::api::schema::AgentInfo, ErrorResponse> {
    let response = dispatch_to_app_with_timeout(
        Request {
            id: format!("{request_id}:agent"),
            method: Method::AgentGet(crate::api::schema::AgentTarget {
                target: target.to_string(),
            }),
        },
        api_tx,
        Some(APP_RESPONSE_TIMEOUT),
    );
    agent_from_response(request_id, &response)
}

fn agent_from_response(
    request_id: &str,
    response: &str,
) -> Result<crate::api::schema::AgentInfo, ErrorResponse> {
    let value: serde_json::Value = serde_json::from_str(response).map_err(|_| ErrorResponse {
        id: request_id.into(),
        error: ErrorBody {
            code: "internal_error".into(),
            message: "failed to decode agent response".into(),
        },
    })?;
    if value.get("error").is_some() {
        let error = serde_json::from_value(value["error"].clone()).map_err(|_| ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "internal_error".into(),
                message: "failed to decode agent error".into(),
            },
        })?;
        return Err(ErrorResponse {
            id: request_id.into(),
            error,
        });
    }
    serde_json::from_value(value["result"]["agent"].clone()).map_err(|_| ErrorResponse {
        id: request_id.into(),
        error: ErrorBody {
            code: "internal_error".into(),
            message: "failed to decode agent result".into(),
        },
    })
}

enum PromptDispatchOutcome {
    Injected(Box<crate::api::schema::AgentInfo>),
    Deferred,
}

/// Parses an `agent.prompt` dispatch response, distinguishing an actually
/// injected prompt (has a prompt effect worth waiting on) from a `deferred`
/// receipt (the target was busy; nothing was injected, so waiting for an
/// effect would be meaningless).
fn agent_prompt_dispatch_outcome(
    request_id: &str,
    response: &str,
) -> Result<PromptDispatchOutcome, ErrorResponse> {
    let value: serde_json::Value = serde_json::from_str(response).map_err(|_| ErrorResponse {
        id: request_id.into(),
        error: ErrorBody {
            code: "internal_error".into(),
            message: "failed to decode agent response".into(),
        },
    })?;
    if value.get("error").is_some() {
        let error = serde_json::from_value(value["error"].clone()).map_err(|_| ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "internal_error".into(),
                message: "failed to decode agent error".into(),
            },
        })?;
        return Err(ErrorResponse {
            id: request_id.into(),
            error,
        });
    }
    if value["result"]["outcome"].as_str() == Some("deferred") {
        return Ok(PromptDispatchOutcome::Deferred);
    }
    serde_json::from_value(value["result"]["agent"].clone())
        .map(|agent| PromptDispatchOutcome::Injected(Box::new(agent)))
        .map_err(|_| ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "internal_error".into(),
                message: "failed to decode agent result".into(),
            },
        })
}

fn agent_wait_success(
    request_id: String,
    agent: crate::api::schema::AgentInfo,
) -> std::io::Result<String> {
    serde_json::to_string(&SuccessResponse {
        id: request_id,
        result: ResponseResult::AgentInfo { agent },
    })
    .map_err(std::io::Error::other)
}

fn agent_wait_timeout(
    request_id: String,
    kind: AgentWaitTimeoutKind,
    current: &crate::api::schema::AgentInfo,
) -> std::io::Result<String> {
    let (code, message) = match kind {
        AgentWaitTimeoutKind::Status => {
            ("timeout", "timed out waiting for agent status".to_string())
        }
        AgentWaitTimeoutKind::PromptStalled {
            baseline,
            timeout_ms,
        } => {
            let status = format!("{:?}", current.agent_status).to_ascii_lowercase();
            (
                "agent_prompt_stalled",
                format!(
                    "agent prompt produced no observed state change within {timeout_ms} ms; status is {status} and state_change_seq remained {baseline}"
                ),
            )
        }
        AgentWaitTimeoutKind::Idle { timeout_ms } => {
            let status = format!("{:?}", current.agent_status).to_ascii_lowercase();
            (
                "agent_busy_timeout",
                format!(
                    "timed out after {timeout_ms} ms waiting for agent to go idle; last observed status is {status}"
                ),
            )
        }
    };
    serde_json::to_string(&ErrorResponse {
        id: request_id,
        error: ErrorBody {
            code: code.into(),
            message,
        },
    })
    .map_err(std::io::Error::other)
}

fn agent_wait_not_running(request_id: String) -> std::io::Result<String> {
    serde_json::to_string(&ErrorResponse {
        id: request_id,
        error: ErrorBody {
            code: "agent_not_running".into(),
            message: "agent is no longer running in the target pane".into(),
        },
    })
    .map_err(std::io::Error::other)
}

fn agent_wait_probe_error(response: ErrorResponse) -> std::io::Result<String> {
    if response.error.code == "agent_not_found" {
        return agent_wait_not_running(response.id);
    }
    serde_json::to_string(&response).map_err(std::io::Error::other)
}

pub(super) fn wait_for_event(
    request_id: String,
    params: crate::api::schema::EventsWaitParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &crate::api::EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    let deadline = params
        .timeout_ms
        .map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));

    // events.wait is check-or-wait: a pane agent-status match must observe the
    // CURRENT state at wait start (via a pane.get probe), not only transitions
    // emitted after the wait begins. Agent-status matches additionally get
    // ActiveSubscription's mid-wait pane_not_found detection below.
    if let crate::api::schema::EventMatch::PaneAgentStatusChanged {
        pane_id,
        agent_status,
    } = &params.match_event
    {
        match crate::api::subscriptions::pane_get(
            format!("{request_id}:wait:probe"),
            pane_id,
            api_tx,
        ) {
            Ok(probe) => {
                if probe.agent_status == *agent_status {
                    let envelope = crate::api::schema::EventEnvelope {
                        event: crate::api::schema::EventKind::PaneAgentStatusChanged,
                        data: crate::api::schema::EventData::PaneAgentStatusChanged {
                            pane_id: probe.pane_id,
                            workspace_id: probe.workspace_id,
                            agent_status: probe.agent_status,
                            agent: probe.agent,
                            title: probe.title,
                            display_agent: probe.display_agent,
                            state_labels: probe.state_labels,
                        },
                    };
                    return Ok(Some(
                        serde_json::to_string(&SuccessResponse {
                            id: request_id,
                            result: ResponseResult::WaitMatched { event: envelope },
                        })
                        .unwrap(),
                    ));
                }
            }
            Err(mut response) => {
                response.id = request_id;
                return Ok(Some(serde_json::to_string(&response).unwrap()));
            }
        }

        let subscription = match event_match_subscription(&request_id, params.match_event) {
            Ok(subscription) => subscription,
            Err(response) => return Ok(Some(serde_json::to_string(&response).unwrap())),
        };
        let mut active =
            match ActiveSubscription::new(subscription, &request_id, 0, api_tx, event_hub) {
                Ok(active) => active,
                Err(mut response) => {
                    response.id = request_id;
                    return Ok(Some(serde_json::to_string(&response).unwrap()));
                }
            };
        loop {
            if should_stop_connection(stream, running)? {
                return Ok(None);
            }

            match active.poll_for_wait(api_tx, event_hub) {
                Ok(Some(event)) => return Ok(Some(wait_matched_response(&request_id, event))),
                Ok(None) => {}
                Err(mut response) if response.error.code == "pane_not_found" => {
                    response.id = request_id;
                    return serde_json::to_string(&response)
                        .map(Some)
                        .map_err(std::io::Error::other);
                }
                Err(_) => {}
            }

            if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
                return Ok(Some(
                    serde_json::to_string(&ErrorResponse {
                        id: request_id,
                        error: ErrorBody {
                            code: "timeout".into(),
                            message: "timed out waiting for event match".into(),
                        },
                    })
                    .unwrap(),
                ));
            }

            std::thread::sleep(CONNECTION_POLL_INTERVAL);
        }
    }

    // Any other match_event is filtered directly against the raw event hub.
    // Start at 0 so a result posted just before the wait (the common
    // post-then-wait orchestration order) is still observed from the buffer.
    let mut last_sequence = 0u64;
    loop {
        if should_stop_connection(stream, running)? {
            return Ok(None);
        }

        for (sequence, envelope) in event_hub.events_after(last_sequence) {
            last_sequence = sequence;
            if crate::api::schema::event_matches(&params.match_event, &envelope) {
                return Ok(Some(
                    serde_json::to_string(&SuccessResponse {
                        id: request_id,
                        result: ResponseResult::WaitMatched { event: envelope },
                    })
                    .unwrap(),
                ));
            }
        }

        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            return Ok(Some(
                serde_json::to_string(&ErrorResponse {
                    id: request_id,
                    error: ErrorBody {
                        code: "timeout".into(),
                        message: "timed out waiting for event match".into(),
                    },
                })
                .unwrap(),
            ));
        }

        std::thread::sleep(CONNECTION_POLL_INTERVAL);
    }
}

fn event_match_subscription(
    request_id: &str,
    match_event: EventMatch,
) -> Result<Subscription, ErrorResponse> {
    match match_event {
        EventMatch::PaneAgentStatusChanged {
            pane_id,
            agent_status,
        } => Ok(Subscription::PaneAgentStatusChanged {
            pane_id,
            agent_status: Some(agent_status),
        }),
        _ => Err(ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "unsupported_event_wait_match".into(),
                message: "events.wait currently supports pane agent status matches".into(),
            },
        }),
    }
}

fn wait_matched_response(request_id: &str, event: serde_json::Value) -> String {
    let Ok(event) = serde_json::from_value::<SubscriptionEventEnvelope>(event) else {
        return serde_json::to_string(&ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "internal_error".into(),
                message: "failed to decode matched event".into(),
            },
        })
        .unwrap();
    };

    let SubscriptionEventData::PaneAgentStatusChanged(data) = event.data else {
        return serde_json::to_string(&ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "unsupported_event_wait_match".into(),
                message: "events.wait currently supports pane agent status matches".into(),
            },
        })
        .unwrap();
    };

    serde_json::to_string(&SuccessResponse {
        id: request_id.into(),
        result: ResponseResult::WaitMatched {
            event: EventEnvelope {
                event: EventKind::PaneAgentStatusChanged,
                data: EventData::PaneAgentStatusChanged {
                    pane_id: data.pane_id,
                    workspace_id: data.workspace_id,
                    agent_status: data.agent_status,
                    agent: data.agent,
                    title: data.title,
                    display_agent: data.display_agent,
                    state_labels: data.state_labels,
                },
            },
        },
    })
    .unwrap()
}

/// Result of a `channel.wait`: the durable answer to "what happened after
/// my cursor", read from the retained transcript — never just an in-memory
/// event.
pub(super) struct ChannelWaitOutcome {
    pub messages: Vec<crate::api::schema::ChannelMessage>,
    pub gap: bool,
    pub oldest_seq: Option<u64>,
    pub timed_out: bool,
}

/// Snapshot truth for a cursor: every retained message with
/// `seq > after_seq`, plus an explicit gap flag when the cursor predates
/// the oldest retained line (rotation dropped messages in between) or the
/// retained history is empty while the cursor is past 0. A gap is reported,
/// never papered over.
fn channel_wait_snapshot(name: &str, after_seq: u64) -> std::io::Result<ChannelWaitOutcome> {
    let since = crate::persist::channels::read_since(name, after_seq)?;
    let gap = match since.oldest_seq {
        // Pre-seq lines read as seq 0, so a 0 cursor never gaps against them.
        Some(oldest) => oldest > after_seq.saturating_add(1),
        None => after_seq > 0,
    };
    Ok(ChannelWaitOutcome {
        messages: since.messages,
        gap,
        oldest_seq: since.oldest_seq,
        timed_out: false,
    })
}

/// Poll ticks between belt-and-braces history re-reads when no matching
/// event was observed — the event hub ring only retains 512 entries, so a
/// busy server can evict our channel's event before we scan it. The
/// transcript on disk is the truth; this bounds how stale the hub-only
/// fast path can go (~1s at the 100ms poll interval).
const CHANNEL_WAIT_SNAPSHOT_EVERY_TICKS: u32 = 10;

/// `channel.wait` core loop, mirroring `wait_for_event`'s poll pattern:
/// backlog first (a snapshot on the first tick returns before any waiting),
/// then watch the event hub for a `channel.message` on this channel and
/// re-read the transcript when one lands. `None` means the caller cancelled
/// (client disconnected); a timeout is a clean `timed_out` outcome.
fn poll_channel_wait(
    name: &str,
    after_seq: u64,
    timeout_ms: Option<u64>,
    event_hub: &EventHub,
    mut cancelled: impl FnMut() -> bool,
) -> std::io::Result<Option<ChannelWaitOutcome>> {
    let deadline =
        timeout_ms.map(|ms| std::time::Instant::now() + std::time::Duration::from_millis(ms));
    let mut hub_sequence = event_hub.current_sequence();
    let mut ticks: u32 = 0;
    loop {
        if cancelled() {
            return Ok(None);
        }
        ticks += 1;

        let mut new_message = false;
        for (sequence, envelope) in event_hub.events_after(hub_sequence) {
            hub_sequence = hub_sequence.max(sequence);
            if matches!(
                &envelope.data,
                EventData::ChannelMessage { channel, .. } if *channel == name
            ) {
                new_message = true;
            }
        }

        // First tick = the backlog check; later ticks re-read on a matching
        // event or periodically so an evicted event can never strand us.
        if ticks == 1 || new_message || ticks.is_multiple_of(CHANNEL_WAIT_SNAPSHOT_EVERY_TICKS) {
            let outcome = channel_wait_snapshot(name, after_seq)?;
            if !outcome.messages.is_empty() || outcome.gap {
                return Ok(Some(outcome));
            }
        }

        if deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline) {
            // Last-chance read before declaring a timeout: the message may
            // be on disk even if its event already rotated out of the hub.
            let mut outcome = channel_wait_snapshot(name, after_seq)?;
            if !outcome.messages.is_empty() || outcome.gap {
                return Ok(Some(outcome));
            }
            outcome.timed_out = true;
            return Ok(Some(outcome));
        }

        std::thread::sleep(crate::api::server::CONNECTION_POLL_INTERVAL);
    }
}

/// `channel.wait` entry point: cursor-based tail follow over the durable
/// channel transcript. Backlog-first, gap-honest, clean timeout — see
/// [`ChannelWaitParams`] for the wire contract.
pub(super) fn wait_for_channel_message(
    request_id: String,
    params: crate::api::schema::ChannelWaitParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    let name = crate::persist::channels::normalize_channel_name(&params.name);
    if name.is_empty() {
        return Ok(Some(
            serde_json::to_string(&ErrorResponse {
                id: request_id,
                error: ErrorBody {
                    code: "invalid_channel_name".into(),
                    message: "channel name must not be empty".into(),
                },
            })
            .unwrap(),
        ));
    }
    let outcome = match poll_channel_wait(
        &name,
        params.after_seq,
        params.timeout_ms,
        event_hub,
        || should_stop_connection(stream, running).unwrap_or(true),
    ) {
        Ok(Some(outcome)) => outcome,
        // The client went away mid-wait; there is nobody to answer.
        Ok(None) => return Ok(None),
        Err(err) => {
            return Ok(Some(
                serde_json::to_string(&ErrorResponse {
                    id: request_id,
                    error: ErrorBody {
                        code: "channel_wait_failed".into(),
                        message: err.to_string(),
                    },
                })
                .unwrap(),
            ));
        }
    };
    advance_channel_wait_cursor(
        &name,
        params.from_pane.as_deref(),
        params.after_seq,
        &outcome,
        api_tx,
    );
    Ok(Some(
        serde_json::to_string(&SuccessResponse {
            id: request_id,
            result: ResponseResult::ChannelWait {
                messages: outcome.messages,
                gap: outcome.gap,
                oldest_seq: outcome.oldest_seq,
                timed_out: outcome.timed_out,
            },
        })
        .unwrap(),
    ))
}

/// Resolves `from_pane` to a channel member via `channel.members` — the
/// same identity machinery `channel.history`'s cursor tracking uses,
/// reached over the existing `api_tx` App round-trip since this poll loop
/// runs off the App thread and has no direct access to its live pane
/// graph. When `from_pane` is a member, advances that member's stored read
/// cursor to the highest seq this call actually returned — or, if nothing
/// new arrived, to the cursor the caller already claims via `after_seq`
/// (never regresses; see `advance_channel_cursor`). A caller with no pane
/// identity, or one that doesn't resolve to a member, reads freely and
/// advances nothing. Never fails the wait: a lookup or persistence error
/// is a `tracing` warning, like every other channel sidecar write.
fn advance_channel_wait_cursor(
    name: &str,
    from_pane: Option<&str>,
    after_seq: u64,
    outcome: &ChannelWaitOutcome,
    api_tx: &ApiRequestSender,
) {
    let Some(from_pane) = from_pane else {
        return;
    };
    let high_water = outcome
        .messages
        .last()
        .map_or(after_seq, |message| message.seq);
    let members_response = dispatch_to_app_with_timeout(
        Request {
            id: "internal:channel-wait-cursor".into(),
            method: Method::ChannelMembers(crate::api::schema::ChannelMembersParams {
                name: name.to_string(),
            }),
        },
        api_tx,
        Some(APP_RESPONSE_TIMEOUT),
    );
    let is_member = serde_json::from_str::<serde_json::Value>(&members_response)
        .ok()
        .and_then(|value| value.get("result")?.get("members")?.as_array().cloned())
        .is_some_and(|members| {
            members
                .iter()
                .any(|member| member.get("pane_id").and_then(|v| v.as_str()) == Some(from_pane))
        });
    if !is_member {
        return;
    }
    if let Err(err) = crate::persist::channels::advance_channel_cursor(name, from_pane, high_water)
    {
        tracing::warn!(
            channel = %name,
            pane = %from_pane,
            error = %err,
            "failed to advance channel read cursor"
        );
    }
}

/// Poll ticks between belt-and-braces history re-reads for `channel.ask`,
/// mirroring `CHANNEL_WAIT_SNAPSHOT_EVERY_TICKS`'s reasoning: the event hub
/// ring is bounded, so a busy server can evict the reply's event before
/// this loop scans it.
const CHANNEL_ASK_SNAPSHOT_EVERY_TICKS: u32 = 10;

/// `channel.ask`'s reply-wait core loop: polls the durable transcript for
/// the first retained message whose `in_reply_to` equals `question_seq`,
/// mirroring `poll_channel_wait`'s backlog-first / event-hub-gated /
/// periodic-snapshot pattern. `Ok(None)` means the caller cancelled
/// (client disconnected); `Ok(Some(None))` is a clean timeout;
/// `Ok(Some(Some(_)))` is the matching reply.
fn poll_channel_ask_reply(
    name: &str,
    question_seq: u64,
    timeout_ms: u64,
    event_hub: &EventHub,
    mut cancelled: impl FnMut() -> bool,
) -> std::io::Result<Option<Option<crate::api::schema::ChannelMessage>>> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    let mut hub_sequence = event_hub.current_sequence();
    let mut ticks: u32 = 0;
    loop {
        if cancelled() {
            return Ok(None);
        }
        ticks += 1;

        let mut new_message = false;
        for (sequence, envelope) in event_hub.events_after(hub_sequence) {
            hub_sequence = hub_sequence.max(sequence);
            if matches!(
                &envelope.data,
                EventData::ChannelMessage { channel, .. } if *channel == name
            ) {
                new_message = true;
            }
        }

        if ticks == 1 || new_message || ticks.is_multiple_of(CHANNEL_ASK_SNAPSHOT_EVERY_TICKS) {
            if let Some(reply) = find_channel_reply(name, question_seq)? {
                return Ok(Some(Some(reply)));
            }
        }

        if std::time::Instant::now() >= deadline {
            // Last-chance read before declaring a timeout: the reply may be
            // on disk even if its event already rotated out of the hub.
            return Ok(Some(find_channel_reply(name, question_seq)?));
        }

        std::thread::sleep(crate::api::server::CONNECTION_POLL_INTERVAL);
    }
}

/// The first retained message on `name` whose `in_reply_to` equals
/// `question_seq`, if any has landed yet.
fn find_channel_reply(
    name: &str,
    question_seq: u64,
) -> std::io::Result<Option<crate::api::schema::ChannelMessage>> {
    let since = crate::persist::channels::read_since(name, question_seq)?;
    Ok(since
        .messages
        .into_iter()
        .find(|message| message.in_reply_to == Some(question_seq)))
}

/// Parses `channel.ask`'s injection-step response (a `channel.send`-shaped
/// `ChannelSent`) for the assigned question `seq`, mirroring
/// `agent_from_response`'s error/result split: an `error` response passes
/// through as `Err` unchanged, a malformed success is an `internal_error`.
fn channel_ask_question_seq(request_id: &str, response: &str) -> Result<u64, ErrorResponse> {
    let value: serde_json::Value = serde_json::from_str(response).map_err(|_| ErrorResponse {
        id: request_id.into(),
        error: ErrorBody {
            code: "internal_error".into(),
            message: "failed to decode channel.ask response".into(),
        },
    })?;
    if value.get("error").is_some() {
        let error = serde_json::from_value(value["error"].clone()).map_err(|_| ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "internal_error".into(),
                message: "failed to decode channel.ask error".into(),
            },
        })?;
        return Err(ErrorResponse {
            id: request_id.into(),
            error,
        });
    }
    value["result"]["seq"]
        .as_u64()
        .ok_or_else(|| ErrorResponse {
            id: request_id.into(),
            error: ErrorBody {
                code: "internal_error".into(),
                message: "failed to decode channel.ask result seq".into(),
            },
        })
}

/// `channel.ask` entry point: appends and injects the question via one
/// normal (fast) App dispatch — `App::handle_channel_ask_question`, a thin
/// wrapper over `handle_channel_send_inner` — then blocks this connection
/// thread, never the App's own request loop, polling the durable
/// transcript for a reply. See [`crate::api::schema::ChannelAskParams`] for
/// the wire contract.
pub(super) fn ask_channel(
    request_id: String,
    params: crate::api::schema::ChannelAskParams,
    stream: &mut LocalStream,
    api_tx: &ApiRequestSender,
    event_hub: &EventHub,
    running: &Arc<AtomicBool>,
) -> std::io::Result<Option<String>> {
    let timeout_ms = params
        .timeout_ms
        .unwrap_or(DEFAULT_ASK_TIMEOUT_MS)
        .min(MAX_ASK_TIMEOUT_MS);
    let name = crate::persist::channels::normalize_channel_name(&params.name);

    let question_response = dispatch_to_app_with_timeout(
        Request {
            id: request_id.clone(),
            method: Method::ChannelAsk(params),
        },
        api_tx,
        None,
    );
    let question_seq = match channel_ask_question_seq(&request_id, &question_response) {
        Ok(seq) => seq,
        // Addressing/validation failed before anything was appended —
        // surface the original error as-is.
        Err(error) => {
            return serde_json::to_string(&error)
                .map(Some)
                .map_err(std::io::Error::other)
        }
    };

    let reply = match poll_channel_ask_reply(&name, question_seq, timeout_ms, event_hub, || {
        should_stop_connection(stream, running).unwrap_or(true)
    }) {
        Ok(Some(reply)) => reply,
        // The client went away mid-wait; there is nobody to answer.
        Ok(None) => return Ok(None),
        Err(err) => {
            return Ok(Some(
                serde_json::to_string(&ErrorResponse {
                    id: request_id,
                    error: ErrorBody {
                        code: "channel_ask_failed".into(),
                        message: err.to_string(),
                    },
                })
                .unwrap(),
            ));
        }
    };
    Ok(Some(
        serde_json::to_string(&SuccessResponse {
            id: request_id,
            result: ResponseResult::ChannelAsked {
                answered: reply.is_some(),
                question_seq,
                reply,
            },
        })
        .unwrap(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_wait_probe_only_translates_agent_disappearance() {
        let disappeared = agent_wait_probe_error(ErrorResponse {
            id: "wait".into(),
            error: ErrorBody {
                code: "agent_not_found".into(),
                message: "missing".into(),
            },
        })
        .unwrap();
        let disappeared: ErrorResponse = serde_json::from_str(&disappeared).unwrap();
        assert_eq!(disappeared.id, "wait");
        assert_eq!(disappeared.error.code, "agent_not_running");

        let unavailable = agent_wait_probe_error(ErrorResponse {
            id: "wait".into(),
            error: ErrorBody {
                code: "server_unavailable".into(),
                message: "timed out waiting for app response".into(),
            },
        })
        .unwrap();
        let unavailable: ErrorResponse = serde_json::from_str(&unavailable).unwrap();
        assert_eq!(unavailable.id, "wait");
        assert_eq!(unavailable.error.code, "server_unavailable");
    }

    mod channel_wait {
        use super::super::*;
        use crate::api::schema::{ChannelMessage, ChannelSenderKind, EventData, EventKind};
        use crate::api::EventHub;

        fn with_isolated_state_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
            let _guard = crate::config::test_config_env_lock().lock();
            let old_state = std::env::var_os("XDG_STATE_HOME");
            let dir = std::env::temp_dir().join(format!(
                "bora-channel-wait-test-{name}-{}",
                std::process::id()
            ));
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

        fn message(text: &str, seq: u64) -> ChannelMessage {
            ChannelMessage {
                ts: "2026-08-15T00:00:00Z".into(),
                seq,
                from_pane: "w1A:p2".into(),
                from_name: "brandos".into(),
                from_kind: ChannelSenderKind::Agent,
                text: text.into(),
                in_reply_to: None,
                to_pane: None,
                to_human: false,
            }
        }

        fn append(name: &str, text: &str) -> ChannelMessage {
            let seq = crate::persist::channels::next_seq(name);
            let appended = message(text, seq);
            crate::persist::channels::append_message(name, &appended).unwrap();
            appended
        }

        fn poll(
            name: &str,
            after_seq: u64,
            timeout_ms: Option<u64>,
            event_hub: &EventHub,
        ) -> ChannelWaitOutcome {
            poll_channel_wait(name, after_seq, timeout_ms, event_hub, || false)
                .expect("channel wait poll")
                .expect("not cancelled")
        }

        #[test]
        fn returns_backlog_immediately_without_waiting() {
            with_isolated_state_dir("backlog", || {
                append("eng", "one");
                append("eng", "two");
                let event_hub = EventHub::default();
                let started = std::time::Instant::now();
                let outcome = poll("eng", 0, Some(2_000), &event_hub);
                // Already-present history must satisfy the wait before any
                // blocking: this returns far under the 2s timeout.
                assert!(started.elapsed() < std::time::Duration::from_secs(1));
                assert!(!outcome.timed_out);
                assert!(!outcome.gap);
                assert_eq!(
                    outcome.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
                    vec![1, 2]
                );
            });
        }

        #[test]
        fn blocks_until_a_message_lands_then_returns_it() {
            with_isolated_state_dir("block", || {
                let event_hub = EventHub::default();
                let hub = event_hub.clone();
                let appender = std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let appended = append("eng", "late");
                    // Mirror what handle_channel_send does: durable append
                    // first, then the wake-up event.
                    hub.push(crate::api::schema::EventEnvelope {
                        event: EventKind::ChannelMessage,
                        data: EventData::ChannelMessage {
                            channel: "eng".into(),
                            seq: appended.seq,
                            from_pane: Some("w1A:p2".into()),
                            from_name: "brandos".into(),
                            text: appended.text,
                            to_pane: None,
                        },
                    });
                });
                let outcome = poll("eng", 0, Some(5_000), &event_hub);
                appender.join().expect("appender");
                assert!(!outcome.timed_out);
                assert_eq!(outcome.messages.len(), 1);
                assert_eq!(outcome.messages[0].text, "late");
                assert_eq!(outcome.messages[0].seq, 1);
            });
        }

        #[test]
        fn detects_rotation_gap_instead_of_silent_loss() {
            with_isolated_state_dir("gap", || {
                for i in 0..10 {
                    append("eng", &format!("msg{i}"));
                }
                let path = crate::persist::channels::channel_file_path("eng");
                // 10 lines > cap 4 -> keep newest 2 (seq 9, 10).
                rotate_for_test(&path, 4);
                let outcome = poll("eng", 3, Some(2_000), &EventHub::default());
                assert!(outcome.gap);
                assert_eq!(outcome.oldest_seq, Some(9));
                assert_eq!(
                    outcome.messages.iter().map(|m| m.seq).collect::<Vec<_>>(),
                    vec![9, 10]
                );
                assert!(!outcome.timed_out);
            });
        }

        #[test]
        fn timeout_is_a_clean_no_message_not_an_error() {
            with_isolated_state_dir("timeout", || {
                let outcome = poll("eng", 0, Some(120), &EventHub::default());
                assert!(outcome.timed_out);
                assert!(outcome.messages.is_empty());
                assert!(!outcome.gap);
                assert_eq!(outcome.oldest_seq, None);
            });
        }

        /// Test seam for `persist::channels::rotate_to_cap` (private there).
        fn rotate_for_test(path: &std::path::Path, max_lines: usize) {
            // Rotation is reachable through append_message's cap policy in
            // production; for a tight test we rewrite to the same newest-
            // half shape directly.
            let lines: Vec<String> = std::fs::read_to_string(path)
                .expect("channel log")
                .lines()
                .map(str::to_string)
                .collect();
            let keep_from = lines.len() - max_lines / 2;
            std::fs::write(path, lines[keep_from..].join("\n") + "\n").expect("rewrite");
        }
    }

    mod channel_ask {
        use super::super::*;
        use crate::api::schema::{ChannelMessage, ChannelSenderKind, EventData, EventKind};
        use crate::api::EventHub;

        fn with_isolated_state_dir<T>(name: &str, f: impl FnOnce() -> T) -> T {
            let _guard = crate::config::test_config_env_lock().lock();
            let old_state = std::env::var_os("XDG_STATE_HOME");
            let dir = std::env::temp_dir().join(format!(
                "bora-channel-ask-test-{name}-{}",
                std::process::id()
            ));
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

        fn message(text: &str, seq: u64, in_reply_to: Option<u64>) -> ChannelMessage {
            ChannelMessage {
                ts: "2026-08-19T00:00:00Z".into(),
                seq,
                from_pane: "w1A:p2".into(),
                from_name: "brandos".into(),
                from_kind: ChannelSenderKind::Agent,
                text: text.into(),
                in_reply_to,
                to_pane: None,
                to_human: false,
            }
        }

        fn append(name: &str, text: &str, in_reply_to: Option<u64>) -> ChannelMessage {
            let seq = crate::persist::channels::next_seq(name);
            let appended = message(text, seq, in_reply_to);
            crate::persist::channels::append_message(name, &appended).unwrap();
            appended
        }

        fn push_message_event(hub: &EventHub, name: &str, appended: &ChannelMessage) {
            hub.push(crate::api::schema::EventEnvelope {
                event: EventKind::ChannelMessage,
                data: EventData::ChannelMessage {
                    channel: name.into(),
                    seq: appended.seq,
                    from_pane: Some(appended.from_pane.clone()),
                    from_name: appended.from_name.clone(),
                    text: appended.text.clone(),
                    to_pane: None,
                },
            });
        }

        #[test]
        fn matching_in_reply_to_resolves_the_wait() {
            with_isolated_state_dir("match", || {
                let question = append("eng", "are you there?", None);
                let question_seq = question.seq;
                let event_hub = EventHub::default();
                let hub = event_hub.clone();
                let replier = std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let reply = append("eng", "yes", Some(question_seq));
                    push_message_event(&hub, "eng", &reply);
                });
                let outcome =
                    poll_channel_ask_reply("eng", question_seq, 5_000, &event_hub, || false)
                        .expect("poll")
                        .expect("not cancelled");
                replier.join().expect("replier");
                let reply = outcome.expect("matching reply must resolve the wait");
                assert_eq!(reply.text, "yes");
                assert_eq!(reply.in_reply_to, Some(question_seq));
            });
        }

        /// Mutation coverage for the `in_reply_to == question_seq` check in
        /// `find_channel_reply`: a reply threaded onto a DIFFERENT seq must
        /// never satisfy this ask's wait — otherwise two concurrent asks on
        /// the same channel would cross-wire their replies. Dropping the
        /// comparison (or replacing it with "any new message") makes this
        /// fail.
        #[test]
        fn mismatched_in_reply_to_does_not_resolve() {
            with_isolated_state_dir("mismatch", || {
                let question = append("eng", "are you there?", None);
                let other = append("eng", "unrelated message", None);
                append("eng", "reply to the wrong question", Some(other.seq));
                let outcome =
                    poll_channel_ask_reply("eng", question.seq, 200, &EventHub::default(), || {
                        false
                    })
                    .expect("poll")
                    .expect("not cancelled");
                assert!(
                    outcome.is_none(),
                    "a reply addressed to a different seq must not resolve this ask: {outcome:?}"
                );
            });
        }

        #[test]
        fn no_reply_times_out_cleanly() {
            with_isolated_state_dir("timeout", || {
                let question = append("eng", "hello?", None);
                let outcome =
                    poll_channel_ask_reply("eng", question.seq, 150, &EventHub::default(), || {
                        false
                    })
                    .expect("poll")
                    .expect("not cancelled");
                assert!(outcome.is_none());
            });
        }
    }
}
