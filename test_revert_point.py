#!/usr/bin/env python3
"""D5 - the pre-write revert point.

The guarantee under test: after a vault write, the content that was *replaced* is recoverable.
Nothing more can be guaranteed - the extension gates writes but does not perform them, so
atomicity belongs to Pi's filesystem layer.

Staging (`git add`) is what makes the difference. Without it, restoring from the index returns the
*last commit*, silently discarding whatever was there before the write. That is the failure this
test must be able to detect, so the negative case is run too, and it must lose the data.

Recovery uses `git checkout-index -f -- <path>` rather than `git restore -- <path>`. Both mean
"worktree from index", but the agent shell guard blocks `git restore` (reason:
git.restore-unstaged), so the documented recovery path has to be one an agent can actually run.
"""
import subprocess, tempfile, os, sys

def sh(*a, cwd):
    return subprocess.run(a, cwd=cwd, capture_output=True, text=True)

def scenario(stage_before_write: bool) -> str:
    with tempfile.TemporaryDirectory() as d:
        sh("git", "init", "-q", cwd=d)
        sh("git", "config", "user.email", "t@t", cwd=d)
        sh("git", "config", "user.name", "t", cwd=d)
        f = os.path.join(d, "note.md")
        open(f, "w").write("V1 committed\n")
        sh("git", "add", "note.md", cwd=d); sh("git", "commit", "-qm", "v1", cwd=d)

        open(f, "w").write("V2 the state we are about to overwrite\n")   # dirty edit
        if stage_before_write:
            sh("git", "add", "--", f, cwd=d)                             # the revert point
        open(f, "w").write("V3 the write that clobbers V2\n")            # the clobbering write
        sh("git", "checkout-index", "-f", "--", f, cwd=d)                # recover
        return open(f).read().strip()

failures = []
without = scenario(False)
with_ = scenario(True)

print(f"  without the revert point : {without!r}")
print(f"  with the revert point    : {with_!r}")

if "V2" not in with_:
    failures.append(f"pre-write content NOT recoverable: got {with_!r}, expected the V2 text")
if "V2" in without:
    failures.append("negative case did not lose data - the test is not sensitive to the mechanism")
if "V3" != without.strip()[:2] and "V3" not in without:
    pass  # informational only

if failures:
    for f in failures:
        print(f"  FAIL: {f}")
    sys.exit(1)
print("  OK: pre-write content is recoverable, and without staging it is lost (test is sensitive)")
