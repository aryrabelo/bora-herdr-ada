import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, test } from "node:test";
import { deliver, isDeadToken, loadDevices, parseDeviceFile } from "../devices.mjs";
import { encodeBase64, open } from "../envelope.mjs";
import {
	ASK_WINDOW_MS,
	askIds,
	FINISHED_COOLDOWN_MS,
	handleChannelMessage,
	handleHook,
	handlePaneStatus,
	paneIds,
	RECHECK_MS,
	sha256Hex,
} from "../push.mjs";

const CONNECTION_ID = "3F2504E0-4F89-11D3-9A0C-0305E82C3301";
/** Independent oracle for the C7 ids: node:crypto, not the WebCrypto code under test. */
const hex = (/** @type {string} */ text) => createHash("sha256").update(text, "utf8").digest("hex");

/** @type {string[]} */
const tempDirs = [];
afterEach(async () => {
	for (const dir of tempDirs.splice(0)) await fs.rm(dir, { recursive: true, force: true });
});

/**
 * @param {string} dir
 * @param {string} name
 * @param {Record<string, unknown>} [overrides]
 */
async function writeDevice(dir, name, overrides = {}) {
	const key = encodeBase64(crypto.getRandomValues(new Uint8Array(32)));
	const device = {
		version: 1,
		deviceToken: hex(`token:${name}`),
		environment: "sandbox",
		key,
		relay: "https://relay.test/",
		label: name,
		connectionId: CONNECTION_ID,
		...overrides,
	};
	await fs.mkdir(dir, { recursive: true });
	await fs.writeFile(path.join(dir, `${name}.json`), JSON.stringify(device));
	return device;
}

/**
 * A plugin world: temp state and push dirs, a fake clock, a fake bora and a fake relay that
 * records every request. `answer` decides the relay's reply per request.
 * @param {{devices?: string[], answer?: (request: any, url: string) => {status: number, body: object}}} [options]
 */
async function world(options = {}) {
	const root = await fs.mkdtemp(path.join(os.tmpdir(), "windhover-push-test-"));
	tempDirs.push(root);
	const pushDir = path.join(root, "push");
	await fs.mkdir(pushDir);
	/** @type {Record<string, any>} */
	const devices = {};
	for (const name of options.devices ?? ["phone"]) devices[name] = await writeDevice(pushDir, name);
	const clock = { now: 1_800_000_000_000 };
	/** @type {{url: string, body: any}[]} */
	const requests = [];
	/** @type {number[]} */
	const sleeps = [];
	/** @type {Record<string, string>} */
	const paneStatus = {};
	/** @type {string[]} */
	const logs = [];
	/** @type {((ms: number) => Promise<void>) | undefined} */
	let duringSleep;
	const answer = options.answer ?? (() => ({ status: 200, body: { source: "apns", status: 200, apnsId: "x" } }));
	const deps = {
		stateDir: path.join(root, "state"),
		pushDir,
		bora: {
			paneGet: async (/** @type {string} */ paneId) => {
				if (!(paneId in paneStatus)) throw new Error(`pane_not_found: ${paneId}`);
				return { agent_status: paneStatus[paneId] };
			},
			workspaceGet: async (/** @type {string} */ workspaceId) => ({ label: `label-of-${workspaceId}` }),
		},
		fetch: async (/** @type {string} */ url, /** @type {RequestInit} */ init) => {
			const body = JSON.parse(String(init.body));
			requests.push({ url, body });
			const reply = answer(body, url);
			return new Response(JSON.stringify(reply.body), { status: reply.status });
		},
		now: () => clock.now,
		sleep: async (/** @type {number} */ ms) => {
			sleeps.push(ms);
			const hook = duringSleep;
			duringSleep = undefined;
			if (hook) await hook(ms);
			clock.now += ms;
		},
		log: (/** @type {string} */ line) => logs.push(line),
	};
	return {
		deps,
		devices,
		clock,
		requests,
		sleeps,
		paneStatus,
		logs,
		/** Runs `fn` once, inside the next `sleep` (that is, during a recheck or ask window). */
		onNextSleep: (/** @type {(ms: number) => Promise<void>} */ fn) => {
			duringSleep = fn;
		},
		/** Opens a recorded request with the key of `device`. */
		openRequest: (/** @type {{body: any}} */ request, device = "phone") => open(request.body.e, devices[device].key),
		/** The one recorded request addressed to `device`. @param {string} device */
		requestFor: (device) => {
			const matching = requests.filter((request) => request.body.deviceToken === devices[device].deviceToken);
			assert.equal(matching.length, 1, `exactly one request for ${device}`);
			return matching[0];
		},
	};
}

