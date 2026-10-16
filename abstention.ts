/**
 * "No match" as a first-class answer, rather than the engine's best guess wearing a
 * confidence it has not earned.
 *
 * The gap this closes is not that the engine returns nothing. It is that the engine has no way
 * to say `shown: 5` is NOT an answer. Measured live on this vault, the `abstention` field is
 * `null` on every search that returns anything at all, so a query whose vocabulary is pure
 * vocabulary-collision comes back as five confident-looking notes, and `rank_score: 0.978` on
 * the top one. `rank_score` is min-max normalised WITHIN the page, so the best result always
 * scores ~1.0 no matter how weak the evidence is. It is a position, not a confidence, and
 * nothing in the payload said so.
 *
 * WHY THIS IS A SEPARATE MODULE. `index.ts` imports Pi's runtime, so no suite can import it and
 * a defect here would hide exactly as well as the 0.3.4 ones did. So the policy is pure and
 * lives here; `index.ts` only supplies evidence and copies out the verdict.
 *
 * WHY IT REUSES `injection-precision.ts` INSTEAD OF ITS OWN WORD LIST. That file already holds
 * `GENERIC`, `contentWords()`, `MIN_MATCH` and `MIN_COVERAGE`, built from 168 recorded
 * injections that failed by colliding on a common word. A second stop list and a second
 * threshold would be a second, divergent definition of "this query has a topic" - and the two
 * would disagree, which is the bug this repo keeps re-finding. There is one vocabulary here and
 * the injection gate reads this module's verdict (it already consumes `abstention`).
 *
 * THE TWO BARS ARE NOT THE SAME, ON PURPOSE. `injection-precision.ts` argues it at length:
 * REQUESTED context and UNSOLICITED context deserve different bars. When an agent calls search it
 * has asked a question, and a weak result is still worth seeing - it can read more, or ignore it.
 * So search's bar here is deliberately the loosest one that is still honest (zero discriminative
 * overlap), while injection keeps its stricter `MIN_MATCH`/`MIN_COVERAGE` and inherits this
 * module's abstention through the field it already reads.
 */

import { contentWords } from "./injection-precision.ts";

/** The shape both engines return for a search, as far as this policy cares. */
export type RetrievalEvidence = {
	results?: Array<{ matched_terms?: string[] }>;
	/** Set by the engine when it itself found nothing. Its reason string is authoritative. */
	abstention?: string | null;
	/** The engine's own "these terms cannot separate notes" flag. */
	low_discrimination?: boolean;
	shown?: number;
};

export type Abstention = { abstained: boolean; reason: string };

const ANSWERED: Abstention = { abstained: false, reason: "" };

/**
 * The gate. Returns `abstained: false` for an ordinary answer, so the caller can distinguish
 * three states that used to collapse into one: "nothing indexed", "nothing matched", and
 * "matched, but not discriminatively" - the third is the one that shipped silently.
 *
 * Four rules, first match wins, none of them introducing a threshold that
 * `injection-precision.ts` does not already own:
 *
 *   1. The engine already abstained. Pass its reason through untouched: "nothing indexed" and
 *      "nothing matched" are different facts and it is the only thing that can tell them apart.
 *   2. The engine flagged `low_discrimination`. Consume the engine's verdict instead of
 *      re-deriving it and disagreeing with it.
 *   3. The query has no discriminative words at all - every word is a function word or shorter
 *      than four characters. This is the observed failure shape ("fix all the issues that you
 *      found") and the reason `contentWords()` exists.
 *   4. The best result matched NONE of the query's content words. Every term that carried it
 *      was generic, so what is on top is a vocabulary neighbour.
 */
export function decideAbstention(query: string, evidence: RetrievalEvidence = {}): Abstention {
	const engineReason = typeof evidence.abstention === "string" ? evidence.abstention.trim() : "";
	if (engineReason) return { abstained: true, reason: engineReason };

	if (evidence.low_discrimination === true) {
		return {
			abstained: true,
			reason: "the engine flagged these terms as non-discriminating; the results are unranked candidates, not a ranked answer",
		};
	}

	const content = contentWords(query);
	if (content.length === 0) {
		return {
			abstained: true,
			reason: "the query has no discriminative words; every term is a function word or shorter than four characters",
		};
	}

	const top = Array.isArray(evidence.results) ? evidence.results[0] : undefined;
	const matched = new Set((top?.matched_terms ?? []).map((t) => String(t).toLowerCase()));
	if (matched.size > 0 && content.every((w) => !matched.has(w))) {
		return {
			abstained: true,
			reason: `the best result matched none of the query's ${content.length} discriminative word(s)`,
		};
	}

	return ANSWERED;
}

/**
 * The exact shape to hand back to a caller, so `index.ts` stays a copy rather than a decision.
 * The engine's own `abstention` reason is preserved when it abstained, and never overwritten.
 */
export function withAbstention<T extends RetrievalEvidence>(query: string, response: T): T & Abstention {
	const verdict = decideAbstention(query, response ?? {});
	const merged = { ...(response ?? ({} as T)) } as T & RetrievalEvidence & Abstention;
	merged.abstained = verdict.abstained;
	merged.abstention = verdict.abstained ? verdict.reason : (response?.abstention ?? null);
	return merged as T & Abstention;
}