// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// add custom hooks/plugins beside this file instead of editing it.
// HERDR_INTEGRATION_ID=pi
// HERDR_INTEGRATION_VERSION=6
// @ts-nocheck

import net from "node:net";

const HERDR_ENV = process.env.HERDR_ENV;
const socketPath = process.env.HERDR_SOCKET_PATH;
const socketEndpoint =
  process.platform === "win32" && socketPath ? `\\\\.\\pipe\\${socketPath}` : socketPath;
const paneId = process.env.HERDR_PANE_ID;
const source = "herdr:pi";

function enabled() {
  return HERDR_ENV === "1" && !!socketPath && !!paneId;
}

function sendRequest(request: unknown): Promise<void> {
  if (!enabled()) {
    return Promise.resolve();
  }

  return new Promise((resolve) => {
    let done = false;
    const finish = () => {
      if (done) return;
      done = true;
      socket.destroy();
      resolve();
    };

    const socket = net.createConnection(socketEndpoint!);
    socket.on("error", () => finish(false));
    socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on("data", finish);
    socket.on("end", finish);
    const timeout = setTimeout(finish, 500);
    timeout.unref?.();
  });
}

type AgentState = "working" | "blocked" | "idle";

type QueuedState = {
  state: AgentState;
  message?: string;
  seq: number;
};

let reportSeq = Date.now() * 1000;
let currentAgentSessionId: string | undefined;
let currentAgentSessionPath: string | undefined;

function nextReportSeq(): number {
  reportSeq += 1;
  return reportSeq;
}

function updateSessionRef(ctx: any): void {
  try {
    const file = ctx?.sessionManager?.getSessionFile?.();
    currentAgentSessionPath =
      typeof file === "string" && file.startsWith("/") ? file : undefined;
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

function currentSessionRef(): Record<string, unknown> | undefined {
  if (currentAgentSessionPath) {
    return { agent_session_path: currentAgentSessionPath };
  }
  if (currentAgentSessionId) {
    return { agent_session_id: currentAgentSessionId };
  }
  return undefined;
}

function reportSession(): Promise<void> {
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
      agent: "pi",
      seq: nextReportSeq(),
      ...sessionRef,
    },
  });
}

function sendState(state: AgentState, message?: string, seq = nextReportSeq()): Promise<void> {
  return sendRequest({
    id: `${source}:${Date.now()}:${Math.random().toString(36).slice(2)}`,
    method: "pane.report_agent",
    params: withSessionRef({
      pane_id: paneId,
      source,
      agent: "pi",
      state,
      message,
      seq,
    }),
  });
}

function releaseAgent(): Promise<void> {
  return sendRequest({
    id: `${source}:release:${Date.now()}:${Math.random().toString(36).slice(2)}`,
    method: "pane.release_agent",
    params: {
      pane_id: paneId,
      source,
      agent: "pi",
      seq: nextReportSeq(),
    },
  });
}

function shouldReleaseOnSessionShutdown(event: any): boolean {
  // Pi tears down and rebinds extension runtimes for internal lifecycle actions
  // such as /reload, /new, /resume, and /fork. Those do not mean the pane's
  // agent process has exited, and releasing hook authority there can suppress
  // legitimate reports from the replacement runtime. Only a user/process quit
  // should release Herdr's full-lifecycle authority.
  const reason = event?.reason;
  return reason === "quit";
}

let sendInFlight = false;
let queuedState: QueuedState | undefined;

function queueState(state: AgentState, message?: string): void {
  queuedState = { state, message, seq: nextReportSeq() };
  if (!sendInFlight) {
    void drainStateQueue();
  }
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
      await sendState(next.state, next.message, next.seq);
    }
  } finally {
    sendInFlight = false;
    if (queuedState) {
      void drainStateQueue();
    }
  }
}

export default function (pi) {
  if (!enabled()) {
    return;
  }

  let agentActiveCount = 0;
  let retryHoldActive = false;
  let failureBlocked = false;
  let failureMessage: string | undefined;
  let idleTimer: ReturnType<typeof setTimeout> | undefined;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let blockedCount = 0;
  let blockedMessage: string | undefined;
  let lastState: AgentState | undefined;
  let lastMessage: string | undefined;
  let rootSession = false;
  let turnRepairHold = false;

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
      return { state: "blocked" as const, message: blockedMessage };
    }
    if (failureBlocked) {
      return { state: "blocked" as const, message: failureMessage };
    }
    if (agentActiveCount > 0 || retryHoldActive || turnRepairHold) {
      return { state: "working" as const, message: undefined };
    }
    return { state: "idle" as const, message: undefined };
  }

  function publishState(force = false) {
    const next = desiredState();
    if (!force && next.state === lastState && next.message === lastMessage) {
      return;
    }
    lastState = next.state;
    lastMessage = next.message;
    queueState(next.state, next.message);
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
      blockedCount = Math.max(0, blockedCount - 1);
      if (blockedCount === 0) {
        blockedMessage = undefined;
      }
      publishState();
      return;
    }

    clearPendingTimers();
    blockedCount += 1;
    blockedMessage = data.label;
    publishState();
  });

  pi.on("session_start", (_event, ctx) => {
    if (ctx?.hasUI !== true) {
      return;
    }
    rootSession = true;
    updateSessionRef(ctx);
    void reportSession();
    publishState(true);
  });

  pi.on("agent_start", (_event, ctx) => {
    if (!rootSession) {
      return;
    }
    updateSessionRef(ctx);
    void reportSession();
    clearPendingTimers();
    clearFailureState();
    turnRepairHold = false;
    agentActiveCount += 1;
    publishState();
  });

  pi.on("agent_end", (event) => {
    if (!rootSession) {
      return;
    }
    if (agentActiveCount === 0) {
      if (turnRepairHold) {
        // The loop we were holding Working for has ended (or a late duplicate
        // end arrived mid-turn; the next turn_start re-holds). Release the
        // repair hold and go idle normally.
        turnRepairHold = false;
        forceResetBlocked();
        scheduleIdle();
        return;
      }
      // Pi can emit duplicate/late end events while auto-retry is already
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

    forceResetBlocked();
    scheduleIdle();
  });

  pi.on("turn_start", (_event, ctx) => {
    if (!rootSession) {
      // A runtime rebound by /reload, /new, /resume, or /fork can miss the
      // original session_start; a turn with UI proves this is the
      // interactive root runtime.
      if (ctx?.hasUI !== true) {
        return;
      }
      rootSession = true;
      updateSessionRef(ctx);
      void reportSession();
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

  pi.on("session_shutdown", async (event) => {
    if (!rootSession) {
      return;
    }
    if (shouldReleaseOnSessionShutdown(event)) {
      await releaseAgent();
    }
  });
}
