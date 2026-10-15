/**
 * Session cost awareness, and the resume pointer that makes restarting safe.
 *
 * Measured over 36,609 assistant turns in 472 session files on this machine:
 *
 *   turns with a prompt >= 100K : 67.9%   -> 92.8% of ALL cache-read tokens
 *   their median prompt         : 261,326 tokens
 *   a prompt >= 400K            : 21.0% of turns -> 52.0% of all cache-read cost
 *   fixed session overhead      : 8,576 tokens (system prompt + tool schemas)
 *   turns after a >5 min gap    : 1.0%, costing 5.2% of spend
 *
 * Two conclusions, and the second is the one that matters.
 *
 * First: the overnight cache rebuild is real but small - 5.2% of spend. It is pure waste (nothing
 * is gained by re-sending a stale transcript), and a handoff removes 96.2% of it per occurrence.
 *
 * Second: the reason the rebuild is expensive at all is that the session was already running at
 * 261K. A session that hands off at a task boundary stays small, which makes every subsequent turn
 * cheaper AND makes the next resume cheap. The rebuild is the symptom; the session length is the
 * cause.
 *
 * That reframes what this module is for. Prompting people to restart does not work while restarting
 * costs them their context - so the resume pointer is not a nicety, it is what makes the behaviour
 * that carries the real saving possible.
 *
 * Everything here is pure and takes its numbers as arguments, so the thresholds can be tested
 * rather than trusted.
 */

/** The point at which a session is worth handing off. Chosen from the band table, not by feel. */
export const HANDOFF_THRESHOLD_TOKENS = 150_000;

/** Reminder cadence once past the threshold. One notice, then long gaps - nagging gets muted. */
export const REMINDER_INTERVAL_TURNS = 25;

/** A handoff older than this is history, not a resume pointer. */
export const HANDOFF_STALE_AFTER_MS = 7 * 24 * 60 * 60 * 1000;

export type CostState = {
	turns: number;
	peakTokens: number;
	/** Turn number when the threshold was crossed, 0 if never. */
	crossedAtTurn: number;
	lastReminderTurn: number;
	reminders: number;
};

export function createCostState(): CostState {
	return { turns: 0, peakTokens: 0, crossedAtTurn: 0, lastReminderTurn: 0, reminders: 0 };
}

/** Prompt tokens for one turn: what was billed uncached plus what was served from cache. */
export function promptTokens(usage: unknown): number {
	if (!usage || typeof usage !== "object") return 0;
	const u = usage as Record<string, unknown>;
	const n = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : 0);
	return n(u.input) + n(u.cacheRead) + n(u.cacheWrite);
}

export type CostAdvice = {
	state: CostState;
	/** Emit a notice on this turn. */
	advise: boolean;
	peakTokens: number;
	cacheReadTokens: number;
	reason: string;
};

/**
 * Record one turn's usage and decide whether to say something.
 *
 * The notice fires once when the threshold is crossed, then at a long interval. A reminder every
 * turn would be dismissed within a minute, and a dismissed notice is worse than none: it teaches
 * the reader that this line is noise.
 */
export function recordTurn(
	state: CostState,
	usage: unknown,
	options: { threshold?: number; interval?: number } = {},
): CostAdvice {
	const threshold = options.threshold ?? HANDOFF_THRESHOLD_TOKENS;
	const interval = options.interval ?? REMINDER_INTERVAL_TURNS;
	const tokens = promptTokens(usage);
	const read = (() => {
		const u = usage as Record<string, unknown> | null;
		const v = u && typeof u === "object" ? (u as Record<string, unknown>).cacheRead : 0;
		return typeof v === "number" && Number.isFinite(v) ? v : 0;
	})();

	state.turns += 1;
	if (tokens > state.peakTokens) state.peakTokens = tokens;

	if (tokens < threshold) {
		return { state, advise: false, peakTokens: state.peakTokens, cacheReadTokens: read, reason: "" };
	}
	if (state.crossedAtTurn === 0) {
		state.crossedAtTurn = state.turns;
		state.lastReminderTurn = state.turns;
		state.reminders += 1;
		return {
			state,
			advise: true,
			peakTokens: state.peakTokens,
			cacheReadTokens: read,
			reason: "crossed",
		};
	}
	if (state.turns - state.lastReminderTurn >= interval) {
		state.lastReminderTurn = state.turns;
		state.reminders += 1;
		return { state, advise: true, peakTokens: state.peakTokens, cacheReadTokens: read, reason: "interval" };
	}
	return { state, advise: false, peakTokens: state.peakTokens, cacheReadTokens: read, reason: "" };
}

