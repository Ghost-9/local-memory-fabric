import { createHash } from "node:crypto";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import { createInterface } from "node:readline";
import { homedir } from "node:os";
import { resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { defineTool, type AgentToolResult, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import {
	isLoopbackModel as loopbackModelFor,
	mayWriteToMemory as mayWriteToMemoryFor,
	parseProviderList,
	providerAllowed as providerAllowedFor,
	remoteFullAccessEnabled,
	shouldScrubContext as shouldScrubContextFor,
	type Model,
} from "./remote-policy.ts";

const SCRIPT = fileURLToPath(new URL("./memory_fabric.py", import.meta.url));
const PYTHON = process.env.MEMORY_FABRIC_PYTHON || "python3";
const RUST_BIN = process.env.MEMORY_FABRIC_BIN || fileURLToPath(new URL("./rust/target/release/memory-fabric", import.meta.url));
const ENGINE = (process.env.MEMORY_FABRIC_ENGINE || "auto").toLowerCase();
const MAX_STDOUT = 256 * 1024;

type Response = Record<string, unknown>;
type Pending = { request: Response; resolve: (value: Response) => void; timer?: ReturnType<typeof setTimeout> };
let rustChild: ChildProcessWithoutNullStreams | undefined;
let rustInFlight: Pending[] = [];
let rustQueue: Pending[] = [];
process.once("exit", () => rustChild?.kill());

function failRustWorker(child: ChildProcessWithoutNullStreams, message: string): void {
	if (rustChild !== child) return;
	const pending = [...rustInFlight, ...rustQueue];
	rustInFlight = [];
	rustQueue = [];
	for (const item of pending) {
		if (item.timer) clearTimeout(item.timer);
		item.resolve({ ok: false, error: message });
	}
	rustChild = undefined;
}

function sendNextRustRequest(child: ChildProcessWithoutNullStreams): void {
	if (rustChild !== child || rustInFlight.length > 0) return;
	const item = rustQueue.shift();
	if (!item) return;
	const timeoutMs = item.request.op === "index" ? 120000 : 15000;
	item.timer = setTimeout(() => {
		child.kill();
		failRustWorker(child, "Local memory operation timed out.");
	}, timeoutMs);
	rustInFlight.push(item);
	child.stdin.write(`${JSON.stringify(item.request)}\n`, (error) => {
		if (error) failRustWorker(child, "Rust memory request could not be sent.");
	});
}

function callRust(request: Response): Promise<Response> {
	return new Promise((resolveResult) => {
		if (rustQueue.length + rustInFlight.length >= 128) {
			resolveResult({ ok: false, error: "Rust memory request queue is full." });
			return;
		}
		if (!rustChild || rustChild.killed) {
			const child = spawn(RUST_BIN, ["--serve"], { stdio: ["pipe", "pipe", "ignore"], windowsHide: true, env: process.env });
			rustChild = child;
			child.on("error", () => failRustWorker(child, "Rust memory engine could not start."));
			child.on("close", () => failRustWorker(child, "Rust memory engine stopped."));
			const lines = createInterface({ input: child.stdout });
			lines.on("line", (line) => {
				if (Buffer.byteLength(line, "utf8") > MAX_STDOUT) {
					child.kill();
					failRustWorker(child, "Rust memory response exceeded its size limit.");
					return;
				}
				const item = rustInFlight.shift();
				if (!item) return;
				if (item.timer) clearTimeout(item.timer);
				try { item.resolve(JSON.parse(line) as Response); }
				catch { item.resolve({ ok: false, error: "Rust memory engine returned an invalid response." }); }
				sendNextRustRequest(child);
			});
		}
		const child = rustChild!;
		rustQueue.push({ request, resolve: resolveResult });
		sendNextRustRequest(child);
	});
}

function callPythonFabric(request: Response): Promise<Response> {
	return new Promise((resolve) => {
		const child = spawn(PYTHON, [SCRIPT, "--json-stdin"], {
			stdio: ["pipe", "pipe", "ignore"],
			windowsHide: true,
			env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
		});
		let stdout = "";
		let settled = false;
		const finish = (value: Response) => {
			if (settled) return;
			settled = true;
			clearTimeout(timer);
			resolve(value);
		};
		const timeoutMs = request.op === "index" ? 120000 : 15000;
		const timer = setTimeout(() => {
			child.kill();
			finish({ ok: false, error: "Local memory operation timed out." });
		}, timeoutMs);
		child.on("error", () => finish({ ok: false, error: "Local memory service could not start." }));
		child.stdout.setEncoding("utf8");
		child.stdout.on("data", (chunk: string) => {
			stdout += chunk;
			if (stdout.length > MAX_STDOUT) {
				child.kill();
				finish({ ok: false, error: "Local memory response exceeded its size limit." });
			}
		});
		child.on("close", () => {
			if (settled) return;
			try {
				const result = JSON.parse(stdout.trim()) as Response;
				finish(result);
			} catch {
				finish({ ok: false, error: "Local memory service returned an invalid response." });
			}
		});
		child.stdin.end(JSON.stringify(request));
	});
}

function callFabric(request: Response): Promise<Response> {
	let requestBytes: number;
	try { requestBytes = Buffer.byteLength(JSON.stringify(request), "utf8"); }
	catch { return Promise.resolve({ ok: false, error: "Local memory request could not be serialized." }); }
	if (requestBytes > 128 * 1024) return Promise.resolve({ ok: false, error: "Local memory request exceeded its size limit." });
	if (ENGINE !== "python" && existsSync(RUST_BIN)) return callRust(request);
	if (ENGINE === "rust") return Promise.resolve({ ok: false, error: "Rust engine requested but its binary is unavailable; build rust/ with cargo build --release." });
	return callPythonFabric(request);
}

function toolResult(value: Response): AgentToolResult<Record<string, unknown>> {
	const text = JSON.stringify(value, null, 2);
	return { content: [{ type: "text", text }], details: value };
}

const LOCAL_PROVIDERS = parseProviderList(process.env.PI_MEMORY_LOCAL_PROVIDERS, "lmstudio,llama.cpp,ollama");
/**
 * Operator exception (2026-10-10, extended 2026-10-11): providers permitted to read
 * canonical memory even though their endpoint is NOT loopback. Without this exception,
 * access requires an allowlisted provider AND a loopback endpoint.
 *
 * An exempted provider gets the same READ access as a loopback one: results are returned
 * to it and its context is not scrubbed, because an exemption that strips the answer is
 * not an exemption. Reads therefore do leave the machine for these providers.
 *
 * WRITE access is separate and off by default. Set PI_MEMORY_REMOTE_FULL_ACCESS=1 to let an
 * exempted provider write; with it unset an exempted provider is read-only and only a
 * loopback provider may write. A provider that is not allowlisted at all may never write,
 * switch or no switch.
 *
 * The three-gate decision table lives in remote-policy.ts and is covered by
 * remote-policy.test.ts. Override or extend the list with
 * PI_MEMORY_REMOTE_ALLOWED_PROVIDERS (comma-separated); set it to an empty string to
 * restore the strict loopback-only default.
 */
const REMOTE_ALLOWED_PROVIDERS = parseProviderList(process.env.PI_MEMORY_REMOTE_ALLOWED_PROVIDERS, "opencode-go,opencode,pi");
const REMOTE_FULL_ACCESS = remoteFullAccessEnabled(process.env.PI_MEMORY_REMOTE_FULL_ACCESS);
const VAULT_ROOT = resolve(process.env.MEMORY_FABRIC_VAULT || resolve(homedir(), "system-memory"));

// Thin bindings so the call sites below read plainly; every decision lives in remote-policy.ts.
const isLoopbackModel = (model?: Model): boolean => loopbackModelFor(model, LOCAL_PROVIDERS);
const providerAllowed = (model?: Model): boolean =>
	providerAllowedFor(model, LOCAL_PROVIDERS, REMOTE_ALLOWED_PROVIDERS);
const shouldScrubContext = (model?: Model): boolean =>
	shouldScrubContextFor(model, LOCAL_PROVIDERS, REMOTE_ALLOWED_PROVIDERS);
const mayWriteToMemory = (model?: Model): boolean =>
	mayWriteToMemoryFor(model, LOCAL_PROVIDERS, REMOTE_ALLOWED_PROVIDERS, REMOTE_FULL_ACCESS);

function providerOf(ctx: { model?: { provider?: string; baseUrl?: string } }): string | undefined {
	return providerAllowed(ctx.model) ? ctx.model?.provider : undefined;
}

function insideVault(value: string, cwd: string): boolean {
	const expanded = value.trim().replace(/^~(?=$|[/\\])/, homedir());
	const target = resolve(cwd, expanded);
	return target === VAULT_ROOT || target.startsWith(VAULT_ROOT + sep);
}

function inputTargetsVault(value: unknown, cwd: string, key = ""): boolean {
	if (typeof value === "string") {
		if (/^(?:path|cwd|directory|dir|root)$/i.test(key) && insideVault(value, cwd)) return true;
		const normalized = value.replaceAll("\\", "/");
		if (key === "command") return normalized.includes(VAULT_ROOT.replaceAll("\\", "/")) ||
			/\bsystem-memory\b/i.test(normalized) || /apple[ _-]*notes|notes[ _-]*export|com\.apple\.notes|notestore\.sqlite/i.test(normalized);
		return normalized.includes(VAULT_ROOT.replaceAll("\\", "/")) ||
			/(?:^|[~/])system-memory(?:[/\\]|$)/i.test(normalized) ||
			/apple[ _-]*notes|notes[ _-]*export|com\.apple\.notes|notestore\.sqlite/i.test(normalized);
	}
	if (Array.isArray(value)) return value.some((item) => inputTargetsVault(item, cwd, key));
	if (value && typeof value === "object") return Object.entries(value).some(([k, item]) => inputTargetsVault(item, cwd, k));
	return false;
}

function inputTouchesAppleNotes(value: unknown): boolean {
	if (typeof value === "string") return /apple[ _-]*notes|notes[ _-]*export|com\.apple\.notes|notestore\.sqlite/i.test(value);
	if (Array.isArray(value)) return value.some(inputTouchesAppleNotes);
	if (value && typeof value === "object") return Object.values(value).some(inputTouchesAppleNotes);
	return false;
}

function fingerprint(message: unknown): string {
	return createHash("sha256").update(JSON.stringify(message) ?? "").digest("hex");
}

function isLocalProviderId(provider: unknown): boolean {
	return typeof provider === "string" && LOCAL_PROVIDERS.has(provider.toLowerCase());
}

export default function (pi: ExtensionAPI) {
	if (process.env.PI_MEMORY_FABRIC === "0") return;

	const localMessageHashes = new Set<string>();
	const localToolCallIds = new Set<string>();
	pi.on("message_end", (event, ctx) => {
		if (!isLoopbackModel(ctx.model)) return;
		const message = event.message as Record<string, unknown>;
		const role = String(message.role ?? "");
		if (role !== "assistant") return;
		try {
			localMessageHashes.add(fingerprint(message));
			if (localMessageHashes.size > 4096) localMessageHashes.delete(localMessageHashes.values().next().value!);
		} catch { /* A non-serializable extension message is not retained for this guard. */ }
	});

	pi.on("context", (event, ctx) => {
		if (!shouldScrubContext(ctx.model)) return;
		const messages = event.messages as Array<Record<string, unknown>>;
		for (const message of messages) {
			if (message.role !== "assistant" || !isLocalProviderId(message.provider)) continue;
			if (!Array.isArray(message.content)) continue;
			for (const part of message.content as Array<Record<string, unknown>>) {
				if (part.type !== "toolCall" || typeof part.id !== "string") continue;
				const toolName = String(part.name ?? "");
				const args = part.arguments ?? part.input;
				if (toolName.startsWith("memory_fabric_") || inputTargetsVault(args, ctx.cwd) || inputTouchesAppleNotes(args)) {
					localToolCallIds.add(part.id);
					if (localToolCallIds.size > 4096) localToolCallIds.delete(localToolCallIds.values().next().value!);
				}
			}
		}
		const filtered = messages.filter((message) => {
			const customType = typeof message.customType === "string" ? message.customType : "";
			if (customType.startsWith("vault-context")) return false;
			if (message.role === "assistant" && isLocalProviderId(message.provider)) return false;
			if (message.role === "toolResult") {
				const name = String(message.toolName ?? "");
				if (name.startsWith("memory_fabric_") || localToolCallIds.has(String(message.toolCallId ?? ""))) return false;
			}
			if (message.role === "assistant") {
				try { if (localMessageHashes.has(fingerprint(message))) return false; } catch { /* Keep unhashable messages. */ }
			}
			return true;
		});
		return filtered.length === messages.length ? undefined : { messages: filtered };
	});

	pi.on("tool_call", (event, ctx) => {
		if (inputTouchesAppleNotes(event.input)) {
			return { block: true, reason: "Apple Notes and exports are excluded from agent access." };
		}
		const explicitVaultPath = inputTargetsVault(event.input, ctx.cwd);
		const cwdIsVault = insideVault(".", ctx.cwd);
		const writeTool = event.toolName === "write" || event.toolName === "edit";
		if (writeTool && explicitVaultPath && !mayWriteToMemory(ctx.model)) {
			return {
				block: true,
				reason: REMOTE_FULL_ACCESS
					? "Canonical-memory writes are restricted to an allowlisted provider; see PI_MEMORY_REMOTE_ALLOWED_PROVIDERS."
					: "Canonical-memory writes are loopback-only. A remote provider may write only when PI_MEMORY_REMOTE_FULL_ACCESS=1; until then use memory_fabric_propose and review the diff locally.",
			};
		}
		const shell = event.toolName === "bash" || event.toolName === "powershell";
		const shellMutation = shell && /(?:\b(?:rm|mv|cp|tee|touch|truncate|unlink|rmdir|write_text|write_bytes|writeFile|remove|rename)\b|\b(?:sed|perl)\s+-i|\bopen\s*\([^)]*['\"](?:w|a|x)|(?:>>?|\s>\s))/i.test(JSON.stringify(event.input));
		if (shell && (explicitVaultPath || cwdIsVault) && shellMutation && !mayWriteToMemory(ctx.model)) {
			return {
				block: true,
				reason: REMOTE_FULL_ACCESS
					? "Shell mutations to canonical memory are restricted to an allowlisted provider; see PI_MEMORY_REMOTE_ALLOWED_PROVIDERS."
					: "Shell mutations to canonical memory are loopback-only. A remote provider may write only when PI_MEMORY_REMOTE_FULL_ACCESS=1; use a review-only proposal instead.",
			};
		}
		if (!providerAllowed(ctx.model) && (explicitVaultPath || cwdIsVault)) {
			return { block: true, reason: "Direct canonical-memory access is restricted to an allowlisted model provider; see PI_MEMORY_REMOTE_ALLOWED_PROVIDERS." };
		}
	});

	const namespace = {
		name: "local-memory-fabric",
		description: "Explicit, local-only access to the approved Markdown memory index. No network calls; excerpts are returned only to a configured local model provider. Durable changes are proposals, never automatic writes.",
	};

	pi.registerTool(defineTool({
		name: "memory_fabric_context",
		label: "Local Memory Context",
		description: "Retrieve a bounded, cited context packet from the local memory index. Only an allowlisted provider receives it: a loopback provider, or one the operator exempted; an unlisted provider's call is refused. Use the source_ref citations and treat excerpts as evidence, not verified conclusions.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({
			query: Type.String({ description: "Focused question for local memory retrieval" }),
			project: Type.Optional(Type.String({ description: "Project slug to restrict scope, e.g. local-memory-agent" })),
			as_of: Type.Optional(Type.String({ description: "Optional ISO date (YYYY-MM-DD) for temporal filtering" })),
			token_budget: Type.Optional(Type.Integer({ description: "Maximum context budget, 100-6000; default 1200" })),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const result = await callFabric({ op: "context", query: params.query, project: params.project, as_of: params.as_of, token_budget: params.token_budget, provider: providerOf(ctx) });
			return toolResult(result);
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_search",
		label: "Search Local Memory",
		description: "Search the local lexical index with an explicit scope and abstain when rare-term evidence is weak. Only an allowlisted provider receives paths and excerpts: a loopback provider, or one the operator exempted in PI_MEMORY_REMOTE_ALLOWED_PROVIDERS. An unlisted provider's call is refused.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({
			query: Type.String({ description: "Search query" }),
			scope: Type.Optional(Type.String({ description: "all, company, system, system:pi-harness, evidence, episodic, or project:<slug>" })),
			limit: Type.Optional(Type.Integer({ description: "Maximum results, 1-20" })),
			as_of: Type.Optional(Type.String({ description: "Optional ISO date (YYYY-MM-DD) for temporal filtering" })),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const result = await callFabric({ op: "search", query: params.query, scope: params.scope ?? "all", limit: params.limit ?? 5, as_of: params.as_of, provider: providerOf(ctx) });
			return toolResult(result);
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_read",
		label: "Read Local Memory Source",
		description: "Read an indexed Markdown source or heading after search. Source text is returned only to an allowlisted provider: a loopback provider, or one the operator exempted. An unlisted provider's call is refused.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({
			source_ref: Type.String({ description: "Exact source_ref returned by memory_fabric_search" }),
			heading: Type.Optional(Type.String({ description: "Optional exact Markdown heading to read" })),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const result = await callFabric({ op: "read", source_ref: params.source_ref, heading: params.heading, provider: providerOf(ctx) });
			return toolResult(result);
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_propose",
		label: "Propose Memory Update",
		description: "Create a review-only candidate for an existing section of one project note. The proposal is stored with mode 0600 outside the vault and never applied. An allowlisted provider receives the response, loopback or operator-exempted. Writing vault content directly is separate and still needs a loopback provider, or PI_MEMORY_REMOTE_FULL_ACCESS=1.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
		parameters: Type.Object({
			project: Type.String({ description: "Project slug" }),
			target: Type.String({ description: "Existing project note path, e.g. projects/local-memory-agent.md" }),
			section: Type.String({ description: "Existing Markdown heading to propose under" }),
			text: Type.String({ description: "Candidate text; potential credentials are rejected locally" }),
			source_refs: Type.Optional(Type.Array(Type.String(), { description: "Supporting indexed source_ref values" })),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const result = await callFabric({ op: "propose", project: params.project, target: params.target, section: params.section,
				text: params.text, source_refs: params.source_refs ?? [], provider: providerOf(ctx) });
			return toolResult(result);
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_proposal_show",
		label: "Review Memory Proposal",
		description: "Show the diff for an unapproved memory proposal. Only an allowlisted local provider may inspect proposal content. No apply operation exists in this version.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({ proposal_id: Type.String({ description: "12-character proposal id" }) }),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const result = await callFabric({ op: "proposal-show", proposal_id: params.proposal_id, provider: providerOf(ctx) });
			return toolResult(result);
		},
	}));

	pi.registerCommand("memory-fabric", {
		description: "Local memory index: status, index, or proposals. Indexing is explicit and never writes to the vault.",
		handler: async (args, ctx) => {
			const action = args.trim() || "status";
			if (action === "tools") {
				const engineStatus = await callFabric({ op: "status" });
				const backend = ENGINE === "python" || (!existsSync(RUST_BIN) && ENGINE !== "rust") ? "python" : existsSync(RUST_BIN) ? "rust" : "rust-unavailable";
				const allTools = pi.getAllTools();
				const tools = allTools.filter((tool) => tool.name.startsWith("memory_fabric_")).map((tool) => `${tool.name}:${tool.exposure}`);
				const active = pi.getActiveTools().filter((name) => name.startsWith("memory_fabric_"));
				ctx.ui.notify(`backend=${backend}; indexed=${engineStatus.indexed === true}; documents=${engineStatus.documents ?? 0}; configured=${allTools.length}; memory_fabric=${tools.length}; active=${active.length}; ${tools.join(", ")}`, "info");
				return;
			}
			if (!["status", "index", "proposals"].includes(action)) {
				ctx.ui.notify("Usage: /memory-fabric [status|index|proposals|tools]", "warning");
				return;
			}
			const op = action === "proposals" ? "proposal-list" : action;
			const result = await callFabric({ op });
			const summary = action === "index"
				? `Indexed local source set: ${result.documents ?? 0} documents; added ${result.added ?? 0}, updated ${result.updated ?? 0}, removed from index ${result.removed_from_index ?? 0}. Vault files were not modified.`
				: action === "proposals"
					? `Review-only proposals: ${Array.isArray(result.proposals) ? result.proposals.length : 0}. Inspect proposal files locally; none has been applied.`
					: `Local memory index: ${result.indexed ? `${result.documents} documents` : "not initialized"}.`;
			ctx.ui.notify(result.ok ? summary : "Local memory operation failed; see local diagnostics.", result.ok ? "info" : "error");
		},
	});
}
