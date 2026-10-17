import { test } from "node:test";
import assert from "node:assert/strict";
import { decideAbstention, withAbstention, isFilenameShaped } from "./abstention.ts";
import { decideInjection, contentWords } from "./injection-precision.ts";

// Evidence captured live on 2026-10-16 against this vault (matched_terms copied verbatim from
// the engine response; no excerpt text is stored here).
const LIVE = {
	"agent_prompt_stalled": {
		results: [{ matched_terms: ["stalled", "prompt", "agent"] }],
		low_discrimination: false,
		abstention: null,
	},
	"t3.json": {
		results: [{ matched_terms: ["t3", "json"] }],
		low_discrimination: false,
		abstention: null,
	},
};

test("THE OBSERVED FAILURE: a vocabulary-collision query says it is not an answer", () => {
	// Before this module the engine answered this with five notes and rank_score 0.978 on the top
	// one. rank_score is normalised within the page, so it is a position, not a confidence.
	const d = decideAbstention("agent_prompt_stalled", LIVE["agent_prompt_stalled"]);
	assert.equal(d.abstained, true);
	assert.match(d.reason, /discriminative/);
});

test("a query with no content word at all abstains on rule 3", () => {
	const d = decideAbstention("fix all the issues that you", {
		results: [{ matched_terms: ["fix", "issues"] }],
	});
	assert.equal(d.abstained, true);
	assert.match(d.reason, /no discriminative words/);
});

test("CLOSED 2026-10-17: the 'found' query now abstains", () => {
	// This test previously asserted the OPPOSITE and was named OPEN. The module header cites
	// "fix all the issues that you found" as the observed failure shape, and `found` was the
	// one word in it that survived contentWords(), so the query still answered. `found` is now
	// in GENERIC, rule 3 fires, and it abstains.
	//
	// "found" is the ONLY word added. The 136-case gold fixture was run before and after:
	// recall@1/@3/@5/@10, MRR@3, answered and nohit_precision are all bit-identical
	// (0.4754 / 0.918 / 0.9262 / 0.9344 / 0.7485 / 0.9754 / 0.9286), so the injection gate and
	// retrieval ranking did not move.
	assert.equal(contentWords("fix all the issues that you found").length, 0);
	const d = decideAbstention("fix all the issues that you found", {
		results: [{ matched_terms: ["fix", "issues", "found"] }],
	});
	assert.equal(d.abstained, true);
	assert.match(d.reason, /no discriminative words/);
});

test("CONTROL: a genuinely discriminative query still answers", () => {
	// A gate that abstains on everything would pass every test above and be worse than none.
	for (const q of ["why does the section ranking produce vocabulary neighbours", "injection precision gate calibration"]) {
		assert.equal(decideAbstention(q, { results: [{ matched_terms: q.toLowerCase().split(" ") }] }).abstained, false, q);
	}
});

test("the engine's own abstention reason survives verbatim", () => {
	// Three states that used to collapse into one empty result: nothing indexed, nothing matched,
	// and matched-but-not-discriminatively. Only the engine can tell the first two apart, so its
	// reason must never be overwritten.
	const nothingIndexed = decideAbstention("anything", { abstention: "no searchable terms" });
	const nothingMatched = decideAbstention("anything", { abstention: "no indexed source contains these terms" });
	assert.equal(nothingIndexed.abstained, true);
	assert.equal(nothingMatched.abstained, true);
	assert.notEqual(nothingIndexed.reason, nothingMatched.reason);
});

test("the engine's low-discrimination flag is honoured, not re-derived", () => {
	const d = decideAbstention("local memory extension indexing behaviour", {
		results: [{ matched_terms: ["local", "memory", "extension"] }],
		low_discrimination: true,
	});
	assert.equal(d.abstained, true);
	assert.match(d.reason, /non-discriminating/);
});

test("a query with no discriminative words abstains", () => {
	assert.equal(decideAbstention("what do we do about it", { results: [{ matched_terms: ["about"] }] }).abstained, true);
});

