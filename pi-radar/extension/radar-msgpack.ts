/**
 * Bounded MessagePack for the daemon's binary frames.
 *
 * Integers are exact: a value outside the safe-integer range is a `bigint`,
 * whatever width the sender used. Maps have unique string keys and extension
 * types are refused, so a decoded value is always plain JSON-shaped data.
 */

export type MsgValue =
	| null
	| boolean
	| number
	| bigint
	| string
	| Uint8Array
	| MsgValue[]
	| { [key: string]: MsgValue };

/** Matches the daemon's own decode depth. */
export const MSGPACK_MAX_DEPTH = 64;

const MIN_I64 = -(2n ** 63n);
const MAX_U64 = 2n ** 64n - 1n;
const text = new TextDecoder("utf-8", { fatal: true });
const utf8 = new TextEncoder();

export class MsgpackError extends Error {}

function fail(message: string): never {
	throw new MsgpackError(message);
}

const exact = (value: bigint): number | bigint =>
	value >= BigInt(Number.MIN_SAFE_INTEGER) && value <= BigInt(Number.MAX_SAFE_INTEGER)
		? Number(value)
		: value;

/** Decode exactly one value; trailing bytes are an error. */
export function decodeMsgpack(bytes: Uint8Array): MsgValue {
	const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
	let at = 0;
	const take = (n: number): number => {
		if (n > bytes.length - at) fail("truncated MessagePack");
		const start = at;
		at += n;
		return start;
	};
	const length = (width: 1 | 2 | 4): number => {
		const start = take(width);
		return width === 1 ? view.getUint8(start) : width === 2 ? view.getUint16(start) : view.getUint32(start);
	};
	const string = (n: number): string => {
		const start = take(n);
		try {
			return text.decode(bytes.subarray(start, start + n));
		} catch {
			return fail("invalid UTF-8 in string");
		}
	};
	const array = (n: number, depth: number): MsgValue[] => {
		// Every element takes a byte, so a larger claim cannot be honest.
		if (n > bytes.length - at) fail("truncated MessagePack");
		return Array.from({ length: n }, () => value(depth + 1));
	};
	const map = (n: number, depth: number): { [key: string]: MsgValue } => {
		if (n * 2 > bytes.length - at) fail("truncated MessagePack");
		const out: { [key: string]: MsgValue } = {};
		for (let i = 0; i < n; i++) {
			const key = value(depth + 1);
			if (typeof key !== "string") fail("map key is not a string");
			if (key === "__proto__" || Object.hasOwn(out, key)) fail(`unusable map key ${key}`);
			out[key] = value(depth + 1);
		}
		return out;
	};
	const value = (depth: number): MsgValue => {
		if (depth > MSGPACK_MAX_DEPTH) fail("MessagePack nests too deeply");
		const tag = view.getUint8(take(1));
		if (tag <= 0x7f) return tag;
		if (tag >= 0xe0) return tag - 0x100;
		if (tag <= 0x8f) return map(tag & 0x0f, depth);
		if (tag <= 0x9f) return array(tag & 0x0f, depth);
		if (tag <= 0xbf) return string(tag & 0x1f);
		switch (tag) {
			case 0xc0: return null;
			case 0xc2: return false;
			case 0xc3: return true;
			case 0xc4: { const n = length(1); const s = take(n); return bytes.slice(s, s + n); }
			case 0xc5: { const n = length(2); const s = take(n); return bytes.slice(s, s + n); }
			case 0xc6: { const n = length(4); const s = take(n); return bytes.slice(s, s + n); }
			case 0xca: return view.getFloat32(take(4));
			case 0xcb: return view.getFloat64(take(8));
			case 0xcc: return view.getUint8(take(1));
			case 0xcd: return view.getUint16(take(2));
			case 0xce: return view.getUint32(take(4));
			case 0xcf: return exact(view.getBigUint64(take(8)));
			case 0xd0: return view.getInt8(take(1));
			case 0xd1: return view.getInt16(take(2));
			case 0xd2: return view.getInt32(take(4));
			case 0xd3: return exact(view.getBigInt64(take(8)));
			case 0xd9: return string(length(1));
			case 0xda: return string(length(2));
			case 0xdb: return string(length(4));
			case 0xdc: return array(length(2), depth);
			case 0xdd: return array(length(4), depth);
			case 0xde: return map(length(2), depth);
			case 0xdf: return map(length(4), depth);
			default: return fail(`unsupported MessagePack type 0x${tag.toString(16)}`);
		}
	};
	const decoded = value(0);
	if (at !== bytes.length) fail("trailing bytes after MessagePack value");
	return decoded;
}

