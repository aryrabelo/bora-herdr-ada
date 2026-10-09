// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// add custom hooks/plugins beside this file instead of editing it.
// HERDR_INTEGRATION_ID=omp
// HERDR_INTEGRATION_VERSION=12
// @ts-nocheck

import net from "node:net";
import path from "node:path";

const HERDR_ENV = process.env.HERDR_ENV;
const socketPath = process.env.HERDR_SOCKET_PATH;
const socketEndpoint =
  process.platform === "win32" && socketPath ? `\\\\.\\pipe\\${socketPath}` : socketPath;
const paneId = process.env.HERDR_PANE_ID;
const source = "herdr:omp";
// OMP marks every shell it spawns with OMPCODE=1. A nested `omp` launched from
// a parent session's shell inherits it, so that process is not the pane's root
// agent and must not report its short-lived session over the parent's.
const nestedOmpSession = process.env.OMPCODE === "1";

function enabled() {
  return HERDR_ENV === "1" && !!socketPath && !!paneId && !nestedOmpSession;
}

let requestQueue = Promise.resolve();

function sendRequestAttempt(request: unknown, timeoutMs: number): Promise<boolean> {
  if (!enabled()) {
    return Promise.resolve(true);
  }

  return new Promise((resolve) => {
    let done = false;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const finish = (delivered: boolean) => {
      if (done) return;
      done = true;
      if (timeout) {
        clearTimeout(timeout);
      }
      socket.destroy();
      resolve(delivered);
    };

    const socket = net.createConnection(socketEndpoint!);
    socket.on("error", () => finish(false));
    socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on("data", () => finish(true));
    socket.on("end", () => finish(false));
    timeout = setTimeout(() => finish(false), timeoutMs);
    timeout.unref?.();
  });
}

async function sendRequestNow(request: unknown): Promise<boolean> {
  if (await sendRequestAttempt(request, 500)) {
    return true;
  }
  return sendRequestAttempt(request, 1500);
}

function sendRequest(request: unknown): Promise<boolean> {
  const send = () => sendRequestNow(request);
  const next = requestQueue.then(send, send);
  requestQueue = next.then(
    () => undefined,
    () => undefined,
  );
  return next;
}

type AgentState = "working" | "blocked" | "idle";

type QueuedState = {
  state: AgentState;
  message?: string;
  background: boolean;
  seq: number;
};

const idleDebounceMs = parseDurationEnv("HERDR_OMP_IDLE_DEBOUNCE_MS", 250);
const retryGraceMs = parseDurationEnv("HERDR_OMP_RETRY_GRACE_MS", 2500);
const heartbeatMs = parseDurationEnv("HERDR_OMP_HEARTBEAT_MS", 15_000);
const retryableErrorPattern =
  /overloaded|provider.?returned.?error|rate.?limit|too many requests|429|500|502|503|504|service.?unavailable|server.?error|internal.?error|network.?error|connection.?error|connection.?refused|connection.?lost|websocket.?closed|websocket.?error|other side closed|fetch failed|upstream.?connect|reset before headers|socket hang up|ended without|http2 request did not get a response|timed? out|timeout|terminated|retry delay/i;
let reportSeq = Date.now() * 1000;
let currentAgentSessionId: string | undefined;
let currentAgentSessionPath: string | undefined;

// Subagent sessions (task-tool async jobs, eval `agent()`, workpools) run in
// this same process with their own extension binding, and keep running after
// the root turn ended. Module globals are shared by every binding, so the
// registry of live subagents lives here; it is pinned on globalThis so a
// duplicate module instance still shares it.
type SubagentRegistry = { active: Set<string>; listeners: Set<() => void> };
type AgentInfo = { kind?: string; id?: string };
type HookCtx = { hasUI?: boolean; agent?: AgentInfo | null } | null | undefined;
const subagentRegistryKey = Symbol.for("herdr.omp.subagents.v1");
// globalThis is an open bag of process-wide slots; the symbol key is ours.
const processSlots = globalThis as unknown as Record<symbol, SubagentRegistry | undefined>;
const subagents: SubagentRegistry = (processSlots[subagentRegistryKey] ??= {
  active: new Set<string>(),
  listeners: new Set<() => void>(),
});
const moduleInstanceTag = Math.random().toString(36).slice(2);
let bindingCounter = 0;

