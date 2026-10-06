// The notification rules. One call handles one bora event hook; everything that has to outlive
// the hook process (last status per pane, finished cooldown, pending asks) lives in small JSON
// files under the plugin state dir, read and replaced under one lock file.
//
//   pane.agent_status_changed
//     blocked                     -> "<workspace>: <agent> needs you", only if bora still shows the
//                                    pane blocked RECHECK_MS later and no newer event arrived
//     done, or idle after working -> "<workspace>: <agent> finished", once per pane per COOLDOWN
//     working, unknown, the rest  -> nothing
//   channel.message with kind == "ask" && to_human
//     one ask in ASK_WINDOW_MS    -> "<from_name> asks", first 120 chars of the text, ask: {channel, seq}
//     two or more                 -> "<N> questions waiting", body = the askers' names, no ask

import fs from "node:fs/promises";
import path from "node:path";
import { deliver, loadDevices } from "./devices.mjs";

export const RECHECK_MS = 3_000;
export const FINISHED_COOLDOWN_MS = 60_000;
export const ASK_WINDOW_MS = 3_000;
export const ASK_BODY_CHARS = 120;

const LOCK_FILE = "state.lock";
const PANES_FILE = "panes.json";
const ASKS_FILE = "asks.json";
const LOCK_WAIT_MS = 5_000;
const LOCK_STALE_MS = 10_000;
const LOCK_RETRY_MS = 15;
/** A leader that never flushed its batch (killed mid-window) is replaced after this long. */
const ASK_LEADER_STALE_MS = 30_000;
/** Pane entries untouched this long are dropped, so closed panes do not pile up. */
const PANE_STATE_TTL_MS = 7 * 24 * 60 * 60 * 1000;

/**
 * @typedef {import("./devices.mjs").Fetch} Fetch
 * @typedef {import("./devices.mjs").Notification} Notification
 * @typedef {import("./devices.mjs").Delivery} Delivery
 * @typedef {{
 *   paneGet(paneId: string): Promise<{agent_status: string}>,
 *   workspaceGet(workspaceId: string): Promise<{label: string}>,
 * }} Bora
 * @typedef {{
 *   stateDir: string, pushDir: string, bora: Bora, fetch: Fetch,
 *   now(): number, sleep(ms: number): Promise<void>, log(line: string): void,
 * }} Deps
 * @typedef {{status: string, gen: number, at: number, finishedAt?: number}} PaneEntry
 * @typedef {{channel: string, seq: number, from_name: string, text: string, at: number}} PendingAsk
 * @typedef {{pending: PendingAsk[], leader: {token: string, at: number} | null}} AskQueue
 */

