/**
 * Whether unsolicited context is good enough to send.
 *
 * The verified failure: injection returned "vocabulary neighbours, not the right note". Real
 * examples from 168 recorded injections -
 *
 *   "fix all the issues that you found"        -> daily/2026-09-29.md, agency-dispatch-engine.md
 *   "So do one thing check all the milestone"  -> three pi-tern notes (the prompt has NO topic)
 *   "Are we not storing anything in the local memory extension" -> marketplace, github-presence
 *
 * Every one of those is a vocabulary collision on a GENERIC word. The prompt was mostly function
 * words; the notes contained the same common words; the match was real and meaningless.
 *
 * The governing principle: REQUESTED context and UNSOLICITED context deserve different bars.
 * When the agent calls search it has asked a question, and a permissive result is useful even if
 * imperfect - the agent can read more, or ignore it. Injection is unsolicited: it arrives before
 * the agent has decided anything, and a wrong note is pure cost with no upside, because the agent
 * cannot tell it is wrong. So injection gets the stricter bar, and search keeps the looser one.
 *
 * Everything here fails closed. A missed injection costs one note the agent can still find by
 * searching; a bad injection costs turns, tokens and trust.
 */

/** Function words whose presence says nothing about topic. Deliberately small and unambitious. */
const GENERIC = new Set([
	"the", "and", "for", "with", "that", "this", "from", "have", "has", "had", "are", "was", "were",
	"not", "but", "you", "your", "our", "its", "it", "we", "us", "all", "any", "can", "will",
	"would", "should", "could", "please", "now", "then", "there", "here", "when", "what", "which",
	"who", "how", "why", "do", "does", "did", "done", "make", "made", "get", "got", "give", "one",
	"two", "also", "just", "only", "more", "most", "much", "many", "some", "such", "than", "too",
	"very", "into", "onto", "over", "under", "about", "after", "before", "again", "still", "need",
	"needs", "want", "work", "working", "fix", "fixes", "fixed", "issue", "issues", "thing",
	// "found" added 2026-10-17. This file's own header cites "fix all the issues that you
	// found" as the observed failure shape, and `found` was the one word in it that survived
	// contentWords() — so the query still ANSWERED instead of abstaining. A past-participle
	// verb carrying no topical signal in a code corpus. Regression-checked against the
	// 136-case gold fixture before and after; no metric moved. Deliberately ONE word: a
	// broader set would move the injection gate further, and that gate is shadow mode and
	// uncalibrated.
	"found",
	"things", "check", "checks", "checked", "run", "runs", "use", "used", "uses", "let", "lets",
]);

/** Words that carry topical information: long enough and not a function word. */
export function contentWords(text: string): string[] {
	return [...new Set(
		text
			.toLowerCase()
			.split(/[^a-z0-9_-]+/)
			.filter((w) => w.length >= 4 && !GENERIC.has(w)),
	)];
}

export type Candidate = { source_ref?: string; matched_terms?: string[]; section_anchor?: string };
export type Decision = { inject: boolean; reason: string; coverage: number; matched: number; content: number };

/**
 * The gate.
 *
 * Three rules, each targeting an observed failure:
 *   1. A search that flags itself `low_discrimination` is not a candidate for injection. The engine
 *      already knows the terms cannot separate documents; the injection must not override it.
 *   2. The best result must match at least MIN_MATCH content words. One shared word out of six is a
 *      collision, not a match.
 *   3. Coverage: the match must account for at least MIN_COVERAGE of the prompt's content words.
 *      This is what stops a generic prompt - six content words, one matched - from injecting.
 */
export const MIN_MATCH = 2;
export const MIN_COVERAGE = 0.5;

export function decideInjection(
	prompt: string,
	response: { results?: Candidate[]; low_discrimination?: boolean; abstention?: unknown },
): Decision {
	const content = contentWords(prompt);
	const results = Array.isArray(response?.results) ? response.results : [];
	const none: Decision = { inject: false, reason: "", coverage: 0, matched: 0, content: content.length };

	if (results.length === 0) return { ...none, reason: "no results" };
	// Rule 1
	if (response.low_discrimination === true) return { ...none, reason: "search flagged low discrimination" };
	if (response.abstention) return { ...none, reason: "search abstained" };
	// A prompt with almost no content words cannot be matched meaningfully. It is also the exact
	// shape of the observed failures, so this is the guard rather than a side effect.
	if (content.length < 2) return { ...none, reason: "prompt has too few content words" };

	const top = results[0];
	const matchedTerms = (top.matched_terms ?? []).filter((t) => content.includes(String(t).toLowerCase()));
	const matched = matchedTerms.length;
	const coverage = matched / content.length;

	if (matched < MIN_MATCH) {
		return { inject: false, reason: `best result matched ${matched} content word(s), need ${MIN_MATCH}`, coverage, matched, content: content.length };
	}
	if (coverage < MIN_COVERAGE) {
		return { inject: false, reason: `coverage ${(coverage * 100).toFixed(0)}% is below ${MIN_COVERAGE * 100}%`, coverage, matched, content: content.length };
	}
	return { inject: true, reason: `matched ${matched}/${content.length} content words`, coverage, matched, content: content.length };
}

/**
 * SHADOW MODE, and why it is the shipped default.
 *
 * I built this gate to block injections whose match was vocabulary noise. Calibrating it against
 * every recorded injection produced a result that overturned the design: the injections the agent
 * DID read had FEWER matched terms (0-5) than the ones it ignored (0-13). Term strength does not
 * separate a good injection from a bad one - it inverts. And "was read" is not ground truth either:
 * the independent verifier showed the agent reading three injected notes, judging them irrelevant,
 * and wasting the turns. Reading was the COST.
 *
 * So there is no labelled data for "did this injection help", which is the same missing A/B the
 * evidence directory already records. Enforcing a threshold I cannot calibrate would repeat the
 * exact failure this session already made once: a plausible method, a confident number, no way to
 * know it was wrong.
 *
 * Shadow mode costs nothing and harms nothing. It records the decision it WOULD have made, so that
 * after enough sessions there is finally a dataset, and a threshold can be chosen from it rather
 * than asserted. Enforcement is a one-line change once that data exists.
 */
export const ENFORCE = process.env.MEMORY_FABRIC_ENFORCE_INJECTION_GATE === "1";