function notifySubagentListeners(): void {
  for (const listener of [...subagents.listeners]) {
    try {
      listener();
    } catch {
      // A failing root binding must not break subagent bookkeeping.
    }
  }
}

// Only real subagents count: the advisor is a "sub" session too, but it
// shadows the root turn and must never hold the pane Working on its own.
// Builds without `ctx.agent` never match, preserving the old behavior.
function isSubagentSession(ctx: HookCtx): boolean {
  return ctx?.agent?.kind === "sub" && ctx.agent.id !== "advisor";
}

function nextReportSeq(): number {
  reportSeq += 1;
  return reportSeq;
}

export function isAbsoluteSessionPath(file: unknown): file is string {
  return (
    typeof file === "string" &&
    (path.posix.isAbsolute(file) || path.win32.isAbsolute(file))
  );
}

function updateSessionRef(ctx: any): void {
  try {
    const file = ctx?.sessionManager?.getSessionFile?.();
    currentAgentSessionPath = isAbsoluteSessionPath(file) ? file : undefined;
  } catch {
    currentAgentSessionPath = undefined;
  }

  try {
    const id = ctx?.sessionManager?.getSessionId?.();
    currentAgentSessionId = typeof id === "string" && id.length > 0 ? id : undefined;
  } catch {
    currentAgentSessionId = undefined;
  }
}

function withSessionRef(params: Record<string, unknown>): Record<string, unknown> {
  if (currentAgentSessionPath) {
    return { ...params, agent_session_path: currentAgentSessionPath };
  }
  if (currentAgentSessionId) {
    return { ...params, agent_session_id: currentAgentSessionId };
  }
  return params;
}

function parseDurationEnv(name: string, fallback: number): number {
  const raw = process.env[name];
  if (!raw) {
    return fallback;
  }
  const parsed = Number.parseInt(raw, 10);
  if (!Number.isFinite(parsed) || parsed < 0) {
    return fallback;
  }
  return parsed;
}

function currentSessionRef(): Record<string, unknown> | undefined {
  if (currentAgentSessionPath) {
    return { agent_session_path: currentAgentSessionPath };
  }
  if (currentAgentSessionId) {
    return { agent_session_id: currentAgentSessionId };
  }
  return undefined;
}

function reportSession(sessionStartSource = "startup"): Promise<void> {
  const sessionRef = currentSessionRef();
  if (!sessionRef) {
    return Promise.resolve();
  }

  return sendRequest({
    id: `${source}:session:${Date.now()}:${Math.random().toString(36).slice(2)}`,
    method: "pane.report_agent_session",
    params: {
      pane_id: paneId,
      source,
      agent: "omp",
      seq: nextReportSeq(),
      session_start_source: sessionStartSource,
      ...sessionRef,
    },
  }).then(() => undefined);
}

function sendState(
  state: AgentState,
  message?: string,
  seq = nextReportSeq(),
  background = false,
): Promise<boolean> {
  const params: Record<string, unknown> = {
    pane_id: paneId,
    source,
    agent: "omp",
    state,
    message,
    seq,
  };
  if (background) {
    // Working only because subagents still run: tells herdr not to reconcile
    // the pane to Idle from omp's idle prompt title.
    params.background = true;
  }
  return sendRequest({
    id: `${source}:${Date.now()}:${Math.random().toString(36).slice(2)}`,
    method: "pane.report_agent",
    params: withSessionRef(params),
  });
}

// A state report that herdr never acknowledged is retried until it lands or
// a newer state supersedes it: a dropped idle would otherwise be final and the
// pane would sit Working in the sidebar until the next turn.
const stateRetryDelaysMs = [250, 1000, 3000];
const stateRetryIntervalMs = 10_000;

let sendInFlight = false;
let queuedState: QueuedState | undefined;
let wakeRetry: (() => void) | undefined;

function queueState(state: AgentState, message?: string, background = false): void {
  queuedState = { state, message, background, seq: nextReportSeq() };
  // A newer state cancels the backoff of the one it supersedes.
  wakeRetry?.();
  if (!sendInFlight) {
    void drainStateQueue();
  }
}

function sleepUntilWoken(ms: number): Promise<void> {
  const { promise, resolve } = Promise.withResolvers<void>();
  const timer = setTimeout(() => {
    wakeRetry = undefined;
    resolve();
  }, ms);
  timer.unref?.();
  wakeRetry = () => {
    clearTimeout(timer);
    wakeRetry = undefined;
    resolve();
  };
  return promise;
}

