import { createHash } from "node:crypto";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync, readFileSync, realpathSync, appendFileSync } from "node:fs";
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
import {
	createScrubState,
	MAX_TRACKED_IDS,
	parseNoticeInterval,
	scrubMessages,
	shouldNotifyScrub,
	trackVaultToolCalls,
} from "./scrub.ts";

const SCRIPT = fileURLToPath(new URL("./memory_fabric.py", import.meta.url));
const PYTHON = process.env.MEMORY_FABRIC_PYTHON || "python3";
const RUST_BIN = process.env.MEMORY_FABRIC_BIN || fileURLToPath(new URL("./rust/target/release/memory-fabric", import.meta.url));
const ENGINE = (process.env.MEMORY_FABRIC_ENGINE || "auto").toLowerCase();
const MAX_STDOUT = 4 * 1024 * 1024;

type Response = Record<string, unknown>;
type Pending = { request: Response; resolve: (value: Response) => void; timer?: ReturnType<typeof setTimeout> };
let rustChild: ChildProcessWithoutNullStreams | undefined;
let rustInFlight: Pending[] = [];
let rustQueue: Pending[] = [];

/**
 * Tools whose use of a vault path counts as *reading* a note, for the read-through measure.
 * Covers both paths deliberately: the built-in file tools, and the fabric's own retrieval, so
 * one hook observes whichever route the session takes.
 */
const READ_TOOLS = new Set([
	"read", "grep", "glob", "find", "ls",
	"memory_fabric_read", "memory_fabric_search", "memory_fabric_context",
]);

/**
 * Refs are resolved once per session and injected once, instead of re-searched on every turn.
 *
 * Two measured reasons. Cost: the hook fires per turn, and on this machine's history that meant
 * 1,077 searches across 84 sessions, about 13 per session. Stability: a block whose content
 * changes each turn sits in the prompt prefix, so the provider re-bills everything after it -
 * measured, the injected refs took 16 distinct values in 19 consecutive turns.
 *
 * The tradeoff is explicit and accepted: refs track the session's first substantive prompt. A
 * later change of topic should use memory_fabric_search - that is what the on-demand tools are
 * for, and a pointer worth reading is worth less than a stable prefix.
 */
let sessionRefs: string[] | null = null;

/**
 * Minimal behavioural record, so "what has this served?" is answerable from state instead of by
 * re-parsing session transcripts. At most one line per session plus one per withholding, never
 * on the hot path, and never allowed to break a turn.
 */