/** Lowercase hex SHA-256 of the UTF-8 of `text` (64 chars). @param {string} text */
export async function sha256Hex(text) {
	const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
	return Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

/** collapseId/threadId of a pane push (contract C7). @param {string} paneId @param {string} workspaceId */
export async function paneIds(paneId, workspaceId) {
	return { collapseId: await sha256Hex(paneId), threadId: await sha256Hex(workspaceId) };
}

/** collapseId/threadId of an ask push (contract C7). @param {string} channel @param {number} seq */
export async function askIds(channel, seq) {
	return { collapseId: await sha256Hex(`ask:${channel}:${seq}`), threadId: await sha256Hex(`channel:${channel}`) };
}

/** The first `ASK_BODY_CHARS` characters (code points, never half a surrogate pair). @param {string} text */
export function askBody(text) {
	return Array.from(text).slice(0, ASK_BODY_CHARS).join("");
}

/** A question to the human (contract C2). @param {Record<string, unknown>} data */
export function isAskToHuman(data) {
	return data.kind === "ask" && data.to_human === true;
}

/**
 * Runs `fn` while holding `<stateDir>/state.lock` (O_EXCL lock file; a lock older than
 * LOCK_STALE_MS belongs to a dead process and is broken).
 * @template T
 * @param {string} stateDir
 * @param {() => Promise<T>} fn
 * @returns {Promise<T>}
 */
async function withLock(stateDir, fn) {
	const lock = path.join(stateDir, LOCK_FILE);
	await fs.mkdir(stateDir, { recursive: true });
	const deadline = Date.now() + LOCK_WAIT_MS;
	for (;;) {
		try {
			await fs.writeFile(lock, String(process.pid), { flag: "wx" });
			break;
		} catch (error) {
			if (/** @type {NodeJS.ErrnoException} */ (error).code !== "EEXIST") throw error;
			const stat = await fs.stat(lock).catch(() => undefined);
			if (stat && Date.now() - stat.mtimeMs > LOCK_STALE_MS) {
				await fs.unlink(lock).catch(() => {});
				continue;
			}
			if (Date.now() > deadline) throw new Error(`timed out waiting for ${lock}`);
			await new Promise((resolve) => setTimeout(resolve, LOCK_RETRY_MS));
		}
	}
	try {
		return await fn();
	} finally {
		await fs.unlink(lock).catch(() => {});
	}
}

/**
 * @template T
 * @param {string} file
 * @param {T} fallback
 * @returns {Promise<T>}
 */
async function readJson(file, fallback) {
	try {
		return JSON.parse(await fs.readFile(file, "utf8"));
	} catch {
		return fallback;
	}
}

/** Atomic replace: write a sibling temp file, then rename over. @param {string} file @param {unknown} value */
async function writeJson(file, value) {
	const temp = `${file}.${process.pid}.tmp`;
	await fs.writeFile(temp, JSON.stringify(value));
	await fs.rename(temp, file);
}

/**
 * Seals and sends one notification to every device, logging one line per device.
 * @param {import("./devices.mjs").Device[]} devices
 * @param {Notification} notification
 * @param {Delivery} delivery
 * @param {Deps} deps
 */
async function sendToAll(devices, notification, delivery, deps) {
	const outcomes = await Promise.all(devices.map((device) => deliver(device, notification, delivery, deps.fetch)));
	for (const outcome of outcomes) deps.log(`${delivery.level} push -> ${outcome}`);
}

/**
 * Handles one `pane.agent_status_changed` hook.
 * @param {Record<string, unknown>} data
 * @param {Deps} deps
 */
export async function handlePaneStatus(data, deps) {
	const { pane_id: paneId, workspace_id: workspaceId, agent_status: status } = data;
	if (typeof paneId !== "string" || typeof workspaceId !== "string" || typeof status !== "string") {
		deps.log("ignored a pane.agent_status_changed event without pane_id/workspace_id/agent_status");
		return;
	}
	const panesFile = path.join(deps.stateDir, PANES_FILE);
	const now = deps.now();
	/** @type {{kind: "needsYou" | "finished" | null, gen: number}} */
	const decision = await withLock(deps.stateDir, async () => {
		/** @type {Record<string, PaneEntry>} */
		const panes = await readJson(panesFile, {});
		for (const [id, entry] of Object.entries(panes)) {
			if (now - entry.at > PANE_STATE_TTL_MS) delete panes[id];
		}
		const previous = panes[paneId];
		/** @type {PaneEntry} */
		const entry = { status, gen: (previous?.gen ?? 0) + 1, at: now };
		if (previous?.finishedAt !== undefined) entry.finishedAt = previous.finishedAt;
		/** @type {"needsYou" | "finished" | null} */
		let kind = null;
		if (status === "blocked") {
			kind = "needsYou";
		} else if (status === "done" || (status === "idle" && previous?.status === "working")) {
			if (entry.finishedAt === undefined || now - entry.finishedAt >= FINISHED_COOLDOWN_MS) {
				kind = "finished";
				entry.finishedAt = now;
			}
		}
		panes[paneId] = entry;
		await writeJson(panesFile, panes);
		return { kind, gen: entry.gen };
	});
	if (decision.kind === null) return;

	const devices = await loadDevices(deps.pushDir, deps.log);
	if (devices.length === 0) return;

	if (decision.kind === "needsYou") {
		await deps.sleep(RECHECK_MS);
		let current;
		try {
			current = await deps.bora.paneGet(paneId);
		} catch (error) {
			deps.log(`blocked recheck of ${paneId} failed, not sending: ${error instanceof Error ? error.message : error}`);
			return;
		}
		if (current.agent_status !== "blocked") return;
		/** @type {Record<string, PaneEntry>} */
		const panes = await readJson(panesFile, {});
		// A newer event for this pane has its own hook (and its own recheck): leave it to that one.
		if (panes[paneId]?.gen !== decision.gen) return;
	}

	let workspace = workspaceId;
	try {
		workspace = (await deps.bora.workspaceGet(workspaceId)).label || workspaceId;
	} catch (error) {
		deps.log(`workspace ${workspaceId} lookup failed, using its id: ${error instanceof Error ? error.message : error}`);
	}
	const agent = [data.display_agent, data.agent].find((name) => typeof name === "string" && name !== "") ?? "agent";
	const body = typeof data.title === "string" ? data.title : "";
	const suffix = decision.kind === "needsYou" ? "needs you" : "finished";
	await sendToAll(
		devices,
		{ title: `${workspace}: ${agent} ${suffix}`, body },
		{ level: decision.kind, ...(await paneIds(paneId, workspaceId)) },
		deps,
	);
}

/**
 * Handles one `channel.message` hook: only a question to the human (contract C2) notifies, and
 * asks that arrive within ASK_WINDOW_MS of the first one go out as a single push. The first ask's
 * hook leads the window; later hooks only enqueue.
 * @param {Record<string, unknown>} data
 * @param {Deps} deps
 */
export async function handleChannelMessage(data, deps) {
	if (!isAskToHuman(data)) return;
	const { channel, seq, from_name: fromName, text } = data;
	if (typeof channel !== "string" || !Number.isSafeInteger(seq) || typeof fromName !== "string" || typeof text !== "string") {
		deps.log("ignored an ask without channel/seq/from_name/text");
		return;
	}
	const devices = await loadDevices(deps.pushDir, deps.log);
	if (devices.length === 0) return;

	const asksFile = path.join(deps.stateDir, ASKS_FILE);
	const now = deps.now();
	const token = `${process.pid}-${now}-${Math.random().toString(36).slice(2)}`;
	const leads = await withLock(deps.stateDir, async () => {
		/** @type {AskQueue} */
		const queue = await readJson(asksFile, { pending: [], leader: null });
		if (queue.leader && now - queue.leader.at > ASK_LEADER_STALE_MS) queue.leader = null;
		if (!queue.pending.some((ask) => ask.channel === channel && ask.seq === seq)) {
			queue.pending.push({ channel, seq: /** @type {number} */ (seq), from_name: fromName, text, at: now });
		}
		const leads = queue.leader === null;
		if (leads) queue.leader = { token, at: now };
		await writeJson(asksFile, queue);
		return leads;
	});
	if (!leads) return;

	await deps.sleep(ASK_WINDOW_MS);
	/** @type {PendingAsk[]} */
	const batch = await withLock(deps.stateDir, async () => {
		/** @type {AskQueue} */
		const queue = await readJson(asksFile, { pending: [], leader: null });
		if (queue.leader?.token !== token) return [];
		const pending = queue.pending;
		await writeJson(asksFile, { pending: [], leader: null });
		return pending;
	});

	const [first] = batch;
	if (first === undefined) return;
	if (batch.length === 1) {
		await sendToAll(
			devices,
			{ title: `${first.from_name} asks`, body: askBody(first.text), ask: { channel: first.channel, seq: first.seq } },
			{ level: "needsYou", ...(await askIds(first.channel, first.seq)) },
			deps,
		);
		return;
	}
	const names = [...new Set(batch.map((ask) => ask.from_name))];
	const channels = new Set(batch.map((ask) => ask.channel));
	/** @type {Delivery} */
	const delivery = { level: "needsYou" };
	if (channels.size === 1) delivery.threadId = await sha256Hex(`channel:${first.channel}`);
	await sendToAll(devices, { title: `${batch.length} questions waiting`, body: names.join(", ") }, delivery, deps);
}

/** Event names as bora's hook env spells them (`HERDR_PLUGIN_EVENT`). */
const HANDLERS = {
	"pane.agent_status_changed": handlePaneStatus,
	"channel.message": handleChannelMessage,
};

/**
 * Dispatches one hook invocation. `eventName` is `HERDR_PLUGIN_EVENT`; `eventJson` is
 * `HERDR_PLUGIN_EVENT_JSON`, the whole event envelope `{event, data}`.
 * @param {string} eventName
 * @param {string} eventJson
 * @param {Deps} deps
 */
export async function handleHook(eventName, eventJson, deps) {
	const handler = /** @type {Record<string, typeof handlePaneStatus>} */ (HANDLERS)[eventName];
	if (handler === undefined) {
		deps.log(`ignored event ${eventName || "(none)"}`);
		return;
	}
	const envelope = JSON.parse(eventJson);
	const data = envelope && typeof envelope === "object" && envelope.data && typeof envelope.data === "object" ? envelope.data : undefined;
	if (data === undefined) throw new Error("HERDR_PLUGIN_EVENT_JSON has no data object");
	await handler(data, deps);
}
