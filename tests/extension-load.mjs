import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync, cpSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const root = resolve(import.meta.dirname, "..");
const source = join(root, "index.ts");
const sourceText = readFileSync(source, "utf8");
const packageManifest = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
assert.ok(sourceText.split("\n").length < 400, "index.ts must remain a transport wrapper under 400 lines");
assert.equal(packageManifest.version, undefined, "Cargo.toml is the only package version source");
assert.equal(packageManifest.dependencies, undefined, "the extension must not ship npm runtime dependencies");
assert.equal(packageManifest.devDependencies, undefined, "the extension must not need an npm test toolchain");
for (const line of sourceText.split("\n").filter((value) => value.startsWith("import "))) {
	const specifier = line.match(/from ["']([^"']+)["']/)?.[1];
	assert.ok(specifier, `unrecognized extension import: ${line}`);
	assert.ok(
		specifier.startsWith("node:") || specifier === "typebox" || specifier === "@earendil-works/pi-coding-agent",
		`index.ts must not import business logic or an npm dependency: ${specifier}`,
	);
}

const sandbox = mkdtempSync(join(tmpdir(), "engram-extension-load-"));
try {
	writeFileSync(join(sandbox, "package.json"), JSON.stringify({ type: "module" }));
	cpSync(source, join(sandbox, "index.ts"));
	const piPath = join(sandbox, "node_modules", "@earendil-works", "pi-coding-agent");
	const typeboxPath = join(sandbox, "node_modules", "typebox");
	mkdirSync(piPath, { recursive: true });
	mkdirSync(typeboxPath, { recursive: true });
	writeFileSync(join(piPath, "package.json"), JSON.stringify({ type: "module", exports: "./index.js" }));
	writeFileSync(join(piPath, "index.js"), "export const defineTool = (tool) => tool;\n");
	writeFileSync(join(typeboxPath, "package.json"), JSON.stringify({ type: "module", exports: "./index.js" }));
	writeFileSync(join(typeboxPath, "index.js"), [
		"const shape = (...args) => ({ args });",
		"export const Type = { Object: shape, Optional: shape, String: shape, Integer: shape, Array: shape, Boolean: shape };",
	].join("\n"));
	const tools = [];
	const events = [];
	const commands = [];
	const pi = {
		registerTool(tool) { tools.push(tool); },
		on(name, handler) { events.push([name, handler]); },
		registerCommand(name, command) { commands.push([name, command]); },
	};
	const extension = await import(pathToFileURL(join(sandbox, "index.ts")).href);
	extension.default(pi);
	const names = tools.map((tool) => tool.name).sort();
	assert.equal(names.length, 17);
	for (const name of ["memory_fabric_search", "memory_fabric_read", "memory_fabric_propose", "memory_fabric_handoff", "memory_fabric_health"]) {
		assert.ok(names.includes(name), `missing registered tool ${name}`);
	}
	assert.deepEqual(events.map(([name]) => name).sort(), ["before_agent_start", "context", "message_end", "tool_call"]);
	assert.equal(commands.length, 1);
	console.log(`  loaded ${names.length} tools and ${events.length} Rust-backed hooks`);
} finally {
	rmSync(sandbox, { recursive: true, force: true });
}