async function drainStateQueue(): Promise<void> {
  if (sendInFlight) {
    return;
  }

  sendInFlight = true;
  try {
    while (queuedState) {
      const next = queuedState;
      queuedState = undefined;
      for (let attempt = 0; !queuedState; attempt += 1) {
        if (await sendState(next.state, next.message, next.seq, next.background)) {
          break;
        }
        if (queuedState) {
          break;
        }
        await sleepUntilWoken(stateRetryDelaysMs[attempt] ?? stateRetryIntervalMs);
      }
    }
  } finally {
    sendInFlight = false;
    if (queuedState) {
      void drainStateQueue();
    }
  }
}

function lastAssistantMessage(messages: unknown[]): any | undefined {
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const message = messages[i] as any;
    if (message?.role === "assistant") {
      return message;
    }
  }
  return undefined;
}

function retryableErrorMessage(event: any): string | undefined {
  const messages = Array.isArray(event?.messages) ? event.messages : [];
  const assistant = lastAssistantMessage(messages);
  if (assistant?.stopReason !== "error") {
    return undefined;
  }

  const errorMessage = String(assistant.errorMessage ?? "");
  if (!retryableErrorPattern.test(errorMessage)) {
    return undefined;
  }
  return errorMessage || "retryable provider error";
}

function askBlockedMessage(args: any): string {
  const questions = Array.isArray(args?.questions) ? args.questions : [];
  const firstQuestion = questions.find((question: any) => typeof question?.question === "string");
  if (firstQuestion?.question) {
    return firstQuestion.question;
  }
  return "waiting for user input";
}