test("missing evidence does not fabricate an abstention", () => {
	const empty: Array<Record<string, unknown>> = [
		{},
		{ results: [] as unknown[] },
		{ results: undefined },
		{ results: [{ matched_terms: [] as string[] }] },
	];
	for (const ev of empty) {
		assert.equal(decideAbstention("section ranking behaviour", ev).abstained, false, JSON.stringify(ev));
	}
});

test("withAbstention adds the signal without overwriting the engine's reason", () => {
	const kept = withAbstention("t3.json", { ok: true, abstention: "no indexed source matches these terms" });
	assert.equal(kept.abstained, true);
	assert.equal(kept.abstention, "no indexed source matches these terms");
	assert.equal(kept.ok, true);

	const fresh = withAbstention("section ranking behaviour", { ok: true, results: [{ matched_terms: ["ranking"] }] });
	assert.equal(fresh.abstained, false);
	assert.equal(fresh.abstention, null);
	assert.equal(fresh.ok, true);
});

test("search keeps a looser bar than injection, on the same evidence", () => {
	// The asymmetry injection-precision.ts argues for: one shared content word is a collision
	// for UNSOLICITED context and merely thin for a question the agent asked.
	const evidence = { results: [{ matched_terms: ["ranking"] }] };
	assert.equal(decideAbstention("ranking precision recall", evidence).abstained, false);
	assert.equal(decideInjection("ranking precision recall", evidence).inject, false);
});

test("CLOSED 2026-10-17: a filename nothing contains abstains; one that IS there answers", () => {
	// Previously recorded as OPEN and unsolvable. That was true of a GENERAL literal rule -
	// measured absent from 13/13 correct identifier answers - but it was over-generalised into
	// "no rule works". The lever never tried was scoping the rule to the query's SHAPE.
	//
	// Measured 2026-10-17 against this live vault: `engram search "t3.json"` returned 5 notes with
	// matched_terms ['t3','json'], and 3 of the top 4 excerpts do not contain the string at all -
	// `t3` was matching "T3 Code", the product name. Segment matches are not the file asked for.
	const collision = {
		results: [
			{ matched_terms: ["t3", "json"], excerpt: "# T3 Code nightly - desktop build fails on t3 Code" },
			{ matched_terms: ["t3", "json"], excerpt: "harness comparison: T3 Code vs pi" },
		],
	};
	const d = decideAbstention("t3.json", collision);
	assert.equal(d.abstained, true, "a filename no source contains must abstain");
	assert.match(d.reason, /literal/);
	assert.match(d.reason, /segments/);

	// The honest positive: one hit DOES contain the literal, and it must be returned.
	const real = {
		results: [{ matched_terms: ["t3", "json"], excerpt: "the workspace file t3.json is missing" }],
	};
	assert.equal(decideAbstention("t3.json", real).abstained, false,
		"a filename that IS present must answer - the rule is not 'filenames never answer'");
});

test("the filename rule does not fire without excerpt evidence", () => {
	// No excerpt means no evidence either way. Abstaining here would be a guess, and a wrong
	// abstention hides a real answer.
	assert.equal(
		decideAbstention("t3.json", { results: [{ matched_terms: ["t3", "json"] }] }).abstained,
		false,
	);
});

test("the filename rule does not fire on natural-language queries", () => {
	// isFilenameShaped is the whole safety property: 0 of the 122 parsed gold-fixture queries
	// contain a dot, so a phrase query can never reach this rule.
	for (const q of ["how does ranking work", "format grep files", "fix all the issues"]) {
		assert.equal(isFilenameShaped(q), false, `"${q}" must not be filename-shaped`);
	}
	for (const q of ["t3.json", "package.json", "config.toml", "agent.rs"]) {
		assert.equal(isFilenameShaped(q), true, `"${q}" must be filename-shaped`);
	}
	const nl = decideAbstention("how does ranking work", {
		results: [{ matched_terms: ["ranking", "work"], excerpt: "ranking work notes" }],
	});
	assert.equal(nl.abstained, false);
});