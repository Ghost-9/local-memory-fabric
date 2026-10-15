import { test } from "node:test";
import assert from "node:assert/strict";
import {
	createCostState, promptTokens, recordTurn, costNotice,
	resumePointer, isResumable, HANDOFF_THRESHOLD_TOKENS,
} from "./session-cost.ts";

test("prompt tokens count what was billed plus what came from cache", () => {
	assert.equal(promptTokens({ input: 100, cacheRead: 900, cacheWrite: 50 }), 1050);
});

test("junk usage yields zero rather than NaN", () => {
	// NaN would poison the comparison and the threshold would silently never fire.
	for (const bad of [null, undefined, "x", 5, {}, { input: "a" }, { input: NaN }, { cacheRead: Infinity }]) {
		const n = promptTokens(bad);
		assert.ok(Number.isFinite(n), `promptTokens(${JSON.stringify(bad)}) was not finite`);
	}
});

test("a small session is never advised", () => {
	const s = createCostState();
	for (let i = 0; i < 50; i += 1) {
		const a = recordTurn(s, { input: 1000, cacheRead: 9000 });
		assert.equal(a.advise, false);
	}
	assert.equal(s.peakTokens, 10000);
	assert.equal(s.crossedAtTurn, 0);
});

test("crossing the threshold advises once, on the turn it happens", () => {
	const s = createCostState();
	recordTurn(s, { input: 1000, cacheRead: 100_000 });
	const a = recordTurn(s, { input: 1000, cacheRead: 160_000 });
	assert.equal(a.advise, true);
	assert.equal(a.reason, "crossed");
	assert.equal(s.crossedAtTurn, 2);
	// and not again immediately
	for (let i = 0; i < 10; i += 1) assert.equal(recordTurn(s, { cacheRead: 160_000 }).advise, false);
});

test("the reminder returns on the interval, and not before", () => {
	const s = createCostState();
	recordTurn(s, { cacheRead: 200_000 });           // crossed on turn 1
	let fired = 0;
	for (let i = 2; i <= 30; i += 1) if (recordTurn(s, { cacheRead: 200_000 }).advise) fired += 1;
	assert.equal(fired, 1, "expected exactly one further reminder");
	assert.equal(s.turns, 30);
});

test("peak is tracked even on turns that do not advise", () => {
	const s = createCostState();
	recordTurn(s, { cacheRead: 5000 });
	recordTurn(s, { cacheRead: 4_000_000 });
	recordTurn(s, { cacheRead: 10 });
	assert.equal(s.peakTokens, 4_000_000);
});

test("the notice states the number and the action", () => {
	const n = costNotice(200_000);
	assert.match(n, /200K tokens/);
	assert.match(n, /memory_fabric_handoff/);
});

test("the resume pointer is a pointer, not a summary", () => {
	const p = resumePointer({ at: new Date(Date.now() - 3600_000).toISOString(),
		done: "settled the retry policy", in_flight: "the migration test", next: "run the gate" },
		Date.now());
	assert.ok(p);
	assert.ok(p!.length < 300, `pointer was ${p!.length} chars`);
	assert.match(p!, /1h ago/);
	assert.match(p!, /inbox\/RESUME\.md/);
});

test("an empty handoff produces no pointer rather than an empty line", () => {
	assert.equal(resumePointer({ at: new Date().toISOString() }, Date.now()), null);
	assert.equal(resumePointer({ at: new Date().toISOString(), done: "   " }, Date.now()), null);
});

test("a stale handoff is not offered as a resume pointer", () => {
	const now = Date.now();
	assert.equal(isResumable({ at: new Date(now - 3600_000).toISOString() }, now), true);
	assert.equal(isResumable({ at: new Date(now - 8 * 86400_000).toISOString() }, now), false);
	assert.equal(isResumable({ at: "not a date" }, now), false);
	assert.equal(isResumable({}, now), false);
});

test("the pointer names the transcript when one was recorded, and stays quiet when not", () => {
	const now = Date.now();
	const withT = resumePointer({ at: new Date(now).toISOString(), next: "run the gate",
		transcript: "/Users/x/.pi/agent/sessions/--a--/t_s.jsonl" }, now);
	assert.match(withT!, /transcript: \/Users\/x/);
	const without = resumePointer({ at: new Date(now).toISOString(), next: "run the gate" }, now);
	assert.doesNotMatch(without!, /transcript:/);
	// A pointer to a file that is not there wastes a turn discovering that.
	assert.ok(withT!.length < 400, `pointer grew to ${withT!.length} chars`);
});
