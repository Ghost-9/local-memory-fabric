# Wiki content

These pages are the **source of truth for the wiki**, kept in the main repository because
GitHub's wiki cannot be written over the API: the wiki git remote is only created once a page is
made through the web UI, so an agent can never initialise or update it.

**To enable the wiki:** visit https://github.com/Ghost-9/local-memory-fabric/wiki and create the
first page (any title, e.g. `Home`). That creates the `.wiki.git` remote. From then on, one
command syncs this directory into it:

```sh
git clone https://github.com/Ghost-9/local-memory-fabric.wiki.git /tmp/wiki-clone
cp docs/wiki/*.md /tmp/wiki-clone/
cd /tmp/wiki-clone && git add -A && git commit -m "sync wiki from docs/wiki" && git push
```

Later, that can be a scheduled job; for now it is a documented one-liner rather than a broken
link.

## What the pages cover

| File | Page |
|---|---|
| `01-home.md` | onboarding, the five rules, where things live |
| `02-access-gates.md` | the three gates, why they are three, the loopback rule |
| `03-cache-stability.md` | how not to break the KV / prefix cache, and what is *not* measured |
| `04-roadmap-index.md` | points at the canonical `ROADMAP.md` and issue #14 |
| `05-measurements.md` | every number this repo claims, plus the findings rejected on evidence |

The rules of precedence: the gate (`./test.sh`) decides, then `remote-policy.test.ts` for the
access table, then these pages. If a page disagrees with a test, the test is right.
