import { spawn } from "node:child_process";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";
import { defineTool, type AgentToolResult, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

type Json = Record<string, unknown>;
type Pending = { line: string; resolve: (value: Json) => void; timer?: ReturnType<typeof setTimeout> };
const BIN = process.env.MEMORY_FABRIC_BIN || fileURLToPath(new URL("./rust/target/release/memory-fabric", import.meta.url));
const MAX_REQUEST_BYTES = 128 * 1024;
const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;
const MAX_PENDING = 128;
let worker: ReturnType<typeof spawn> | undefined;
let queue: Pending[] = [];
let active: Pending | undefined;

function failWorker(child: ReturnType<typeof spawn>, reason: string): void {
	if (worker !== child) return;
	worker = undefined;
	const requests = [active, ...queue].filter((x): x is Pending => Boolean(x));
	active = undefined;
	queue = [];
	for (const request of requests) {
		if (request.timer) clearTimeout(request.timer);
		request.resolve({ ok: false, error: reason });
	}
}

function runNext(): void {
	if (active || queue.length === 0) return;
	const request = queue.shift()!;
	let child = worker;
	if (!child) {
		child = spawn(BIN, ["--serve"], { stdio: ["pipe", "pipe", "pipe"], windowsHide: true, env: process.env });
		worker = child;
		let stderr = "";
		child.stderr?.setEncoding("utf8");
		child.stderr?.on("data", (chunk: string) => { stderr = (stderr + chunk).slice(-8192); });
		child.on("error", () => failWorker(child!, stderr.trim().split("\n").pop() || "Rust memory engine could not start."));
		child.on("close", () => failWorker(child!, stderr.trim().split("\n").pop() || "Rust memory engine stopped."));
		const lines = createInterface({ input: child.stdout! });
		lines.on("line", (line) => {
			const item = active;
			if (!item) return;
			active = undefined;
			if (item.timer) clearTimeout(item.timer);
			if (Buffer.byteLength(line, "utf8") > MAX_RESPONSE_BYTES) item.resolve({ ok: false, error: "Rust memory response exceeded its size limit." });
			else {
				try { item.resolve(JSON.parse(line) as Json); }
				catch { item.resolve({ ok: false, error: "Rust memory engine returned invalid JSON." }); }
			}
			runNext();
		});
	}
	active = request;
	const timeoutMs = request.line.includes('"op":"index"') ? 120_000 : 15_000;
	request.timer = setTimeout(() => {
		child!.kill();
		failWorker(child!, "Local memory operation timed out.");
	}, timeoutMs);
	child.stdin!.write(`${request.line}\n`, (error) => {
		if (error) failWorker(child!, "Rust memory request could not be sent.");
	});
}

/** Serialize an extension observation to the Rust policy/engine process. */
function callEngine(request: Json, ctx?: any): Promise<Json> {
	return new Promise((resolve) => {
		const model = ctx?.model ? { provider: ctx.model.provider, base_url: ctx.model.baseUrl } : undefined;
		const body: Json = {
			...request,
			...(model ? { model, provider: model.provider, base_url: model.base_url } : {}),
			...(ctx?.cwd ? { cwd: ctx.cwd } : {}),
			...(ctx?.sessionId || ctx?.session?.id ? { session_id: String(ctx.sessionId ?? ctx.session.id) } : {}),
		};
		let line: string;
		try { line = JSON.stringify(body); }
		catch { resolve({ ok: false, error: "Local memory request could not be serialized." }); return; }
		if (Buffer.byteLength(line, "utf8") > MAX_REQUEST_BYTES) { resolve({ ok: false, error: "Local memory request exceeded its size limit." }); return; }
		if (queue.length + Number(Boolean(active)) >= MAX_PENDING) { resolve({ ok: false, error: "Local memory request queue is full." }); return; }
		queue.push({ line, resolve });
		runNext();
	});
}

function render(value: Json): AgentToolResult<Record<string, unknown>> {
	return { content: [{ type: "text", text: JSON.stringify(value, null, 2) }], details: value };
}

const object = (fields: Record<string, unknown> = {}) => Type.Object(fields);
const optionalString = (description: string) => Type.Optional(Type.String({ description }));
const toolSpecs = [
	{ name: "memory_fabric_context", op: "context", label: "Local Memory Context", description: "Retrieve a bounded, cited context packet from the local memory index.", parameters: object({ query: Type.String(), project: optionalString("Project scope"), as_of: optionalString("Historical date"), token_budget: Type.Optional(Type.Integer({ minimum: 100, maximum: 6000 })) }) },
	{ name: "memory_fabric_search", op: "search", label: "Search Local Memory", description: "Search approved Markdown memory and return cited candidates.", parameters: object({ query: Type.String(), scope: Type.Optional(Type.String()), limit: Type.Optional(Type.Integer({ minimum: 1, maximum: 20 })), as_of: optionalString("Historical date") }) },
	{ name: "memory_fabric_read", op: "read", label: "Read Local Memory", description: "Read a cited Markdown note or heading.", parameters: object({ source_ref: Type.String(), heading: optionalString("Optional exact heading") }) },
	{ name: "memory_fabric_propose", op: "propose", label: "Propose a Memory Change", description: "Create a review-only candidate under an existing section of a project note.", parameters: object({ project: Type.String(), target: Type.String(), section: Type.String(), text: Type.String(), source_refs: Type.Optional(Type.Array(Type.String())) }) },
	{ name: "memory_fabric_sandbox", op: "sandbox", label: "Store a Large Result", description: "Store a safe payload outside canonical memory for deliberate later retrieval.", parameters: object({ title: optionalString("Short title"), text: Type.String(), scope: optionalString("Scope label") }) },
	{ name: "memory_fabric_apply", op: "apply", label: "Apply a Memory Proposal", description: "Apply a reviewed proposal when the Rust write policy permits it.", parameters: object({ proposal_id: Type.String() }) },
	{ name: "memory_fabric_proposal_show", op: "proposal-show", label: "Show a Proposal", description: "Inspect a pending proposal.", parameters: object({ proposal_id: Type.String() }) },
	{ name: "memory_fabric_alarms", op: "alarms", label: "Credential Alarms", description: "List credential exposure alarms without returning secret values.", parameters: object() },
	{ name: "memory_fabric_audit", op: "audit", label: "Audit Memory", description: "Report stale notes, duplicates, contradictions, and broken links.", parameters: object() },
	{ name: "memory_fabric_links", op: "links", label: "Memory Links", description: "List links to and from a cited note.", parameters: object({ source_ref: Type.String() }) },
	{ name: "memory_fabric_sections", op: "sections", label: "Memory Sections", description: "List headings and anchors for a cited note.", parameters: object({ source_ref: Type.String() }) },
	{ name: "memory_fabric_sync_agency", op: "sync-agency", label: "Sync Agency Session", description: "Create a reviewable proposal from the active Agency session record.", parameters: object() },
	{ name: "memory_fabric_project", op: "project", label: "Project Memory", description: "Create, update, consolidate, archive, or inspect a project memory card.", parameters: object({ action: Type.String(), project: Type.String(), content: optionalString("Content for the requested action"), section: optionalString("Section name") }) },
	{ name: "memory_fabric_handoff", op: "create-handoff", label: "Hand Off This Session", description: "Record a project-scoped handoff for the next session.", parameters: object({ done: optionalString("What is finished"), in_flight: optionalString("What is unfinished"), blocked: optionalString("What is blocked"), next: optionalString("Next action"), pointers: Type.Optional(Type.Array(Type.String())) }) },
	{ name: "memory_fabric_consolidate", op: "consolidate", label: "Consolidate a Long Note", description: "Preview or write an extractive digest beside a long note.", parameters: object({ ref: optionalString("Note to digest"), write: Type.Optional(Type.Boolean()) }) },
	{ name: "memory_fabric_health", op: "engine-health", label: "Memory Engine Health", description: "Read the Rust engine health state for this session.", parameters: object() },
	{ name: "memory_fabric_resolve", op: "resolve", label: "Resolve Credential Alarm", description: "Mark an alarm resolved after the key has been rotated or removed.", parameters: object({ alarm_id: Type.String() }) },
];

export default function (pi: ExtensionAPI) {
	if (process.env.PI_MEMORY_FABRIC === "0") return;
	const namespace = { name: "local-memory-fabric", description: "Local Markdown memory tools. Rust owns provider, privacy, scope, injection, and write policy." };
	for (const spec of toolSpecs) {
		pi.registerTool(defineTool({
			...spec,
			exposure: "direct",
			namespace,
			annotations: { readOnlyHint: !["propose", "sandbox", "apply", "create-handoff", "resolve", "sync-agency", "project"].includes(spec.op), destructiveHint: false, idempotentHint: true, openWorldHint: false },
			async execute(_id: string, params: Json, _signal: AbortSignal, _update: unknown, ctx: any) {
				return render(await callEngine({ op: spec.op, ...params }, ctx));
			},
		}));
	}

	pi.on("context", async (event: any, ctx: any) => {
		const result = await callEngine({ op: "scrub-context", messages: event.messages, now_ms: Date.now() }, ctx);
		if (result.ok === false) return { messages: [] };
		if (result.removed_anything !== true) return;
		if (result.notice === true) ctx.ui?.notify?.("Local memory context was withheld by the Rust privacy policy for this provider.", "warning");
		return { messages: result.messages };
	});

	pi.on("before_agent_start", async (event: any, ctx: any) => {
		const result = await callEngine({ op: "before-agent-start", message_type: event.messageType, prompt: event.prompt ?? "", messages: event.messages ?? [] }, ctx);
		return result.message ? { message: result.message } : undefined;
	});

	pi.on("message_end", async (event: any, ctx: any) => {
		const message = event.message ?? {};
		const result = await callEngine({ op: "message-end", role: message.role, message, usage: message.usage ?? {} }, ctx);
		if (typeof result.notice === "string") ctx.ui?.notify?.(result.notice, "info");
	});

	pi.on("tool_call", async (event: any, ctx: any) => {
		const result = await callEngine({ op: "tool-guard", tool_name: event.toolName, input: event.input ?? {} }, ctx);
		if (result.block === true) return { block: true, reason: String(result.reason ?? "Rust memory policy refused this operation.") };
		if (result.ok === false) return { block: true, reason: "Rust memory policy is unavailable; the tool call was blocked." };
	});

	pi.registerCommand("memory-fabric", {
		description: "Inspect or index the local memory engine.",
		handler: async (args: string, ctx: any) => {
			const result = await callEngine({ op: "extension-command", command: args.trim() || "status" }, ctx);
			ctx.ui?.notify?.(JSON.stringify(result), result.ok === false ? "error" : "info");
		},
	});
}
