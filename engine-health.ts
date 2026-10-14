/**
 * Engine failure memory — circuit breaker and crash-loop guard, as pure decisions.
 *
 * Extracted from `index.ts` so it can be tested. The version of these rules that lived inline
 * was unverifiable: a broken engine costs a process spawn plus a timeout on *every* turn, and
 * the only way to check the fix was to break a real engine and watch. These are the same rules
 * with the clock injected, so the behaviour is assertable in milliseconds.
 *
 * Two bounds, both counted rather than assumed:
 *  - **Breaker:** after `threshold` consecutive failures, refuse locally for `windowMs`. A
 *    refusal is cheap; a spawn plus timeout is not. After the window, one probe is allowed
 *    through and its outcome decides whether the breaker reopens or resets.
 *  - **Respawn guard:** if the worker dies `respawnLimit` times inside `respawnWindowMs`, stop
 *    respawning for the session. A binary that exits instantly would otherwise become a spawn
 *    storm — one process per turn, each failing immediately.
 *
 * Deliberately not a rate limiter: success resets the counter, because an engine that works
 * once and fails once is not failing.
 */

export type HealthState = {
	consecutiveFailures: number;
	breakerOpenedAt: number;
	respawnTimes: number[];
	refusals: number;
	respawnRefusals: number;
};

export type HealthOptions = {
	threshold?: number;
	windowMs?: number;
	respawnLimit?: number;
	respawnWindowMs?: number;
};

export function createHealth(options: HealthOptions = {}) {
	const threshold = options.threshold ?? 3;
	const windowMs = options.windowMs ?? 60_000;
	const respawnLimit = options.respawnLimit ?? 3;
	const respawnWindowMs = options.respawnWindowMs ?? 60_000;

	const state: HealthState = {
		consecutiveFailures: 0,
		breakerOpenedAt: 0,
		respawnTimes: [],
		refusals: 0,
		respawnRefusals: 0,
	};

	return {
		state,

		/** Record an engine outcome. Success resets everything, including an open breaker. */
		record(ok: boolean, now: number): void {
			if (ok) {
				state.consecutiveFailures = 0;
				state.breakerOpenedAt = 0;
				return;
			}
			state.consecutiveFailures += 1;
			if (state.consecutiveFailures >= threshold && state.breakerOpenedAt === 0) {
				state.breakerOpenedAt = now;
			}
		},

		/**
		 * May a request be attempted? Once the window has elapsed, exactly one probe is allowed:
		 * the breaker clears so the call proceeds, and its result decides what happens next.
		 * Refusals are counted, and the counter is what distinguishes "tripped once" from
		 * "tripping constantly" in the usage record.
		 */
		allow(now: number): boolean {
			if (state.breakerOpenedAt === 0) return true;
			if (now - state.breakerOpenedAt < windowMs) {
				state.refusals += 1;
				return false;
			}
			// Window elapsed: this call is the probe.
			state.breakerOpenedAt = 0;
			state.consecutiveFailures = 0;
			return true;
		},

		/** May a worker be respawned, or would that be a storm? */
		allowRespawn(now: number): boolean {
			state.respawnTimes = state.respawnTimes.filter((t) => now - t < respawnWindowMs);
			if (state.respawnTimes.length >= respawnLimit) {
				state.respawnRefusals += 1;
				return false;
			}
			state.respawnTimes.push(now);
			return true;
		},

		/** Human-readable refusal, so the failure names itself instead of reading as flakiness. */
		refusalMessage(now: number): string {
			const remaining = Math.max(0, Math.ceil((windowMs - (now - state.breakerOpenedAt)) / 1000));
			return `Local memory is unavailable: the engine failed ${state.consecutiveFailures} times in a row; retrying in ${remaining}s. Run /memory-fabric status to inspect it.`;
		},
	};
}
