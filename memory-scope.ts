/**
 * Memory scoping: which memory may reach which session.
 *
 * This exists because of a real leak. A session working on an unrelated project ("Fikr") received a
 * `[RESUME]` block describing THIS project's cache-economics work and a ranking fixture. The
 * handoff was a single global file and the injection checked only *time* — `isResumable(handoff)` —
 * never *place*. Any session, in any project, got whatever the last handoff happened to say.
 *
 * The lesson is not "add a project field". It is that an unscoped memory injection is a context
 * pollution bug by construction: the more useful the memory, the more damaging it is when it
 * arrives in the wrong session, because the receiving agent has no way to tell it is stale and
 * unrelated. A wrong note is worse than no note, and the receiving session cannot detect it.
 *
 * So scoping fails CLOSED here in three places:
 *   1. A handoff with no recorded project is never injected. Unknown provenance is not a licence.
 *   2. A handoff only reaches a session whose project key matches exactly.
 *   3. Project-tier notes are only eligible in a session whose project they belong to.
 *
 * Everything here is pure and takes its inputs as arguments, so the rules can be tested rather than
 * trusted. See memory-scope.test.ts — one of those tests is the Fikr leak, reproduced.
 */

/** Tiers, in the order they should be considered for injection. */
export type Tier = "global" | "project" | "episodic";

/**
 * A stable key for the project a session belongs to.
 *
 * Deliberately the normalised cwd, not a git remote: two checkouts of the same repo on one machine
 * are different working contexts, and a session should not resume one from the other's handoff.
 * Trailing separators and `~` are normalised so the same directory always yields the same key.
 */
export function projectKey(cwd: string | undefined | null): string | null {
	if (typeof cwd !== "string") return null;
	const trimmed = cwd.trim().replace(/\/+$/, "");
	if (!trimmed) return null;
	// A bare "/" or "." is not a project.
	if (trimmed === "/" || trimmed === ".") return null;
	return trimmed;
}

/**
 * Which tier a vault path belongs to.
 *
 * `global` notes state things true everywhere (operator preferences, tool invariants). `project`
 * notes are about one codebase and are meaningless — or actively misleading — anywhere else.
 * Everything else is episodic and never auto-injected.
 */
export function tierOf(relpath: string): Tier {
	const p = relpath.replace(/^\/+/, "");
	if (p.startsWith("user/") || p.startsWith("invariants/")) return "global";
	if (p.startsWith("projects/") || p.startsWith("pi-harness/") || p.startsWith("company/")) {
		return "project";
	}
	return "episodic";
}

/**
 * Whether a `projects/x.md` note may be injected into a session at `cwd`.
 *
 * The vault names project notes by slug, not by path, so the link is made on the note's own
 * declared identity rather than a guess: a note is eligible when its slug appears in the project
 * key. Deliberately narrow. A false negative costs one note the agent can still find by search; a
 * false positive is the leak this file exists to prevent.
 */
export function projectNoteMatches(relpath: string, cwd: string | null): boolean {
	const key = projectKey(cwd);
	if (!key) return false;
	const m = /^projects\/([^/]+)\.md$/.exec(relpath);
	if (!m) return false;
	const slug = m[1].toLowerCase();
	// Match on the cwd's LAST segment, not on the whole path. The first version asked whether the
	// path contains the slug, which fails for the common case: the directory is
	// `local-memory-fabric` while the note is `local-memory-fabric-extension`. A note name is often
	// the directory name plus a qualifier, so the prefix relationship runs that way.
	const base = key.split("/").filter(Boolean).pop()?.toLowerCase() ?? "";
	if (base.length < 5) return false;                 // too short to be distinctive
	return slug === base || slug.startsWith(`${base}-`) || base.startsWith(`${slug}-`);
}

/** The stored handoffs, keyed by project key. */
export type HandoffStore = Record<string, unknown>;

/** Read a handoff for one session. Returns null when the key is absent or the entry is unshaped. */
export function handoffForProject(
	store: HandoffStore | null | undefined,
	cwd: string | null | undefined,
): Record<string, unknown> | null {
	const key = projectKey(cwd);
	if (!key || !store || typeof store !== "object") return null;
	const entry = store[key];
	if (!entry || typeof entry !== "object") return null;
	return entry as Record<string, unknown>;
}

/**
 * A handoff is only usable if it names the project it belongs to AND that matches.
 *
 * The project check is repeated here rather than assumed, so a caller that reads a handoff out of
 * some other place — an older file, a test fixture, a future export — still cannot inject it into
 * the wrong session. The check belongs at the last gate before the prompt, not only at the door.
 */
export function mayInjectHandoff(
	handoff: Record<string, unknown> | null,
	cwd: string | null | undefined,
): boolean {
	if (!handoff) return false;
	const key = projectKey(cwd);
	if (!key) return false;
	const owner = typeof handoff.cwd === "string" ? projectKey(handoff.cwd) : null;
	// Unknown provenance fails closed. Every handoff written before this change had no cwd, and
	// those are exactly the ones that leaked.
	if (!owner) return false;
	return owner === key;
}