/** @param {string} status @param {Record<string, unknown>} [extra] */
const paneEvent = (status, extra = {}) => ({
	type: "pane_agent_status_changed",
	pane_id: "p_1",
	workspace_id: "w_1",
	agent_status: status,
	agent: "claude",
	title: "claude: refactor",
	...extra,
});

/** @param {Record<string, unknown>} [extra] */
const askEvent = (extra = {}) => ({
	type: "channel_message",
	channel: "teste",
	seq: 12,
	from_pane: "p_2",
	from_name: "builder",
	text: "qual banco?",
	kind: "ask",
	in_reply_to: 7,
	to_human: true,
	...extra,
});

describe("blocked recheck", () => {
	test("sends needs-you when the pane is still blocked after the recheck delay", async () => {
		const w = await world();
		w.paneStatus.p_1 = "blocked";
		await handlePaneStatus(paneEvent("blocked", { display_agent: "Claude Code" }), w.deps);
		assert.deepEqual(w.sleeps, [RECHECK_MS]);
		assert.equal(RECHECK_MS, 3000);
		assert.equal(w.requests.length, 1);
		const [request] = w.requests;
		assert.equal(request.url, "https://relay.test/v1/push");
		assert.equal(request.body.level, "needsYou");
		assert.equal(request.body.deviceToken, hex("token:phone"));
		assert.equal(request.body.environment, "sandbox");
		assert.deepEqual(await w.openRequest(request), {
			title: "label-of-w_1: Claude Code needs you",
			body: "claude: refactor",
			connectionId: CONNECTION_ID,
		});
	});

	test("sends nothing when bora no longer shows the pane blocked", async () => {
		const w = await world();
		w.paneStatus.p_1 = "blocked";
		w.onNextSleep(async () => {
			w.paneStatus.p_1 = "working";
		});
		await handlePaneStatus(paneEvent("blocked"), w.deps);
		assert.equal(w.requests.length, 0);
	});

	test("sends nothing when the pane is gone at recheck time", async () => {
		const w = await world();
		await handlePaneStatus(paneEvent("blocked"), w.deps);
		assert.equal(w.requests.length, 0);
		assert.match(w.logs.join("\n"), /pane_not_found/);
	});

	test("a flap (blocked, working, blocked) inside the window sends one push, from the newest hook", async () => {
		const w = await world();
		w.paneStatus.p_1 = "blocked";
		w.onNextSleep(async () => {
			await handlePaneStatus(paneEvent("working"), w.deps);
			await handlePaneStatus(paneEvent("blocked"), w.deps);
		});
		await handlePaneStatus(paneEvent("blocked"), w.deps);
		assert.equal(w.requests.length, 1);
		assert.deepEqual(w.sleeps, [RECHECK_MS, RECHECK_MS]);
	});

	test("does not wait or send without device files", async () => {
		const w = await world({ devices: [] });
		w.paneStatus.p_1 = "blocked";
		await handlePaneStatus(paneEvent("blocked"), w.deps);
		assert.deepEqual(w.sleeps, []);
		assert.equal(w.requests.length, 0);
	});
});

