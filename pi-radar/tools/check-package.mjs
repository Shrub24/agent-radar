#!/usr/bin/env node
// A published `types` entry resolves its own relative imports, so every module a
// shipped source file imports must ship too. Missing one breaks consumers without
// failing any test: the bundled `dist` inlines it.
import { readFileSync } from "node:fs";
import { dirname, join, normalize } from "node:path";

const root = join(dirname(new URL(import.meta.url).pathname), "..");
const manifest = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const shipped = manifest.files.filter((entry) => entry.endsWith(".ts"));

const missing = [];
for (const entry of shipped) {
	const source = readFileSync(join(root, entry), "utf8");
	for (const [, specifier] of source.matchAll(/from\s+"(\.[^"]+\.ts)"/g)) {
		const target = normalize(join(dirname(entry), specifier));
		if (!shipped.includes(target)) missing.push(`${entry} imports ${target}`);
	}
}
if (missing.length > 0) {
	console.error(`package files leave these imports unresolved:\n  ${missing.join("\n  ")}`);
	process.exit(1);
}
console.log(`package files resolve every relative import across ${shipped.length} shipped modules`);
