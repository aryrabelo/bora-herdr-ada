// Regression tests for the agent-state integration plugins.
//
// Bug class: an agent's pane went idle in herdr while it was still working,
// because overlapping/concurrent agent activity was tracked imprecisely.
//   - omp/pi tracked active agents with a boolean, so the first agent_end among
//     overlapping subagents reported idle while others were still running.
//   - kilo reported state for every session id, so a subagent (child) session
//     going idle clobbered the pane while the root agent was still working.
//
// These tests drive the *real* shipped assets through a fake `pi`/plugin host
// and a mocked node:net that captures the JSON-RPC reports synchronously, with
// fake timers controlling the idle debounce (no wall-clock waits).

import { beforeAll, afterEach, expect, jest, mock, test } from "bun:test";

// The assets read env and bind node:net at module load, so both must be in
// place before the asset is imported.
process.env.HERDR_ENV = "1";
process.env.HERDR_SOCKET_PATH = "/tmp/herdr-agent-state-test.sock"; // unused; net is mocked
process.env.HERDR_PANE_ID = "test-pane";
process.env.HERDR_OMP_IDLE_DEBOUNCE_MS = "50";
process.env.HERDR_PI_IDLE_DEBOUNCE_MS = "50";

type Report = { method?: string; params?: { state?: string } };

let reportedStates: string[] = [];

function capture(raw: unknown): void {
  for (const line of String(raw).split("\n")) {
    if (!line.trim()) continue;
    let parsed: Report;
    try {
      parsed = JSON.parse(line) as Report;
    } catch {
      continue;
    }
    if (parsed?.method === "pane.report_agent" && typeof parsed.params?.state === "string") {
      reportedStates.push(parsed.params.state);
    }
  }
}

// Fake unix socket: captures the written payload, then resolves the asset's
// send promise by emitting connect/data/end/close on later microtasks (after
// the asset has registered its listeners). No real I/O, no timers.
function fakeCreateConnection(_path: string, connectListener?: () => void) {
  const listeners = new Map<string, (...args: unknown[]) => void>();
  const socket = {
    on(event: string, cb: (...args: unknown[]) => void) {
      listeners.set(event, cb);
      return socket;
    },
    write(data: unknown) {
      capture(data);
      queueMicrotask(() => {
        listeners.get("data")?.(Buffer.from(""));
        listeners.get("end")?.();
        listeners.get("close")?.();
      });
      return true;
    },
    setTimeout() {
      return socket;
    },
    destroy() {},
    end() {},
    unref() {
      return socket;
    },
  };
  queueMicrotask(() => {
    connectListener?.();
    listeners.get("connect")?.();
  });
  return socket;
}

// Drain the bounded microtask chain the fake socket / state queue produce.
async function flush(): Promise<void> {
  for (let i = 0; i < 16; i += 1) {
    await Promise.resolve();
  }
}

beforeAll(async () => {
  await mock.module("node:net", () => ({
    createConnection: fakeCreateConnection,
    default: { createConnection: fakeCreateConnection },
  }));
});

afterEach(() => {
  reportedStates = [];
  jest.useRealTimers();
});

