// Windhover push envelope v1: a JavaScript port of Windhover's `host/envelope.ts`
// (aryrabelo/windhover), itself the WebCrypto twin of WindhoverKit's `PushEnvelope`.
//
//   envelope  = base64( version[1]=0x01 || nonce[12] || AES-256-GCM ciphertext || tag[16] )
//   AAD       = UTF-8 of "windhover-push-v1"
//   plaintext = UTF-8 JSON {"title", "body", "connectionId"?: UUID, "ask"?: {"channel", "seq"}}
//
// `ask` is the only addition to the TypeScript original: it marks a push about a channel
// question so the app can open its "Needs you" screen. Shared vectors:
// tests/fixtures/push-envelope-v1.json, copied unchanged from Windhover's
// Tests/Fixtures/push-envelope-v1.json; every implementation must agree byte for byte.
// Plain ESM with WebCrypto only, so it runs under Bun and under Node >= 20.

export const VERSION = 1;
export const AAD = "windhover-push-v1";

const NONCE_LENGTH = 12;
const TAG_LENGTH = 16;
const KEY_LENGTH = 32;
const MIN_LENGTH = 1 + NONCE_LENGTH + TAG_LENGTH;
const UUID = /^[0-9A-Fa-f]{8}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{4}-[0-9A-Fa-f]{12}$/;
const BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

/**
 * @typedef {{channel: string, seq: number}} PushAsk
 * @typedef {{title: string, body: string, connectionId?: string, ask?: PushAsk}} PushMessage
 * @typedef {"malformed" | "unsupportedVersion" | "authentication" | "invalidMessage"} PushEnvelopeErrorCode
 */

/** Thrown by `open`; `code` (and `message`) is one of the contract's error strings. */
export class PushEnvelopeError extends Error {
	/** @param {PushEnvelopeErrorCode} code */
	constructor(code) {
		super(code);
		this.name = "PushEnvelopeError";
		this.code = code;
	}
}

/**
 * Strict standard base64 (with padding); `undefined` for anything else.
 * @param {string} text
 * @returns {Uint8Array | undefined}
 */
export function decodeBase64(text) {
	if (!BASE64.test(text)) return undefined;
	const binary = atob(text);
	const bytes = new Uint8Array(binary.length);
	for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
	return bytes;
}

/** @param {Uint8Array} bytes */
export function encodeBase64(bytes) {
	let binary = "";
	for (const byte of bytes) binary += String.fromCharCode(byte);
	return btoa(binary);
}

/**
 * @param {string} keyB64
 * @param {"encrypt" | "decrypt"} usage
 */
async function importKey(keyB64, usage) {
	const raw = decodeBase64(keyB64);
	if (!raw || raw.length !== KEY_LENGTH) throw new Error("push key must be base64 of 32 bytes");
	return crypto.subtle.importKey("raw", raw, "AES-GCM", false, [usage]);
}

/** @param {Uint8Array} nonce */
function gcmParams(nonce) {
	return { name: "AES-GCM", iv: nonce, additionalData: new TextEncoder().encode(AAD), tagLength: TAG_LENGTH * 8 };
}

/** @param {unknown} ask */
function isAsk(ask) {
	if (typeof ask !== "object" || ask === null || Array.isArray(ask)) return false;
	const { channel, seq } = /** @type {Record<string, unknown>} */ (ask);
	return typeof channel === "string" && channel.length > 0 && Number.isSafeInteger(seq) && /** @type {number} */ (seq) >= 0;
}

/**
 * The plaintext JSON for `message`, keys in the order the vectors use (`ask` last).
 * @param {PushMessage} message
 */
export function encodeMessage(message) {
	if (message.connectionId !== undefined && !UUID.test(message.connectionId)) {
		throw new Error("connectionId must be a UUID");
	}
	if (message.ask !== undefined && !isAsk(message.ask)) throw new Error("ask must be {channel, seq}");
	/** @type {PushMessage} */
	const json = { title: message.title, body: message.body };
	if (message.connectionId !== undefined) json.connectionId = message.connectionId.toUpperCase();
	if (message.ask !== undefined) json.ask = { channel: message.ask.channel, seq: message.ask.seq };
	return JSON.stringify(json);
}

/**
 * @internal Seals raw plaintext with a caller-chosen nonce. Tests only: production uses `seal`.
 * @param {string} plaintext
 * @param {string} keyB64
 * @param {Uint8Array} nonce
 */
export async function sealPlaintext(plaintext, keyB64, nonce) {
	if (nonce.length !== NONCE_LENGTH) throw new Error("nonce must be 12 bytes");
	const key = await importKey(keyB64, "encrypt");
	const sealed = new Uint8Array(await crypto.subtle.encrypt(gcmParams(nonce), key, new TextEncoder().encode(plaintext)));
	const out = new Uint8Array(1 + NONCE_LENGTH + sealed.length);
	out[0] = VERSION;
	out.set(nonce, 1);
	out.set(sealed, 1 + NONCE_LENGTH);
	return encodeBase64(out);
}

/**
 * @internal Seals `message` with a caller-chosen nonce. Tests only: production uses `seal`.
 * @param {PushMessage} message
 * @param {string} keyB64
 * @param {Uint8Array} nonce
 */
export function sealWithNonce(message, keyB64, nonce) {
	return sealPlaintext(encodeMessage(message), keyB64, nonce);
}

/**
 * Seals `message` for the device whose key is `keyB64` (base64 of 32 bytes), with a random nonce.
 * @param {PushMessage} message
 * @param {string} keyB64
 */
export function seal(message, keyB64) {
	return sealWithNonce(message, keyB64, crypto.getRandomValues(new Uint8Array(NONCE_LENGTH)));
}

/**
 * Opens an envelope. Throws `PushEnvelopeError` (malformed, unsupportedVersion, authentication, invalidMessage).
 * @param {string} envelope
 * @param {string} keyB64
 * @returns {Promise<PushMessage>}
 */
export async function open(envelope, keyB64) {
	const bytes = decodeBase64(envelope);
	if (!bytes || bytes.length < MIN_LENGTH) throw new PushEnvelopeError("malformed");
	if (bytes[0] !== VERSION) throw new PushEnvelopeError("unsupportedVersion");

	const key = await importKey(keyB64, "decrypt");
	const nonce = bytes.subarray(1, 1 + NONCE_LENGTH);
	let plaintext;
	try {
		plaintext = await crypto.subtle.decrypt(gcmParams(nonce), key, bytes.subarray(1 + NONCE_LENGTH));
	} catch {
		throw new PushEnvelopeError("authentication");
	}
	return decodeMessage(plaintext);
}

/** @param {ArrayBuffer} plaintext */
function decodeMessage(plaintext) {
	let json;
	try {
		json = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(plaintext));
	} catch {
		throw new PushEnvelopeError("invalidMessage");
	}
	if (typeof json !== "object" || json === null || Array.isArray(json)) throw new PushEnvelopeError("invalidMessage");
	const { title, body, connectionId, ask } = json;
	if (typeof title !== "string" || typeof body !== "string") throw new PushEnvelopeError("invalidMessage");
	/** @type {PushMessage} */
	const message = { title, body };
	if (connectionId !== undefined && connectionId !== null) {
		if (typeof connectionId !== "string" || !UUID.test(connectionId)) throw new PushEnvelopeError("invalidMessage");
		message.connectionId = connectionId.toUpperCase();
	}
	if (ask !== undefined && ask !== null) {
		if (!isAsk(ask)) throw new PushEnvelopeError("invalidMessage");
		message.ask = { channel: ask.channel, seq: ask.seq };
	}
	return message;
}
