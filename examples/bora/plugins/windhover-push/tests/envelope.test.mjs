import assert from "node:assert/strict";
import fs from "node:fs/promises";
import { describe, test } from "node:test";
import { decodeBase64, encodeBase64, open, PushEnvelopeError, seal, sealPlaintext, sealWithNonce } from "../envelope.mjs";

// Copied unchanged from Windhover's Tests/Fixtures/push-envelope-v1.json.
const vectors = JSON.parse(await fs.readFile(new URL("./fixtures/push-envelope-v1.json", import.meta.url), "utf8"));

/** @param {string} envelope @param {string} key */
async function openError(envelope, key) {
	try {
		await open(envelope, key);
	} catch (error) {
		if (error instanceof PushEnvelopeError) return error.code;
		throw error;
	}
	throw new Error("open succeeded");
}

describe("push envelope v1 vectors", () => {
	test("fixture is v1 with the shared AAD", () => {
		assert.equal(vectors.version, 1);
		assert.equal(vectors.aad, "windhover-push-v1");
		assert.ok(vectors.valid.length > 0);
		assert.ok(vectors.invalid.length > 0);
	});

	for (const v of vectors.valid) {
		test(`opens ${v.name}`, async () => {
			const message = await open(v.envelope, v.key);
			assert.deepEqual({ ...message, connectionId: message.connectionId ?? null }, v.message);
		});

		test(`reproduces ${v.name} byte for byte from its plaintext and nonce`, async () => {
			assert.equal(await sealPlaintext(v.plaintext, v.key, /** @type {Uint8Array} */ (decodeBase64(v.nonce))), v.envelope);
		});

		const message = { title: v.message.title, body: v.message.body, connectionId: v.message.connectionId ?? undefined };
		if (JSON.stringify(message) === v.plaintext) {
			test(`seals ${v.name} from its message to the same envelope`, async () => {
				assert.equal(await sealWithNonce(message, v.key, /** @type {Uint8Array} */ (decodeBase64(v.nonce))), v.envelope);
			});
		}
	}

	for (const v of vectors.invalid) {
		test(`rejects ${v.name} with ${v.error}`, async () => {
			assert.equal(await openError(v.envelope, v.key), v.error);
		});
	}
});

describe("seal", () => {
	const key = vectors.valid[0].key;

	test("round-trips and uses a fresh nonce each time", async () => {
		const message = { title: "t", body: "b", connectionId: "3F2504E0-4F89-11D3-9A0C-0305E82C3301" };
		const a = await seal(message, key);
		const b = await seal(message, key);
		assert.notEqual(a, b);
		assert.deepEqual(await open(a, key), message);
		assert.deepEqual(await open(b, key), message);
	});

	test("normalises connectionId to uppercase, as WindhoverKit encodes UUIDs", async () => {
		const opened = await open(await seal({ title: "t", body: "b", connectionId: "3f2504e0-4f89-11d3-9a0c-0305e82c3301" }, key), key);
		assert.equal(opened.connectionId, "3F2504E0-4F89-11D3-9A0C-0305E82C3301");
	});

	test("refuses a key that is not 32 bytes", async () => {
		await assert.rejects(seal({ title: "t", body: "b" }, "AAEC"), /32 bytes/);
	});

	test("refuses a message without a string title or body", () => {
		assert.throws(() => sealWithNonce(/** @type {any} */ ({ body: "b" }), key, new Uint8Array(12)), /title and body/);
		assert.throws(() => sealWithNonce(/** @type {any} */ ({ title: "t", body: 1 }), key, new Uint8Array(12)), /title and body/);
	});

	test("puts ask last in the plaintext and round-trips it (contract C6)", async () => {
		const nonce = new Uint8Array(12);
		const envelope = await sealWithNonce(
			{ title: "builder asks", body: "qual banco?", connectionId: "3F2504E0-4F89-11D3-9A0C-0305E82C3301", ask: { channel: "teste", seq: 12 } },
			key,
			nonce,
		);
		const expected = await sealPlaintext(
			'{"title":"builder asks","body":"qual banco?","connectionId":"3F2504E0-4F89-11D3-9A0C-0305E82C3301","ask":{"channel":"teste","seq":12}}',
			key,
			nonce,
		);
		assert.equal(envelope, expected);
		assert.deepEqual((await open(envelope, key)).ask, { channel: "teste", seq: 12 });
	});

	test("refuses a malformed ask", async () => {
		assert.throws(() => sealWithNonce({ title: "t", body: "b", ask: /** @type {any} */ ({ channel: "c", seq: "1" }) }, key, new Uint8Array(12)), /ask/);
	});
});

describe("open: invalidMessage", () => {
	const key = vectors.valid[0].key;
	const nonce = new Uint8Array(12);

	for (const [name, plaintext] of [
		["not JSON", "hello"],
		["JSON array", "[]"],
		["JSON null", "null"],
		["missing body", '{"title":"t"}'],
		["non-string title", '{"title":1,"body":"b"}'],
		["connectionId not a UUID", '{"title":"t","body":"b","connectionId":"nope"}'],
		["connectionId not a string", '{"title":"t","body":"b","connectionId":7}'],
		["ask without seq", '{"title":"t","body":"b","ask":{"channel":"c"}}'],
	]) {
		test(name, async () => {
			assert.equal(await openError(await sealPlaintext(plaintext, key, nonce), key), "invalidMessage");
		});
	}

	test("null connectionId opens as absent", async () => {
		const opened = await open(await sealPlaintext('{"title":"t","body":"b","connectionId":null}', key, nonce), key);
		assert.deepEqual(opened, { title: "t", body: "b" });
	});

	test("checks version before authentication", async () => {
		const bytes = /** @type {Uint8Array} */ (decodeBase64(vectors.valid[0].envelope));
		bytes[0] = 0;
		assert.equal(await openError(encodeBase64(bytes), "QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8="), "unsupportedVersion");
	});
});
