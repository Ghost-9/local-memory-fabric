/**
 * The context scrub, pinned. Run with:
 *
 *   node --experimental-strip-types --test scrub.test.ts
 *
 * These tests exist because the scrub had none. It was inline in `index.ts`, unreachable
 * from any suite, and shipped two defects green: a notification that sat after an
 * unconditional `return` (dead on arrival), and a tracking set that evicted its oldest entry
 * at the cap, which turned withheld vault content back into transmittable content. Both are
 * regression-guarded below; the eviction case is the reason the cap now fails closed.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
	createScrubState,
	DEFAULT_SCRUB_NOTICE_INTERVAL_MS,
	MAX_TRACKED_IDS,
	parseNoticeInterval,
	scrubMessages,
	shouldNotifyScrub,
	trackVaultToolCalls,
	type ScrubMessage,
} from "./scrub.ts";

const LOCAL = new Set(["lmstudio", "llama.cpp", "ollama"]);
const VAULT = "/Users/batra/system-memory";
const fingerprint = (m: unknown): string => createHash("sha256").update(JSON.stringify(m) ?? "").digest("hex");
const touchesVault = (args: unknown): boolean => JSON.stringify(args ?? "").includes(VAULT);
// Same pattern the extension uses, so this suite exercises the real guard rather than a
// weaker local copy that would pass while production stayed broken.
const touchesAppleNotes = (args: unknown): boolean =>
	/apple[ _-]*notes|notes[ _-]*export|com\.apple\.notes|notestore\.sqlite/i.test(JSON.stringify(args ?? ""));

/** One vault-touching bash call from an ALLOWED (local) provider, plus its result. */
function vaultCall(i: number): ScrubMessage[] {
	return [
		{
			role: "assistant",
			provider: "ollama",
			content: [{ type: "toolCall", id: `c${i}`, name: "bash", arguments: { command: `cat ${VAULT}/note${i}.md` } }],
		},
		{ role: "toolResult", toolName: "bash", toolCallId: `c${i}`, content: `VAULT CONTENT ${i}` },
	];
}

function runScrub(messages: ScrubMessage[]) {
	const state = createScrubState();
	trackVaultToolCalls(messages, state, LOCAL, touchesVault, touchesAppleNotes);
	return { state, kept: scrubMessages(messages, state, LOCAL, new Set(), fingerprint) };
}

const leaked = (kept: ScrubMessage[]): ScrubMessage[] =>
	kept.filter((m) => String(m.content ?? "").startsWith("VAULT CONTENT"));

test("a vault-touching tool result is withheld, and its caller's turn is too", () => {
	const messages: ScrubMessage[] = [
		{ role: "user", content: "read the note" },
		...vaultCall(0),
		{ role: "user", content: "thanks" },
	];
	const { kept } = runScrub(messages);
	assert.equal(leaked(kept).length, 0, "vault content must never survive the scrub");
	assert.equal(kept.filter((m) => m.role === "toolResult").length, 0);
	assert.equal(kept.filter((m) => m.role === "user").length, 2, "unrelated turns are kept");
});

test("the injected vault-context block is always dropped", () => {
	const messages: ScrubMessage[] = [
		{ role: "user", customType: "vault-context-route", content: "routing hint" },
		{ role: "user", content: "real question" },
	];
	const { kept } = runScrub(messages);
	assert.equal(kept.length, 1);
	assert.equal(kept[0].content, "real question");
});

test("an exempt fabric tool result is dropped by tool name, with no tracking needed", () => {
	const messages: ScrubMessage[] = [
		{ role: "assistant", provider: "opencode-go", content: [{ type: "toolCall", id: "x", name: "memory_fabric_search", arguments: {} }] },
		{ role: "toolResult", toolName: "memory_fabric_search", toolCallId: "x", content: "excerpts" },
	];
	const { kept } = runScrub(messages);
	assert.equal(kept.filter((m) => m.role === "toolResult").length, 0);
});

