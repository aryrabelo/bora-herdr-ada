// Device files and the relay request.
//
// A device file is what the Windhover app writes over SSH when the user turns on
// notifications for this host (`PushDeviceFile` in WindhoverKit):
//   <dir>/<device-id>.json = {version: 1, deviceToken, environment, key, relay, label, connectionId}
// Every push goes to every device file, sealed with that device's own key, through the
// device's own relay: POST <relay>/v1/push {deviceToken, environment, level, e, collapseId?, threadId?}.

import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { seal } from "./envelope.mjs";

const ENVIRONMENTS = ["sandbox", "production"];
const UUID = /^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$/;
const RELAY_TIMEOUT_MS = 10_000;

/**
 * @typedef {{version: 1, deviceToken: string, environment: "sandbox" | "production", key: string,
 *   relay: string, label: string, connectionId?: string}} PushDeviceFile
 * @typedef {{file: string, device: PushDeviceFile}} Device
 * @typedef {{title: string, body: string, ask?: {channel: string, seq: number}}} Notification
 * @typedef {{level: "needsYou" | "finished", collapseId?: string, threadId?: string}} Delivery
 * @typedef {(url: string, init: RequestInit) => Promise<Response>} Fetch
 */

/** `WINDHOVER_PUSH_DIR`, else `~/.config/windhover/push`. @param {Record<string, string | undefined>} env */
export function pushDir(env) {
	return env.WINDHOVER_PUSH_DIR || path.join(os.homedir(), ".config", "windhover", "push");
}

/**
 * Validates a device file, as Windhover's `host/send.ts` does; unknown keys are ignored and
 * `connectionId` may be missing.
 * @param {string} text
 * @returns {PushDeviceFile}
 */
export function parseDeviceFile(text) {
	const json = JSON.parse(text);
	if (typeof json !== "object" || json === null || Array.isArray(json)) throw new Error("not a JSON object");
	const { version, deviceToken, environment, key, relay, label } = json;
	if (version !== 1) throw new Error(`unsupported version ${String(version)}`);
	if (typeof deviceToken !== "string" || !/^(?:[0-9a-fA-F]{2})+$/.test(deviceToken)) {
		throw new Error("deviceToken must be hex");
	}
	if (!ENVIRONMENTS.includes(environment)) throw new Error("environment must be sandbox or production");
	if (typeof key !== "string" || typeof relay !== "string" || typeof label !== "string") {
		throw new Error("key, relay and label must be strings");
	}
	const connectionId = json.connectionId ?? undefined;
	if (connectionId !== undefined && (typeof connectionId !== "string" || !UUID.test(connectionId))) {
		throw new Error("connectionId must be a UUID");
	}
	return { version, deviceToken, environment, key, relay, label, connectionId };
}

/**
 * Every valid `*.json` in `dir`. A missing dir means no devices; an invalid file is reported
 * through `log` and skipped (never deleted: only the relay's verdict deletes a file).
 * @param {string} dir
 * @param {(line: string) => void} log
 * @returns {Promise<Device[]>}
 */
export async function loadDevices(dir, log) {
	let names;
	try {
		names = await fs.readdir(dir);
	} catch (error) {
		if (/** @type {NodeJS.ErrnoException} */ (error).code === "ENOENT") return [];
		throw error;
	}
	/** @type {Device[]} */
	const devices = [];
	for (const name of names.filter((entry) => entry.endsWith(".json")).sort()) {
		const file = path.join(dir, name);
		try {
			devices.push({ file, device: parseDeviceFile(await fs.readFile(file, "utf8")) });
		} catch (error) {
			log(`skip ${name}: ${error instanceof Error ? error.message : String(error)}`);
		}
	}
	return devices;
}

/**
 * True when the relay says APNs no longer knows this token: the device file is dead.
 * @param {unknown} result the relay's JSON answer
 */
export function isDeadToken(result) {
	if (typeof result !== "object" || result === null) return false;
	const { source, status, reason } = /** @type {Record<string, unknown>} */ (result);
	return source === "apns" && (status === 410 || (status === 400 && reason === "BadDeviceToken"));
}

/**
 * Seals `notification` for one device and POSTs it to the device's relay. Deletes the device
 * file when the relay reports a dead token. Never throws; returns a one-line outcome for the log
 * (device label and relay answer only, never the notification text or the token).
 * @param {Device} target
 * @param {Notification} notification
 * @param {Delivery} delivery
 * @param {Fetch} fetchFn
 */
export async function deliver(target, notification, delivery, fetchFn) {
	const { file, device } = target;
	const name = `${path.basename(file)} (${device.label})`;
	let e;
	try {
		e = await seal({ ...notification, connectionId: device.connectionId }, device.key);
	} catch (error) {
		return `${name}: cannot seal: ${error instanceof Error ? error.message : String(error)}`;
	}
	/** @type {Record<string, string>} */
	const request = { deviceToken: device.deviceToken, environment: device.environment, level: delivery.level, e };
	if (delivery.collapseId !== undefined) request.collapseId = delivery.collapseId;
	if (delivery.threadId !== undefined) request.threadId = delivery.threadId;
	let response;
	let text;
	try {
		response = await fetchFn(`${device.relay.replace(/\/+$/, "")}/v1/push`, {
			method: "POST",
			headers: { "content-type": "application/json" },
			body: JSON.stringify(request),
			signal: AbortSignal.timeout(RELAY_TIMEOUT_MS),
		});
		text = (await response.text()).trim();
	} catch (error) {
		return `${name}: relay unreachable: ${error instanceof Error ? error.message : String(error)}`;
	}
	let result;
	try {
		result = JSON.parse(text);
	} catch {
		result = undefined;
	}
	if (!isDeadToken(result)) return `${name}: ${response.status} ${text}`;
	// The app rewrites the file when its token changes; a verdict on the old token must not
	// delete the new registration.
	let current;
	try {
		current = parseDeviceFile(await fs.readFile(file, "utf8")).deviceToken;
	} catch {
		current = undefined;
	}
	if (current?.toLowerCase() !== device.deviceToken.toLowerCase()) {
		return `${name}: ${response.status} ${text}; the device file changed since, kept it`;
	}
	try {
		await fs.unlink(file);
		return `${name}: ${response.status} ${text}; deleted the device file`;
	} catch (error) {
		return `${name}: ${response.status} ${text}; cannot delete the device file: ${error instanceof Error ? error.message : String(error)}`;
	}
}
