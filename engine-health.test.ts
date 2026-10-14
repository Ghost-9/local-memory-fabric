/**
 * Decision table for the engine breaker and respawn guard. Run with:
 *
 *   node --experimental-strip-types --test engine-health.test.ts
 *
 * The clock is injected, so these assert behaviour rather than wall time.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import { createHealth } from "./engine-health.ts";

test("the breaker stays closed while the engine is healthy", () => {
	const h = createHealth();
	for (let i = 0; i < 50; i++) h.record(true, i);
	assert.equal(h.allow(100), true);
	assert.equal(h.state.refusals, 0);
});

test("three consecutive failures open it, and a refusal is counted", () => {
	const h = createHealth(); // threshold 3, window 60s
	h.record(false, 0);
	assert.equal(h.allow(0), true, "one failure is not enough to refuse");
	h.record(false, 1);
	assert.equal(h.allow(1), true, "two failures is not enough either");
	h.record(false, 2);
	assert.equal(h.allow(2), false, "third failure trips it");
	assert.equal(h.allow(3), false, "and it stays refused");
	assert.equal(h.state.refusals, 2, "refusals are counted, not silent");
});

test("one success resets an unopened breaker, because flaky is not broken", () => {
	const h = createHealth();
	h.record(false, 0);
	h.record(false, 1);
	h.record(true, 2);
	h.record(false, 3);
	h.record(false, 4);
	assert.equal(h.allow(5), true, "two failures after a success is still under the threshold");
});

test("the window elapses into a probe, and that probe gets the normal allowance", () => {
	const h = createHealth();
	for (let i = 0; i < 3; i++) h.record(false, i);
	assert.equal(h.allow(10), false, "refused while the breaker is open");
	// The breaker opened at the third failure (t=2), so the window ends at 60_002.
	assert.equal(h.allow(60_001), false, "one millisecond early is still refused");
	assert.equal(h.allow(60_003), true, "after the window, one call is allowed through as a probe");

	// A failed probe does NOT immediately re-trip: the window reset the counter, so the probe
	// and the calls after it get the ordinary three-failure allowance. Asserted deliberately,
	// not tolerated - the alternative (reopen on one failure) means a single transient blip
	// right after the window keeps the breaker shut forever.
	const before = h.state.refusals;
	h.record(false, 60_004);
	assert.equal(h.allow(60_004), true, "one failure after a reset is not a trip");
	h.record(false, 60_005);
	h.record(false, 60_006);
	assert.equal(h.allow(60_006), false, "three fresh failures re-trip it");
	assert.equal(h.state.refusals, before + 1);
});

test("a probe that succeeds clears the breaker entirely", () => {
	const h = createHealth();
	for (let i = 0; i < 3; i++) h.record(false, i);
	assert.equal(h.allow(120_000), true);
	h.record(true, 120_001);
	for (let i = 0; i < 20; i++) h.record(true, 120_002 + i);
	assert.equal(h.allow(200_000), true);
	assert.equal(h.state.consecutiveFailures, 0);
});

test("respawns are bounded per window, and the refusal is counted", () => {
	const h = createHealth(); // limit 3 per 60s
	assert.equal(h.allowRespawn(0), true);
	assert.equal(h.allowRespawn(1_000), true);
	assert.equal(h.allowRespawn(2_000), true);
	assert.equal(h.allowRespawn(3_000), false, "the fourth respawn inside the window is a storm");
	assert.equal(h.allowRespawn(4_000), false);
	assert.equal(h.state.respawnRefusals, 2);
});

test("the respawn budget refills once the window passes", () => {
	const h = createHealth();
	for (let i = 0; i < 3; i++) h.allowRespawn(i * 100);
	assert.equal(h.allowRespawn(50_000), false);
	assert.equal(h.allowRespawn(61_000), true,
		"a full window later, respawning is allowed again - a slow crash loop is a different fault");
});

test("the refusal message names the failure instead of reading as flakiness", () => {
	const h = createHealth();
	for (let i = 0; i < 3; i++) h.record(false, i);
	const msg = h.refusalMessage(10_000);
	assert.match(msg, /failed 3 times in a row/);
	assert.match(msg, /retrying in 51s/);
	assert.match(msg, /memory-fabric status/, "it must say what to do next");
});