test("REGRESSION: overflow fails CLOSED — an evicted id never re-admits withheld content", () => {
	// The defect: the tracked-id set evicted its oldest entry past the cap. An evicted id made
	// an old vault tool result untrackable, so it looked safe and was sent. Measured on the old
	// code: 4,100 calls leaked 4 results, 4,146 leaked 50, always at the front of the payload.
	const messages: ScrubMessage[] = [];
	for (let i = 0; i < MAX_TRACKED_IDS + 50; i++) messages.push(...vaultCall(i));

	const { state, kept } = runScrub(messages);

	assert.equal(state.overflowed, true, "exceeding the cap must trip the overflow flag");
	assert.equal(leaked(kept).length, 0, "no vault content may reappear once tracking overflowed");
	// Fail-closed costs context, not secrecy: every remaining tool result is withheld.
	assert.equal(kept.filter((m) => m.role === "toolResult").length, 0);
	// And the overflow is sticky — a later, smaller turn must not silently relax back.
	const later: ScrubMessage[] = [{ role: "user", content: "still here" }];
	const keptLater = scrubMessages(later, state, LOCAL, new Set(), fingerprint);
	assert.equal(keptLater.length, 1, "plain user turns are never dropped, even after overflow");
});

test("just under the cap, the scrub stays precise and withholds only memory-derived content", () => {
	const messages: ScrubMessage[] = [];
	for (let i = 0; i < 100; i++) messages.push(...vaultCall(i));
	const { state, kept } = runScrub(messages);
	assert.equal(state.overflowed, false, "100 calls is far below the cap");
	assert.equal(leaked(kept).length, 0);

	// The precision claim: at the same unsaturated state, an unrelated tool result SURVIVES.
	// Without this, "withhold everything" would pass every test above while destroying the
	// transcript — fail-closed must not become fail-useless.
	const withOther = [...messages, { role: "toolResult", toolName: "bash", toolCallId: "unrelated", content: "ls output" }];
	const keptOther = scrubMessages(withOther, state, LOCAL, new Set(), fingerprint);
	assert.equal(leaked(keptOther).length, 0, "vault content still withheld");
	assert.equal(
		keptOther.filter((m) => m.toolCallId === "unrelated").length,
		1,
		"an untracked, non-vault tool result is kept when tracking has not overflowed",
	);
});

test("a loopback assistant turn is dropped via provider id without needing a fingerprint", () => {
	const messages: ScrubMessage[] = [{ role: "assistant", provider: "ollama", content: "local turn" }];
	const { kept } = runScrub(messages);
	assert.equal(kept.length, 0, "a local-provider turn is withheld even with an empty hash set");
});

test("a recorded loopback fingerprint is dropped even when the provider id is not local", () => {
	// pi records the fingerprint of loopback turns at message_end. If a turn is later re-labelled
	// (provider switched mid-flight), the hash is the only thing that still identifies it.
	const message: ScrubMessage = { role: "assistant", provider: "anthropic", content: "loopback turn" };
	const hashes = new Set([fingerprint(message)]);
	const state = createScrubState();
	const kept = scrubMessages([message], state, LOCAL, hashes, fingerprint);
	assert.equal(kept.length, 0, "the fingerprint path must still withhold");
});

test("an unhashable message is kept, because hashing failure is not evidence of harm", () => {
	const exploding = () => {
		throw new Error("unserialisable");
	};
	const message: ScrubMessage = { role: "assistant", provider: "anthropic", content: "fine" };
	const kept = scrubMessages([message], createScrubState(), LOCAL, new Set(), exploding);
	assert.equal(kept.length, 1);
});

test("Apple Notes tool calls are tracked and their results withheld", () => {
	const messages: ScrubMessage[] = [
		{
			role: "assistant",
			provider: "ollama",
			content: [{ type: "toolCall", id: "an", name: "bash", arguments: { command: "cat ~/Library/Group Containers/group.com.apple.notes/notes.sqlite" } }],
		},
		{ role: "toolResult", toolName: "bash", toolCallId: "an", content: "APPLE NOTES CONTENT" },
	];
	const state = createScrubState();
	trackVaultToolCalls(messages, state, LOCAL, touchesVault, touchesAppleNotes);
	const kept = scrubMessages(messages, state, LOCAL, new Set(), fingerprint);
	assert.equal(kept.filter((m) => m.role === "toolResult").length, 0, "Apple Notes content is never sent");
});

