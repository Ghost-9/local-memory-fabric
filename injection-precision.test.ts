import { test } from "node:test";
import assert from "node:assert/strict";
import { decideInjection, contentWords } from "./injection-precision.ts";

test("THE OBSERVED FAILURE: a generic prompt injects nothing", () => {
	// Real prompt from the transcripts. It has no topic; the notes it returned were vocabulary noise.
	const d = decideInjection("So do one thing check all the milestone and its correspondence implementation", {
		results: [{ source_ref: "pi-harness/47-pi-tern-extension-mvp.md", matched_terms: ["check"] }],
	});
	assert.equal(d.inject, false);
	assert.match(d.reason, /content word|coverage/);
});

test("THE OBSERVED FAILURE: a mostly-function-word prompt does not inject", () => {
	const d = decideInjection("fix all the issues that you found and create their GitHub issues", {
		results: [{ source_ref: "daily/2026-09-29.md", matched_terms: ["fix", "issues"] }],
	});
	assert.equal(d.inject, false);
});

test("a low-discrimination search never injects, whatever it returned", () => {
	const d = decideInjection("local memory extension indexing behaviour", {
		results: [{ source_ref: "projects/x.md", matched_terms: ["local", "memory", "extension"] }],
		low_discrimination: true,
	});
	assert.equal(d.inject, false);
	assert.match(d.reason, /low discrimination/);
});

test("one shared word out of many is a collision, not a match", () => {
	const d = decideInjection("temporal supersession provenance determinism abstention calibration", {
		results: [{ source_ref: "projects/x.md", matched_terms: ["provenance"] }],
	});
	assert.equal(d.inject, false);
	assert.match(d.reason, /need 2/);
});

test("a genuinely on-topic prompt DOES inject - the gate is not simply 'never'", () => {
	// This is the control. A gate that blocks everything would pass every test above and be useless.
	const d = decideInjection("why does the section ranking produce vocabulary neighbours", {
		results: [{ source_ref: "projects/local-memory-fabric-extension.md",
			matched_terms: ["section", "ranking", "vocabulary", "neighbours"] }],
	});
	assert.equal(d.inject, true);
	assert.ok(d.coverage >= 0.5, `coverage ${d.coverage}`);
});

test("coverage is computed against the prompt, not the result", () => {
	const d = decideInjection("ranking precision recall", {
		results: [{ source_ref: "a.md", matched_terms: ["ranking", "precision", "recall", "extra"] }],
	});
	assert.equal(d.matched, 3);
	assert.equal(d.content, 3);
	assert.equal(d.inject, true);
});

test("content words exclude function words and short tokens", () => {
	const w = contentWords("fix all the issues and check that the local memory extension works");
	assert.ok(!w.includes("fix") && !w.includes("check") && !w.includes("all") && !w.includes("the"));
	assert.ok(w.includes("local") && w.includes("memory") && w.includes("extension"));
});

test("empty or missing results never inject", () => {
	for (const r of [{}, { results: [] }, { results: undefined }]) {
		assert.equal(decideInjection("section ranking behaviour", r as any).inject, false);
	}
});
