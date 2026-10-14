import { test } from "node:test";
import assert from "node:assert/strict";
import { createInjectionAccount } from "./injection.ts";

test("the first injection sets the baseline and is not a break", () => {
	const a = createInjectionAccount();
	const r = a.record(412, 3);
	assert.equal(r.prefixBreak, false);
	assert.equal(r.turn, 1);
	assert.equal(a.state().baselineChars, 412);
});

test("a stable block is never a break, however many turns", () => {
	const a = createInjectionAccount();
	for (let i = 0; i < 20; i += 1) assert.equal(a.record(412, 3).prefixBreak, false);
	assert.equal(a.state().prefixBreaks, 0);
	assert.equal(a.state().turns, 20);
});

test("a changed block IS detected - the case the detector exists for", () => {
	const a = createInjectionAccount();
	a.record(412, 3);
	const r = a.record(430, 3);          // two refs got longer; the prefix after them is void
	assert.equal(r.prefixBreak, true);
	assert.equal(a.state().prefixBreaks, 1);
});

test("a break that returns to the baseline is still counted once per deviation", () => {
	const a = createInjectionAccount();
	a.record(412, 3);
	assert.equal(a.record(412, 3).prefixBreak, false);
	assert.equal(a.record(999, 5).prefixBreak, true);
	assert.equal(a.record(412, 3).prefixBreak, false);
	assert.equal(a.record(413, 3).prefixBreak, true);
	assert.equal(a.state().prefixBreaks, 2);
});

test("session characters accumulate, so the per-session cost is measured not assumed", () => {
	const a = createInjectionAccount();
	a.record(400, 3);
	a.record(400, 3);
	const r = a.record(400, 3);
	assert.equal(r.sessionChars, 1200);
	assert.equal(a.state().sessionChars, 1200);
});

test("no injections reads as zero rather than a fabricated baseline", () => {
	const a = createInjectionAccount();
	assert.deepEqual(a.state(), { turns: 0, sessionChars: 0, baselineChars: null, prefixBreaks: 0 });
});
