#!/usr/bin/env python3
"""Local-only, review-first Markdown memory index. Python stdlib + SQLite FTS5 only."""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import re
import sqlite3
import sys
import uuid
from contextlib import closing
from datetime import date, datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any

VERSION = "0.2.0"
DEFAULT_INCLUDE_DIRS = ["agents", "company", "daily", "evidence", "pi-harness", "projects"]
DEFAULT_INCLUDE_FILES = ["README.md", "ROADMAP.md", "STATUS.md", "Welcome.md"]
DEFAULT_EXCLUDED_DIRS = {".git", ".obsidian", ".trash", "archive", "archives", "templates", "apple-notes", "apple_notes", "notes-export", "notes_export"}
STOPWORDS = set("a an and are as at be been being but by can could did do does for from had has have how i in into is it its may might of on or our should so than that the their then there these this those to was we were what when where which who why will with would you your".split())
TOKEN_RE = re.compile(r"[A-Za-z0-9]{3,}")
APPLE_PATH_RE = re.compile(r"(?:apple[ _-]*notes|notes[ _-]*export)", re.I)
SECRET_PATTERNS = [
    ("private_key", re.compile(r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----")),
    ("aws_access_key", re.compile(r"\bAKIA[0-9A-Z]{16}\b")),
    ("github_token", re.compile(r"\b(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})\b")),
    ("slack_token", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b")),
    ("stripe_live_key", re.compile(r"\b(?:sk|rk)_live_[A-Za-z0-9]{16,}\b")),
    ("google_api_key", re.compile(r"\bAIza[0-9A-Za-z_-]{30,}\b")),
    ("openai_api_key", re.compile(r"(?<![A-Za-z0-9])sk-(?:proj-)?[A-Za-z0-9_-]{20,}(?![A-Za-z0-9])")),
    ("jwt", re.compile(r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{8,}\b")),
    ("bearer_credential", re.compile(r"\bBearer\s+[A-Za-z0-9._~+/-]{24,}={0,2}", re.I)),
    ("credential_assignment", re.compile(r"(?i)(?<![A-Za-z0-9])([A-Za-z0-9_-]*(?:api[_-]?key|client[_-]?secret|service[_-]?role[_-]?key|access[_-]?key|private[_-]?key|secret|password|passwd|token)[A-Za-z0-9_-]*)\s*[:=]\s*['\"]?([^\s'\"]{12,})")),
    ("credential_url", re.compile(r"\b[a-z][a-z0-9+.-]*://[^\s/:@]+:[^\s/@]+@", re.I)),
]


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def tokens(text: str) -> set[str]:
    return {m.group(0).lower() for m in TOKEN_RE.finditer(text) if len(m.group(0)) >= 3 and m.group(0).lower() not in STOPWORDS}


def title_for(relpath: str, text: str) -> str:
    for line in text.splitlines()[:80]:
        m = re.match(r"^#\s+(.+?)\s*#*\s*$", line)
        if m:
            return m.group(1).strip()
        m = re.match(r"^title:\s*[\"']?(.+?)[\"']?\s*$", line, re.I)
        if m:
            return m.group(1).strip()
    return Path(relpath).stem.replace("-", " ").replace("_", " ")


def headings_for(text: str) -> str:
    return "\n".join(line.strip() for line in text.splitlines() if re.match(r"^#{1,6}\s+", line))


def scope_for(relpath: str) -> str:
    p = PurePosixPath(relpath)
    parts = p.parts
    if parts and parts[0] == "projects":
        project = parts[1] if len(parts) > 2 else (Path(parts[1]).stem if len(parts) == 2 else "unknown")
        return f"project:{project}"
    if parts and parts[0] == "company":
        return "company"
    if parts and parts[0] == "evidence":
        return "evidence"
    if parts and parts[0] == "daily":
        return "episodic"
    if parts and parts[0] == "pi-harness":
        return "system:pi-harness"
    return "system"


def frontmatter_value(text: str, key: str) -> str | None:
    if not text.startswith("---\n"):
        return None
    end = text.find("\n---", 4)
    if end < 0:
        return None
    m = re.search(rf"(?im)^{re.escape(key)}\s*:\s*[\"']?([^\"'\n#]+)", text[4:end])
    return m.group(1).strip() if m else None


def is_skipped_name(name: str, excluded: set[str]) -> bool:
    return name.startswith(".") or name.casefold() in {x.casefold() for x in excluded} or bool(APPLE_PATH_RE.search(name))


def secret_signature(text: str) -> str | None:
    placeholders = ("placeholder", "redacted", "example", "sample", "change-me", "changeme", "your_", "your-", "insert_", "insert-", "replace_", "replace-", "not-a-real", "not_a_real", "dummy", "fake")
    for kind, pattern in SECRET_PATTERNS:
        match = pattern.search(text)
        if not match:
            continue
        if kind == "credential_assignment":
            value = match.group(2).strip("'\" ").casefold()
            if any(marker in value for marker in placeholders) or re.fullmatch(r"<[^>]+>", value):
                continue
        return kind
    return None


def safe_relpath(value: str) -> str:
    p = PurePosixPath(value.replace("\\", "/"))
    if p.is_absolute() or not p.parts or any(part in {"", ".", ".."} for part in p.parts):
        raise ValueError("source reference is not an allowed relative path")
    return p.as_posix()


class MemoryFabric:
    def __init__(self, home: str | Path | None = None, vault: str | Path | None = None, config: dict[str, Any] | None = None):
        self.home = Path(home or os.environ.get("MEMORY_FABRIC_HOME", Path.home() / ".pi/agent/scratch/memory-fabric")).expanduser()
        self.vault = Path(vault or os.environ.get("MEMORY_FABRIC_VAULT", Path.home() / "system-memory")).expanduser()
        self.db_path = self.home / "index.sqlite3"
        self.proposals_path = self.home / "proposals"
        self._ensure_home()
        defaults = {
            "include_dirs": DEFAULT_INCLUDE_DIRS,
            "include_files": DEFAULT_INCLUDE_FILES,
            "excluded_dirs": sorted(DEFAULT_EXCLUDED_DIRS),
            "local_providers": ["lmstudio", "llama.cpp", "ollama"],
            "max_result_chars": 6000,
            "enforce_export_policy": False,
        }
        config_path = self.home / "config.json"
        if config_path.is_symlink():
            raise RuntimeError("Memory Fabric config must not be a symlink")
        if config is not None:
            defaults.update(config)
        elif config_path.exists():
            loaded = json.loads(config_path.read_text(encoding="utf-8"))
            if not isinstance(loaded, dict):
                raise RuntimeError("Memory Fabric config must be a JSON object")
            defaults.update(loaded)
        else:
            env_providers = os.environ.get("PI_MEMORY_LOCAL_PROVIDERS")
            if env_providers:
                defaults["local_providers"] = [p.strip() for p in env_providers.split(",") if p.strip()]
            fd = os.open(config_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as out:
                json.dump(defaults, out, indent=2)
                out.write("\n")
        env_providers = os.environ.get("PI_MEMORY_LOCAL_PROVIDERS")
        if env_providers:
            defaults["local_providers"] = [p.strip() for p in env_providers.split(",") if p.strip()]
        try:
            config_path.chmod(0o600)
        except OSError:
            pass
        if not all(isinstance(defaults.get(k), list) for k in ("include_dirs", "include_files", "excluded_dirs", "local_providers")):
            raise RuntimeError("Memory Fabric config list fields have invalid types")
        try:
            defaults["include_dirs"] = [safe_relpath(str(x)) for x in defaults["include_dirs"]]
            defaults["include_files"] = [safe_relpath(str(x)) for x in defaults["include_files"]]
        except ValueError as exc:
            raise RuntimeError("Memory Fabric source allowlist must contain relative paths only") from exc
        if any(APPLE_PATH_RE.search(x) for x in defaults["include_dirs"] + defaults["include_files"]):
            raise RuntimeError("Apple Notes paths cannot be added to the source allowlist")
        self.config = defaults

    def _ensure_home(self) -> None:
        if self.home.is_symlink():
            raise RuntimeError("Memory Fabric state directory must not be a symlink")
        self.home.mkdir(parents=True, exist_ok=True, mode=0o700)
        if self.home.is_symlink():
            raise RuntimeError("Memory Fabric state directory must not be a symlink")
        try:
            self.home.chmod(0o700)
        except OSError:
            pass

    def _connect(self) -> sqlite3.Connection:
        if self.db_path.is_symlink():
            raise RuntimeError("Memory index must not be a symlink")
        if not self.db_path.exists():
            raise RuntimeError("Memory index is not initialized; run the explicit 'index' command first")
        con = sqlite3.connect(self.db_path, timeout=5.0)
        con.row_factory = sqlite3.Row
        con.execute("PRAGMA busy_timeout=5000")
        con.execute("PRAGMA foreign_keys=ON")
        return con

    def _init_db(self, con: sqlite3.Connection) -> None:
        con.executescript("""
            CREATE TABLE IF NOT EXISTS documents (
                id INTEGER PRIMARY KEY,
                relpath TEXT NOT NULL UNIQUE,
                title TEXT NOT NULL,
                scope TEXT NOT NULL,
                classification TEXT NOT NULL,
                source_status TEXT NOT NULL DEFAULT 'unspecified',
                valid_from TEXT,
                valid_to TEXT,
                superseded_by TEXT,
                provider_policy TEXT NOT NULL DEFAULT 'loopback_allowlist_only',
                export_policy TEXT NOT NULL DEFAULT 'deny_remote',
                body TEXT NOT NULL,
                content_hash TEXT NOT NULL,
                mtime_ns INTEGER NOT NULL,
                indexed_at TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
                relpath UNINDEXED, title, headings, body,
                tokenize='porter unicode61'
            );
            CREATE TABLE IF NOT EXISTS doc_terms (
                relpath TEXT NOT NULL,
                term TEXT NOT NULL,
                PRIMARY KEY(relpath, term)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS doc_terms_by_term ON doc_terms(term);
            CREATE TABLE IF NOT EXISTS term_df (term TEXT PRIMARY KEY, df INTEGER NOT NULL) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
        """)
        version = con.execute("SELECT value FROM metadata WHERE key='schema_version'").fetchone()
        migrated = version is None or version[0] != "2"
        columns = {row[1] for row in con.execute("PRAGMA table_info(documents)")}
        migrations = {
            "source_status": "TEXT NOT NULL DEFAULT 'unspecified'",
            "valid_from": "TEXT",
            "valid_to": "TEXT",
            "superseded_by": "TEXT",
            "provider_policy": "TEXT NOT NULL DEFAULT 'loopback_allowlist_only'",
            "export_policy": "TEXT NOT NULL DEFAULT 'deny_remote'",
        }
        for name, declaration in migrations.items():
            if name not in columns:
                con.execute(f"ALTER TABLE documents ADD COLUMN {name} {declaration}")
                migrated = True
        con.execute("INSERT INTO metadata(key,value) VALUES('schema_version','2') ON CONFLICT(key) DO UPDATE SET value=excluded.value")
        return migrated

    def _iter_sources(self) -> list[Path]:
        root = self.vault
        try:
            root_st = root.lstat()
        except OSError:
            raise RuntimeError("Configured vault root is unavailable")
        if root.is_symlink() or not root_st:
            raise RuntimeError("Configured vault root must not be a symlink")
        allowed_dirs = self.config.get("include_dirs", DEFAULT_INCLUDE_DIRS)
        allowed_files = set(self.config.get("include_files", DEFAULT_INCLUDE_FILES))
        excluded = set(self.config.get("excluded_dirs", DEFAULT_EXCLUDED_DIRS))
        found: list[Path] = []

        def walk(directory: Path) -> None:
            try:
                entries = list(os.scandir(directory))
            except OSError:
                return
            for entry in entries:
                # Name policy is checked before stat/open; Apple Notes paths are never traversed.
                if is_skipped_name(entry.name, excluded):
                    continue
                path = Path(entry.path)
                try:
                    if entry.is_symlink():
                        continue
                    if entry.is_dir(follow_symlinks=False):
                        walk(path)
                    elif entry.is_file(follow_symlinks=False) and path.suffix.lower() == ".md":
                        found.append(path)
                except OSError:
                    continue

        for rel in allowed_dirs:
            if is_skipped_name(str(rel), excluded):
                continue
            candidate = root / rel
            try:
                if candidate.is_symlink() or not candidate.is_dir():
                    continue
            except OSError:
                continue
            walk(candidate)
        for name in allowed_files:
            if is_skipped_name(name, excluded):
                continue
            candidate = root / name
            try:
                if not candidate.is_symlink() and candidate.is_file() and candidate.suffix.lower() == ".md":
                    found.append(candidate)
            except OSError:
                continue
        return sorted(set(found))

    def index(self, force: bool = False) -> dict[str, Any]:
        self._ensure_home()
        if self.db_path.is_symlink():
            raise RuntimeError("Memory index must not be a symlink")
        con = sqlite3.connect(self.db_path, timeout=5.0)
        con.execute("PRAGMA journal_mode=WAL")
        con.execute("PRAGMA busy_timeout=5000")
        schema_migrated = self._init_db(con)
        con.commit()
        try:
            existing = {row[0]: row[1] for row in con.execute("SELECT relpath,content_hash FROM documents")}
            found: set[str] = set()
            added = updated = unchanged = excluded_sensitive = 0
            con.execute("BEGIN IMMEDIATE")
            for path in self._iter_sources():
                try:
                    resolved = path.resolve(strict=True)
                    resolved.relative_to(self.vault.resolve(strict=True))
                    if path.is_symlink() or not path.is_file():
                        continue
                    text = path.read_text(encoding="utf-8")
                    # os.stat, not Path.stat: the latter only accepted follow_symlinks from
                    # Python 3.10, and macOS ships 3.9 as /usr/bin/python3. A missing or
                    # relinked Homebrew python3 therefore used to break indexing outright.
                    st = os.stat(path, follow_symlinks=False)
                except (OSError, UnicodeError, ValueError):
                    continue
                rel = path.relative_to(self.vault).as_posix()
                found.add(rel)
                classification = (frontmatter_value(text, "classification") or "local_only").lower()
                if classification in {"secret", "restricted", "excluded"} or secret_signature(text):
                    excluded_sensitive += 1
                    if rel in existing:
                        con.execute("DELETE FROM documents_fts WHERE relpath=?", (rel,))
                        con.execute("DELETE FROM documents WHERE relpath=?", (rel,))
                        con.execute("DELETE FROM doc_terms WHERE relpath=?", (rel,))
                    continue
                digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
                if not force and not schema_migrated and existing.get(rel) == digest:
                    unchanged += 1
                    continue
                scope = scope_for(rel)
                title = title_for(rel, text)
                headings = headings_for(text)
                source_status = (frontmatter_value(text, "status") or "unspecified").strip().lower()
                valid_from = frontmatter_value(text, "valid_from")
                valid_to = frontmatter_value(text, "valid_to")
                superseded_by = frontmatter_value(text, "superseded_by")
                if rel in existing:
                    updated += 1
                    con.execute("DELETE FROM documents_fts WHERE relpath=?", (rel,))
                    con.execute("DELETE FROM doc_terms WHERE relpath=?", (rel,))
                else:
                    added += 1
                con.execute("""INSERT INTO documents(relpath,title,scope,classification,source_status,valid_from,valid_to,superseded_by,
                    provider_policy,export_policy,body,content_hash,mtime_ns,indexed_at)
                    VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(relpath) DO UPDATE SET title=excluded.title,scope=excluded.scope,
                    classification=excluded.classification,source_status=excluded.source_status,valid_from=excluded.valid_from,
                    valid_to=excluded.valid_to,superseded_by=excluded.superseded_by,provider_policy=excluded.provider_policy,
                    export_policy=excluded.export_policy,body=excluded.body,content_hash=excluded.content_hash,
                    mtime_ns=excluded.mtime_ns,indexed_at=excluded.indexed_at""",
                    (rel, title, scope, classification, source_status, valid_from, valid_to, superseded_by,
                     "loopback_allowlist_only", "deny_remote", text, digest, st.st_mtime_ns, utc_now()))
                con.execute("INSERT INTO documents_fts(relpath,title,headings,body) VALUES(?,?,?,?)", (rel, title, headings, text))
                con.executemany("INSERT OR IGNORE INTO doc_terms(relpath,term) VALUES(?,?)", ((rel, term) for term in tokens(title + "\n" + headings + "\n" + text)))
            removed = 0
            for rel in set(existing) - found:
                con.execute("DELETE FROM documents_fts WHERE relpath=?", (rel,))
                con.execute("DELETE FROM documents WHERE relpath=?", (rel,))
                con.execute("DELETE FROM doc_terms WHERE relpath=?", (rel,))
                removed += 1
            con.execute("DELETE FROM term_df")
            con.execute("INSERT INTO term_df(term,df) SELECT term,COUNT(*) FROM doc_terms GROUP BY term")
            con.execute("INSERT INTO metadata(key,value) VALUES('last_indexed_at',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (utc_now(),))
            con.execute("INSERT INTO metadata(key,value) VALUES('vault_path',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value", (str(self.vault.resolve()),))
            con.commit()
        except Exception:
            con.rollback()
            raise
        finally:
            con.close()
        try:
            self.db_path.chmod(0o600)
        except OSError:
            pass
        with closing(sqlite3.connect(self.db_path)) as stats:
            document_count = int(stats.execute("SELECT COUNT(*) FROM documents").fetchone()[0])
        return {"ok": True, "version": VERSION, "added": added, "updated": updated, "unchanged": unchanged,
                "removed_from_index": removed, "excluded_sensitive": excluded_sensitive, "documents": document_count}

    def is_local_provider(self, provider: str | None) -> bool:
        if not provider:
            return False
        allowed = {str(x).strip().casefold() for x in self.config.get("local_providers", [])}
        return provider.strip().casefold() in allowed

    def status(self) -> dict[str, Any]:
        if not self.db_path.exists():
            return {"ok": True, "version": VERSION, "indexed": False, "documents": 0, "message": "Run the explicit index command to initialize."}
        with closing(self._connect()) as con:
            count = con.execute("SELECT COUNT(*) FROM documents").fetchone()[0]
            last = con.execute("SELECT value FROM metadata WHERE key='last_indexed_at'").fetchone()
        return {"ok": True, "version": VERSION, "indexed": True, "documents": count,
                "last_indexed_at": last[0] if last else None}

    def search(self, query: str, scope: str = "all", limit: int = 5, provider: str | None = None, as_of: str | None = None,
                loopback: bool | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Memory content is available only to an explicitly allowed local model provider."}
        if not query or not query.strip():
            return {"ok": False, "error": "query is required"}
        try:
            as_of_date = date.fromisoformat(as_of) if as_of else None
        except ValueError:
            return {"ok": False, "error": "as_of must be an ISO date (YYYY-MM-DD)"}
        terms = tokens(query)
        if not terms:
            return {"ok": True, "results": [], "abstention": "no searchable terms"}
        with closing(self._connect()) as con:
            n_docs = int(con.execute("SELECT COUNT(*) FROM documents").fetchone()[0])
            # Document frequencies for this query's own tokens only. Reading the whole term_df
            # table was the only super-linear step in the hot path.
            probe = list(terms)[:64]
            if probe:
                marks = ",".join("?" * len(probe))
                df_rows = con.execute(f"SELECT term,df FROM term_df WHERE term IN ({marks})", probe).fetchall()
            else:
                df_rows = []
            df = {r[0]: int(r[1]) for r in df_rows}
            max_df = max(2, int(n_docs * 0.25))
            indexed = [t for t in terms if df.get(t, 0) > 0]
            useful = [t for t in indexed if df[t] <= max_df]
            useful.sort(key=lambda t: (df[t], -len(t), t))
            useful = useful[:16]
            # Every token is indexed but all are too common to discriminate. Silence here reads as
            # "the vault has nothing on this"; answer over the query's own tokens and label it.
            low_discrimination = False
            if not useful:
                if not indexed:
                    return {"ok": True, "results": [], "shown": 0, "total_matches": 0,
                            "low_discrimination": False,
                            "abstention": "no indexed source contains these terms"}
                low_discrimination = True
                useful = sorted(indexed, key=lambda t: (df[t], -len(t), t))[:16]
            match_query = " OR ".join('"' + t.replace('"', '') + '"' for t in useful)
            where = ["documents_fts MATCH ?"]
            params: list[Any] = [match_query]
            if scope not in {"", "all", None}:
                where.append("d.scope=?")
                params.append(scope)
            # Total matches for the same predicate, so a caller can tell "no match" from "no data".
            total_matches = int(con.execute(
                f"""SELECT COUNT(*) FROM documents_fts JOIN documents d ON d.relpath=documents_fts.relpath
                    WHERE {' AND '.join(where)}""", params).fetchone()[0])
            page_params = list(params) + [max(1, min(int(limit), 20)) * 10]
            rows = con.execute(f"""SELECT d.relpath,d.title,d.scope,d.classification,d.source_status,d.valid_from,d.valid_to,d.superseded_by,
                d.provider_policy,d.export_policy,d.body,d.indexed_at,bm25(documents_fts,0.0,4.0,1.5,1.0) AS rank
                FROM documents_fts JOIN documents d ON d.relpath=documents_fts.relpath
                WHERE {' AND '.join(where)} ORDER BY rank LIMIT ?""", page_params).fetchall()
            # One lookup for every candidate instead of one query per candidate.
            terms_by_relpath: dict[str, set[str]] = {}
            if rows:
                rps = [r["relpath"] for r in rows]
                marks = ",".join("?" * len(rps))
                for rel, term in con.execute(f"SELECT relpath,term FROM doc_terms WHERE relpath IN ({marks})", rps):
                    terms_by_relpath.setdefault(rel, set()).add(term)
            results = []
            excluded_temporal = 0
            temporal_without_as_of = False
            # Per-note export policy. Off by default so the operator's blanket read
            # exemption keeps working; when on, a note must be marked allow_remote
            # before a non-loopback caller may see it, and the count is reported.
            withheld = 0
            enforcing_export = bool(self.config.get("enforce_export_policy", False))
            if enforcing_export and loopback is not True:
                for row in rows:
                    if str(row["export_policy"]) != "allow_remote":
                        withheld += 1
                rows = [r for r in rows if str(r["export_policy"]) == "allow_remote"]
            for row in rows:
                try:
                    valid_from = date.fromisoformat(row["valid_from"]) if row["valid_from"] else None
                    valid_to = date.fromisoformat(row["valid_to"]) if row["valid_to"] else None
                except ValueError:
                    excluded_temporal += 1
                    continue
                status = str(row["source_status"]).strip().lower()
                if not as_of_date and (valid_from or valid_to):
                    temporal_without_as_of = True
                if as_of_date and ((valid_from and as_of_date < valid_from) or (valid_to and as_of_date > valid_to)):
                    continue
                if status in {"superseded", "archived", "deprecated", "historical"} and (not as_of_date or not valid_to or as_of_date > valid_to):
                    continue
                doc_terms = terms_by_relpath.get(row["relpath"], set())
                matched = [t for t in useful if t in doc_terms]
                distinctive_single = any(len(t) >= 9 and df.get(t) == 1 for t in matched)
                if len(matched) < 2 and not distinctive_single:
                    continue
                body = row["body"]
                # Return a bounded extract near the first matched term, not an unbounded document.
                lower = body.casefold()
                positions = [lower.find(t) for t in matched if lower.find(t) >= 0]
                start = max(0, min(positions, default=0) - 240)
                excerpt = body[start:start + 1000].strip()
                if start:
                    excerpt = "…" + excerpt
                if start + 1000 < len(body):
                    excerpt += "…"
                results.append({"source_ref": row["relpath"], "title": row["title"], "scope": row["scope"],
                    "classification": row["classification"], "source_status": row["source_status"],
                    "valid_from": row["valid_from"], "valid_to": row["valid_to"], "superseded_by": row["superseded_by"],
                    "provider_policy": row["provider_policy"], "export_policy": row["export_policy"],
                    "excerpt": excerpt, "matched_terms": matched,
                    "bm25_rank": round(float(row["rank"]), 6), "indexed_at": row["indexed_at"]})
                if len(results) >= max(1, min(int(limit), 20)):
                    break
        warning = None
        for i, left in enumerate(results):
            for right in results[i + 1:]:
                if left["scope"] != right["scope"] or left["source_status"] == right["source_status"]:
                    continue
                left_start, left_end = left["valid_from"] or "0001-01-01", left["valid_to"] or "9999-12-31"
                right_start, right_end = right["valid_from"] or "0001-01-01", right["valid_to"] or "9999-12-31"
                if left_start <= right_end and right_start <= left_end:
                    warning = "Overlapping source lifecycle records; inspect provenance and do not auto-resolve the conflict."
                    break
            if warning:
                break
        if excluded_temporal:
            warning = "Some candidates had invalid temporal metadata and were excluded."
        elif temporal_without_as_of and not as_of:
            warning = "No as_of date supplied; validity bounds were not evaluated against the host clock."
        if low_discrimination and not warning:
            warning = "Query terms were too common in this index to discriminate; these are unfiltered candidates, not a ranked answer."
        shown = len(results)
        if shown:
            abstention = None
        elif total_matches == 0:
            abstention = "no indexed source matches these terms"
        else:
            abstention = "matches exist but none passed the overlap or temporal gate; inspect total_matches and the date"
        return {"ok": True, "results": results, "shown": shown, "total_matches": total_matches,
                "low_discrimination": low_discrimination, "abstention": abstention,
                "temporal_warning": warning, "as_of": as_of, "withheld_export": withheld,
                "local_only": True, "note": "Lexical candidates are not verified facts; inspect cited source passages."}

    def context(self, project: str | None, query: str, provider: str | None, token_budget: int = 1200, as_of: str | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Memory content is available only to an explicitly allowed local model provider."}
        scope = f"project:{project}" if project else "all"
        hits = self.search(query, scope=scope, limit=10, provider=provider, as_of=as_of)
        if not hits.get("ok"):
            return hits
        budget = max(100, min(int(token_budget), 6000))
        char_budget = budget * 4  # Conservative transport cap; not a tokenizer measurement.
        sections = []
        used = 0
        for item in hits["results"]:
            temporal = f"; valid {item['valid_from'] or 'unspecified'} to {item['valid_to'] or 'open'}"
            block = (f"### {item['title']} — {item['source_ref']}\n"
                     f"status={item['source_status']}; class={item['classification']}{temporal}; "
                     f"provider={item['provider_policy']}; export={item['export_policy']}\n{item['excerpt']}\n")
            if used + len(block) > char_budget:
                remaining = char_budget - used
                if remaining > 100:
                    partial = block[:remaining]
                    sections.append(partial)
                    used += len(partial)
                break
            sections.append(block)
            used += len(block)
        return {"ok": True, "project": project, "source_count": len(sections), "estimated_characters": used,
                "char_budget": char_budget, "context": "\n".join(sections),
                "abstention": hits.get("abstention"), "as_of": hits.get("as_of"),
                "temporal_warning": hits.get("temporal_warning"),
                "instruction": "Treat excerpts as source evidence, not verified conclusions; cite source_ref and distinguish inference."}

    def _indexed_doc(self, ref: str, con: sqlite3.Connection) -> sqlite3.Row:
        rel = safe_relpath(ref)
        if APPLE_PATH_RE.search(rel) or self.vault.is_symlink():
            raise ValueError("source path is excluded or unsafe")
        row = con.execute("SELECT * FROM documents WHERE relpath=?", (rel,)).fetchone()
        if row is None:
            raise ValueError("source reference is not in the approved index")
        # Revalidate every path component to prevent symlink replacement after indexing.
        path = self.vault
        for part in PurePosixPath(rel).parts:
            path = path / part
            try:
                if path.is_symlink():
                    raise ValueError("source path is no longer safe")
            except OSError:
                raise ValueError("source path is no longer available")
        try:
            path.resolve(strict=True).relative_to(self.vault.resolve(strict=True))
        except (OSError, ValueError):
            raise ValueError("source path is no longer inside the approved vault")
        return row

    def read(self, ref: str, provider: str | None = None, heading: str | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Memory content is available only to an explicitly allowed local model provider."}
        try:
            with closing(self._connect()) as con:
                row = self._indexed_doc(ref, con)
                body = row["body"]
                try:
                    current = (self.vault / safe_relpath(ref)).read_text(encoding="utf-8")
                    stale = hashlib.sha256(current.encode("utf-8")).hexdigest() != row["content_hash"]
                except (OSError, UnicodeError):
                    stale = True
                if stale:
                    return {"ok": False, "stale": True, "message": "Source changed or disappeared; run explicit reindex before reading."}
        except ValueError as exc:
            return {"ok": False, "error": str(exc)}
        if heading:
            lines = body.splitlines()
            start = next((i for i, line in enumerate(lines) if re.match(r"^#{1,6}\s+" + re.escape(heading) + r"\s*#*\s*$", line.strip(), re.I)), None)
            if start is None:
                return {"ok": False, "error": "requested heading was not found"}
            level = len(lines[start]) - len(lines[start].lstrip("#"))
            end = len(lines)
            for i in range(start + 1, len(lines)):
                m = re.match(r"^(#{1,6})\s+", lines[i])
                if m and len(m.group(1)) <= level:
                    end = i
                    break
            body = "\n".join(lines[start:end])
        cap = max(500, min(int(self.config.get("max_result_chars", 6000)), 12000))
        return {"ok": True, "source_ref": row["relpath"], "title": row["title"], "scope": row["scope"],
                "classification": row["classification"], "source_status": row["source_status"],
                "valid_from": row["valid_from"], "valid_to": row["valid_to"], "superseded_by": row["superseded_by"],
                "provider_policy": row["provider_policy"], "export_policy": row["export_policy"],
                "content": body[:cap], "truncated": len(body) > cap, "local_only": True}

    def propose(self, project: str, target: str, section: str, text: str, source_refs: list[str] | None = None,
                provider: str | None = None) -> dict[str, Any]:
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,79}", project or ""):
            return {"ok": False, "error": "invalid project identifier"}
        try:
            rel = safe_relpath(target)
        except ValueError as exc:
            return {"ok": False, "error": str(exc)}
        if not (rel == f"projects/{project}.md" or rel.startswith(f"projects/{project}/")):
            return {"ok": False, "error": "proposal target must be inside the selected project"}
        if not section or len(section) > 120 or not text.strip() or len(text) > 20000:
            return {"ok": False, "error": "section and bounded non-empty proposal text are required"}
        secret_kind = secret_signature(text)
        if secret_kind:
            return {"ok": False, "error": f"proposal rejected by local secret filter ({secret_kind}); no content was stored"}
        refs: list[str] = []
        try:
            with closing(self._connect()) as con:
                target_row = self._indexed_doc(rel, con)
                for ref in source_refs or []:
                    source = self._indexed_doc(ref, con)
                    refs.append(source["relpath"])
                base = target_row["body"]
        except (ValueError, RuntimeError) as exc:
            return {"ok": False, "error": str(exc)}
        lines = base.splitlines(keepends=True)
        start = next((i for i, line in enumerate(lines) if re.match(r"^#{1,6}\s+" + re.escape(section) + r"\s*#*\s*$", line.strip(), re.I)), None)
        if start is None:
            return {"ok": False, "error": "proposal section heading must already exist"}
        level = len(lines[start]) - len(lines[start].lstrip("#"))
        end = len(lines)
        for i in range(start + 1, len(lines)):
            m = re.match(r"^(#{1,6})\s+", lines[i])
            if m and len(m.group(1)) <= level:
                end = i
                break
        addition = text.rstrip() + "\n"
        insert_at = end
        while insert_at > start + 1 and not lines[insert_at - 1].strip():
            insert_at -= 1
        proposal_text = "".join(lines[:insert_at]) + ("\n" if insert_at and lines[insert_at - 1].strip() else "") + addition + ("\n" if end < len(lines) else "") + "".join(lines[end:])
        diff = "".join(difflib.unified_diff(base.splitlines(keepends=True), proposal_text.splitlines(keepends=True),
                                              fromfile=f"a/{rel}", tofile=f"proposal/{rel}"))
        proposal_id = uuid.uuid4().hex[:12]
        record = {"id": proposal_id, "created_at": utc_now(), "status": "proposed_review_required",
                  "classification": "local_only", "provider_policy": "loopback_allowlist_only", "export_policy": "deny_remote",
                  "project": project, "target": rel, "section": section, "source_refs": refs,
                  "proposal_text": text, "diff": diff}
        self.proposals_path.mkdir(parents=True, exist_ok=True, mode=0o700)
        if self.proposals_path.is_symlink():
            return {"ok": False, "error": "Proposal directory must not be a symlink"}
        try:
            self.proposals_path.chmod(0o700)
        except OSError:
            pass
        destination = self.proposals_path / f"{proposal_id}.json"
        fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as out:
            json.dump(record, out, ensure_ascii=False, indent=2)
            out.write("\n")
        # The model must not receive a diff containing existing vault text unless it is local.
        if self.is_local_provider(provider):
            return {"ok": True, "proposal_id": proposal_id, "status": record["status"], "diff": diff,
                    "writeback": "none; proposal is stored outside the vault for human review"}
        return {"ok": True, "proposal_id": proposal_id, "status": record["status"],
                "writeback": "none; proposal is stored outside the vault for human review"}

    def proposal_show(self, proposal_id: str, provider: str | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Proposal contents are available only to an explicitly allowed local model provider."}
        if not re.fullmatch(r"[a-f0-9]{12}", proposal_id or ""):
            return {"ok": False, "error": "invalid proposal id"}
        path = self.proposals_path / f"{proposal_id}.json"
        if path.is_symlink():
            return {"ok": False, "error": "proposal path is unsafe"}
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {"ok": False, "error": "proposal not found"}
        return {"ok": True, "proposal_id": proposal_id, "status": record.get("status"),
                "target": record.get("target"), "diff": record.get("diff", "")}

    def proposal_list(self) -> dict[str, Any]:
        if not self.proposals_path.exists():
            return {"ok": True, "proposals": []}
        items = []
        if self.proposals_path.is_symlink():
            return {"ok": False, "error": "proposal directory is unsafe"}
        for path in sorted(self.proposals_path.glob("[a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9][a-f0-9].json")):
            if path.is_symlink():
                continue
            try:
                record = json.loads(path.read_text(encoding="utf-8"))
                items.append({"proposal_id": record["id"], "created_at": record["created_at"], "status": record["status"], "project": record["project"]})
            except (OSError, ValueError, KeyError):
                continue
        return {"ok": True, "proposals": items}


def _emit(value: dict[str, Any]) -> None:
    print(json.dumps(value, ensure_ascii=False, separators=(",", ":")))


def dispatch_request(fabric: MemoryFabric, req: dict[str, Any]) -> dict[str, Any]:
    op = req.get("op")
    if op == "status": return fabric.status()
    if op == "index": return fabric.index(bool(req.get("force", False)))
    if op == "search": return fabric.search(str(req.get("query", "")), str(req.get("scope", "all")), int(req.get("limit", 5)), req.get("provider"), req.get("as_of"))
    if op == "context": return fabric.context(req.get("project"), str(req.get("query", "")), req.get("provider"), int(req.get("token_budget", 1200)), req.get("as_of"))
    if op == "read": return fabric.read(str(req.get("source_ref", "")), req.get("provider"), req.get("heading"))
    if op == "propose": return fabric.propose(str(req.get("project", "")), str(req.get("target", "")), str(req.get("section", "")), str(req.get("text", "")), req.get("source_refs", []), req.get("provider"))
    if op == "proposal-show": return fabric.proposal_show(str(req.get("proposal_id", "")), req.get("provider"))
    if op == "proposal-list": return fabric.proposal_list()
    return {"ok": False, "error": "unsupported memory operation"}


def exit_code_for(result: dict[str, Any]) -> int:
    """Exit-code taxonomy for the CLI, mirroring the Rust engine.

    0 returned content, 1 nothing to return, 2 refused by policy, 3 error. A caller
    should not have to parse prose to tell "no match" from "the engine broke".
    """
    if result.get("blocked") is not None:
        return 2
    if not result.get("ok"):
        return 3
    if any(key in result for key in ("results", "abstention", "content")):
        results = result.get("results")
        content = result.get("content")
        if not (isinstance(results, list) and results) and not (isinstance(content, str) and content):
            return 1
    return 0


def classify_storage_error(exc: Exception) -> str:
    """Say whether a storage failure is transient contention or genuine breakage.
    Retrying helps only the first, so the caller has to be able to tell them apart.
    """
    if isinstance(exc, sqlite3.OperationalError):
        detail = str(exc).lower()
        if "locked" in detail or "busy" in detail:
            return f"local memory index is busy; retry shortly [{exc}]"
    return str(exc)


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if argv == ["--json-stdin"]:
        try:
            raw = sys.stdin.read(131073)
            if len(raw) > 131072:
                _emit({"ok": False, "error": "request exceeds size limit"})
                return 3
            request = json.loads(raw)
            if not isinstance(request, dict):
                _emit({"ok": False, "error": "request must be a JSON object"})
                return 3
            fabric = MemoryFabric()
            result = dispatch_request(fabric, request)
            _emit(result)
            return exit_code_for(result)
        except sqlite3.OperationalError as exc:
            _emit({"ok": False, "error": classify_storage_error(exc)})
            return 3
        except (OSError, ValueError, TypeError, json.JSONDecodeError, sqlite3.Error, RuntimeError) as exc:
            _emit({"ok": False, "error": str(exc)})
            return 3
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--vault", default=None)
    parser.add_argument("--home", default=None)
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("status")
    p_index = sub.add_parser("index")
    p_index.add_argument("--force", action="store_true")
    p_search = sub.add_parser("search")
    p_search.add_argument("query")
    p_search.add_argument("--scope", default="all")
    p_search.add_argument("--limit", type=int, default=5)
    p_search.add_argument("--provider", default="")
    p_search.add_argument("--as-of")
    p_context = sub.add_parser("context")
    p_context.add_argument("query")
    p_context.add_argument("--project")
    p_context.add_argument("--token-budget", type=int, default=1200)
    p_context.add_argument("--provider", default="")
    p_context.add_argument("--as-of")
    p_read = sub.add_parser("read")
    p_read.add_argument("source_ref")
    p_read.add_argument("--heading")
    p_read.add_argument("--provider", default="")
    p_propose = sub.add_parser("propose")
    p_propose.add_argument("--project", required=True)
    p_propose.add_argument("--target", required=True)
    p_propose.add_argument("--section", required=True)
    p_propose.add_argument("--text", required=True)
    p_propose.add_argument("--source", action="append", default=[])
    p_propose.add_argument("--provider", default="")
    p_show = sub.add_parser("proposal-show")
    p_show.add_argument("proposal_id")
    p_show.add_argument("--provider", default="")
    sub.add_parser("proposal-list")
    args = parser.parse_args(argv)
    try:
        fabric = MemoryFabric(home=args.home, vault=args.vault)
    except (OSError, ValueError, TypeError, RuntimeError, sqlite3.Error) as exc:
        _emit({"ok": False, "error": str(exc)})
        return 3
    try:
        if args.command == "status": result = fabric.status()
        elif args.command == "index": result = fabric.index(force=args.force)
        elif args.command == "search": result = fabric.search(args.query, args.scope, args.limit, args.provider, args.as_of)
        elif args.command == "context": result = fabric.context(args.project, args.query, args.provider, args.token_budget, args.as_of)
        elif args.command == "read": result = fabric.read(args.source_ref, args.provider, args.heading)
        elif args.command == "propose": result = fabric.propose(args.project, args.target, args.section, args.text, args.source, args.provider)
        elif args.command == "proposal-show": result = fabric.proposal_show(args.proposal_id, args.provider)
        elif args.command == "proposal-list": result = fabric.proposal_list()
        else: raise AssertionError("unhandled command")
        _emit(result)
        return exit_code_for(result)
    except sqlite3.OperationalError as exc:
        _emit({"ok": False, "error": classify_storage_error(exc)})
        return 3
    except (OSError, sqlite3.Error, RuntimeError, ValueError) as exc:
        _emit({"ok": False, "error": str(exc)})
        return 3


if __name__ == "__main__":
    raise SystemExit(main())
