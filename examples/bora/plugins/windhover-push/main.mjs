// Entry point: `run.sh hook`, run by bora for each `[[events]]` entry of herdr-plugin.toml.
// bora sets HERDR_PLUGIN_EVENT (dotted event name), HERDR_PLUGIN_EVENT_JSON (the event
// envelope), HERDR_PLUGIN_STATE_DIR and HERDR_SOCKET_PATH for every hook.

import { createBora } from "./bora_api.mjs";
import { pushDir } from "./devices.mjs";
import { handleHook } from "./push.mjs";

const USAGE = "usage: run.sh hook   (run by bora for pane.agent_status_changed and channel.message)";

/** @param {Record<string, string | undefined>} env */
function lazyBora(env) {
	/** @type {ReturnType<typeof createBora> | undefined} */
	let bora;
	const get = () => (bora ??= createBora(env));
	return {
		/** @param {string} paneId */
		paneGet: async (paneId) => get().paneGet(paneId),
		/** @param {string} workspaceId */
		workspaceGet: async (workspaceId) => get().workspaceGet(workspaceId),
	};
}

async function main() {
	const args = process.argv.slice(2);
	if (args.length !== 1 || args[0] !== "hook") {
		console.error(USAGE);
		return 2;
	}
	const env = process.env;
	const stateDir = env.HERDR_PLUGIN_STATE_DIR;
	if (!stateDir) {
		console.error("windhover-push: HERDR_PLUGIN_STATE_DIR is not set; bora sets it for plugin hooks");
		return 2;
	}
	await handleHook(env.HERDR_PLUGIN_EVENT ?? "", env.HERDR_PLUGIN_EVENT_JSON ?? "{}", {
		stateDir,
		pushDir: pushDir(env),
		bora: lazyBora(env),
		fetch: (url, init) => fetch(url, init),
		now: () => Date.now(),
		sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
		log: (line) => console.log(line),
	});
	return 0;
}

main().then(
	(code) => {
		process.exitCode = code;
	},
	(error) => {
		console.error(`windhover-push: ${error instanceof Error ? error.message : String(error)}`);
		process.exitCode = 1;
	},
);
