/**
 * Decision table for the fabric's access policy. Run with:
 *
 *   node --experimental-strip-types --test remote-policy.test.ts
 *
 * The matrix is the contract: read access follows the exemption, write access does not
 * unless the operator switches it on, and an unlisted provider is granted nothing.
 */
import { test } from "node:test";
import assert from "node:assert/strict";
import {
	isExemptedProvider,
	isLoopbackModel,
	mayWriteToMemory,
	parseProviderList,
	providerAllowed,
	remoteFullAccessEnabled,
	shouldScrubContext,
} from "./remote-policy.ts";

const LOCAL = parseProviderList(undefined, "lmstudio,llama.cpp,ollama");
const EXEMPT = parseProviderList(undefined, "opencode-go,opencode,pi");
const NONE = parseProviderList("", "lmstudio,llama.cpp,ollama");

const loopback = { provider: "llama.cpp", baseUrl: "http://127.0.0.1:8080/v1" };
const loopbackHost = { provider: "ollama", baseUrl: "http://localhost:11434" };
const loopbackV6 = { provider: "lmstudio", baseUrl: "http://[::1]:1234/v1" };
const exempted = { provider: "opencode-go", baseUrl: "https://api.opencode.ai/v1" };
const exemptedMixedCase = { provider: "OpenCode-Go", baseUrl: "https://api.opencode.ai/v1" };
const unlisted = { provider: "google", baseUrl: "https://generativelanguage.googleapis.com" };
const noUrl = { provider: "opencode-go" };
const localButRemoteUrl = { provider: "llama.cpp", baseUrl: "https://api.example.com/v1" };

test("loopback detection accepts http and https on localhost, 127.0.0.1 and ::1", () => {
	assert.equal(isLoopbackModel(loopback, LOCAL), true);
	assert.equal(isLoopbackModel(loopbackHost, LOCAL), true);
	assert.equal(isLoopbackModel(loopbackV6, LOCAL), true);
	assert.equal(isLoopbackModel({ provider: "ollama", baseUrl: "https://127.0.0.1:11434" }, LOCAL), true);
});

test("loopback detection rejects a provider that is allowed but pointed off-machine", () => {
	assert.equal(isLoopbackModel(localButRemoteUrl, LOCAL), false);
	assert.equal(isLoopbackModel(unlisted, LOCAL), false);
	assert.equal(isLoopbackModel(undefined, LOCAL), false);
});

test("gate 1: loopback and exempted providers may read, everything else may not", () => {
	assert.equal(providerAllowed(loopback, LOCAL, EXEMPT), true);
	assert.equal(providerAllowed(exempted, LOCAL, EXEMPT), true);
	assert.equal(providerAllowed(unlisted, LOCAL, EXEMPT), false);
	assert.equal(providerAllowed(noUrl, LOCAL, EXEMPT), false);
	assert.equal(providerAllowed(undefined, LOCAL, EXEMPT), false);
});

test("the exemption is case-insensitive and needs a usable endpoint", () => {
	assert.equal(isExemptedProvider(exemptedMixedCase, EXEMPT), true);
	assert.equal(isExemptedProvider(noUrl, EXEMPT), false);
	assert.equal(isExemptedProvider(exempted, NONE), false, "an empty list must restore loopback-only");
});

test("gate 2: an exempted provider is NOT scrubbed, since it is allowed to read", () => {
	assert.equal(shouldScrubContext(loopback, LOCAL, EXEMPT), false);
	assert.equal(shouldScrubContext(exempted, LOCAL, EXEMPT), false);
	// The provider-switch case this scrub exists for: an unlisted provider still loses
	// earlier local-only material.
	assert.equal(shouldScrubContext(unlisted, LOCAL, EXEMPT), true);
	assert.equal(shouldScrubContext(undefined, LOCAL, EXEMPT), true);
});

test("gate 3: remote write access requires the switch; loopback never needs it", () => {
	// Switch off (the default): loopback may write, exempted may not.
	assert.equal(mayWriteToMemory(loopback, LOCAL, EXEMPT, false), true);
	assert.equal(mayWriteToMemory(exempted, LOCAL, EXEMPT, false), false);
	// Switch on: exempted may write too.
	assert.equal(mayWriteToMemory(exempted, LOCAL, EXEMPT, true), true);
	// The switch cannot grant access to a provider that is not allowlisted at all.
	assert.equal(mayWriteToMemory(unlisted, LOCAL, EXEMPT, true), false);
	assert.equal(mayWriteToMemory(undefined, LOCAL, EXEMPT, true), false);
	assert.equal(mayWriteToMemory(noUrl, LOCAL, EXEMPT, true), false);
});

test("the switch parses only explicit affirmatives", () => {
	for (const on of ["1", "true", "TRUE", "yes", "on", " on "]) {
		assert.equal(remoteFullAccessEnabled(on), true, `${on} should enable`);
	}
	for (const off of [undefined, "", "0", "false", "no", "off", "maybe", "2"]) {
		assert.equal(remoteFullAccessEnabled(off), false, `${off} should not enable`);
	}
});

test("an endpoint that is local-but-unrecognised is scrubbed, not mis-trusted", () => {
	// localhost.localdomain and 0.0.0.0 resolve to this machine but are not in the loopback set,
	// so they must be scrubbed. Accepting them would widen the deny-all; the cost of being
	// conservative is a lost context, which the extension now announces. Never treat an
	// unknown hostname as loopback to make a session work.
	const model = { provider: "ollama", baseUrl: "http://localhost.localdomain:11434" };
	assert.equal(isLoopbackModel(model, LOCAL), false);
	assert.equal(shouldScrubContext(model, LOCAL, EXEMPT), true);
	const bindAll = { provider: "ollama", baseUrl: "http://0.0.0.0:11434" };
	assert.equal(isLoopbackModel(bindAll, LOCAL), false);
	assert.equal(shouldScrubContext(bindAll, LOCAL, EXEMPT), true);
	// Credentials embedded in a URL must not make a remote provider look local.
	const creds = { provider: "ollama", baseUrl: "http://user:pass@127.0.0.1:11434" };
	assert.equal(isLoopbackModel(creds, LOCAL), true, "loopback host is loopback, creds or not");
});

test("an exempted provider over plain http is exempted but never loopback", () => {
	// The exemption is the operator's named decision, so it stands over http; what must never
	// happen is that same URL being classified as local, which would silently drop the scrub.
	const model = { provider: "opencode-go", baseUrl: "http://api.opencode.ai" };
	assert.equal(isLoopbackModel(model, LOCAL), false);
	assert.equal(isExemptedProvider(model, EXEMPT), true);
	assert.equal(mayWriteToMemory(model, LOCAL, EXEMPT, false), false);
	assert.equal(mayWriteToMemory(model, LOCAL, EXEMPT, true), true);
});

test("an empty provider list means nothing is allowlisted", () => {
	assert.equal(parseProviderList("", "lmstudio").size, 0);
	assert.equal(parseProviderList(" a , B ,, ", "x").has("b"), true);
});
