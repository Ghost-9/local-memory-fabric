import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const readme = readFileSync(join(root, "README.md"), "utf8");

assert.ok(
  !/\b74\.59\s*%/.test(readme) && !/\b0\.9344\b/.test(readme),
  "README must not repeat the retired benchmark comparison",
);
assert.ok(
  readme.includes("bench/evidence/ordering_ab_results.json"),
  "README must identify the retained retrieval evidence",
);
assert.ok(
  readme.includes("bench/evidence/retired-latency-findings.md"),
  "README must label unreproducible historical latency findings",
);

console.log("  README benchmark claims are traceable or explicitly retired");
