/**
 * Access policy for the local memory fabric. Pure and side-effect free, so it can be
 * tested (see `remote-policy.test.ts`) without loading the Pi extension runtime.
 *
 * There are three separate gates, and they do not ask the same question:
 *
 *   providerAllowed      gate 1 - may this model touch canonical memory at all?
 *   shouldScrubContext   gate 2 - may memory-derived content reach its context?
 *   mayWriteToMemory     gate 3 - may this model write to canonical memory?
 *
 * Operator decisions recorded 2026-10-11:
 *   - An exempted provider gets full READ access, like a loopback one. An earlier build
 *     stripped an exempted provider's own fabric results, which made the exemption
 *     pointless: the call succeeded and the answer was removed before the model saw it.
 *   - WRITE access for an exempted provider is off unless the operator switches on
 *     `PI_MEMORY_REMOTE_FULL_ACCESS`. Read-only is the default for a remote provider.
 *   - A provider that is not allowlisted is never granted anything, whatever the switch.
 */

export type Model = { provider?: string; baseUrl?: string };

export const LOOPBACK_HOSTS = new Set(["localhost", "127.0.0.1", "::1"]);

/** Parse a comma-separated provider list from the environment, falling back to a default. */
export function parseProviderList(value: string | undefined, fallback: string): Set<string> {
	return new Set(
		(value ?? fallback)
			.split(",")
			.map((provider) => provider.trim().toLowerCase())
			.filter(Boolean),
	);
}

/**
 * Remote write-back switch. **Default: enabled.**
 *
 * Changed from opt-in to opt-out on the operator's explicit instruction (2026-10-14), so that a
 * repository can save and update memory without first having to discover this variable. Only an
 * explicit negative disables it; unset or empty means enabled.
 *
 * What this grants: a provider that is loopback **or** listed in PI_MEMORY_REMOTE_ALLOWED_PROVIDERS
 * may write to canonical memory. A provider that is not allowlisted still may not - the switch
 * cannot reach past the allowlist, and that is asserted by test.
 *
 * Recorded risk, because it is not obvious from the call site: the knowledge base is not under
 * version control, so a wrong write is not revertable. That is roadmap item P0, and the reason to
 * run `git init` there before relying on this. Set PI_MEMORY_REMOTE_FULL_ACCESS=0 to restore
 * loopback-only writes at any time.
 */
export function remoteFullAccessEnabled(value: string | undefined): boolean {
	// Only an explicit negative disables it.
	return !/^(0|false|no|off)$/i.test((value ?? "").trim());
}

function httpUrl(model: Model | undefined): URL | undefined {
	if (!model?.provider || !model.baseUrl) return undefined;
	try {
		const url = new URL(model.baseUrl);
		return url.protocol === "http:" || url.protocol === "https:" ? url : undefined;
	} catch {
		return undefined;
	}
}

/** The model's configured endpoint is loopback, so nothing leaves the machine. */
export function isLoopbackModel(model: Model | undefined, localProviders: Set<string>): boolean {
	if (!model?.provider || !localProviders.has(model.provider.toLowerCase())) return false;
	const url = httpUrl(model);
	if (!url) return false;
	return LOOPBACK_HOSTS.has(url.hostname.replace(/^\[|\]$/g, "").toLowerCase());
}

/** The operator's named exception: a remote provider that is explicitly permitted. */
export function isExemptedProvider(model: Model | undefined, exempted: Set<string>): boolean {
	if (!model?.provider || !exempted.has(model.provider.toLowerCase())) return false;
	return httpUrl(model) !== undefined;
}

/** Gate 1: may this model touch canonical memory at all? */
export function providerAllowed(
	model: Model | undefined,
	localProviders: Set<string>,
	exempted: Set<string>,
): boolean {
	return isLoopbackModel(model, localProviders) || isExemptedProvider(model, exempted);
}

/**
 * Gate 2: may memory-derived content reach this model's context?
 *
 * Only a model that is not allowed at all is scrubbed. An exempted provider is allowed to
 * read, so its results are kept - that is what the exemption is for. The provider-switch
 * protection survives: moving to an unlisted provider still strips earlier local-only
 * material, which is the case this scrub exists for.
 */
export function shouldScrubContext(
	model: Model | undefined,
	localProviders: Set<string>,
	exempted: Set<string>,
): boolean {
	return !providerAllowed(model, localProviders, exempted);
}

/**
 * Gate 3: may this model write to canonical memory?
 *
 * A loopback provider may. An exempted remote provider may only when the operator has
 * switched on remote full access; otherwise it is read-only. Anything else may not write.
 */
export function mayWriteToMemory(
	model: Model | undefined,
	localProviders: Set<string>,
	exempted: Set<string>,
	remoteFullAccess: boolean,
): boolean {
	if (isLoopbackModel(model, localProviders)) return true;
	if (!isExemptedProvider(model, exempted)) return false;
	return remoteFullAccess;
}