/**
 * Encode one value in its smallest form. Object keys keep insertion order and
 * `undefined` properties are omitted. A whole-valued `number` is an integer on
 * the wire, so a field the daemon types as a float must be sent as a fraction.
 */
export function encodeMsgpack(input: MsgValue | undefined, maxBytes: number): Uint8Array {
	let out = new Uint8Array(256);
	let size = 0;
	const room = (n: number): DataView => {
		if (size + n > maxBytes) fail("MessagePack exceeds its size limit");
		if (size + n > out.length) {
			const grown = new Uint8Array(Math.min(maxBytes, Math.max(out.length * 2, size + n)));
			grown.set(out.subarray(0, size));
			out = grown;
		}
		return new DataView(out.buffer, size, n);
	};
	const put = (tag: number, width = 0, write?: (view: DataView) => void): void => {
		const view = room(1 + width);
		view.setUint8(0, tag);
		write?.(new DataView(out.buffer, size + 1, width));
		size += 1 + width;
	};
	const raw = (bytes: Uint8Array): void => {
		room(bytes.length);
		out.set(bytes, size);
		size += bytes.length;
	};
	const header = (n: number, small: number, limit: number, tag8: number | undefined, tag16: number, tag32: number): void => {
		if (n < limit) put(small | n);
		else if (tag8 !== undefined && n < 0x100) put(tag8, 1, (v) => v.setUint8(0, n));
		else if (n < 0x10000) put(tag16, 2, (v) => v.setUint16(0, n));
		else put(tag32, 4, (v) => v.setUint32(0, n));
	};
	const integer = (n: bigint): void => {
		if (n < MIN_I64 || n > MAX_U64) fail("integer outside 64 bits");
		if (n >= 0n) {
			if (n < 0x80n) put(Number(n));
			else if (n < 0x100n) put(0xcc, 1, (v) => v.setUint8(0, Number(n)));
			else if (n < 0x10000n) put(0xcd, 2, (v) => v.setUint16(0, Number(n)));
			else if (n < 0x100000000n) put(0xce, 4, (v) => v.setUint32(0, Number(n)));
			else put(0xcf, 8, (v) => v.setBigUint64(0, n));
		} else if (n >= -32n) put(Number(n) + 0x100);
		else if (n >= -0x80n) put(0xd0, 1, (v) => v.setInt8(0, Number(n)));
		else if (n >= -0x8000n) put(0xd1, 2, (v) => v.setInt16(0, Number(n)));
		else if (n >= -0x80000000n) put(0xd2, 4, (v) => v.setInt32(0, Number(n)));
		else put(0xd3, 8, (v) => v.setBigInt64(0, n));
	};
	const write = (v: MsgValue | undefined, depth: number): void => {
		if (depth > MSGPACK_MAX_DEPTH) fail("MessagePack nests too deeply");
		if (v === null) put(0xc0);
		else if (typeof v === "boolean") put(v ? 0xc3 : 0xc2);
		else if (typeof v === "bigint") integer(v);
		else if (typeof v === "number") {
			if (Number.isSafeInteger(v)) integer(BigInt(v));
			else if (Number.isFinite(v)) put(0xcb, 8, (view) => view.setFloat64(0, v));
			else fail("MessagePack cannot carry a non-finite number");
		} else if (typeof v === "string") {
			const bytes = utf8.encode(v);
			header(bytes.length, 0xa0, 32, 0xd9, 0xda, 0xdb);
			raw(bytes);
		} else if (v instanceof Uint8Array) {
			header(v.length, 0, 0, 0xc4, 0xc5, 0xc6);
			raw(v);
		} else if (Array.isArray(v)) {
			header(v.length, 0x90, 16, undefined, 0xdc, 0xdd);
			for (const item of v) write(item, depth + 1);
		} else if (typeof v === "object" && v !== undefined) {
			const entries = Object.entries(v).filter(([, item]) => item !== undefined);
			header(entries.length, 0x80, 16, undefined, 0xde, 0xdf);
			for (const [key, item] of entries) {
				write(key, depth + 1);
				write(item, depth + 1);
			}
		} else fail("value has no MessagePack form");
	};
	write(input, 0);
	return out.slice(0, size);
}
