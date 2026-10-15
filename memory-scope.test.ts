import { test } from "node:test";
import assert from "node:assert/strict";
import { projectKey, tierOf, projectNoteMatches, handoffForProject, mayInjectHandoff } from "./memory-scope.ts";

test("THE FIKR LEAK: a handoff from one project is never injected into another", () => {
	// Reproduced from the report: a session on an unrelated project received this project's
	// handoff. That must now be impossible.
	const leaked = {
		cwd: "/Users/batra/.pi/agent/extensions/local-memory-fabric",
		at: new Date().toISOString(),
		next: "build the real-corpus ranking fixture",
	};
	assert.equal(mayInjectHandoff(leaked, "/Users/batra/work/fikr"), false);
	assert.equal(mayInjectHandoff(leaked, "/Users/batra/work/fikr/app"), false);
	assert.equal(mayInjectHandoff(leaked, "/tmp"), false);
	assert.equal(mayInjectHandoff(leaked, "/Users/batra/.pi/agent/extensions/local-memory-fabric"), true);
});

test("a handoff with no recorded project fails closed", () => {
	// Every handoff written before this fix had no cwd, and those are the ones that leaked.
	const anon = { at: new Date().toISOString(), next: "something" };
	assert.equal(mayInjectHandoff(anon, "/anywhere"), false);
	assert.equal(mayInjectHandoff({ cwd: "", at: "x" }, "/anywhere"), false);
	assert.equal(mayInjectHandoff(null, "/anywhere"), false);
});

test("a session with no project gets no project handoff", () => {
	const h = { cwd: "/Users/batra/work/a", next: "x" };
	for (const cwd of [null, undefined, "", "/", ".", "   "]) {
		assert.equal(mayInjectHandoff(h, cwd as string | null), false, `cwd=${JSON.stringify(cwd)}`);
		assert.equal(projectKey(cwd as string), null);
	}
});

test("project keys normalise trailing separators so one directory is one key", () => {
	assert.equal(projectKey("/a/b/"), projectKey("/a/b"));
	assert.equal(projectKey("/a/b///"), "/a/b");
	assert.notEqual(projectKey("/a/b"), projectKey("/a/c"));
});

test("tiers classify the vault the way injection needs", () => {
	assert.equal(tierOf("user/preferences.md"), "global");
	assert.equal(tierOf("invariants/tool-quirks.md"), "global");
	assert.equal(tierOf("projects/agency-dispatch-engine.md"), "project");
	// Corrected: this line used to assert "project", which encoded the bug. Classifying harness
	// knowledge as project-scoped blocked it from most sessions by the census.
	assert.equal(tierOf("pi-harness/19-reasoning-audit.md"), "global");
	assert.equal(tierOf("daily/2026-09-29.md"), "episodic");
	assert.equal(tierOf("inbox/RESUME.md"), "episodic");
});

test("a project note only matches its own project", () => {
	const cwd = "/Users/batra/.pi/agent/extensions/local-memory-fabric";
	assert.equal(projectNoteMatches("projects/local-memory-fabric-extension.md", cwd), true);
	assert.equal(projectNoteMatches("projects/fikr.md", cwd), false);
	assert.equal(projectNoteMatches("projects/preferences.md", cwd), false);
	// A session with no project matches nothing, rather than everything.
	assert.equal(projectNoteMatches("projects/local-memory-fabric-extension.md", null), false);
});

test("the store is keyed per project, so one project cannot overwrite another's handoff", () => {
	const store = {
		"/w/a": { cwd: "/w/a", next: "a's next step" },
		"/w/b": { cwd: "/w/b", next: "b's next step" },
	};
	assert.equal((handoffForProject(store, "/w/a") as any)?.next, "a's next step");
	assert.equal((handoffForProject(store, "/w/b") as any)?.next, "b's next step");
	assert.equal(handoffForProject(store, "/w/c"), null);
	assert.equal(handoffForProject(null, "/w/a"), null);
});

test("REGRESSION: harness knowledge reaches a session in an unrelated project", () => {
	// I classified pi-harness/ as project tier, which blocked it everywhere the cwd did not look
	// like a project - most sessions by the census. The data contradicted it: the injections the
	// agent actually read included pi-harness notes read during entirely different work.
	const cwd = "/Users/batra/Samarohh";
	assert.equal(tierOf("pi-harness/19-reasoning-audit.md"), "global");
	assert.equal(tierOf("evidence/measurements.md"), "global");
	assert.equal(tierOf("agents/ENTRY.md"), "global");
	assert.equal(tierOf("user/preferences.md"), "global");
	assert.equal(tierOf("invariants/tool-quirks.md"), "global");
	// and project notes still do not leak across projects
	assert.equal(tierOf("projects/samarohh.md"), "project");
	assert.equal(projectNoteMatches("projects/samarohh.md", cwd), true);
	assert.equal(projectNoteMatches("projects/fikr.md", cwd), false);
});

test("THE REGRESSION, measured: what a home-directory session can now receive", () => {
	const keep = (ref: string, cwd: string) => {
		const t = tierOf(ref);
		if (t === "global") return true;
		if (t === "project") return projectNoteMatches(ref.split("#")[0], cwd);
		return false;
	};
	const notes = [
		"projects/local-memory-fabric-extension.md",
		"pi-harness/19-reasoning-audit.md",
		"user/preferences.md",
		"ROADMAP.md",
	];
	const kept = notes.filter((n) => keep(n, "/Users/batra"));
	// Before the fix this was 1 of 4; a session in the home directory lost every harness note.
	assert.ok(kept.includes("pi-harness/19-reasoning-audit.md"), "harness notes must survive");
	assert.ok(kept.includes("user/preferences.md"));
	assert.equal(kept.includes("projects/local-memory-fabric-extension.md"), false, "project notes still must not leak");
});