// omp and pi share the same counter-based state machine.
for (const agent of ["omp", "pi"] as const) {
  test(`${agent}: overlapping subagents keep the pane working until the last agent_end`, async () => {
    jest.useFakeTimers();
    // Module-loading boundary: env + node:net mock must be applied before the
    // asset binds them at load, so this import is intentionally dynamic.
    const mod = await import(`./${agent}/herdr-agent-state.ts`);
    const handlers = new Map<string, (...args: unknown[]) => void>();
    const pi = {
      on: (name: string, cb: (...args: unknown[]) => void) => {
        handlers.set(name, cb);
      },
      events: {
        on: (name: string, cb: (...args: unknown[]) => void) => {
          handlers.set(`events:${name}`, cb);
        },
      },
    };
    mod.default(pi);
    const fire = (name: string, ...args: unknown[]) => handlers.get(name)?.(...args);

    fire("session_start", {}, { hasUI: true });
    await flush();
    fire("agent_start", {}, {}); // agent A
    await flush();
    fire("agent_start", {}, {}); // agent B (concurrent)
    await flush();
    expect(reportedStates.at(-1)).toBe("working");

    // A ends while B is still active: the pane MUST stay working.
    fire("agent_end", {});
    jest.advanceTimersByTime(200); // well past the idle debounce
    await flush();
    expect(reportedStates.at(-1)).toBe("working");

    // B (the last agent) ends: only now should the pane go idle.
    fire("agent_end", {});
    jest.advanceTimersByTime(200);
    await flush();
    expect(reportedStates.at(-1)).toBe("idle");
  });
}

