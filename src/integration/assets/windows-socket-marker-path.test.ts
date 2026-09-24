// Ported from upstream's shared herdr-agent-state.test.ts. The fork's own
// rewrite of that shared file (herdr-agent-state.test.ts, alongside this one)
// dropped upstream's generic `integrations`/`socketPlugins` win32-marker-path
// parametrization entirely when it moved to a single-shared-import harness
// that cannot reconfigure `process.platform` per test. That coverage - every
// integration must map a Windows `HERDR_SOCKET_PATH` marker to a named pipe
// endpoint, not use it as a literal path - is real regression coverage, not
// upstream test debt, so it is restored here with a fresh-import-per-test
// harness matching upstream's original shape. OpenCode's own version of this
// test already lives in opencode/herdr-agent-state.test.ts, next to its
// existing harness.
//
// Uses `mock.module` rather than mutating `net.createConnection` directly:
// bun runs test files in one process, and a raw property mutation on the
// real `node:net` module singleton is visible to every other test file's
// own net mock/state that happens to execute concurrently, which corrupted
// an unrelated omp test in `opencode/herdr-tui-session.test.ts` (measured).

import { afterEach, expect, mock, test } from "bun:test";

const originalPlatform = process.platform;
const originalEnvironment = {
  HERDR_ENV: process.env.HERDR_ENV,
  HERDR_PANE_ID: process.env.HERDR_PANE_ID,
  HERDR_SOCKET_PATH: process.env.HERDR_SOCKET_PATH,
  OMPCODE: process.env.OMPCODE,
};

let importCounter = 0;
let connectedPath: string | undefined;

mock.module("node:net", () => ({
  default: {
    createConnection(path: string, onConnect?: () => void) {
      connectedPath = path;
      const handlers = new Map<string, () => void>();
      const socket = {
        on(event: string, handler: () => void) {
          handlers.set(event, handler);
          return socket;
        },
        write() {
          return true;
        },
        setTimeout(_ms: number, onTimeout?: () => void) {
          queueMicrotask(() => onTimeout?.());
          return socket;
        },
        destroy() {},
        end() {},
        unref() {
          return socket;
        },
      };
      queueMicrotask(() => onConnect?.());
      return socket;
    },
  },
}));

afterEach(() => {
  Object.defineProperty(process, "platform", { value: originalPlatform });
  connectedPath = undefined;
  for (const [name, value] of Object.entries(originalEnvironment)) {
    if (value === undefined) {
      delete process.env[name];
    } else {
      process.env[name] = value;
    }
  }
});

function importFresh(modulePath: string) {
  importCounter += 1;
  return import(`${modulePath}?test=${importCounter}`);
}

function configureIntegrationEnvironment(recordingSocketPath: string) {
  delete process.env.OMPCODE;
  process.env.HERDR_ENV = "1";
  process.env.HERDR_SOCKET_PATH = recordingSocketPath;
  process.env.HERDR_PANE_ID = "test:p1";
}

type PiHandler = (event: unknown, context: unknown) => unknown;

function createPiHarness() {
  const handlers = new Map<string, PiHandler>();
  return {
    handlers,
    pi: {
      on(event: string, handler: PiHandler) {
        handlers.set(event, handler);
      },
      events: {
        on(_event: string, _handler: PiHandler) {
          return () => {};
        },
      },
    },
  };
}

const piStyleIntegrations = [
  { name: "Pi", modulePath: "./pi/herdr-agent-state.ts" },
  { name: "Oh My Pi", modulePath: "./omp/herdr-agent-state.ts" },
] as const;

for (const integration of piStyleIntegrations) {
  test(`${integration.name} maps the Windows socket marker path to a named pipe endpoint`, async () => {
    const markerPath = `herdr-${integration.name.toLowerCase().replaceAll(" ", "-")}-${process.pid}.sock`;
    configureIntegrationEnvironment(markerPath);
    Object.defineProperty(process, "platform", { value: "win32" });
    const { handlers, pi } = createPiHarness();

    const { default: install } = await importFresh(integration.modulePath);
    install(pi);
    await handlers.get("session_start")?.(
      { reason: "startup" },
      {
        hasUI: true,
        mode: "tui",
        isIdle: () => true,
        sessionManager: {
          getSessionFile: () => undefined,
          getSessionId: () => "test-session",
        },
      },
    );

    expect(connectedPath).toBe(`\\\\.\\pipe\\${markerPath}`);
  });
}

test("Kilo maps the Windows socket marker path to a named pipe endpoint", async () => {
  const markerPath = `herdr-kilo-${process.pid}.sock`;
  configureIntegrationEnvironment(markerPath);
  Object.defineProperty(process, "platform", { value: "win32" });

  const { HerdrAgentStatePlugin } = await importFresh("./kilo/herdr-agent-state.js");
  const plugin = await HerdrAgentStatePlugin();
  await plugin["chat.message"]?.({ sessionID: "windows-marker-session" });

  expect(connectedPath).toBe(`\\\\.\\pipe\\${markerPath}`);
});
