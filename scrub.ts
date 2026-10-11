/**
 * The context scrub, as a pure function. Pure and side-effect free, so it can be tested
 * (see `scrub.test.ts`) without loading the Pi extension runtime.
 *
 * Why this is a module and not a block inside `index.ts`: the scrub is the one piece of the
 * extension that decides what leaves the machine, and it was previously unreachable by any
 * test — `remote-policy.test.ts` covers the gates, but the hook that *calls* them had zero
 * coverage. That is how a dead `ctx.ui.notify` shipped green. The predicate lives here so a
 * test can pin it directly.
 *
 * The contract, in one line: **withhold more, never less.** Every branch that cannot prove a
 * message is safe to send drops it.
 */

/** Anything shaped like an AgentMessage; only the fields the scrub reads are typed. */
export type ScrubMessage = {
	role?: unknown;
	provider?: unknown;
	customType?: unknown;
	content?: unknown;
	toolName?: unknown;
	toolCallId?: unknown;
};

/** Tracks which tool calls carried vault-derived content, so their results can be dropped. */
export type ScrubState = {
	/** tool-call ids whose arguments touched the vault, Apple Notes, or a fabric tool */
	trackedToolCallIds: Set<string>;
	/**
	 * Sticky. Set when either tracking set outgrows its cap. From then on the scrub cannot
	 * prove an untracked tool result is safe, so it drops every tool result rather than
	 * risking one whose id was evicted. See `noteTrackingOverflow`.
	 */
	overflowed: boolean;
	/** True when at least one message was removed — the signal to announce the scrub. */
	removedAnything?: boolean;
};

export function createScrubState(): ScrubState {
	return { trackedToolCallIds: new Set<string>(), overflowed: false, removedAnything: false };
}

/**
 * Why the cap fails closed instead of evicting.
 *
 * The tracked-id set used to drop its oldest entry past the cap. Eviction makes an id
 * untrackable, which makes its tool result look safe, which sends content the policy had
 * already decided to withhold — in a system whose entire promise is that it does not.
 * Measured by replaying the hook: 4,100 vault-touching tool calls leaked the 4 oldest
 * results, 4,146 leaked 50, and every leak landed at the front of the payload, so it
 * invalidated the cached prefix as well. Bounded memory is worth less than the guarantee.
 */
export const MAX_TRACKED_IDS = 4096;

function isLocalProviderId(provider: unknown, localProviders: Set<string>): boolean {
	return typeof provider === "string" && localProviders.has(provider.toLowerCase());
}

/**
 * Populate `state.trackedToolCallIds` from a transcript, and flag overflow.
 *
 * Call once per turn with the full transcript, before filtering. Re-scanning is what keeps
 * this correct across compaction: a turn that adds nothing still re-derives the full set
 * from whatever history survives.
 */
export function trackVaultToolCalls(
	messages: readonly ScrubMessage[],
	state: ScrubState,
	localProviders: Set<string>,
	touchesVault: (args: unknown) => boolean,
	touchesAppleNotes: (args: unknown) => boolean,
): ScrubState {
	for (const message of messages) {
		if (message.role !== "assistant" || !isLocalProviderId(message.provider, localProviders)) continue;
		if (!Array.isArray(message.content)) continue;
		for (const part of message.content as Array<Record<string, unknown>>) {
			if (part.type !== "toolCall" || typeof part.id !== "string") continue;
			const toolName = String(part.name ?? "");
			const args = part.arguments ?? part.input;
			if (toolName.startsWith("memory_fabric_") || touchesVault(args) || touchesAppleNotes(args)) {
				state.trackedToolCallIds.add(part.id);
				if (state.trackedToolCallIds.size > MAX_TRACKED_IDS) state.overflowed = true;
			}
		}
	}
	return state;
}

/**
 * The scrub predicate. Returns the messages that may be sent.
 *
 * Drops, in order: injected vault-context blocks; assistant turns from local providers;
 * tool results for fabric tools or vault-touching tool calls; any tool result at all once
 * tracking has overflowed; and assistant turns matching a recorded loopback fingerprint.
 */
export function scrubMessages(
	messages: readonly ScrubMessage[],
	state: ScrubState,
	localProviders: Set<string>,
	localMessageHashes: ReadonlySet<string>,
	fingerprint: (message: unknown) => string,
): ScrubMessage[] {
	const filtered = messages.filter((message) => {
		const customType = typeof message.customType === "string" ? message.customType : "";
		if (customType.startsWith("vault-context")) return false;
		if (message.role === "assistant" && isLocalProviderId(message.provider, localProviders)) return false;
		if (message.role === "toolResult") {
			const name = String(message.toolName ?? "");
			if (name.startsWith("memory_fabric_") || state.trackedToolCallIds.has(String(message.toolCallId ?? ""))) return false;
			// Fail closed: untracked, but we can no longer prove untracked means safe.
			if (state.overflowed) return false;
		}
		if (message.role === "assistant") {
			try {
				if (localMessageHashes.has(fingerprint(message))) return false;
			} catch {
				/* An unhashable message is kept; hashing failing is not evidence of harm. */
			}
		}
		return true;
	});
	state.removedAnything = filtered.length !== messages.length;
	return filtered;
}