export default function (pi) {
  if (!enabled()) {
    return;
  }

  let agentActiveCount = 0;
  let retryHoldActive = false;
  let failureBlocked = false;
  let failureMessage: string | undefined;
  let blockedCount = 0;
  let blockedMessage: string | undefined;
  let lastState: AgentState | undefined;
  let lastMessage: string | undefined;
  let lastBackground = false;
  let idleTimer: ReturnType<typeof setTimeout> | undefined;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let heartbeatTimer: ReturnType<typeof setInterval> | undefined;
  let rootSession = false;
  let turnRepairHold = false;
  // The root loop ended with `willContinue: true`: omp has a continuation
  // scheduled (e.g. it will be woken by an async subagent's result) and sits
  // at the idle prompt until then. Still Working, and when subagents are what
  // it waits on, Working in the background.
  let awaitingContinuation = false;
  const bindingSerial = `${moduleInstanceTag}:${++bindingCounter}`;
  // Registry keys this binding added, one per subagent id it hosts.
  const ownedSubagentKeys = new Set<string>();

  function addSubagent(key: string) {
    if (subagents.active.has(key)) {
      return;
    }
    subagents.active.add(key);
    ownedSubagentKeys.add(key);
    notifySubagentListeners();
  }

  function removeSubagents(keys: Iterable<string>) {
    let removed = false;
    for (const key of [...keys]) {
      ownedSubagentKeys.delete(key);
      removed = subagents.active.delete(key) || removed;
    }
    if (removed) {
      notifySubagentListeners();
    }
  }

  // Keys an end/shutdown event refers to: the ctx's subagent when it carries
  // agent info, else whatever this binding registered (ctx-less events).
  function endingSubagentKeys(ctx: HookCtx): string[] {
    if (isSubagentSession(ctx)) {
      return [`sub:${ctx?.agent?.id}:${bindingSerial}`];
    }
    return ctx?.agent == null ? [...ownedSubagentKeys] : [];
  }

  const onSubagentsChanged = () => {
    if (rootSession) {
      publishState();
    }
  };

  function clearTimer(timer: ReturnType<typeof setTimeout> | undefined) {
    if (timer) {
      clearTimeout(timer);
    }
  }

  function clearPendingTimers() {
    clearTimer(idleTimer);
    clearTimer(retryTimer);
    idleTimer = undefined;
    retryTimer = undefined;
  }

  function clearFailureState() {
    retryHoldActive = false;
    failureBlocked = false;
    failureMessage = undefined;
  }

  function desiredState() {
    if (blockedCount > 0) {
      return { state: "blocked" as const, message: blockedMessage, background: false };
    }
    if (failureBlocked) {
      return { state: "blocked" as const, message: failureMessage, background: false };
    }
    const rootWorking = agentActiveCount > 0 || retryHoldActive || turnRepairHold;
    const subagentsRunning = subagents.active.size > 0;
    if (rootWorking || subagentsRunning || awaitingContinuation) {
      return {
        state: "working" as const,
        message: undefined,
        background: !rootWorking && subagentsRunning,
      };
    }
    return { state: "idle" as const, message: undefined, background: false };
  }

  function publishState(force = false) {
    const next = desiredState();
    if (
      !force &&
      next.state === lastState &&
      next.message === lastMessage &&
      next.background === lastBackground
    ) {
      return;
    }
    lastState = next.state;
    lastMessage = next.message;
    lastBackground = next.background;
    queueState(next.state, next.message, next.background);
  }

  function scheduleIdle() {
    clearPendingTimers();
    clearFailureState();
    idleTimer = setTimeout(() => {
      idleTimer = undefined;
      publishState();
    }, idleDebounceMs);
    idleTimer.unref?.();
  }

  function holdForRetry(message: string) {
    clearPendingTimers();
    retryHoldActive = true;
    failureBlocked = false;
    failureMessage = message;
    publishState();

    retryTimer = setTimeout(() => {
      retryTimer = undefined;
      retryHoldActive = false;
      failureBlocked = true;
      publishState();
    }, retryGraceMs);
    retryTimer.unref?.();
  }

  function activateRootSession(ctx: any, sessionStartSource = "startup"): boolean {
    if (ctx?.hasUI !== true || isSubagentSession(ctx)) {
      return false;
    }
    rootSession = true;
    subagents.listeners.add(onSubagentsChanged);
    updateSessionRef(ctx);
    void reportSession(sessionStartSource);
    armHeartbeat();
    return true;
  }

  // While this runtime owns the pane, periodically re-send the current state
  // regardless of change dedupe. Together with the delivery retries this
  // bounds how long herdr can hold a state the extension no longer believes.
  function armHeartbeat() {
    if (heartbeatMs === 0 || heartbeatTimer) {
      return;
    }
    heartbeatTimer = setInterval(() => publishState(true), heartbeatMs);
    heartbeatTimer.unref?.();
  }

  function clearHeartbeat() {
    if (heartbeatTimer) {
      clearInterval(heartbeatTimer);
      heartbeatTimer = undefined;
    }
  }

  function resetSessionState() {
    clearPendingTimers();
    clearFailureState();
    agentActiveCount = 0;
    turnRepairHold = false;
    awaitingContinuation = false;
    blockedCount = 0;
    blockedMessage = undefined;
  }

  function activateBlocked(message: string | undefined) {
    clearPendingTimers();
    blockedCount += 1;
    blockedMessage = message;
    publishState();
  }

  function deactivateBlocked() {
    blockedCount = Math.max(0, blockedCount - 1);
    if (blockedCount === 0) {
      blockedMessage = undefined;
    }
    publishState();
  }

  function forceResetBlocked() {
    // A turn ending is authoritative that nothing is blocked. Clear any leaked
    // blockedCount (unmatched approval/ask or a dropped herdr:blocked deactivate)
    // so a stuck block can't survive into Idle.
    blockedCount = 0;
    blockedMessage = undefined;
  }

  pi.events.on("herdr:blocked", (data) => {
    if (!rootSession) {
      return;
    }
    if (!data?.active) {
      deactivateBlocked();
      return;
    }

    activateBlocked(data.label);
  });

  pi.on("session_start", (_event, ctx) => {
    if (!activateRootSession(ctx)) {
      return;
    }

    publishState(true);
  });

  pi.on("session_switch", (event, ctx) => {
    if (!activateRootSession(ctx, event?.reason || "resume")) {
      return;
    }
    resetSessionState();
    publishState(true);
  });

  // Subagent lifecycle (handled before the root-session guards): omp's task
  // async jobs, eval `agent()` and workpools keep running after the root turn
  // ended and the idle prompt is back. Each live subagent counts toward the
  // root pane's Working (reported as `background`), so the pane only goes Idle
  // once the last one finishes.
  pi.on("agent_start", (_event, ctx) => {
    if (isSubagentSession(ctx)) {
      addSubagent(`sub:${ctx.agent.id}:${bindingSerial}`);
      return;
    }
    if (!rootSession && !activateRootSession(ctx)) {
      return;
    }
    updateSessionRef(ctx);
    void reportSession();
    clearPendingTimers();
    clearFailureState();
    turnRepairHold = false;
    awaitingContinuation = false;
    agentActiveCount += 1;
    publishState();
  });

  pi.on("tool_approval_requested", (event, ctx) => {
    if (!rootSession && !activateRootSession(ctx)) {
      return;
    }
    const label = event?.reason || `${event?.toolName || "Tool"} approval`;
    activateBlocked(label);
  });

  pi.on("tool_approval_resolved", (_event, ctx) => {
    if (!rootSession && !activateRootSession(ctx)) {
      return;
    }
    deactivateBlocked();
  });

  pi.on("tool_execution_start", (event, ctx) => {
    if (event?.toolName !== "ask") {
      return;
    }
    if (!rootSession && !activateRootSession(ctx)) {
      return;
    }
    activateBlocked(askBlockedMessage(event.args));
  });

  pi.on("tool_execution_end", (event, ctx) => {
    if (event?.toolName !== "ask") {
      return;
    }
    if (!rootSession && !activateRootSession(ctx)) {
      return;
    }
    deactivateBlocked();
  });

  pi.on("agent_end", (event, ctx) => {
    const subagentKeys = endingSubagentKeys(ctx);
    if (subagentKeys.length > 0) {
      // A scheduled continuation keeps the subagent alive.
      if (event?.willContinue !== true) {
        removeSubagents(subagentKeys);
      }
      return;
    }
    if (!rootSession) {
      return;
    }
    if (event?.willContinue === true) {
      // A continuation is already scheduled: this loop ended, and omp fires
      // agent_start again when the continuation runs (measured on omp 18.8.7
      // with an async task). Release this loop's count so that agent_start
      // does not double-count it, and hold Working until the continuation
      // runs or the final end settles. Older builds omit the field.
      agentActiveCount = Math.max(0, agentActiveCount - 1);
      turnRepairHold = false;
      awaitingContinuation = true;
      clearPendingTimers();
      publishState();
      return;
    }
    if (agentActiveCount === 0) {
      if (turnRepairHold || awaitingContinuation) {
        // The loop we were holding Working for has ended (or a late duplicate
        // end arrived mid-turn; the next turn_start re-holds). Release the
        // hold and go idle normally.
        turnRepairHold = false;
        awaitingContinuation = false;
        forceResetBlocked();
        scheduleIdle();
        return;
      }
      // OMP can emit duplicate/late end events while auto-retry is already
      // holding the pane in Working, and a concurrent subagent's end can
      // arrive after the count already drained. Ignore unmatched ends so they
      // cannot cancel a retry hold or publish a false Idle.
      return;
    }

    agentActiveCount -= 1;
    if (agentActiveCount > 0) {
      // Other concurrent agents (e.g. parallel subagents) are still running;
      // stay Working until the last one ends.
      return;
    }

    const retryableMessage = retryableErrorMessage(event);
    if (retryableMessage) {
      holdForRetry(retryableMessage);
      return;
    }

    awaitingContinuation = false;
    forceResetBlocked();
    scheduleIdle();
  });

  pi.on("turn_start", (_event, ctx) => {
    if (isSubagentSession(ctx)) {
      return;
    }
    // A runtime rebound by /reload, /new, /resume, or /fork can miss the
    // original session_start; a turn with UI proves this is the interactive
    // root runtime.
    if (!rootSession && !activateRootSession(ctx)) {
      return;
    }
    // A turn proves the agent loop is alive: duplicate/late agent_end events
    // can drain agentActiveCount mid-run (e.g. concurrent subagent fan-out),
    // and a fire-and-forget report can be dropped. Hold Working until real
    // bookkeeping resumes (agent_start) or the loop ends (agent_end), and
    // force a re-publish so the pane self-heals on every turn.
    if (agentActiveCount === 0 && !turnRepairHold) {
      clearPendingTimers();
      clearFailureState();
      turnRepairHold = true;
    }
    publishState(true);
  });

  pi.on("session_shutdown", (_event, ctx) => {
    removeSubagents(endingSubagentKeys(ctx));
    if (rootSession) {
      subagents.listeners.delete(onSubagentsChanged);
      clearPendingTimers();
      clearHeartbeat();
    }
  });
}