describe("finished", () => {
	test("done notifies at once, then at most once per pane per minute", async () => {
		const w = await world();
		await handlePaneStatus(paneEvent("done"), w.deps);
		assert.equal(w.requests.length, 1);
		assert.deepEqual(w.sleeps, []);
		assert.equal(w.requests[0].body.level, "finished");
		assert.deepEqual(await w.openRequest(w.requests[0]), {
			title: "label-of-w_1: claude finished",
			body: "claude: refactor",
			connectionId: CONNECTION_ID,
		});

		w.clock.now += 30_000;
		await handlePaneStatus(paneEvent("working"), w.deps);
		await handlePaneStatus(paneEvent("done"), w.deps);
		assert.equal(w.requests.length, 1, "second finish inside the cooldown is dropped");

		w.clock.now += FINISHED_COOLDOWN_MS;
		await handlePaneStatus(paneEvent("working"), w.deps);
		await handlePaneStatus(paneEvent("done"), w.deps);
		assert.equal(w.requests.length, 2);
	});

	test("the cooldown is per pane", async () => {
		const w = await world();
		await handlePaneStatus(paneEvent("done"), w.deps);
		await handlePaneStatus(paneEvent("done", { pane_id: "p_2" }), w.deps);
		assert.equal(w.requests.length, 2);
	});

	test("idle right after working is finished; idle after anything else is not", async () => {
		const w = await world();
		await handlePaneStatus(paneEvent("idle"), w.deps);
		await handlePaneStatus(paneEvent("blocked", { pane_id: "p_9" }), w.deps);
		await handlePaneStatus(paneEvent("idle", { pane_id: "p_9" }), w.deps);
		assert.equal(w.requests.length, 0);
		await handlePaneStatus(paneEvent("working"), w.deps);
		await handlePaneStatus(paneEvent("idle"), w.deps);
		assert.equal(w.requests.length, 1);
		assert.equal(w.requests[0].body.level, "finished");
	});

	test("working and unknown never notify", async () => {
		const w = await world();
		for (const status of ["working", "unknown", "working", "unknown"]) {
			await handlePaneStatus(paneEvent(status), w.deps);
		}
		assert.equal(w.requests.length, 0);
		assert.deepEqual(w.sleeps, []);
	});
});

describe("asks", () => {
	test("an ask to the human sends '<from_name> asks' with the ask reference", async () => {
		const w = await world();
		await handleChannelMessage(askEvent(), w.deps);
		assert.deepEqual(w.sleeps, [ASK_WINDOW_MS]);
		assert.equal(w.requests.length, 1);
		const [request] = w.requests;
		assert.equal(request.body.level, "needsYou");
		assert.deepEqual(await w.openRequest(request), {
			title: "builder asks",
			body: "qual banco?",
			connectionId: CONNECTION_ID,
			ask: { channel: "teste", seq: 12 },
		});
	});

	test("the body is the first 120 characters, never half an emoji", async () => {
		const w = await world();
		const text = `${"🚀".repeat(119)}é and everything after`;
		await handleChannelMessage(askEvent({ text }), w.deps);
		const opened = await w.openRequest(w.requests[0]);
		assert.equal(Array.from(opened.body).length, 120);
		assert.equal(opened.body, `${"🚀".repeat(119)}é`);
	});

	for (const [name, extra] of [
		["a plain message to the human", { kind: "message" }],
		["an ask to another pane", { to_human: false }],
		["a message from a bora without kind/to_human", { kind: undefined, to_human: undefined }],
		["to_human given as a string", { to_human: "true" }],
	]) {
		test(`ignores ${name}`, async () => {
			const w = await world();
			await handleChannelMessage(askEvent(extra), w.deps);
			assert.equal(w.requests.length, 0);
			assert.deepEqual(w.sleeps, []);
			await assert.rejects(fs.stat(path.join(w.deps.stateDir, "asks.json")));
		});
	}

	test("asks inside one window coalesce into one '<N> questions waiting' push without ask", async () => {
		const w = await world({ devices: ["phone", "ipad"] });
		w.onNextSleep(async () => {
			await handleChannelMessage(askEvent({ seq: 13, from_name: "reviewer", text: "posso mergear?" }), w.deps);
			await handleChannelMessage(askEvent({ seq: 14, text: "e o schema?" }), w.deps);
			await handleChannelMessage(askEvent({ seq: 13, from_name: "reviewer", text: "posso mergear?" }), w.deps);
		});
		await handleChannelMessage(askEvent(), w.deps);
		assert.deepEqual(w.sleeps, [ASK_WINDOW_MS], "only the first ask's hook waits");
		assert.equal(w.requests.length, 2, "one push per device");
		for (const device of ["ipad", "phone"]) {
			const request = w.requestFor(device);
			assert.equal(request.body.level, "needsYou");
			assert.equal(request.body.collapseId, undefined);
			assert.equal(request.body.threadId, hex("channel:teste"));
			assert.deepEqual(await w.openRequest(request, device), {
				title: "3 questions waiting",
				body: "builder, reviewer",
				connectionId: CONNECTION_ID,
			});
		}

		await handleChannelMessage(askEvent({ seq: 20, from_name: "late" }), w.deps);
		assert.equal(w.requests.length, 4, "the next ask opens a new window");
		assert.equal((await w.openRequest(w.requests[3])).title, "late asks");
	});

	test("a coalesced push across channels has no thread id", async () => {
		const w = await world();
		w.onNextSleep(async () => {
			await handleChannelMessage(askEvent({ channel: "outro", seq: 1 }), w.deps);
		});
		await handleChannelMessage(askEvent(), w.deps);
		assert.equal(w.requests.length, 1);
		assert.equal(w.requests[0].body.threadId, undefined);
		assert.equal((await w.openRequest(w.requests[0])).title, "2 questions waiting");
	});
});