// turn_start is a per-turn heartbeat: a turn proves the agent loop is alive, so
// the handler repairs a drained agentActiveCount (a duplicate/late agent_end
// during subagent fan-out, or a dropped fire-and-forget report) and force-
// re-publishes, and it adopts a rebound runtime that missed session_start.
for (const agent of ["omp", "pi"] as const) {
  // Build a fresh fake host bound to the real shipped asset. Dynamic import is
  // intentional and load-order sensitive: the specifier is runtime-selected by
  // `agent`, and the env + node:net mock must be applied before the asset binds
  // them at module load. Returns `fire(name, ...args)` for the captured hooks.
  const spawn = async () => {
    const mod = await import(`./${agent}/herdr-agent-state.ts`);
    const handlers = new Map<string, (...args: unknown[]) => void>();
    const pi = {
      on: (name: string, cb: (...args: unknown[]) => void) => {
        handlers.set(name, cb);
      },
      events: {
        on: (name: string, cb: (...args: unknown[]) => void) => {
          handlers.set(`events:${name}`, cb);
        },
      },
    };
    mod.default(pi);
    return (name: string, ...args: unknown[]) => handlers.get(name)?.(...args);
  };

  test(`${agent}: turn_start repairs a count drained by a duplicate agent_end`, async () => {
    jest.useFakeTimers();
    const fire = await spawn();

    fire("session_start", {}, { hasUI: true });
    await flush();
    fire("agent_start", {}, {}); // agent A
    await flush();
    fire("agent_start", {}, {}); // agent B (concurrent)
    await flush();
    expect(reportedStates.at(-1)).toBe("working");

    // Two ends drain the count to 0; the third is a late duplicate the count==0
    // guard ignores. The pane goes idle mid-run — the bug this heals.
    fire("agent_end", {});
    fire("agent_end", {});
    fire("agent_end", {}); // duplicate/late
    jest.advanceTimersByTime(200); // past the idle debounce
    await flush();
    expect(reportedStates.at(-1)).toBe("idle");

    // A new turn proves the loop is still alive: repair the count and force a
    // re-publish so the pane self-heals back to working.
    fire("turn_start", {}, {});
    await flush();
    expect(reportedStates.at(-1)).toBe("working");

    // The repaired count drains normally on the next real agent_end.
    fire("agent_end", {});
    jest.advanceTimersByTime(200);
    await flush();
    expect(reportedStates.at(-1)).toBe("idle");
  });

  test(`${agent}: turn_start with UI adopts a rebound runtime that missed session_start`, async () => {
    jest.useFakeTimers();

    // A headless runtime (no UI) never claims the pane: the rootSession guard
    // holds, so a turn without UI reports nothing.
    const fireHeadless = await spawn();
    fireHeadless("turn_start", {}, { hasUI: false });
    await flush();
    expect(reportedStates).toHaveLength(0);

    // A rebound interactive runtime (/reload, /new, /resume, /fork) missed
    // session_start, but a turn with UI proves it is the interactive root: it
    // starts reporting working.
    const fireUi = await spawn();
    fireUi("turn_start", {}, { hasUI: true });
    await flush();
    expect(reportedStates.at(-1)).toBe("working");
  });

  test(`${agent}: turn_start while working forces an extra working heartbeat`, async () => {
    jest.useFakeTimers();
    const fire = await spawn();

    fire("session_start", {}, { hasUI: true });
    await flush();
    fire("agent_start", {}, {});
    await flush();
    expect(reportedStates.at(-1)).toBe("working");

    // Already working: the forced re-publish bypasses client-side dedupe, so a
    // lost socket report or stale server state is repaired every turn.
    const before = reportedStates.length;
    fire("turn_start", {}, {});
    await flush();
    expect(reportedStates.length).toBe(before + 1);
    expect(reportedStates.at(-1)).toBe("working");
  });

  test(`${agent}: a nominal turn with heartbeats still idles at the end`, async () => {
    jest.useFakeTimers();
    const fire = await spawn();

    fire("session_start", {}, { hasUI: true });
    await flush();
    fire("agent_start", {}, {}); // real bookkeeping: count 1
    await flush();
    expect(reportedStates.at(-1)).toBe("working");

    // Heartbeats fire while the count is already positive: they must NOT take a
    // repair hold (that would outlive agent_end and stick the pane working);
    // they only force a working re-publish.
    fire("turn_start", {}, {});
    await flush();
    fire("turn_start", {}, {});
    await flush();
    expect(reportedStates.at(-1)).toBe("working");

    // The single matching agent_end drains the count to 0. With no lingering
    // hold, the ordinary lifecycle idles the pane.
    fire("agent_end", {});
    jest.advanceTimersByTime(200); // past the idle debounce
    await flush();
    expect(reportedStates.at(-1)).toBe("idle");
  });

  test(`${agent}: turn_start before agent_start holds working then idles (no sticky double-count)`, async () => {
    jest.useFakeTimers();
    const fire = await spawn();

    fire("session_start", {}, { hasUI: true });
    await flush();

    // Inversion: turn_start fires before its loop's agent_start. The count is 0,
    // so the handler takes a repair hold and publishes working.
    fire("turn_start", {}, {});
    await flush();
    expect(reportedStates.at(-1)).toBe("working");

    // Real bookkeeping resumes: agent_start clears the hold (count 1), and the
    // single matching agent_end drains back to 0. The old count-bump repair
    // would have double-counted (hold->1, start->2, one end->1) and stuck the
    // pane working forever; the hold design idles.
    fire("agent_start", {}, {});
    await flush();
    fire("agent_end", {});
    jest.advanceTimersByTime(200); // past the idle debounce
    await flush();
    expect(reportedStates.at(-1)).toBe("idle");
  });
}

test("kilo: a subagent session going idle does not idle the pane while the root agent works", async () => {
  // Module-loading boundary: see note above; dynamic import is intentional.
  const mod = await import("./kilo/herdr-agent-state.js");
  const plugin = await mod.HerdrAgentStatePlugin();

  // Root agent is working.
  await plugin.event({
    event: { type: "session.status", properties: { sessionID: "root", status: "busy" } },
  });
  await flush();
  expect(reportedStates.at(-1)).toBe("working");

  // A subagent (child) session is created, then goes idle.
  await plugin.event({
    event: { type: "session.created", properties: { sessionID: "child", info: { id: "child", parentID: "root" } } },
  });
  await flush();
  await plugin.event({
    event: { type: "session.idle", properties: { sessionID: "child" } },
  });
  await flush();

  // The child's idle must be dropped; the pane stays working.
  expect(reportedStates.filter((s) => s === "idle")).toHaveLength(0);
  expect(reportedStates.at(-1)).toBe("working");

  // The root agent going idle is the real completion.
  await plugin.event({
    event: { type: "session.idle", properties: { sessionID: "root" } },
  });
  await flush();
  expect(reportedStates.at(-1)).toBe("idle");
});