const USAGE_LOG = resolve(homedir(), ".pi/agent/scratch/memory-fabric/usage.jsonl");
function logUsage(record: Record<string, unknown>): void {
	try {
		appendFileSync(USAGE_LOG, `${JSON.stringify({ t: new Date().toISOString(), ...record })}\n`, { mode: 0o600 });
	} catch { /* Telemetry must never cost a turn. */ }
}
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
			const child = spawn(RUST_BIN, ["--serve"], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true, env: process.env });
			rustChild = child;
			let stderr = "";
			child.stderr?.setEncoding("utf8");
			child.stderr?.on("data", (chunk: string) => {
				stderr = (stderr + chunk).slice(-8192);
			});
			child.on("error", () => failRustWorker(child, stderrHint(stderr) ?? "Rust memory engine could not start."));
			child.on("close", () => failRustWorker(child, stderrHint(stderr) ?? "Rust memory engine stopped."));
			const lines = createInterface({ input: child.stdout });
			lines.on("line", (line) => {
				if (Buffer.byteLength(line, "utf8") > MAX_STDOUT) {
					const item = rustInFlight.shift();
					if (item) {
						if (item.timer) clearTimeout(item.timer);
						item.resolve({ ok: false, error: "Rust memory response exceeded its size limit." });
					}
					sendNextRustRequest(child);
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

/**
 * Last line of a child process's stderr, capped and lightly redacted.
 *
 * Engines used to discard stderr entirely, so a startup failure arrived as "could not start"
 * with the cause on a stream nobody read - the shape that hid a one-character config bug for
 * weeks. Keep the window small and strip credential-shaped assignments, because an error from
 * a memory tool is still memory-adjacent text.
 */
function stderrHint(stream: string | null | undefined): string | undefined {
	if (!stream) return undefined;
	const line = stream
		.split("\n")
		.map((l) => l.trim())
		.filter(Boolean)
		.pop();
	if (!line) return undefined;
	// KEY=value / "key": "value" shapes could carry a secret; keep the label only.
	const redacted = line.replace(/([A-Za-z0-9_.-]*(?:key|token|secret|password|authorization)[A-Za-z0-9_.-]*)\s*[=:]\s*["']?[^"'\s]+["']?/gi, "$1=<redacted>");
	return redacted.length > 400 ? `${redacted.slice(0, 400)}...` : redacted;
}

function callPythonFabric(request: Response): Promise<Response> {
	return new Promise((resolve) => {
		const child = spawn(PYTHON, [SCRIPT, "--json-stdin"], {
			stdio: ["pipe", "pipe", "pipe"],
			windowsHide: true,
			env: { ...process.env, PYTHONDONTWRITEBYTECODE: "1" },
		});
		let stdout = "";
		let stderr = "";
		let settled = false;
		const finish = (value: Response) => {
			if (settled) return;
			settled = true;
			clearTimeout(timer);
			resolve(value);
		};
		const failWithHint = (message: string) => {
			const hint = stderrHint(stderr);
			return hint ? `${message} [engine stderr: ${hint}]` : message;
		};
		const timeoutMs = request.op === "index" ? 120000 : 15000;
		const timer = setTimeout(() => {
			child.kill();
			finish({ ok: false, error: failWithHint("Local memory operation timed out.") });
		}, timeoutMs);
		child.on("error", () => finish({ ok: false, error: failWithHint("Local memory service could not start.") }));
		child.stderr?.setEncoding("utf8");
		child.stderr?.on("data", (chunk: string) => {
			stderr = (stderr + chunk).slice(-8192);
		});
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
				finish({ ok: false, error: failWithHint("Local memory service returned an invalid response.") });
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
 * WRITE access is separate and **on by default** (operator instruction, 2026-10-14): an
 * allowlisted provider may write unless PI_MEMORY_REMOTE_FULL_ACCESS=0. An unlisted provider
 * still may not. Set it to 0 to let an
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

/**
 * Per-process scrub-notice state. Previously a bare boolean guarded a block that sat AFTER an
 * unconditional `return`, so the notice could never fire at all (CHANGELOG 0.3.4). It is now a
 * timestamp, so the reminder can repeat per session without becoming a per-turn nag (#10).
 */
const scrubNotice: { lastNotifiedAt?: number } = {};
const SCRUB_NOTICE_INTERVAL = parseNoticeInterval(process.env.MEMORY_FABRIC_SCRUB_NOTICE_MS);
const VAULT_ROOT = resolve(process.env.MEMORY_FABRIC_VAULT || resolve(homedir(), "system-memory"));

// Thin bindings so the call sites below read plainly; every decision lives in remote-policy.ts.
let ACTIVE_MEMORY_MODE = (process.env.PI_MEMORY_MODE ?? "local").trim().toLowerCase();
const isRemoteMode = () => ACTIVE_MEMORY_MODE === "remote" || ACTIVE_MEMORY_MODE === "all" || ACTIVE_MEMORY_MODE === "unrestricted";

const isLoopbackModel = (model?: Model): boolean => isRemoteMode() || loopbackModelFor(model, LOCAL_PROVIDERS);
const providerAllowed = (model?: Model): boolean =>
	isRemoteMode() ? Boolean(model?.provider || true) : providerAllowedFor(model, LOCAL_PROVIDERS, REMOTE_ALLOWED_PROVIDERS);
const shouldScrubContext = (model?: Model): boolean =>
	isRemoteMode() ? false : shouldScrubContextFor(model, LOCAL_PROVIDERS, REMOTE_ALLOWED_PROVIDERS);
const mayWriteToMemory = (model?: Model): boolean =>
	isRemoteMode() ? true : mayWriteToMemoryFor(model, LOCAL_PROVIDERS, REMOTE_ALLOWED_PROVIDERS, REMOTE_FULL_ACCESS);

function providerOf(ctx: { model?: { provider?: string; baseUrl?: string } }): string | undefined {
	return providerAllowed(ctx.model) ? (ctx.model?.provider || (isRemoteMode() ? "remote-agent" : undefined)) : undefined;
}

function insideVault(value: string, cwd: string): boolean {
	const expanded = value.trim().replace(/^~(?=$|[/\\])/, homedir());
	const target = resolve(cwd, expanded);
	let realTarget = target;
	try {
		if (existsSync(target)) realTarget = realpathSync(target);
	} catch {}
	return realTarget === VAULT_ROOT || realTarget.startsWith(VAULT_ROOT + sep);
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

export default function (pi: ExtensionAPI) {
	if (process.env.PI_MEMORY_FABRIC === "0") return;

	// The scrub predicate and its bounded-memory tracking live in `scrub.ts`, pure and
	// unit-tested. They used to live inline here, where no test could reach them — which is how
	// a dead notification and an evicting tracker both shipped green.
	const localMessageHashes = new Set<string>();
	const scrubState = createScrubState();
	pi.on("before_agent_start", async (event: any, ctx: any) => {
		const prompt = String(event?.prompt ?? "").trim();
		const isCompactedTurn = prompt.includes("# Resuming from a compaction") || prompt.includes("Compaction Resume Step");
		if (!isCompactedTurn) return;
		if (!providerAllowed(ctx?.model)) return;

		try {
			const status = await callFabric({ op: "status" });
			if (!status.ok || !status.indexed) return;
			const contextBrief = `\n[LOCAL MEMORY: COMPACTION RE-HYDRATION]\n• Memory Fabric active: ${status.documents} indexed documents in canonical vault.\n• Secret alarms: ${status.secret_alarms ?? 0} open.\n• Note: Memory excerpts and active anchors are restored. Query memory_fabric_context or memory_fabric_search for deep recall.`;
			return {
				message: {
					customType: "memory-context",
					content: contextBrief,
					display: false,
				},
			};
		} catch {
			// Fail-safe: do not block agent startup on re-hydration error
		}
	});

	pi.on("message_end", (event, ctx) => {
		if (!isLoopbackModel(ctx.model)) return;
		const message = event.message as Record<string, unknown>;
		const role = String(message.role ?? "");
		if (role !== "assistant") return;
		try {
			localMessageHashes.add(fingerprint(message));
			// Same fail-closed rule as the tool-id set: past the cap, stop trusting the set so
			// the scrub withholds rather than admits. An unhashable message is simply not
			// remembered, which is not evidence that anything may be sent.
			if (localMessageHashes.size > MAX_TRACKED_IDS) scrubState.overflowed = true;
		} catch { /* A non-serializable extension message is not retained for this guard. */ }
	});

	pi.on("context", (event, ctx) => {
		if (!shouldScrubContext(ctx.model)) return;
		const messages = event.messages as Array<Record<string, unknown>>;
		trackVaultToolCalls(messages, scrubState, LOCAL_PROVIDERS,
			(args) => inputTargetsVault(args, ctx.cwd), inputTouchesAppleNotes);
		const filtered = scrubMessages(messages, scrubState, LOCAL_PROVIDERS, localMessageHashes, fingerprint);
		if (filtered.length === messages.length) return undefined;
		// Something was actually removed, so say so: a fail-closed policy with no message reads
		// as "memory stopped working" with no way to tell why. The first scrub always announces;
		// after that a reminder repeats at most once per interval, per session (#10) - per request
		// would be noise people learn to dismiss. Names only the provider and the two knobs,
		// never a path, never content.
		const now = Date.now();
		if (shouldNotifyScrub(scrubNotice, now, SCRUB_NOTICE_INTERVAL)) {
			scrubNotice.lastNotifiedAt = now;
			ctx.ui.notify(
				`Local memory withheld this request: provider "${ctx.model?.provider ?? "unknown"}" is neither loopback nor listed in PI_MEMORY_REMOTE_ALLOWED_PROVIDERS, so fabric results and injected vault context were removed before sending. Nothing left the machine.`,
				"warning",
			);
		}
		return { messages: filtered };
	});

	pi.on("tool_call", (event, ctx) => {
		if (inputTouchesAppleNotes(event.input)) {
			return { block: true, reason: "Apple Notes and exports are excluded from agent access." };
		}
		const explicitVaultPath = inputTargetsVault(event.input, ctx.cwd);
		const cwdIsVault = insideVault(".", ctx.cwd);
		// Read instrumentation, and the reason it lives here rather than in an analysis script:
		// read-through is the only honest denominator for whether injected refs earned their
		// tokens, and it cannot be reconstructed afterwards. Fabric tool results are stripped
		// from history by the scrub, and reads normally go through the built-in `read`/`grep`
		// tools, so a post-hoc scan sees neither. Measured attempt: a loose detector reported 78%
		// by matching the injection itself, a strict one reported 0% because the evidence is not
		// in the transcript. Recording it at the moment it happens is the only way it exists.
		// Best-effort and non-blocking: it must never affect the call it observes.
		if (explicitVaultPath && sessionRefs && sessionRefs.length > 0 && READ_TOOLS.has(event.toolName)) {
			try {
				const raw = JSON.stringify(event.input);
				for (const ref of sessionRefs) {
					const stem = ref.split("/").pop() ?? ref;
					if (raw.includes(ref) || (stem.length > 0 && raw.includes(stem))) {
						logUsage({ kind: "read", ref, tool: event.toolName });
						break;
					}
				}
			} catch { /* Observing a call must never break it. */ }
		}
		const writeTool = event.toolName === "write" || event.toolName === "edit";
		if (writeTool && explicitVaultPath && !mayWriteToMemory(ctx.model)) {
			return {
				block: true,
				reason: REMOTE_FULL_ACCESS
					? "Canonical-memory writes are restricted to an allowlisted provider; see PI_MEMORY_REMOTE_ALLOWED_PROVIDERS."
					: "Canonical-memory writes are restricted to an allowlisted provider; this provider is not on the list. See PI_MEMORY_REMOTE_ALLOWED_PROVIDERS, or use memory_fabric_propose.",
			};
		}
		const shell = event.toolName === "bash" || event.toolName === "powershell";
		const shellMutation = shell && /(?:\b(?:rm|mv|cp|tee|touch|truncate|unlink|rmdir|write_text|write_bytes|writeFile|remove|rename)\b|\b(?:sed|perl)\s+-i|\bopen\s*\([^)]*['\"](?:w|a|x)|(?:>>?|\s>\s))/i.test(JSON.stringify(event.input));
		if (shell && (explicitVaultPath || cwdIsVault) && shellMutation && !mayWriteToMemory(ctx.model)) {
			return {
				block: true,
				reason: REMOTE_FULL_ACCESS
					? "Shell mutations to canonical memory are restricted to an allowlisted provider; see PI_MEMORY_REMOTE_ALLOWED_PROVIDERS."
					: "Shell mutations to canonical memory are restricted to an allowlisted provider; this provider is not on the list. See PI_MEMORY_REMOTE_ALLOWED_PROVIDERS, or use a review-only proposal.",
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

	let statusInjected = false;
	pi.on("before_agent_start", async (event, ctx) => {
		if (!providerAllowed(ctx.model)) return;
		const additions: string[] = [];

		if (!statusInjected) {
			statusInjected = true;
			const statusPath = resolve(homedir(), "system-memory", "STATUS.md");
			if (existsSync(statusPath)) {
				try {
					const body = readFileSync(statusPath, "utf8");
					const open = [...body.matchAll(/^- \\[ \\]\\s+(.+)$/gm)].map((m) => m[1].trim()).slice(0, 8);
					const done = [...body.matchAll(/^- \\[x\\]\\s+(.+)$/gim)].length;
					if (open.length > 0) {
						additions.push(`\n## Memory status (session start)\nOpen items (${open.length} shown, ${done} done): ${open.join("; ")}\nFull record: ~/system-memory/STATUS.md. Read with engram before asking.`);
					}
				} catch {}
			}
		}

		const prompt = event.prompt ?? "";
		// Resolve refs once per session; every later turn reuses them, so the injected block is
		// byte-stable and no further search is issued.
		if (prompt.length >= 24 && sessionRefs === null) {
			try {
				const searchRes = await callFabric({
					op: "search",
					query: prompt,
					scope: "all",
					limit: 3,
					provider: providerOf(ctx),
					loopback: isLoopbackModel(ctx.model),
				});
				const hits: Array<{ source_ref?: string }> = Array.isArray(searchRes.results) ? searchRes.results : [];
				sessionRefs = hits.map((h) => String(h.source_ref ?? "")).filter(Boolean);
				logUsage({
					kind: "session_refs", refs: sessionRefs.length, chars: sessionRefs.join(" ").length,
					shown: searchRes.shown ?? null, withheld: searchRes.withheld_export ?? null,
				});
			} catch {
				// Deliberately leave sessionRefs null on failure so the next turn retries. Caching []
				// here would disable injection for the rest of the session after one transient
				// engine hiccup - and the whole point of resolving once is that the first answer is
				// reused, so a wrong first answer is expensive in a way a wrong per-turn answer is
				// not. An empty result from a *successful* search still caches, which is correct.
			}
		}
		if (sessionRefs && sessionRefs.length > 0) {
			additions.push(`\n## Memory has relevant notes\n${sessionRefs.map((r) => `[[${r}]]`).join(" ")}\nRead these first via engram or view_file before re-deriving.`);
		}

		if (additions.length > 0) {
			return { message: { customType: "vault-context", content: additions.join("\n") } };
		}
	});

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
			// By default, do not set any limit for 1M+ context models; respect explicit token_budget only if passed.
			const budget = params.token_budget;
			let project = params.project;
			if (!project) {
				const cwd = ctx.cwd ?? "";
				const m = /projects[\\/]([^\\/]+)/i.exec(cwd);
				if (m) project = m[1];
			}
			const result = await callFabric({ op: "context", query: params.query, project, as_of: params.as_of, token_budget: budget, provider: providerOf(ctx), loopback: isLoopbackModel(ctx.model) });
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
			token_budget: Type.Optional(Type.Integer({ description: "Optional token budget cap to prevent search payload overflow" })),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			let scope = params.scope ?? "all";
			if (scope === "auto" || !params.scope) {
				const cwd = ctx.cwd ?? "";
				const m = /projects[\\/]([^\\/]+)/i.exec(cwd);
				if (m) scope = `project:${m[1]}`;
			}
			const result = await callFabric({ op: "search", query: params.query, scope, limit: params.limit ?? 5, as_of: params.as_of, token_budget: params.token_budget, provider: providerOf(ctx), loopback: isLoopbackModel(ctx.model) });
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
			const result = await callFabric({ op: "read", source_ref: params.source_ref, heading: params.heading, provider: providerOf(ctx), loopback: isLoopbackModel(ctx.model) });
			return toolResult(result);
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_propose",
		label: "Propose Memory Update",
		description: "Create a review-only candidate for an existing section of one project note. The proposal is stored with mode 0600 outside the vault and never applied. An allowlisted provider receives the response, loopback or operator-exempted. Writing vault content directly is separate and allowed for an allowlisted provider unless PI_MEMORY_REMOTE_FULL_ACCESS=0.",
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
		name: "memory_fabric_sandbox",
		label: "Sandbox Large Output",
		description: "Keep a large tool result out of the context window. Stores the payload outside the canonical memory at mode 0600 and indexes it, returning only a bounded head plus a handle; retrieve deliberately later with memory_fabric_search or memory_fabric_read. Only an allowlisted provider receives the handle: a loopback provider, or one the operator exempted; an unlisted provider's call is refused.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({
			title: Type.String({ description: "Short label so the payload is findable later" }),
			text: Type.String({ description: "Raw payload; rejected locally if it matches a credential shape" }),
			scope: Type.Optional(Type.String({ description: "Optional scope, default episodic" })),
		}),
		async execute(_id, params, _signal, ctx) {
			const result = await callFabric({ op: "sandbox", title: params.title, text: params.text,
				scope: params.scope ?? "episodic", provider: providerOf(ctx), loopback: isLoopbackModel(ctx.model) });
			return toolResult(result);
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_apply",
		label: "Apply Memory Proposal",
		description: "Apply an unapproved memory proposal cleanly to the target vault note. Restricted to an allowlisted local model provider. Atomically updates the vault note and commits the change to vault git repository if version-controlled.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
		parameters: Type.Object({ proposal_id: Type.String({ description: "12-character proposal id" }) }),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const result = await callFabric({ op: "apply", proposal_id: params.proposal_id, provider: providerOf(ctx) });
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

	pi.registerTool(defineTool({
		name: "memory_fabric_alarms",
		label: "Check Credential Alarms",
		description: "List unresolved credential-exposure alarms: vault notes the secret filter excluded because they contain a key. Returns the note path, the KIND of secret and a non-reversible fingerprint - never the value. Use this to tell the user which key still needs rotating, then call memory_fabric_resolve once they confirm it is changed.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({}),
		async execute(_id, _params, _signal, _onUpdate, _ctx) {
			return toolResult(await callFabric({ op: "alarms" }));
		},
	}));

	
	pi.registerTool(defineTool({
		name: "memory_fabric_audit",
		label: "Audit Memory Vault",
		description: "Audit the local memory vault for stale/expired notes, contradictions between superseded and current notes, duplicate content, and broken wikilinks. Advisory only: outputs candidate proposals and does not modify the vault.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({}),
		async execute(_id, _params, _signal, _onUpdate, _ctx) {
			return toolResult(await callFabric({ op: "audit" }));
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_links",
		label: "Inspect Note Links and Provenance",
		description: "Inspect outlinks and backlinks for an indexed Markdown source. Shows which notes cite this note and what this note depends on.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({
			source_ref: Type.String({ description: "Exact relative path of the note, e.g. projects/local-memory-agent.md" }),
		}),
		async execute(_id, params, _signal, _onUpdate, _ctx) {
			return toolResult(await callFabric({ op: "links", source_ref: params.source_ref }));
		},
	}));
	pi.registerTool(defineTool({
		name: "memory_fabric_sections",
		label: "Extract Markdown Sections & Breadcrumbs",
		description: "Extract hierarchical Markdown sections with breadcrumb paths, anchors, and line bounds for an indexed note. Only an allowlisted local provider receives section data.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: true, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({
			source_ref: Type.String({ description: "Exact relative path of the note, e.g. projects/local-memory-agent.md" }),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			return toolResult(await callFabric({ op: "sections", source_ref: params.source_ref, provider: providerOf(ctx), loopback: isLoopbackModel(ctx.model) }));
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_sync_agency",
		label: "Sync Agency Episodic Session",
		description: "Propose an episodic memory record into vault/episodic/squad-sessions.md from the most recent active agency session in ~/.cache/agency-skills/active_session.json.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({}),
		async execute(_id, _params, _signal, _onUpdate, ctx) {
			const agencyPath = resolve(homedir(), ".cache", "agency-skills", "active_session.json");
			if (!existsSync(agencyPath)) {
				return toolResult({ ok: false, error: "No active agency session found." });
			}
			try {
				const session = JSON.parse(readFileSync(agencyPath, "utf8"));
				const record = `# Squad Session Record: ${new Date().toISOString().split("T")[0]}\n\n- Active Anchor: ${session.sessionAnchor ?? "none"}\n- Anchor Tier: ${session.anchorTier ?? 1}\n- Consulted Squad: ${(session.consultedSquad ?? []).join(", ")}\n- Last Action: ${session.lastAction ?? "n/a"}\n- Timestamp: ${session.updatedAt ? new Date(session.updatedAt).toISOString() : new Date().toISOString()}\n`;
				const res = await callFabric({
					op: "propose",
					target: "episodic/squad-sessions.md",
					content: record,
					summary: `Episodic anchor sync for squad ${session.sessionAnchor}`,
					provider: providerOf(ctx),
				});
				return toolResult(res);
			} catch (e: any) {
				return toolResult({ ok: false, error: e.message });
			}
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_project",
		label: "Manage Project Memory Lifecycle",
		description: "Standardized lifecycle manager for project-specific memory cards in the vault (projects/<project>.md). Supports create, update, consolidate, archive, and status operations with structured frontmatter.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false, openWorldHint: false },
		parameters: Type.Object({
			action: Type.Union([
				Type.Literal("status"),
				Type.Literal("create"),
				Type.Literal("update"),
				Type.Literal("consolidate"),
				Type.Literal("archive"),
			], { description: "Lifecycle action to perform on the project card" }),
			project: Type.String({ description: "Project identifier, e.g. pi-fsearch, avartanalabs" }),
			content: Type.Optional(Type.String({ description: "Markdown body content to add, update, or consolidate" })),
			section: Type.Optional(Type.String({ description: "Target section header (e.g. Overview, Architecture & Invariants, Status & Next Milestones)" })),
		}),
		async execute(_id, params, _signal, _onUpdate, ctx) {
			const relpath = `projects/${params.project.trim().toLowerCase().replace(/\.md$/, "")}.md`;
			const today = new Date().toISOString().split("T")[0];
			if (params.action === "status") {
				const res = await callFabric({ op: "read", source_ref: relpath, provider: providerOf(ctx) });
				return toolResult(res);
			}
			if (params.action === "create") {
				const body = params.content || "Initial project memory card created.";
				const doc = `---\ntype: project\nstatus: active\ncreated: "${today}"\nupdated: "${today}"\ntags: [project, ${params.project}]\n---\n\n# ${params.project}\n\n## Overview\n${body}\n\n## Architecture & Invariants\n\n## Status & Next Milestones\n`;
				const res = await callFabric({
					op: "propose",
					target: relpath,
					content: doc,
					summary: `Create standardized project card for ${params.project}`,
					provider: providerOf(ctx),
				});
				return toolResult(res);
			}
			if (params.action === "update") {
				const existing = await callFabric({ op: "read", source_ref: relpath, provider: providerOf(ctx) });
				let newContent = "";
				if (existing.ok && existing.content) {
					let body = typeof existing.content === "string" ? existing.content : "";
					const sec = params.section ? `## ${params.section.replace(/^#+\s*/, "")}` : "## Updates";
					const addition = `\n\n### Update (${today})\n${params.content || ""}\n`;
					if (body.includes(sec)) {
						newContent = body.replace(sec, `${sec}${addition}`);
					} else {
						newContent = `${body}\n\n${sec}${addition}`;
					}
					newContent = newContent.replace(/updated: "[^"]*"/, `updated: "${today}"`);
				} else {
					newContent = `---\ntype: project\nstatus: active\ncreated: "${today}"\nupdated: "${today}"\ntags: [project, ${params.project}]\n---\n\n# ${params.project}\n\n## ${params.section || "Overview"}\n${params.content || ""}\n`;
				}
				const res = await callFabric({
					op: "propose",
					target: relpath,
					content: newContent,
					summary: `Update project card for ${params.project}`,
					provider: providerOf(ctx),
				});
				return toolResult(res);
			}
			if (params.action === "consolidate") {
				const existing = await callFabric({ op: "read", source_ref: relpath, provider: providerOf(ctx) });
				const existingBody = existing.ok && typeof existing.content === "string" ? existing.content : "";
				const consolidation = `\n\n## Consolidated Milestones (${today})\n${params.content || "Consolidated session decisions."}\n`;
				const finalContent = existingBody
					? existingBody.replace(/updated: "[^"]*"/, `updated: "${today}"`) + consolidation
					: `---\ntype: project\nstatus: active\ncreated: "${today}"\nupdated: "${today}"\ntags: [project, ${params.project}]\n---\n\n# ${params.project}\n\n## Overview\nConsolidated project memory.\n${consolidation}`;
				const res = await callFabric({
					op: "propose",
					target: relpath,
					content: finalContent,
					summary: `Consolidate memory card for ${params.project}`,
					provider: providerOf(ctx),
				});
				return toolResult(res);
			}
			if (params.action === "archive") {
				const existing = await callFabric({ op: "read", source_ref: relpath, provider: providerOf(ctx) });
				if (!existing.ok || !existing.content) {
					return toolResult({ ok: false, error: `Project note ${relpath} does not exist.` });
				}
				let archived = (typeof existing.content === "string" ? existing.content : "").replace(/status: active/, "status: archived");
				archived = archived.replace(/updated: "[^"]*"/, `updated: "${today}"\narchived_at: "${today}"`);
				const res = await callFabric({
					op: "propose",
					target: relpath,
					content: archived,
					summary: `Archive project card for ${params.project}`,
					provider: providerOf(ctx),
				});
				return toolResult(res);
			}
			return toolResult({ ok: false, error: "Unsupported project action" });
		},
	}));

	pi.registerTool(defineTool({
		name: "memory_fabric_resolve",
		label: "Resolve Credential Alarm",
		description: "Mark one credential-exposure alarm as done, which stops the reminder that appears on every memory fetch. ONLY call this after the user states they have rotated or removed the key - resolving does not verify anything and does not change the vault. Use memory_fabric_alarms to obtain the alarm id.",
		exposure: "deferred",
		namespace,
		annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false },
		parameters: Type.Object({
			alarm_id: Type.String({ description: "12-character alarm id from memory_fabric_alarms" }),
		}),
		async execute(_id, params, _signal, _onUpdate, _ctx) {
			return toolResult(await callFabric({ op: "resolve", alarm_id: params.alarm_id }));
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
			if (!["status", "index", "proposals", "alarms", "audit", "history", "sync-agency"].includes(action) && !action.startsWith("resolve") && !action.startsWith("apply") && !action.startsWith("revert") && !action.startsWith("links") && !action.startsWith("sections") && !action.startsWith("mode") && !action.startsWith("project")) {
				ctx.ui.notify("Usage: /memory-fabric [status|index|proposals|alarms|audit|history|sync-agency|mode [local|remote]|project <act> <name>|resolve <id>|apply <id>|revert <id>|links <ref>|sections <ref>|tools]", "warning");
				return;
			}
			const applyMatch = /^apply\s+([0-9a-f]{12})$/i.exec(action);
			if (applyMatch) {
				const result = await callFabric({ op: "apply", proposal_id: applyMatch[1], provider: providerOf(ctx) });
				ctx.ui.notify(result.ok ? `Proposal ${applyMatch[1]} applied: ${result.note ?? "applied"}` : `Apply failed: ${result.error ?? result.blocked}`, result.ok ? "info" : "error");
				return;
			}
			if (action === "audit") {
				const res = await callFabric({ op: "audit" });
				ctx.ui.notify(`Vault Audit: ${res.stale_count ?? 0} stale notes, ${res.contradiction_count ?? 0} contradictions, ${res.duplicate_count ?? 0} duplicates, ${res.broken_link_count ?? 0} broken wikilinks.`, "info");
				return;
			}
			if (action === "sync-agency") {
				const agencyPath = resolve(homedir(), ".cache", "agency-skills", "active_session.json");
				if (!existsSync(agencyPath)) {
					ctx.ui.notify("No active agency session found in ~/.cache/agency-skills/active_session.json", "warning");
					return;
				}
				try {
					const session = JSON.parse(readFileSync(agencyPath, "utf8"));
					const record = `# Squad Session Record: ${new Date().toISOString().split("T")[0]}\n\n- Active Anchor: ${session.sessionAnchor ?? "none"}\n- Anchor Tier: ${session.anchorTier ?? 1}\n- Consulted Squad: ${(session.consultedSquad ?? []).join(", ")}\n- Last Action: ${session.lastAction ?? "n/a"}\n- Timestamp: ${session.updatedAt ? new Date(session.updatedAt).toISOString() : new Date().toISOString()}\n`;
					const res = await callFabric({
						op: "propose",
						target: "episodic/squad-sessions.md",
						content: record,
						summary: `Episodic anchor sync for squad ${session.sessionAnchor}`,
						provider: providerOf(ctx),
					});
					ctx.ui.notify(res.ok ? `Agency squad session proposed: ${res.proposal_id ?? "ok"}` : `Agency sync failed: ${res.error ?? res.blocked}`, res.ok ? "info" : "error");
				} catch (err: any) {
					ctx.ui.notify(`Agency sync error: ${err.message}`, "error");
				}
				return;
			}
			const modeMatch = /^mode(?:\s+(local|remote|status))?$/i.exec(action);
			if (modeMatch) {
				const targetMode = modeMatch[1]?.toLowerCase();
				if (targetMode === "remote") {
					ACTIVE_MEMORY_MODE = "remote";
					ctx.ui.notify("Memory Fabric mode switched to REMOTE: Autonomous read/write enabled for any active model.", "info");
				} else if (targetMode === "local") {
					ACTIVE_MEMORY_MODE = "local";
					ctx.ui.notify("Memory Fabric mode switched to LOCAL: Strict fail-closed loopback isolation enabled.", "info");
				} else {
					ctx.ui.notify(`Memory Fabric mode is currently: ${ACTIVE_MEMORY_MODE.toUpperCase()} (remoteMode=${isRemoteMode()})`, "info");
				}
				return;
			}
			const projectMatch = /^project\s+(create|update|consolidate|archive|status)\s+([a-zA-Z0-9._-]+)(?:\s+(.+))?$/i.exec(action);
			if (projectMatch) {
				const [, pAct, pName, pContent] = projectMatch;
				const relpath = `projects/${pName.toLowerCase().replace(/\.md$/, "")}.md`;
				const today = new Date().toISOString().split("T")[0];
				if (pAct === "status") {
					const res = await callFabric({ op: "read", source_ref: relpath, provider: providerOf(ctx) });
					ctx.ui.notify(res.ok ? `Project ${pName}: read successfully (${typeof res.content === "string" ? res.content.length : 0} chars).` : `Project ${pName} not found: ${res.error ?? res.blocked}`, res.ok ? "info" : "warning");
					return;
				}
				const doc = pAct === "create"
					? `---\ntype: project\nstatus: active\ncreated: "${today}"\nupdated: "${today}"\ntags: [project, ${pName}]\n---\n\n# ${pName}\n\n## Overview\n${pContent || "Initial project memory card."}\n\n## Architecture & Invariants\n\n## Status & Next Milestones\n`
					: pContent || "Updated section content.";
				const res = await callFabric({
					op: "propose",
					target: relpath,
					content: doc,
					summary: `${pAct} project memory card for ${pName}`,
					provider: providerOf(ctx),
				});
				ctx.ui.notify(res.ok ? `Project ${pName} proposal generated: ${res.proposal_id ?? "ok"}` : `Project action failed: ${res.error ?? res.blocked}`, res.ok ? "info" : "error");
				return;
			}
			if (action === "history") {
				const res = await callFabric({ op: "proposal-history" });
				const hist = Array.isArray(res.history) ? res.history : [];
				ctx.ui.notify(`Proposal History: ${hist.length} applied proposals. ${hist.slice(0, 3).map((h: any) => `${h.proposal_id}: ${h.target}`).join("; ")}`, "info");
				return;
			}
			const revertMatch = /^revert\s+([0-9a-f]{12})$/i.exec(action);
			if (revertMatch) {
				const res = await callFabric({ op: "proposal-revert", proposal_id: revertMatch[1], provider: providerOf(ctx) });
				ctx.ui.notify(res.ok ? `Proposal ${revertMatch[1]} reverted successfully.` : `Revert failed: ${res.error ?? res.blocked}`, res.ok ? "info" : "error");
				return;
			}
			const sectionsMatch = /^sections\s+(.+)$/i.exec(action);
			if (sectionsMatch) {
				const res = await callFabric({ op: "sections", source_ref: sectionsMatch[1].trim(), provider: providerOf(ctx), loopback: isLoopbackModel(ctx.model) });
				const secCount = Array.isArray(res.sections) ? res.sections.length : 0;
				ctx.ui.notify(res.ok ? `Note ${sectionsMatch[1]}: ${secCount} sections parsed.` : `Sections query failed: ${res.error ?? res.blocked}`, res.ok ? "info" : "error");
				return;
			}
			const linksMatch = /^links\s+(.+)$/i.exec(action);
			if (linksMatch) {
				const res = await callFabric({ op: "links", source_ref: linksMatch[1].trim() });
				const outCount = Array.isArray(res.outlinks) ? res.outlinks.length : 0;
				const backCount = Array.isArray(res.backlinks) ? res.backlinks.length : 0;
				ctx.ui.notify(res.ok ? `Note ${linksMatch[1]}: ${outCount} outlinks, ${backCount} backlinks.` : `Links query failed: ${res.error ?? "error"}`, res.ok ? "info" : "error");
				return;
			}
			const resolveMatch = /^resolve\s+([0-9a-f]{4,16})$/i.exec(action);
			const op = action === "proposals" ? "proposal-list" : action;
			const result = resolveMatch
				? await callFabric({ op: "resolve", alarm_id: resolveMatch[1] })
				: await callFabric({ op });
			const alarms = Array.isArray(result.alarms) ? result.alarms : [];
			const summary = action === "index"
				? `Indexed local source set: ${result.documents ?? 0} documents; added ${result.added ?? 0}, updated ${result.updated ?? 0}, removed from index ${result.removed_from_index ?? 0}. Vault files were not modified.`
				: action === "proposals"
					? `Review-only proposals: ${Array.isArray(result.proposals) ? result.proposals.length : 0}. Inspect proposal files locally; none has been applied.`
					: action === "alarms"
						? alarms.length === 0
							? "No credential alarms. No key is known to be sitting in an indexed note."
							: `${alarms.length} credential alarm(s): ${alarms.map((a: { relpath: string; secret_kind: string; alarm_id: string }) => `${a.relpath} (${a.secret_kind}, id ${a.alarm_id})`).join("; ")}. Values are never shown. Rotate the key, then: /memory-fabric resolve ${alarms[0].alarm_id}`
						: resolveMatch
							? `Alarm ${result.alarm_id ?? resolveMatch[1]} marked resolved (${result.already_resolved ? "already was" : "now"}); ${result.unresolved_remaining ?? 0} still open.`
							: `Local memory index: ${result.indexed ? `${result.documents} documents` : "not initialized"}.`
								+ (result.secret_alarms ? ` CREDENTIAL ALARM: ${result.secret_alarms} note(s) contain a key - run /memory-fabric alarms.` : "");
			ctx.ui.notify(result.ok ? summary : "Local memory operation failed; see local diagnostics.", result.ok ? "info" : "error");
		},
	});
}