describe("collapse and thread ids (contract C7)", () => {
	test("sha256Hex is 64 lowercase hex chars of SHA-256", async () => {
		assert.equal(await sha256Hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
		assert.match(await sha256Hex("p_1"), /^[0-9a-f]{64}$/);
	});

	test("pane ids hash the pane and the workspace", async () => {
		assert.deepEqual(await paneIds("p_1", "w_1"), { collapseId: hex("p_1"), threadId: hex("w_1") });
	});

	test("ask ids hash 'ask:<channel>:<seq>' and 'channel:<channel>'", async () => {
		assert.deepEqual(await askIds("teste", 12), { collapseId: hex("ask:teste:12"), threadId: hex("channel:teste") });
	});

	test("the relay request carries them", async () => {
		const w = await world();
		w.paneStatus.p_1 = "blocked";
		await handlePaneStatus(paneEvent("blocked"), w.deps);
		await handleChannelMessage(askEvent(), w.deps);
		assert.equal(w.requests[0].body.collapseId, hex("p_1"));
		assert.equal(w.requests[0].body.threadId, hex("w_1"));
		assert.equal(w.requests[1].body.collapseId, hex("ask:teste:12"));
		assert.equal(w.requests[1].body.threadId, hex("channel:teste"));
	});
});

describe("device files", () => {
	/** @param {number} status @param {object} body */
	const relayAnswer = (status, body) => () => ({ status, body });

	for (const [name, status, body, deleted] of [
		["410 Unregistered", 410, { source: "apns", status: 410, reason: "Unregistered" }, true],
		["400 BadDeviceToken", 400, { source: "apns", status: 400, reason: "BadDeviceToken" }, true],
		["the relay's own 400", 400, { source: "relay", reason: "InvalidRequest", detail: "x" }, false],
		["a relay 429", 429, { source: "relay", reason: "RateLimited" }, false],
		["APNs 400 for another reason", 400, { source: "apns", status: 400, reason: "BadCollapseId" }, false],
		["APNs 200", 200, { source: "apns", status: 200, apnsId: "x" }, false],
	]) {
		test(`${deleted ? "deletes" : "keeps"} the device file on ${name}`, async () => {
			const w = await world({ answer: relayAnswer(/** @type {number} */ (status), /** @type {object} */ (body)) });
			await handlePaneStatus(paneEvent("done"), w.deps);
			assert.equal(w.requests.length, 1);
			const exists = await fs.stat(path.join(w.deps.pushDir, "phone.json")).then(() => true, () => false);
			assert.equal(exists, !deleted);
		});
	}

	test("only the dead device is deleted; every device gets its own envelope", async () => {
		const w = await world({
			devices: ["alive", "dead"],
			answer: (body) =>
				body.deviceToken === hex("token:dead")
					? { status: 400, body: { source: "apns", status: 400, reason: "BadDeviceToken" } }
					: { status: 200, body: { source: "apns", status: 200 } },
		});
		await handlePaneStatus(paneEvent("done"), w.deps);
		assert.equal(w.requests.length, 2);
		assert.deepEqual(await fs.readdir(w.deps.pushDir), ["alive.json"]);
		assert.equal((await w.openRequest(w.requestFor("alive"), "alive")).title, "label-of-w_1: claude finished");
		assert.equal((await w.openRequest(w.requestFor("dead"), "dead")).title, "label-of-w_1: claude finished");
		await assert.rejects(w.openRequest(w.requestFor("dead"), "alive"), { code: "authentication" });
		assert.match(w.logs.join("\n"), /dead\.json \(dead\): 400 .*BadDeviceToken.*deleted the device file/);
	});

	test("an invalid device file is skipped and kept", async () => {
		const w = await world();
		await fs.writeFile(path.join(w.deps.pushDir, "broken.json"), "{");
		await fs.writeFile(path.join(w.deps.pushDir, "notes.txt"), "not a device");
		await handlePaneStatus(paneEvent("done"), w.deps);
		assert.equal(w.requests.length, 1);
		assert.deepEqual((await fs.readdir(w.deps.pushDir)).sort(), ["broken.json", "notes.txt", "phone.json"]);
		assert.match(w.logs.join("\n"), /skip broken\.json/);
	});

	test("a missing push dir means no devices", async () => {
		assert.deepEqual(await loadDevices(path.join(os.tmpdir(), "windhover-push-does-not-exist"), () => {}), []);
	});

	test("parseDeviceFile mirrors Windhover's validation", () => {
		const base = { version: 1, deviceToken: "abcd", environment: "production", key: "k", relay: "r", label: "l" };
		assert.equal(parseDeviceFile(JSON.stringify(base)).connectionId, undefined);
		assert.throws(() => parseDeviceFile(JSON.stringify({ ...base, version: 2 })), /version/);
		assert.throws(() => parseDeviceFile(JSON.stringify({ ...base, deviceToken: "abc" })), /hex/);
		assert.throws(() => parseDeviceFile(JSON.stringify({ ...base, environment: "dev" })), /environment/);
		assert.throws(() => parseDeviceFile(JSON.stringify({ ...base, connectionId: "nope" })), /UUID/);
	});

	test("isDeadToken needs an APNs verdict", () => {
		assert.equal(isDeadToken({ source: "apns", status: 410 }), true);
		assert.equal(isDeadToken({ source: "apns", status: 400, reason: "BadDeviceToken" }), true);
		assert.equal(isDeadToken({ status: 400, reason: "BadDeviceToken" }), false);
		assert.equal(isDeadToken(undefined), false);
	});

	test("an unreachable relay is logged, never thrown, and keeps the file", async () => {
		const pushDir = path.join(await fs.mkdtemp(path.join(os.tmpdir(), "windhover-push-test-")), "push");
		tempDirs.push(path.dirname(pushDir));
		await writeDevice(pushDir, "phone");
		const [target] = await loadDevices(pushDir, () => {});
		const outcome = await deliver(target, { title: "t", body: "b" }, { level: "finished" }, async () => {
			throw new Error("ECONNREFUSED");
		});
		assert.match(outcome, /relay unreachable: ECONNREFUSED/);
		assert.deepEqual(await fs.readdir(pushDir), ["phone.json"]);
	});
});

describe("handleHook", () => {
	test("dispatches on HERDR_PLUGIN_EVENT with the {event, data} envelope bora passes", async () => {
		const w = await world();
		await handleHook("pane.agent_status_changed", JSON.stringify({ event: "pane_agent_status_changed", data: paneEvent("done") }), w.deps);
		await handleHook("channel.message", JSON.stringify({ event: "channel_message", data: askEvent() }), w.deps);
		await handleHook("workspace.created", "{}", w.deps);
		assert.equal(w.requests.length, 2);
		assert.match(w.logs.join("\n"), /ignored event workspace\.created/);
	});
});
