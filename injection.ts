/**
 * #97 / #105 - what the injected memory block costs, and whether it stayed stable.
 *
 * Extracted from index.ts so it can be tested. This repo already learned the lesson once: the
 * scrub predicate lived inline where no test could reach it, and shipped a dead notification while
 * its suite stayed green.
 *
 * The prefix-cache argument for resolving refs once per session and injecting them once rests on
 * the block being *identical* on every turn. That was true by construction and asserted nowhere,
 * so a change that reintroduced per-turn variation — a timestamp, a count, a re-sorted ref list,
 * a note whose title changed mid-session — would have silently undone the caching it exists to
 * preserve, and the only symptom would be a slower session that still looked correct.
 */
export type InjectionRecord = {
	/** 1-based index of this injection within the session. */
	turn: number;
	chars: number;
	refs: number;
	sessionChars: number;
	/** True when the block differs from the first one injected this session. */
	prefixBreak: boolean;
};

export function createInjectionAccount() {
	let baseline: number | null = null;
	let turns = 0;
	let sessionChars = 0;
	let breaks = 0;

	return {
		/**
		 * Record one injection. The first call sets the baseline; any later call with a different
		 * size is a prefix break. Size is the signal rather than content because size is what the
		 * cache prefix depends on, and hashing 1 KB per turn to prove a property that size already
		 * disproves would be work for its own sake.
		 */
		record(chars: number, refs: number): InjectionRecord {
			turns += 1;
			sessionChars += chars;
			let prefixBreak = false;
			if (baseline === null) {
				baseline = chars;
			} else if (chars !== baseline) {
				prefixBreak = true;
				breaks += 1;
			}
			return { turn: turns, chars, refs, sessionChars, prefixBreak };
		},

		state() {
			return { turns, sessionChars, baselineChars: baseline, prefixBreaks: breaks };
		},
	};
}