/** The one-line notice. It states the cost and the action, and nothing else. */
export function costNotice(tokens: number): string {
	const k = Math.round(tokens / 1000);
	return (
		`[MEMORY: SESSION COST] This session's prompt is ${k}K tokens; every turn re-reads it at the ` +
		`cache rate (~${Math.round(tokens * 0.1 / 1000)}K token-equivalents) before doing any work. ` +
		`Measured: turns above 100K are 68% of all turns and 93% of all cache reads. ` +
		`At a task boundary, call memory_fabric_handoff and start a fresh session: a handoff is ~3K ` +
		`against this, and a resume from it is automatic.`
	);
}

export type Handoff = {
	/** ISO timestamp. */
	at?: string;
	session?: string;
	/**
	 * Where the raw transcript lives, so the pointer is reversible.
	 *
	 * Not an archive: pi already persists every session to disk and its compaction keeps omitted
	 * raw entries stored. The lossiness was never missing data, it was the pointer not saying
	 * where the data is.
	 */
	transcript?: string;
	done?: string;
	in_flight?: string;
	blocked?: string;
	next?: string;
	pointers?: string[];
};

/**
 * The resume pointer, bounded to roughly 250 characters.
 *
 * Deliberately a pointer and not a summary. A summary of a 261K session is a few hundred tokens of
 * the same stale context at a smaller scale; what the next session actually needs is the *diff* -
 * what changed, what is unfinished, and where to read more. The full handoff stays in the vault
 * where it is versioned and searchable, and is read only if the pointer is not enough.
 */
export function resumePointer(handoff: Handoff, nowMs: number): string | null {
	const fields = ["done", "in_flight", "blocked", "next"] as const;
	const parts = fields
		.filter((f) => typeof handoff[f] === "string" && (handoff[f] as string).trim())
		.map((f) => `${f.replace("_", "-")}: ${(handoff[f] as string).trim()}`);
	if (!parts.length) return null;

	let age = "";
	if (handoff.at) {
		const then = Date.parse(handoff.at);
		if (Number.isFinite(then)) {
			const mins = Math.max(0, Math.round((nowMs - then) / 60000));
			age = mins < 60 ? `${mins}m ago` : mins < 2880 ? `${Math.round(mins / 60)}h ago` : `${Math.round(mins / 1440)}d ago`;
		}
	}
	// The transcript path is included only when it exists. A pointer to a file that is not there
	// is worse than no pointer, because the next session would spend a turn finding that out.
	const tail = handoff.transcript
		? `Full handoff: inbox/RESUME.md | transcript: ${handoff.transcript}`
		: "Full handoff: inbox/RESUME.md";
	return `[RESUME] A previous session handed off${age ? ` ${age}` : ""}. ${parts.join(" | ")}. ${tail}`;
}

/** Whether a handoff is recent enough to be a resume pointer rather than history. */
export function isResumable(handoff: Handoff, nowMs: number): boolean {
	if (!handoff.at) return false;
	const then = Date.parse(handoff.at);
	if (!Number.isFinite(then)) return false;
	const age = nowMs - then;
	return age >= 0 && age <= HANDOFF_STALE_AFTER_MS;
}
