// Minimal NDJSON client for the bora socket API (node:net, no dependencies).
//
// The socket path is read ONLY from HERDR_SOCKET_PATH. There is deliberately no default or
// guessed path: a manual or test run without the variable fails loudly instead of reaching
// whichever server happens to be running.

import net from "node:net";

export const SOCKET_ENV = "HERDR_SOCKET_PATH";
const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

let nextRequestId = 1;

export class ApiError extends Error {
	/**
	 * @param {string} code
	 * @param {string} message
	 */
	constructor(code, message) {
		super(`${code}: ${message}`);
		this.name = "ApiError";
		this.code = code;
	}
}

/** @param {Record<string, string | undefined>} env */
export function socketPath(env) {
	const path = env[SOCKET_ENV] ?? "";
	if (!path) throw new Error(`${SOCKET_ENV} is not set; this runs from a bora plugin hook, which sets it`);
	return path;
}

/**
 * Sends one request and resolves with its `result`. Rejects with `ApiError` on an error reply.
 * @param {string} path
 * @param {string} method
 * @param {Record<string, unknown>} params
 * @param {number} [timeoutMs]
 * @returns {Promise<any>}
 */
export function call(path, method, params, timeoutMs = 5000) {
	const id = `windhover-push-${process.pid}-${nextRequestId++}`;
	return new Promise((resolve, reject) => {
		const socket = net.createConnection({ path });
		/** @type {Buffer[]} */
		const chunks = [];
		let size = 0;
		let settled = false;
		/** @param {() => void} settle */
		const finish = (settle) => {
			if (settled) return;
			settled = true;
			clearTimeout(timer);
			socket.destroy();
			settle();
		};
		const timer = setTimeout(
			() => finish(() => reject(new Error(`bora socket ${path}: no reply to ${method} in ${timeoutMs} ms`))),
			timeoutMs,
		);
		socket.on("connect", () => socket.write(`${JSON.stringify({ id, method, params })}\n`));
		socket.on("error", (error) => finish(() => reject(new Error(`cannot reach bora socket ${path}: ${error.message}`))));
		socket.on("data", (chunk) => {
			chunks.push(chunk);
			size += chunk.length;
			if (size > MAX_RESPONSE_BYTES) {
				finish(() => reject(new Error(`reply from ${path} exceeded ${MAX_RESPONSE_BYTES} bytes`)));
				return;
			}
			const text = Buffer.concat(chunks).toString("utf8");
			const newline = text.indexOf("\n");
			if (newline === -1) return;
			finish(() => {
				let reply;
				try {
					reply = JSON.parse(text.slice(0, newline));
				} catch (error) {
					reject(new Error(`unparseable reply from ${path}: ${error instanceof Error ? error.message : error}`));
					return;
				}
				if (reply.error) reject(new ApiError(reply.error.code ?? "unknown", reply.error.message ?? ""));
				else resolve(reply.result ?? {});
			});
		});
		socket.on("end", () => finish(() => reject(new Error(`bora socket ${path} closed without a reply`))));
	});
}

/**
 * The live view of the bora server a hook runs under.
 * @param {Record<string, string | undefined>} env
 */
export function createBora(env) {
	const path = socketPath(env);
	return {
		/** @param {string} paneId @returns {Promise<{agent_status: string}>} */
		paneGet: async (paneId) => (await call(path, "pane.get", { pane_id: paneId })).pane,
		/** @param {string} workspaceId @returns {Promise<{label: string}>} */
		workspaceGet: async (workspaceId) => (await call(path, "workspace.get", { workspace_id: workspaceId })).workspace,
	};
}