test("nothing is removed when the transcript holds no memory-derived content", () => {
	const messages: ScrubMessage[] = [
		{ role: "user", content: "hello" },
		{ role: "assistant", provider: "anthropic", content: "hi" },
	];
	const state = createScrubState();
	trackVaultToolCalls(messages, state, LOCAL, touchesVault, touchesAppleNotes);
	const kept = scrubMessages(messages, state, LOCAL, new Set(), fingerprint);
	assert.equal(kept.length, messages.length, "an unrelated transcript passes through untouched");
});
// --- Scrub notice cadence (issue #10) -------------------------------------------------
// The decision the issue asked for: NOT once per process (too little — reopen a scrubbed
// session and you saw it once, then silence, which is the symptom it exists to remove) and
// NOT per request (noise people learn to dismiss). First scrub, then rate-limited per session.

test("the first scrub notice always fires", () => {
	assert.equal(shouldNotifyScrub({}, 1_000, DEFAULT_SCRUB_NOTICE_INTERVAL_MS), true);
});

test("a repeat inside the interval is suppressed, and the interval is honoured", () => {
	const state: { lastNotifiedAt?: number } = { lastNotifiedAt: 1_000 };
	const interval = DEFAULT_SCRUB_NOTICE_INTERVAL_MS;
	assert.equal(shouldNotifyScrub(state, 1_001, interval), false, "immediately after: quiet");
	assert.equal(shouldNotifyScrub(state, 1_000 + interval - 1, interval), false, "one ms early: quiet");
	assert.equal(shouldNotifyScrub(state, 1_000 + interval, interval), true, "exactly at the interval: due");
	assert.equal(shouldNotifyScrub(state, 1_000 + interval * 3, interval), true, "well past it: due");
});

test("a long scrubbed session is reminded periodically, not every turn", () => {
	const state: { lastNotifiedAt?: number } = {};
	const interval = DEFAULT_SCRUB_NOTICE_INTERVAL_MS;
	let fired = 0;
	// 200 requests, one every 30s — a long session being scrubbed continuously.
	for (let i = 0; i < 200; i++) {
		const now = i * 30_000;
		if (shouldNotifyScrub(state, now, interval)) {
			state.lastNotifiedAt = now;
			fired++;
		}
	}
	assert.ok(fired > 1, `expected periodic reminders, got ${fired}`);
	assert.ok(fired < 20, `must not be per-request; got ${fired} notices in 200 requests`);
});

test("interval 0 silences the reminder but keeps the first notice; negative repeats always", () => {
	const state: { lastNotifiedAt?: number } = {};
	assert.equal(shouldNotifyScrub(state, 1_000, 0), true, "first notice still fires");
	state.lastNotifiedAt = 1_000;
	assert.equal(shouldNotifyScrub(state, 1_001, 0), false);
	assert.equal(shouldNotifyScrub(state, 99_999_999, 0), false);
	assert.equal(shouldNotifyScrub(state, 1_001, -1), true, "negative = every request");
});

test("the interval parser accepts explicit integers and rejects junk", () => {
	assert.equal(parseNoticeInterval(undefined), DEFAULT_SCRUB_NOTICE_INTERVAL_MS);
	assert.equal(parseNoticeInterval(" 60000 "), 60_000);
	assert.equal(parseNoticeInterval("0"), 0);
	assert.equal(parseNoticeInterval("-1"), -1);
	for (const junk of ["", "  ", "abc", "10s", "1.5", "1e3", "NaN"]) {
		assert.equal(parseNoticeInterval(junk), DEFAULT_SCRUB_NOTICE_INTERVAL_MS, `${junk} must fall back`);
	}
});
