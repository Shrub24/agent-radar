import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { decodeMsgpack, encodeMsgpack, MsgpackError, type MsgValue } from "./radar-msgpack.ts";

const fixtures = new URL("../../tests/fixtures/", import.meta.url);
const bytes = (hex: string): Uint8Array => Uint8Array.from(Buffer.from(hex.trim(), "hex"));
const hex = (data: Uint8Array): string => Buffer.from(data).toString("hex");
const LIMIT = 1 << 20;

const cases = JSON.parse(readFileSync(new URL("msgpack/cases.json", fixtures), "utf8")) as {
	cases: { name: string; json: MsgValue; hex: string }[];
	big: { name: string; decimal: string; hex: string }[];
};

test("every independently written case decodes and re-encodes to the same bytes", () => {
	for (const { name, json, hex: expected } of cases.cases) {
		assert.deepEqual(decodeMsgpack(bytes(expected)), json, name);
		assert.equal(hex(encodeMsgpack(json, LIMIT)), expected, name);
	}
});

test("64-bit integers beyond the safe range stay exact", () => {
	for (const { name, decimal, hex: expected } of cases.big) {
		const exactValue = BigInt(decimal);
		const decoded = decodeMsgpack(bytes(expected));
		const safe = Number.isSafeInteger(Number(exactValue));
		assert.equal(decoded, safe ? Number(exactValue) : exactValue, name);
		assert.equal(hex(encodeMsgpack(exactValue, LIMIT)), expected, name);
	}
});

test("the TUI publication fixtures decode, including a gate past 2^53", () => {
	const session = decodeMsgpack(bytes(readFileSync(new URL("tui/session.msgpack.hex", fixtures), "utf8"))) as {
		gate: bigint;
	};
	assert.equal(session.gate, 9007199254740993n);
	for (const name of ["tree", "observation"]) {
		assert.ok(decodeMsgpack(bytes(readFileSync(new URL(`tui/${name}.msgpack.hex`, fixtures), "utf8"))) !== null);
	}
});

test("binary data round-trips and undefined properties are omitted", () => {
	const data = decodeMsgpack(bytes("c403010203"));
	assert.deepEqual(data, Uint8Array.of(1, 2, 3));
	assert.equal(hex(encodeMsgpack(data, LIMIT)), "c403010203");
	assert.equal(hex(encodeMsgpack({ a: 1, b: undefined } as unknown as MsgValue, LIMIT)), "81a16101");
});

test("malformed or unusable input is refused rather than guessed at", () => {
	const refused: [string, string][] = [
		["a truncated string", "a361"],
		["a truncated body", "a36162"],
		["trailing bytes", "0001"],
		["an extension type", "d40100"],
		["the reserved tag", "c1"],
		["a non-string map key", "810101"],
		["a duplicate key", "82a16101a16102"],
		["an unsafe key", "81a95f5f70726f746f5f5f01"],
		["invalid UTF-8", "a1ff"],
		["an array longer than its input", "dd7fffffff"],
		["a map longer than its input", "df7fffffff"],
	];
	for (const [name, input] of refused) {
		assert.throws(() => decodeMsgpack(bytes(input)), MsgpackError, name);
	}
	assert.throws(() => decodeMsgpack(bytes("91".repeat(70) + "00")), MsgpackError, "nesting depth");
});

test("encoding refuses what the wire cannot carry or what exceeds its limit", () => {
	assert.throws(() => encodeMsgpack(Number.NaN, LIMIT), MsgpackError);
	assert.throws(() => encodeMsgpack(2n ** 64n, LIMIT), MsgpackError);
	assert.throws(() => encodeMsgpack("x".repeat(100), 50), MsgpackError);
});
