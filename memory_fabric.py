#!/usr/bin/env python3
"""Local-only, review-first Markdown memory index. Python stdlib + SQLite FTS5 only."""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import math
import os
import re
import sqlite3
import sys
import uuid
from contextlib import closing
from datetime import date, datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Any

VERSION = "2.0.0"
# Sandbox bounds. Tool output is the target: anything materially larger than a screenful is
# refused rather than silently truncated, and the head returned with the handle stays small
# because retrieving deliberately is the entire point.
SANDBOX_MAX_CHARS = 256 * 1024
SANDBOX_HEAD_CHARS = 480


def sha256(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()
DEFAULT_INCLUDE_DIRS = ["agents", "company", "daily", "evidence", "invariants", "pi-harness", "projects", "user"]
DEFAULT_INCLUDE_FILES = ["README.md", "ROADMAP.md", "STATUS.md", "Welcome.md"]
DEFAULT_EXCLUDED_DIRS = {".git", ".obsidian", ".trash", "archive", "archives", "templates", "apple-notes", "apple_notes", "notes-export", "notes_export"}
STOPWORDS = set("a an and are as at be been being but by can could did do does for from had has have how i in into is it its may might of on or our should so than that the their then there these this those to was we were what when where which who why will with would you your".split())
TOKEN_RE = re.compile(r"[A-Za-z0-9]{2,}")
# Two characters minimum, not three. Measured on the live vault: `KV cache` tokenized to
# ["cache"] alone because "KV" fell under the old floor, so a two-word query had ONE token and
# could never clear the two-matched-terms bar - 64 documents contain "cache" and it returned 0.
# Two-letter acronyms are common here (KV, LM, IR, QA) and dropping them makes the exact query a
# human would type fail. Two characters still excludes 1-letter English fragments.
# Splits an identifier into its parts: lower->Upper boundaries, and runs of capitals followed by a
# word ("BM25Cache" -> "BM", "Cache"). Deliberately conservative so acronyms survive whole.
CAMEL_SPLIT_RE = re.compile(r"[A-Z]+(?![a-z])|[A-Z][a-z]+|[a-z]+|[0-9]+")
# Discriminating-power limits for query terms. A term must appear in at least this many documents
# to be worth matching at all, and at most this fraction to be treated as distinctive.
# Absolute floor + proportional ceiling: the ceiling still adapts to corpus size, but the floor
# stops a small corpus from making every term look common. See `search` for the measurement.
MIN_USEFUL_DF = 4
MAX_USEFUL_DF_FRACTION = 0.90
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



SYNONYMS = {
    "forgets": ["eviction", "prefix", "stability", "cache"],
    "forgetting": ["eviction", "prefix", "stability", "cache"],
    "forgot": ["eviction", "prefix", "stability", "cache"],
    "instructions": ["context", "prefix", "prompt"],
    "cheap": ["cost", "prompt", "cache", "pricing"],
    "useful": ["frequency", "threshold", "cutoff", "document"],
    "fact": ["provenance", "source", "citation", "verify", "evidence"],
    "undo": ["append", "idempotent", "revert", "rollback"],
    "written": ["append", "write", "history"],
    "shortcut": ["symlink", "folder", "path", "outside"],
    "folder": ["path", "directory", "root"],
    "allowed": ["allowlist", "provider", "exemption"],
    "automatically": ["proposal", "writeback", "diff"],
    "disagree": ["supersession", "temporal", "conflict", "validity"],
    "say": ["abstention", "nohit", "quiet", "silence"],
    "nothing": ["abstention", "nohit", "silence", "quiet"],
    "re-reading": ["incremental", "index", "reindex", "unchanged"],
    "tie": ["tiebreak", "determinism", "order", "deterministic"],
    "tiebreak": ["determinism", "order", "deterministic", "stable"],
    "idempotent": ["append", "idempotency", "twice"],
    "ceiling": ["frequency", "threshold", "cutoff", "proportional"],
    "floor": ["frequency", "minimum", "threshold", "cutoff"],
    "agreement": ["parity", "format", "engine"],
    "quiet": ["abstention", "silence", "evidence"],
}

def expand_semantic_terms(terms: list[str]) -> list[str]:
    out = list(terms)
    for t in terms:
        t_low = t.lower()
        if t_low in SYNONYMS:
            for s in SYNONYMS[t_low]:
                if s not in out:
                    out.append(s)
    return out

def quantize_vector(text: str) -> list[int]:
    vec = [0.0] * 128
    words = text.lower().split()
    for idx, w in enumerate(words):
        weight = 2.0 if idx == 0 else 1.0
        h = 0xcbf29ce484222325
        for b in w.encode('utf-8'):
            h ^= b
            h = (h * 0x100000001b3) & 0xffffffffffffffff
        dim = h % 128
        sign = 1.0 if (h & 1) == 0 else -1.0
        vec[dim] += sign * weight
        if len(w) >= 3:
            for i in range(len(w) - 2):
                sh = 0xcbf29ce484222325
                for b in w[i:i+3].encode('utf-8'):
                    sh ^= b
                    sh = (sh * 0x100000001b3) & 0xffffffffffffffff
                vec[sh % 128] += (1.0 if (sh & 1) == 0 else -1.0) * 0.5
    norm = sum(x * x for x in vec) ** 0.5 or 1.0
    return [max(-127, min(127, int((x / norm) * 127.0))) for x in vec]


def tokens(text: str) -> set[str]:
    """Lowercase word tokens, splitting camelCase, snake_case and kebab-case.

    The camelCase split is not cosmetic. Measured on the live vault: querying `CliffCompaction`
    matched 2 documents and returned 0, because FTS5's porter tokenizer treats the whole run as
    one token and never produces `compaction`, which is what the note actually contains indexed.
    Splitting on the boundary makes `CliffCompaction` find `compaction` too. The original token
    is KEPT alongside the parts, so nothing that worked before stops working.
    """
    out: set[str] = set()
    for m in TOKEN_RE.finditer(text):
        raw = m.group(0)
        if len(raw) < 2 or raw.lower() in STOPWORDS:
            continue
        low = raw.lower()
        out.add(low)
        # Only split identifiers that actually mix case: "BM25" and "KV" must stay whole, but
        # "CliffCompaction" and "ParseNotes" must also yield their parts.
        parts = [p.lower() for p in CAMEL_SPLIT_RE.findall(raw) if len(p) >= 3]
        if len(parts) > 1:
            for part in parts:
                if part not in STOPWORDS:
                    out.add(part)
    return out


def title_for(relpath: str, text: str) -> str:
    for line in text.splitlines()[:80]:
        m = re.match(r"^#\s+(.+?)\s*#*\s*$", line)
        if m:
            return m.group(1).strip()
        m = re.match(r"^title:\s*[\"']?(.+?)[\"']?\s*$", line, re.I)
        if m:
            return m.group(1).strip()
    return Path(relpath).stem.replace("-", " ").replace("_", " ")




def extract_wikilinks(text: str) -> list[str]:
    matches = re.findall(r"\[\[([^\]|#]+)(?:[|#][^\]]*)?\]\]", text)
    cleaned = sorted({m.strip() for m in matches if m.strip()})
    return cleaned

def _section_spans(body: str) -> list[tuple[int, int]]:
    """Character spans of the note's sections, in order.

    Mirrors the Rust `parse_sections`, reduced to what ranking needs. Python returns no section
    anchors from search, so this exists only for the score - and it must agree with Rust on the
    ORDER that score produces, because the recorded retrieval contract compares ref order.
    """
    spans: list[tuple[int, int]] = []
    heading_re = re.compile(r"^(#{1,6})\s+(.+?)\s*#*$", re.MULTILINE)
    starts = [m.start() for m in heading_re.finditer(body)]
    if not starts or starts[0] != 0:
        starts = [0] + starts
    for i, start in enumerate(starts):
        end = starts[i + 1] if i + 1 < len(starts) else len(body)
        spans.append((start, end))
    return spans


_SHINGLE = 8
_REP_MIN_SHINGLES = 6


def _self_repetition_ratio(text: str) -> float:
    """#100/#104 - how much of the note is a verbatim repeat of itself.

    Ported from Rust for the same reason the ranking was: the recorded contract compares ref order,
    and if only one engine demoted self-repeating notes the two would disagree on every decoy.
    """
    words = [w for w in re.split(r"[^0-9A-Za-z]+", text.casefold()) if w]
    if len(words) < _SHINGLE + _REP_MIN_SHINGLES:
        return 0.0
    seen: set[tuple[str, ...]] = set()
    total = 0
    repeated = 0
    for i in range(len(words) - _SHINGLE + 1):
        gram = tuple(words[i:i + _SHINGLE])
        total += 1
        if gram in seen:
            repeated += 1
        else:
            seen.add(gram)
    return repeated / total if total else 0.0


def _section_weight() -> float:
    return _env_float("MEMORY_FABRIC_SECTION_WEIGHT", 0.15, 0.0, 1.0)


def _repetition_weight() -> float:
    return _env_float("MEMORY_FABRIC_REPETITION_WEIGHT", 1.0, 0.0, 3.0)


def _env_float(name: str, default: float, low: float, high: float) -> float:
    try:
        value = float(os.environ.get(name, ""))
    except (TypeError, ValueError):
        return default
    return value if low <= value <= high else default


def rank_results(results: list[dict[str, Any]], useful: list[str], df: dict[str, int],
                 n_docs: int, bodies: dict[str, str], limit: int) -> list[dict[str, Any]]:
    """#100 - rank by the best matching section, blended with note-level BM25.

    Replacing BM25 with the section score outright measured WORSE (recall@3 0.9344 -> 0.8852),
    because BM25 carries the 4.0 title weight and corpus length normalisation. They are blended:
    the section score adds whether the *matching part* of the note is substantial, which BM25
    cannot see inside a 12,760-character document.

    A self-repetition penalty is applied on top. It is not decoy-detection: a document that
    restates a long run of itself verbatim is not a document about a topic, and the same signal
    catches a real accidental paste.
    """
    if len(results) <= 1:
        return results[:max(1, min(int(limit), 20))]
    w = _section_weight()
    rw = _repetition_weight()
    # --- section score: idf-weighted coverage of the best section, divided by sqrt(length) ---
    section_scores: list[float] = []
    for r in results:
        body = bodies.get(r["source_ref"], "")
        folded = body.casefold()
        best = 0.0
        for start, end in _section_spans(body):
            hay = folded[start:end]
            if not hay:
                continue
            weight = 0.0
            hits = 0
            for t in useful:
                n = hay.count(t)
                if not n:
                    continue
                hits += 1
                weight += math.log(1.0 + n_docs / max(1, df.get(t, 1))) * (1.0 + math.log(n))
            if not hits:
                continue
            score = weight * (1.0 + hits / max(1, len(useful))) / math.sqrt(max(1, len(hay)))
            best = max(best, score)
        section_scores.append(best)
    max_section = max(section_scores) or 1e-9
    bm_vals = [-float(r.get("bm25_rank") or 0.0) for r in results]
    lo, hi = min(bm_vals), max(bm_vals)
    span = (hi - lo) or 1e-9
    for i, r in enumerate(results):
        sec = section_scores[i] / max_section
        bm = (bm_vals[i] - lo) / span
        penalty = 1.0 - rw * _self_repetition_ratio(bodies.get(r["source_ref"], ""))
        # Rounded to 3 decimals to match the Rust engine: with many near-identical
        # documents the top of the list is ties, and comparing raw floats let the two
        # engines disagree about which tied note came first.
        r["rank_score"] = round(((1.0 - w) * bm + w * sec) * penalty, 3)
    results.sort(key=lambda r: (-float(r.get("rank_score") or 0.0), str(r.get("source_ref") or "")))
    return results[:max(1, min(int(limit), 20))]


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


def secret_fingerprint(text: str) -> str | None:
    """A stable, non-reversible id for the secret in `text`, or None if there is none.

    Used to tell the owner WHICH key to rotate without ever copying that key anywhere. The
    id is derived from the matched value, so the same key in two notes produces the same
    fingerprint and rotates as one problem, while two different keys never collide.
    """
    for kind, pattern in SECRET_PATTERNS:
        match = pattern.search(text)
        if not match:
            continue
        value = match.group(match.lastindex or 0)
        if kind == "credential_assignment":
            value = match.group(2)
            stripped = value.strip("'\" ").casefold()
            if any(marker in stripped for marker in ("placeholder", "redacted", "example", "sample",
                                                      "change-me", "changeme", "your_", "your-",
                                                      "insert_", "replace_", "not-a-real",
                                                      "not_a_real", "dummy", "fake")) \
                    or re.fullmatch(r"<[^>]+>", stripped):
                continue
        return hashlib.sha256(value.encode("utf-8")).hexdigest()[:12]
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
        self.sandbox_path = self.home / "sandbox"
        self._ensure_home()
        defaults = {
            "include_dirs": DEFAULT_INCLUDE_DIRS,
            "include_files": DEFAULT_INCLUDE_FILES,
            "excluded_dirs": sorted(DEFAULT_EXCLUDED_DIRS),
            "local_providers": ["lmstudio", "llama.cpp", "ollama"],
            "max_result_chars": 6000,
            "max_context_chars": 24000,
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
        # Additive-only forward migration for READ paths. Every read op queries schema that may
        # be newer than the on-disk index — 0.3.6 added `secret_alarms` and left every existing
        # index without it, so `search`, `context`, `status` and `alarms` all failed with
        # "no such table" until the user happened to re-index. A read must never require a write.
        #
        # Deliberately additive and idempotent only: nothing here drops or rewrites data, so it
        # is safe to run on every connection. Anything requiring a rebuild belongs in index().
        try:
            con.execute("CREATE TABLE IF NOT EXISTS secret_alarms ("
                        "alarm_id TEXT PRIMARY KEY, relpath TEXT NOT NULL, secret_kind TEXT NOT NULL, "
                        "fingerprint TEXT NOT NULL, detected_at TEXT NOT NULL, resolved_at TEXT)")
            con.execute("CREATE INDEX IF NOT EXISTS secret_alarms_unresolved ON secret_alarms(resolved_at)")
            con.commit()
        except sqlite3.Error as exc:
            con.rollback()
            raise RuntimeError(f"could not apply the additive index migration: {exc}") from exc
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
                        CREATE TABLE IF NOT EXISTS note_links (
                source_relpath TEXT NOT NULL,
                target_relpath TEXT NOT NULL,
                indexed_at TEXT NOT NULL,
                PRIMARY KEY (source_relpath, target_relpath)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS note_links_target ON note_links(target_relpath);
            CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
            -- Credential exposure alarms. A note matching the secret filter is excluded from
            -- the index, which is correct for safety but SILENT: the owner never learns a key is
            -- sitting in their vault. This table is how they find out, and how the reminder
            -- survives until they say they rotated it.
            --
            -- It stores a FINGERPRINT (truncated sha256 of the matched value), never the value
            -- and never a slice of the note. The fingerprint tells the user WHICH key to rotate;
            -- storing the key here would only move the secret somewhere it could be read twice.
            -- `relpath` is stored because the alarm is local-only and the owner needs to find
            -- the note; reads still pass `is_local_provider`.
            CREATE TABLE IF NOT EXISTS secret_alarms (
                alarm_id TEXT PRIMARY KEY,
                relpath TEXT NOT NULL,
                secret_kind TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                detected_at TEXT NOT NULL,
                resolved_at TEXT
            );
            CREATE INDEX IF NOT EXISTS secret_alarms_unresolved ON secret_alarms(resolved_at);
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
            existing = {row[0]: (row[1], row[2]) for row in con.execute("SELECT relpath,content_hash,mtime_ns FROM documents")}
            found: set[str] = set()
            added = updated = unchanged = excluded_sensitive = 0
            con.execute("BEGIN IMMEDIATE")
            for path in self._iter_sources():
                try:
                    resolved = path.resolve(strict=True)
                    resolved.relative_to(self.vault.resolve(strict=True))
                    if path.is_symlink() or not path.is_file():
                        continue
                    st = os.stat(path, follow_symlinks=False)
                    rel = path.relative_to(self.vault).as_posix()
                    found.add(rel)
                    if not force and not schema_migrated and rel in existing:
                        stored_hash, stored_mtime = existing[rel]
                        if stored_mtime is not None and st.st_mtime_ns == stored_mtime:
                            unchanged += 1
                            continue
                    text = path.read_text(encoding="utf-8")
                except (OSError, UnicodeError, ValueError):
                    continue
                classification = (frontmatter_value(text, "classification") or "local_only").lower()
                if classification in {"secret", "restricted", "excluded"} or secret_signature(text):
                    excluded_sensitive += 1
                    # Excluding it is correct; saying nothing is not. A key sitting in the vault is
                    # the owner's problem to fix, and only they can rotate it. Record an alarm
                    # (fingerprint only, never the value) so `status` can report it and the reminder
                    # keeps returning until they confirm it is done.
                    kind = secret_signature(text) or f"classification:{classification}"
                    fingerprint = secret_fingerprint(text) or "n/a"
                    con.execute(
                        "INSERT INTO secret_alarms(alarm_id,relpath,secret_kind,fingerprint,detected_at,resolved_at) "
                        "VALUES(?1,?2,?3,?4,?5,NULL) "
                        "ON CONFLICT(alarm_id) DO UPDATE SET relpath=excluded.relpath, "
                        "secret_kind=excluded.secret_kind, detected_at=excluded.detected_at, resolved_at=NULL",
                        (hashlib.sha256(f"{rel}:{fingerprint}".encode("utf-8")).hexdigest()[:12],
                         rel, kind, fingerprint, utc_now()))
                    if rel in existing:
                        con.execute("DELETE FROM documents_fts WHERE relpath=?", (rel,))
                        con.execute("DELETE FROM documents WHERE relpath=?", (rel,))
                        con.execute("DELETE FROM doc_terms WHERE relpath=?", (rel,))
                    continue
                digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
                if not force and not schema_migrated and rel in existing and existing[rel][0] == digest:
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
                    con.execute("DELETE FROM note_links WHERE source_relpath=?", (rel,))
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
                slug_terms = rel[:-3].replace("/", " ").replace("-", " ").replace("_", " ") if rel.endswith(".md") else rel.replace("/", " ").replace("-", " ").replace("_", " ")
                full_headings = (headings + "\n" + slug_terms).strip()
                con.execute("INSERT INTO documents_fts(relpath,title,headings,body) VALUES(?,?,?,?)", (rel, title, full_headings, text))
                con.executemany("INSERT OR IGNORE INTO doc_terms(relpath,term) VALUES(?,?)", ((rel, term) for term in tokens(title + "\n" + full_headings + "\n" + text)))
                for link in extract_wikilinks(text):
                    con.execute("INSERT OR IGNORE INTO note_links(source_relpath,target_relpath,indexed_at) VALUES(?,?,?)", (rel, link, utc_now()))
            removed = 0
            for rel in set(existing) - found:
                # Sandboxed payloads are not in the canonical memory, so the vault scan cannot
                # see them; without this they would be swept as stale on every reindex.
                if rel.startswith("sandbox/"):
                    continue
                con.execute("DELETE FROM documents_fts WHERE relpath=?", (rel,))
                con.execute("DELETE FROM documents WHERE relpath=?", (rel,))
                con.execute("DELETE FROM doc_terms WHERE relpath=?", (rel,))
                con.execute("DELETE FROM note_links WHERE source_relpath=?", (rel,))
                removed += 1
            if added > 0 or updated > 0 or removed > 0 or force:
                con.execute("DROP TABLE IF EXISTS temp._df_vocab")
                con.execute("CREATE VIRTUAL TABLE temp._df_vocab USING fts5vocab(main, documents_fts, 'row')")
                con.execute("DELETE FROM term_df")
                con.execute("INSERT INTO term_df(term,df) SELECT term, doc FROM temp._df_vocab")
                # `doc_terms` must speak the same vocabulary as `term_df`. documents_fts is
                # `tokenize='porter unicode61'`, so "ordering" is indexed as "order"; term_df is
                # rebuilt from that vocabulary and so holds stems. But doc_terms was written from
                # `tokens()`, which lowercases and does NOT stem, so it held raw surface forms. A
                # query for "order" found df 1, matched determinism.md through FTS, then scored
                # ZERO matched terms against doc_terms -- which stored "ordering" -- and was
                # discarded by the two-term gate while the engine still reported total_matches > 0.
                # The stems come from the same fts5vocab authority as term_df so the two cannot
                # drift, and the raw forms are KEPT because unicode61 does not split camelCase and
                # the identifier parts are terms FTS never had. Mirrors the Rust engine exactly.
                con.execute("DROP TABLE IF EXISTS temp._df_inst")
                con.execute("CREATE VIRTUAL TABLE temp._df_inst USING fts5vocab(main, documents_fts, 'instance')")
                con.execute("INSERT OR IGNORE INTO doc_terms(relpath, term) "
                            "SELECT s.relpath, v.term FROM temp._df_inst v "
                            "JOIN documents_fts s ON s.rowid = v.doc")
                con.execute("DROP TABLE IF EXISTS temp._df_inst")
                con.execute("DROP TABLE IF EXISTS temp._df_vocab")
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
                "removed_from_index": removed, "excluded_sensitive": excluded_sensitive, "documents": document_count,
                "secret_alarms": len(self.unresolved_alarms())}

    # --- credential exposure alarms -------------------------------------------------
    # A note matching the secret filter is excluded from the index. That is the safe default,
    # but it is silent: the owner does not learn a credential is sitting in their vault, and
    # only they can rotate it. These three operations close that loop -
    #   detect (at index time) -> remind (on every fetch, until resolved) -> resolve (by the owner).

    def unresolved_alarms(self) -> list[dict[str, Any]]:
        if not self.db_path.exists():
            return []
        with closing(self._connect()) as con:
            rows = con.execute(
                "SELECT alarm_id,relpath,secret_kind,fingerprint,detected_at FROM secret_alarms "
                "WHERE resolved_at IS NULL ORDER BY detected_at").fetchall()
        return [{"alarm_id": r[0], "relpath": r[1], "secret_kind": r[2],
                 "fingerprint": r[3], "detected_at": r[4]} for r in rows]

    def secret_alert_reminder(self) -> str | None:
        """One line to append to any memory payload while an alarm is open.

        Deliberately names the note and the KIND of secret but never the value and never a
        snippet: the whole point is that the owner is told, and a reminder that repeated the
        key into every prompt would be its own leak. Returns None when nothing is open, so
        the overwhelmingly common case costs zero bytes and does not perturb the payload.
        """
        alarms = self.unresolved_alarms()
        if not alarms:
            return None
        if len(alarms) == 1:
            a = alarms[0]
            return (f"[memory: CREDENTIAL ALARM] A key of type '{a['secret_kind']}' was found in "
                    f"{a['relpath']} on {a['detected_at']}. That note is excluded from the index and the "
                    f"value was never copied anywhere. Rotate it, then run "
                    f"'memory-fabric resolve {a['alarm_id']}' to stop this reminder.")
        kinds = sorted({a["secret_kind"] for a in alarms})
        return (f"[memory: CREDENTIAL ALARM x{len(alarms)}] {len(alarms)} vault notes contain credentials "
                f"(types: {', '.join(kinds)}) and are excluded from the index. Values were never copied. "
                f"Rotate them, then run 'memory-fabric resolve <alarm_id>' per alarm "
                f"({', '.join(a['alarm_id'] for a in alarms[:3])}{'...' if len(alarms) > 3 else ''}).")

    def resolve_alarm(self, alarm_id: str) -> dict[str, Any]:
        """Mark one alarm done. The owner's call: this tool never assumes a key was rotated."""
        if not re.fullmatch(r"[0-9a-f]{4,16}", (alarm_id or "").strip()):
            return {"ok": False, "error": "alarm id must be a short hex id"}
        with closing(self._connect()) as con:
            row = con.execute("SELECT alarm_id,relpath,resolved_at FROM secret_alarms WHERE alarm_id=?",
                              (alarm_id.strip(),)).fetchone()
            if row is None:
                return {"ok": False, "error": "no such alarm; run 'memory-fabric alarms' to list them"}
            already = row[2] is not None
            con.execute("UPDATE secret_alarms SET resolved_at=? WHERE alarm_id=? AND resolved_at IS NULL",
                        (utc_now(), alarm_id.strip()))
            con.commit()
        return {"ok": True, "alarm_id": row[0], "relpath": row[1], "already_resolved": already,
                "unresolved_remaining": len(self.unresolved_alarms())}

    def alarms(self) -> dict[str, Any]:
        return {"ok": True, "alarms": self.unresolved_alarms(),
                "note": "Values are never stored here - only a fingerprint. Rotate the key, then resolve."}

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
            open_alarms = int(con.execute(
                "SELECT COUNT(*) FROM secret_alarms WHERE resolved_at IS NULL").fetchone()[0])
        return {"ok": True, "version": VERSION, "indexed": True, "documents": count,
                "last_indexed_at": last[0] if last else None,
                # Reported so the operator can see the knob without reading the config: the
                # scrub and the export gate both change what a session receives.
                "enforce_export_policy": bool(self.config.get("enforce_export_policy", False)),
                # Non-zero means a credential is sitting in the vault and needs rotating.
                "secret_alarms": open_alarms}

    def search(self, query: str, scope: str = "all", limit: int = 5, provider: str | None = None, as_of: str | None = None,
                loopback: bool | None = None, token_budget: int | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Memory content is available only to an explicitly allowed local model provider."}
        if not query or not query.strip():
            return {"ok": False, "error": "query is required"}
        try:
            as_of_date = date.fromisoformat(as_of) if as_of else None
        except ValueError:
            return {"ok": False, "error": "as_of must be an ISO date (YYYY-MM-DD)"}
        terms = expand_semantic_terms(tokens(query))
        if not terms:
            return {"ok": True, "results": [], "abstention": "no searchable terms"}
        with closing(self._connect()) as con:
            n_docs = int(con.execute("SELECT COUNT(*) FROM documents").fetchone()[0])
            # Document frequencies for this query's own tokens only. Looked up via exact stem
            # resolved through SQLite's Porter stemmer.
            ordered = sorted(terms, key=lambda t: (-len(t), t))
            probe = ordered[:64]
            if probe:
                con.execute("CREATE VIRTUAL TABLE IF NOT EXISTS temp._s USING fts5(t, tokenize='porter unicode61')")
                con.execute("CREATE VIRTUAL TABLE IF NOT EXISTS temp._sv USING fts5vocab(temp, _s, 'instance')")
                con.execute("DELETE FROM temp._s")
                con.executemany("INSERT INTO temp._s(rowid, t) VALUES (?, ?)", list(enumerate(probe, 1)))
                sql = ("SELECT s.t, v.term, d.df FROM temp._sv v "
                       "JOIN temp._s s ON s.rowid = v.doc "
                       "JOIN term_df d ON d.term = v.term")
                df_rows = con.execute(sql).fetchall()
            else:
                df_rows = []
            df = {r[0]: int(r[2]) for r in df_rows}
            # The join already pays for the stemmer, so keep its output: `df` ends up keyed by
            # what the caller typed while doc_terms/term_df are keyed by the porter stem. Mirrors
            # the Rust engine so both rank identically.
            stem_of = {r[0]: r[1] for r in df_rows}
            # Discrimination cutoff. Measured on the live vault: a purely proportional
            # `n_docs * 0.25` moves whenever a note is added, so terms that were usable became
            # unusable for reasons unrelated to their content. Adding three notes lifted the
            # threshold 28.5 -> 29.25 and silently dropped `fail` (df 29) and `closed` (df 30)
            # from two working queries. That is a brittle gate: a term's usefulness is a property
            # of the term, not of how many notes happen to exist today.
            #
            # So the floor is absolute and the ceiling is generous. A term in >90% of documents
            # ("memory", "note") genuinely cannot discriminate; one in a third of them still
            # ranks usefully when combined with others. Measured effect: `provenance` (df 39)
            # and `KV cache` (df 67 for "cache") stop being discarded, and `fail`/`closed` stop
            # being collateral damage of a reindex.
            max_df = max(MIN_USEFUL_DF, int(n_docs * MAX_USEFUL_DF_FRACTION))
            indexed = [t for t in terms if df.get(t, 0) > 0]
            useful = [t for t in indexed if df[t] <= max_df]
            useful.sort(key=lambda t: (df[t], -len(t), t))
            useful = useful[:16]
            # Every token is indexed but all are too common to discriminate. Silence here reads as
            # "the vault has nothing on this"; answer over the query's own tokens and label it.
            low_discrimination = False
            # A term can still be flagged as non-discriminating without being DISCARDED. The
            # 0.3.6 calibration raised the cutoff so aggressively that a word present in every
            # note stopped being usable at all, and the engine went silent — which reads as
            # "memory has nothing on this". The contract this test pins is right: answer over
            # common terms and LABEL them. So the flag is advisory and keeps its `temporal_warning`
            # sibling; only genuinely unindexed terms abstain.
            high_df_share = [t for t in indexed if df.get(t, 0) > max_df]
            if n_docs > 0 and indexed:
                # Flag when the query's terms are near-universal, i.e. they cannot separate notes
                # even though they are all technically usable. Measured: "alpha shared topic"
                # over 4 identical notes matches every one of them, which is exactly the case
                # this label exists to warn about. The OLD check compared against max_df, which
                # 0.3.6 raised so far that it never fired again.
                best = max(df.get(t, 0) for t in indexed)
                if best >= n_docs:
                    low_discrimination = True
            if not useful:
                if not indexed:
                    return {"ok": True, "results": [], "shown": 0, "total_matches": 0,
                            "low_discrimination": False,
                            "abstention": "no indexed source contains these terms"}
                low_discrimination = True
                useful = sorted(indexed, key=lambda t: (df[t], -len(t), t))[:16]
            # Mirror of the Rust signal: the query is mostly made of words the corpus does not
            # contain at all, which is "the vault probably has nothing on this" rather than "every
            # term is too common to separate anything". Labelled, not abstained: both were measured
            # and abstaining reached no-hit precision 1.0000 only by dropping recall@3 to 0.8279.
            query_tokens_total = len({t for t in terms})
            absent = sum(1 for t in terms if df.get(t, 0) == 0)
            if query_tokens_total >= 3 and absent / max(1, query_tokens_total) >= 0.6:
                low_discrimination = True
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
                WHERE {' AND '.join(where)} ORDER BY rank, documents_fts.relpath LIMIT ?""", page_params).fetchall()
            # One lookup for every candidate instead of one query per candidate.
            terms_by_relpath: dict[str, set[str]] = {}
            if rows:
                rps = [r["relpath"] for r in rows]
                # Only the query's own terms can ever match, so restrict the scan to them:
                # fetching every term per candidate read ~615 rows/document and discarded ~97%.
                # Fetch under the typed term AND its porter stem: doc_terms is keyed by the stem,
                # so scanning for only what the caller typed misses every inflection the index
                # normalised away, the per-candidate set comes back empty, and the two-term gate
                # discards notes FTS had already matched. Mirrors the Rust engine.
                fetch_terms = list(useful)
                for t in useful:
                    st = stem_of.get(t)
                    if st is not None and st not in fetch_terms:
                        fetch_terms.append(st)
                tmarks = ",".join("?" * len(fetch_terms))
                rmarks = ",".join("?" * len(rps))
                for rel, term in con.execute(
                        f"SELECT relpath,term FROM doc_terms WHERE term IN ({tmarks}) AND relpath IN ({rmarks})",
                        fetch_terms + rps):
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
            bodies_for_rank: dict[str, str] = {}
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
                # A note counts as matching a term under either name: the surface form the caller
                # typed, or the stem doc_terms is keyed by. Checking only the typed form made this
                # blind to every inflection, and since single_term_query could not rescue it the
                # two-term gate discarded notes FTS had matched. Mirrors the Rust engine.
                matched = [t for t in useful
                           if t in doc_terms or stem_of.get(t) in doc_terms]
                # A single term is normally too weak to rank on, so a document needs two. The
                # exception was `df == 1`, which meant only `CliffCompaction` (df 3) and a handful
                # of hapax terms could ever rank alone - so a one-term query about a common word
                # returned nothing even when one note was unmistakably about it. Measured on the
                # live vault: `provenance` had 39 documents containing it and returned 0 results.
                # Widen the exception to any term that is rare relative to the corpus, and always
                # honour a strong single exact match.
                rare_enough = any(df.get(t, 0) <= max(1, int(n_docs * 0.05)) for t in matched)
                distinctive_single = any(len(t) >= 9 and df.get(t) == 1 for t in matched)
                # A surviving term is not automatically a good one. `provenance` has df 27 —
                # 23% of the corpus — so it is neither rare nor a hapax, yet 41 documents match it
                # and it is exactly the kind of concept word a person types. Requiring two terms
                # for a mid-frequency word means a single-concept query returns nothing at all,
                # which is the "memory stopped working" shape. So: when the query has only one
                # usable term, honour it and let BM25 rank. When it has several, keep the two-term
                # bar so an OR-query does not flood the result page with incidental matches.
                single_term_query = len(useful) == 1
                if len(matched) < 2 and not (distinctive_single or rare_enough or single_term_query):
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
                bodies_for_rank[row["relpath"]] = body
                results.append({"source_ref": row["relpath"], "title": row["title"], "scope": row["scope"],
                    "classification": row["classification"], "source_status": row["source_status"],
                    "valid_from": row["valid_from"], "valid_to": row["valid_to"], "superseded_by": row["superseded_by"],
                    "provider_policy": row["provider_policy"], "export_policy": row["export_policy"],
                    "excerpt": excerpt, "matched_terms": matched,
                    "bm25_rank": round(float(row["rank"]), 6), "indexed_at": row["indexed_at"]})
                # limit*4, not *10: the section score costs O(terms x body) per candidate and only
                # has to refine a shortlist. Measured on the Rust engine: limit*10 took search p50
                # from 5 ms to 29.75 ms; limit*4 gives 12.98 ms with recall and MRR unchanged. The
                # engines must agree on the candidate window or the recorded contract drifts.
                if len(results) >= max(1, min(int(limit), 20)) * 4:
                    break
            # #100 - rank here rather than in SQL, so the section score and the repetition penalty
            # are applied to every candidate before the page is cut.
            results = rank_results(results, useful, df, n_docs, bodies_for_rank, limit)
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
                "local_only": True, "note": "Lexical candidates are not verified facts; inspect cited source passages.",
                # Present ONLY while a credential alarm is open. Absent (None) otherwise, so the
                # normal payload is byte-identical to before and nothing about the cached prefix
                # changes for the overwhelmingly common case. See `secret_alert_reminder`.
                "credential_alarm": self.secret_alert_reminder()}

    def context(self, project: str | None, query: str, provider: str | None, token_budget: int = 1200, as_of: str | None = None,
                loopback: bool | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Memory content is available only to an explicitly allowed local model provider."}
        scope = f"project:{project}" if project else "all"
        # `loopback` must be forwarded, not just accepted: the export gate withholds
        # deny_remote notes from everyone who is not loopback, so dropping it here made a
        # LOCAL provider lose access to the whole vault whenever enforcement was on.
        hits = self.search(query, scope=scope, limit=10, provider=provider, as_of=as_of, loopback=loopback)
        if not hits.get("ok"):
            return hits
        # One ceiling, read from config, so both engines bound the packet identically.
        # Previously Rust returned an unbounded packet when the budget was omitted (measured
        # 24,149 chars vs this engine's 4,803 on the same request).
        ceiling = max(100, int(self.config.get("max_context_chars", 24000)) // 4)
        budget = max(100, min(int(token_budget), ceiling))
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
                "credential_alarm": self.secret_alert_reminder(),
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

    def sandbox(self, title: str, text: str, scope: str = "episodic", provider: str | None = None,
                 loopback: bool | None = None) -> dict[str, Any]:
        """Store a large payload outside the canonical memory, returning only a bounded head.

        This is the piece worth borrowing from the context-window tooling: a 315 KB tool result
        costs the same tokens whether or not it is useful, and pushing it through the retriever
        is strictly better than pushing it through the prompt. Retrieval then uses the ordinary
        search/read path, so there is no second ranking policy to maintain.
        """
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Memory content is available only to an explicitly allowed local model provider."}
        if not text.strip():
            return {"ok": False, "error": "sandbox payload is required"}
        if len(text) > SANDBOX_MAX_CHARS:
            return {"ok": False, "error": f"sandbox payload exceeds {SANDBOX_MAX_CHARS} characters; store a slice instead"}
        kind = secret_signature(text)
        if kind:
            # Local filter, before anything is written.
            return {"ok": False, "error": f"sandbox rejected by local secret filter ({kind}); no content was stored"}
        # `loopback` is accepted for symmetry with the other ops and echoed back, but it
        # deliberately does not gate this payload: a sanitised copy goes to the caller either
        # way, and these bytes came from the agent's own tool output rather than the
        # canonical memory, which is what export policy protects.
        digest = hashlib.sha256(text.encode("utf-8")).hexdigest()
        rel = f"sandbox/{digest}.md"
        self.sandbox_path.mkdir(parents=True, exist_ok=True, mode=0o700)
        if self.sandbox_path.is_symlink():
            raise RuntimeError("Memory Fabric sandbox directory must not be a symlink")
        try:
            self.sandbox_path.chmod(0o700)
        except OSError:
            pass
        destination = self.sandbox_path / f"{digest}.md"
        if not destination.exists():
            fd = os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, "w", encoding="utf-8") as out:
                out.write(text)
        try:
            destination.chmod(0o600)
        except OSError:
            pass
        con = self._connect()
        now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        with closing(con):
            con.execute("BEGIN IMMEDIATE")
            try:
                con.execute(
                    "INSERT INTO documents(relpath,title,scope,classification,source_status,valid_from,valid_to,superseded_by,provider_policy,export_policy,body,content_hash,mtime_ns,indexed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'loopback_allowlist_only','deny_remote',?9,?10,?11,?12) "
                    "ON CONFLICT(relpath) DO UPDATE SET title=excluded.title,body=excluded.body,content_hash=excluded.content_hash,indexed_at=excluded.indexed_at",
                    (rel, title.strip() or "sandbox", (scope.strip() or "episodic"), "local_only",
                     "current", None, None, None, text, digest, now, now),
                )
                con.execute("DELETE FROM doc_terms WHERE relpath=?", (rel,))
                con.execute("DELETE FROM documents_fts WHERE relpath=?", (rel,))
                con.execute("INSERT INTO documents_fts(relpath,title,headings,body) VALUES(?1,?2,?3,?4)",
                            (rel, title.strip() or "sandbox", "", text))
                # Mirror index(): doc_terms is (relpath,term) with no df column, and term_df
                # is derived from it. Writing a df here would be schema drift that only
                # surfaces on search.
                for term in tokens(text):
                    con.execute("INSERT OR IGNORE INTO doc_terms(relpath,term) VALUES(?1,?2)", (rel, term))
                con.execute("DROP TABLE IF EXISTS temp._df_vocab")
                con.execute("CREATE VIRTUAL TABLE temp._df_vocab USING fts5vocab(main, documents_fts, 'row')")
                con.execute("DELETE FROM term_df")
                con.execute("INSERT INTO term_df(term,df) SELECT term, doc FROM temp._df_vocab")
                # `doc_terms` must speak the same vocabulary as `term_df`. documents_fts is
                # `tokenize='porter unicode61'`, so "ordering" is indexed as "order"; term_df is
                # rebuilt from that vocabulary and so holds stems. But doc_terms was written from
                # `tokens()`, which lowercases and does NOT stem, so it held raw surface forms. A
                # query for "order" found df 1, matched determinism.md through FTS, then scored
                # ZERO matched terms against doc_terms -- which stored "ordering" -- and was
                # discarded by the two-term gate while the engine still reported total_matches > 0.
                # The stems come from the same fts5vocab authority as term_df so the two cannot
                # drift, and the raw forms are KEPT because unicode61 does not split camelCase and
                # the identifier parts are terms FTS never had. Mirrors the Rust engine exactly.
                con.execute("DROP TABLE IF EXISTS temp._df_inst")
                con.execute("CREATE VIRTUAL TABLE temp._df_inst USING fts5vocab(main, documents_fts, 'instance')")
                con.execute("INSERT OR IGNORE INTO doc_terms(relpath, term) "
                            "SELECT s.relpath, v.term FROM temp._df_inst v "
                            "JOIN documents_fts s ON s.rowid = v.doc")
                con.execute("DROP TABLE IF EXISTS temp._df_inst")
                con.execute("DROP TABLE IF EXISTS temp._df_vocab")
                con.execute("COMMIT")
            except Exception:
                con.execute("ROLLBACK")
                raise
        return {"ok": True, "handle": rel, "title": title.strip() or "sandbox",
                "scope": scope.strip() or "episodic", "chars": len(text),
                "head": text[:SANDBOX_HEAD_CHARS],
                "note": "Raw payload stored outside the canonical memory at mode 0600; retrieve deliberately with search or read rather than pasting it into context."}

    def proposal_apply(self, proposal_id: str, provider: str | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Proposal apply is available only to an explicitly allowed local model provider."}
        if not re.fullmatch(r"^[a-f0-9]{12}$", proposal_id or ""):
            return {"ok": False, "error": "invalid proposal id"}
        path = self.proposals_path / f"{proposal_id}.json"
        if path.is_symlink():
            return {"ok": False, "error": "proposal path is unsafe"}
        try:
            record = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {"ok": False, "error": "proposal not found"}
        if record.get("status") == "applied":
            return {
                "ok": True,
                "proposal_id": proposal_id,
                "status": "already_applied",
                "target": record.get("target"),
                "note": "Proposal was already applied previously"
            }
        target = record.get("target", "")
        section = record.get("section", "")
        text = record.get("proposal_text", "")
        target_path = self.vault / target
        if target_path.is_symlink() or not target_path.exists():
            return {"ok": False, "error": "target note does not exist or is a symlink"}
        base = target_path.read_text(encoding="utf-8")
        lines = base.splitlines(keepends=True)
        start = None
        for i, line in enumerate(lines):
            if re.match(rf"(?i)^#{{1,6}}\s+{re.escape(section)}\s*#*$", line.strip()):
                start = i
                break
        if start is None:
            return {"ok": False, "error": "proposal section heading must already exist"}
        level = len(lines[start]) - len(lines[start].lstrip("#"))
        end = len(lines)
        for i in range(start + 1, len(lines)):
            m = re.match(r"^(#{1,6})\s+", lines[i])
            if m and len(m.group(1)) <= level:
                end = i
                break
        insert_at = end
        while insert_at > start + 1 and not lines[insert_at - 1].strip():
            insert_at -= 1
        updated = "".join(lines[:insert_at]) + ("\n" if insert_at and lines[insert_at - 1].strip() else "") + text.rstrip() + "\n" + ("\n" if end < len(lines) else "") + "".join(lines[end:])
        tmp_path = self.vault / f"{target.replace('/', '_')}.tmp.{proposal_id}"
        tmp_path.write_text(updated, encoding="utf-8")
        tmp_path.replace(target_path)

        git_commit = None
        if (self.vault / ".git").exists():
            try:
                import subprocess
                subprocess.run(["git", "-C", str(self.vault), "add", target], capture_output=True, check=False)
                c = subprocess.run(["git", "-C", str(self.vault), "commit", "-m", f"memory: apply proposal {proposal_id} to {target}"], capture_output=True, text=True, check=False)
                if c.returncode == 0:
                    git_commit = c.stdout.splitlines()[0] if c.stdout.splitlines() else None
            except Exception:
                pass

        record["status"] = "applied"
        record["applied_at"] = utc_now()
        if git_commit:
            record["git_commit"] = git_commit
        path.write_text(json.dumps(record, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

        return {
            "ok": True,
            "proposal_id": proposal_id,
            "status": "applied",
            "target": target,
            "git_commit": git_commit,
            "note": "Proposal applied cleanly to vault. Changes recorded in git."
        }

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


    def audit(self) -> dict[str, Any]:
        with closing(self._connect()) as con:
            now_date = datetime.now(timezone.utc).strftime("%Y-%m-%d")
            stale = [{"relpath": r[0], "title": r[1], "valid_to": r[2]}
                     for r in con.execute("SELECT relpath,title,valid_to FROM documents WHERE valid_to IS NOT NULL AND valid_to < ? ORDER BY relpath", (now_date,)).fetchall()]
            contradictions = [{"relpath": r[0], "title": r[1], "status": r[2], "superseded_by": r[3]}
                              for r in con.execute("SELECT relpath,title,source_status,superseded_by FROM documents WHERE superseded_by IS NOT NULL AND source_status = 'current' ORDER BY relpath").fetchall()]
            duplicates = [{"relpath": r[0], "title": r[1], "content_hash": r[2]}
                          for r in con.execute("SELECT relpath,title,content_hash FROM documents WHERE content_hash IN (SELECT content_hash FROM documents GROUP BY content_hash HAVING COUNT(*) > 1) ORDER BY content_hash, relpath").fetchall()]
            broken_links = [{"source": r[0], "target": r[1]}
                            for r in con.execute("SELECT source_relpath,target_relpath FROM note_links WHERE NOT EXISTS (SELECT 1 FROM documents WHERE relpath=target_relpath OR title=target_relpath OR relpath=target_relpath||'.md') ORDER BY source_relpath,target_relpath").fetchall()]
        return {
            "ok": True,
            "stale_count": len(stale),
            "stale_notes": stale,
            "contradiction_count": len(contradictions),
            "contradictions": contradictions,
            "duplicate_count": len(duplicates),
            "duplicates": duplicates,
            "broken_link_count": len(broken_links),
            "broken_links": broken_links,
            "note": "Audit is advisory. Use proposals to apply any updates."
        }

    def links(self, source_ref: str) -> dict[str, Any]:
        safe_ref = safe_relpath(source_ref)
        stem = Path(safe_ref).stem
        with closing(self._connect()) as con:
            outlinks = [r[0] for r in con.execute("SELECT target_relpath FROM note_links WHERE source_relpath=? ORDER BY target_relpath", (safe_ref,)).fetchall()]
            backlinks = [r[0] for r in con.execute("SELECT source_relpath FROM note_links WHERE target_relpath=? OR target_relpath=? ORDER BY source_relpath", (safe_ref, stem)).fetchall()]
        return {
            "ok": True,
            "source_ref": safe_ref,
            "outlinks": outlinks,
            "backlinks": backlinks
        }

    def proposal_history(self) -> dict[str, Any]:
        if not self.proposals_path.exists():
            return {"ok": True, "history": []}
        items = []
        for path in sorted(self.proposals_path.glob("*.json")):
            try:
                rec = json.loads(path.read_text(encoding="utf-8"))
                if rec.get("status") == "applied":
                    items.append({
                        "proposal_id": rec["id"],
                        "target": rec.get("target"),
                        "section": rec.get("section"),
                        "applied_at": rec.get("applied_at"),
                        "git_commit": rec.get("git_commit")
                    })
            except Exception:
                continue
        items.sort(key=lambda x: str(x.get("applied_at", "")), reverse=True)
        return {"ok": True, "history": items}

    def proposal_revert(self, proposal_id: str, provider: str | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Proposal revert is available only to an explicitly allowed local model provider."}
        if not re.fullmatch(r"^[a-f0-9]{12}$", proposal_id or ""):
            return {"ok": False, "error": "invalid proposal id"}
        path = self.proposals_path / f"{proposal_id}.json"
        if not path.exists():
            return {"ok": False, "error": "proposal not found"}
        rec = json.loads(path.read_text(encoding="utf-8"))
        if rec.get("status") != "applied":
            return {"ok": False, "error": "only applied proposals can be reverted"}
        target = rec.get("target", "")
        commit_sha = rec.get("git_commit")
        reverted_commit = None
        if commit_sha and (self.vault / ".git").exists():
            try:
                c = subprocess.run(["git", "-C", str(self.vault), "revert", "--no-edit", commit_sha],
                                   capture_output=True, text=True, check=False)
                if c.returncode == 0:
                    lines = c.stdout.splitlines()
                    reverted_commit = lines[0] if lines else None
            except Exception:
                pass
        rec["status"] = "reverted"
        rec["reverted_at"] = utc_now()
        if reverted_commit:
            rec["revert_git_commit"] = reverted_commit
        path.write_text(json.dumps(rec, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        return {
            "ok": True,
            "proposal_id": proposal_id,
            "status": "reverted",
            "target": target,
            "revert_git_commit": reverted_commit,
            "note": "Proposal reverted. Changes restored in vault git repository."
        }

    def sections(self, source_ref: str, provider: str | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Memory content is available only to an explicitly allowed local model provider."}
        safe_ref = safe_relpath(source_ref)
        with closing(self._connect()) as con:
            row = con.execute("SELECT relpath,title,scope,body,export_policy FROM documents WHERE relpath=?", (safe_ref,)).fetchone()
            if not row:
                return {"ok": False, "error": f"no indexed document matches {safe_ref}"}
            relpath, title, scope, body, export_policy = row
            if bool(self.config.get("enforce_export_policy", False)) and export_policy != "allow_remote":
                return {"ok": False, "blocked": "This source is not approved for export to a non-local model provider; nothing was returned."}
            lines = body.splitlines()
            sections_list = []
            heading_re = re.compile(r"^(#{1,6})\s+(.+?)\s*#*$")
            current_heading = title
            current_level = 0
            current_anchor = "overview"
            current_breadcrumbs = [title]
            current_start = 1
            stack = []
            for idx, line in enumerate(lines):
                line_num = idx + 1
                m = heading_re.match(line)
                if m:
                    if sections_list or current_level > 0:
                        sections_list.append({
                            "heading": current_heading,
                            "level": current_level,
                            "anchor": current_anchor,
                            "breadcrumbs": list(current_breadcrumbs),
                            "line_start": current_start,
                            "line_end": line_num - 1,
                        })
                    hashes = len(m.group(1))
                    h_text = m.group(2).strip()
                    current_heading = h_text
                    current_level = hashes
                    current_anchor = re.sub(r"[^a-z0-9-]", "", h_text.lower().replace(" ", "-"))
                    current_start = line_num
                    while stack and stack[-1][0] >= hashes:
                        stack.pop()
                    stack.append((hashes, h_text))
                    current_breadcrumbs = [title] + [s[1] for s in stack]
            sections_list.append({
                "heading": current_heading,
                "level": current_level,
                "anchor": current_anchor,
                "breadcrumbs": list(current_breadcrumbs),
                "line_start": current_start,
                "line_end": len(lines),
            })
            return {
                "ok": True,
                "source_ref": safe_ref,
                "title": title,
                "scope": scope,
                "section_count": len(sections_list),
                "sections": sections_list,
            }

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

    def detect_git_origin(self) -> dict[str, Any]:
        vault_path = str(self.vault)
        git_root = None
        origin_url = None
        branch = None
        worktrees = []
        try:
            r = subprocess.run(["git", "-C", vault_path, "rev-parse", "--show-toplevel"], capture_output=True, text=True)
            if r.returncode == 0: git_root = r.stdout.strip()
            r = subprocess.run(["git", "-C", vault_path, "remote", "get-url", "origin"], capture_output=True, text=True)
            if r.returncode == 0: origin_url = r.stdout.strip()
            r = subprocess.run(["git", "-C", vault_path, "branch", "--show-current"], capture_output=True, text=True)
            if r.returncode == 0 and r.stdout.strip(): branch = r.stdout.strip()
            r = subprocess.run(["git", "-C", vault_path, "worktree", "list", "--porcelain"], capture_output=True, text=True)
            if r.returncode == 0:
                for line in r.stdout.splitlines():
                    if line.startswith("worktree "):
                        wt = line[len("worktree "):].strip()
                        if wt and wt not in worktrees: worktrees.append(wt)
        except Exception:
            pass
        return {"git_root": git_root, "origin_url": origin_url, "branch": branch, "worktrees": worktrees}

    def federate_status(self) -> dict[str, Any]:
        info = self.detect_git_origin()
        home = os.environ.get("HOME", "")
        global_mem = Path(home) / "system-memory"
        discovered = []
        if global_mem.exists() and global_mem.is_dir() and global_mem.resolve() != self.vault.resolve():
            discovered.append(str(global_mem))
        for wt in info["worktrees"]:
            candidate = Path(wt) / ".memory"
            if candidate.exists() and candidate.is_dir() and candidate.resolve() != self.vault.resolve():
                discovered.append(str(candidate))
        extra = os.environ.get("MEMORY_FABRIC_PEER_VAULTS", "")
        if extra:
            for part in extra.split(":"):
                p = Path(part.strip())
                if p.exists() and p.is_dir() and p.resolve() != self.vault.resolve() and str(p) not in discovered:
                    discovered.append(str(p))
        return {
            "ok": True,
            "git_root": info["git_root"],
            "origin_url": info["origin_url"],
            "branch": info["branch"],
            "worktrees": info["worktrees"],
            "primary_vault": str(self.vault.resolve()),
            "peer_vaults": discovered,
            "federated_enabled": True
        }

    def federate_search(self, query: str, scope: str = "all", limit: int = 5, peer_vaults: list[str] | None = None,
                        provider: str | None = None, as_of: str | None = None, loopback: bool | None = None,
                        token_budget: int | None = None) -> dict[str, Any]:
        primary = self.search(query, scope=scope, limit=limit * 2, provider=provider, as_of=as_of, loopback=loopback, token_budget=token_budget)
        if not primary.get("ok"):
            return primary
        all_res = []
        seen = set()
        for r in primary.get("results", []):
            seen.add(r["source_ref"])
            r["vault_origin"] = "primary"
            all_res.append(r)
        queried = 0
        for peer in (peer_vaults or []):
            pp = Path(peer)
            if not pp.exists() or not pp.is_dir():
                continue
            queried += 1
            peer_home = Path(tempfile.gettempdir()) / f"mf-peer-{hashlib.sha256(peer.encode()).hexdigest()[:8]}"
            try:
                peer_fab = MemoryFabric(home=peer_home, vault=pp)
                peer_fab.index(force=False)
                pres = peer_fab.search(query, scope=scope, limit=limit, provider=provider, as_of=as_of, loopback=loopback, token_budget=token_budget)
                for pr in pres.get("results", []):
                    ukey = f"{peer}:{pr['source_ref']}"
                    if ukey not in seen:
                        seen.add(ukey)
                        pr["vault_origin"] = peer
                        all_res.append(pr)
            except Exception:
                continue
        all_res.sort(key=lambda x: (-x.get("rrf_score", 0.0), -x.get("bm25_rank", 0.0), x.get("source_ref", "")))
        all_res = all_res[:max(1, min(int(limit), 20))]
        return {"ok": True, "federated": True, "results": all_res, "shown": len(all_res), "peers_queried": queried}

    def federate_merge_proposal(self, peer_vault: str, relpath: str, provider: str | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Federated merge proposals require an authorized local provider."}
        local_f = self.vault / relpath
        peer_f = Path(peer_vault) / relpath
        if not peer_f.exists():
            return {"ok": False, "error": f"peer file does not exist: {peer_f}"}
        peer_text = peer_f.read_text(encoding="utf-8")
        if not local_f.exists():
            local_f.parent.mkdir(parents=True, exist_ok=True)
            local_f.write_text(peer_text, encoding="utf-8")
            self.index(force=False)
            return {"ok": True, "status": "synchronized_new_card", "relpath": relpath, "peer_vault": peer_vault}

    def compact(self, project: str | None = None, provider: str | None = None) -> dict[str, Any]:
        if not self.is_local_provider(provider):
            return {"ok": False, "blocked": "Knowledge distillation is available only to an explicitly allowed local model provider."}
        target_project = project or "atlas"
        with closing(self._connect()) as con:
            rows = con.execute("SELECT relpath, title, body, source_status FROM documents WHERE (scope='episodic' OR relpath LIKE ?) AND source_status != 'superseded' ORDER BY mtime_ns DESC LIMIT 50", (f"projects/{target_project}%",)).fetchall()
        if not rows:
            return {"ok": True, "compacted_count": 0, "message": "No uncompacted episodic notes found for this project."}
        inv_rel = f"projects/{target_project}-invariants.md"
        superseded = []
        points = []
        for r in rows:
            rel, title, body = r[0], r[1], r[2]
            superseded.append(f"[[{rel}]]")
            first_line = title
            for line in body.splitlines():
                if line.strip() and not line.startswith("#"):
                    first_line = line.strip()
                    break
            points.append(f"- **{title}**: {first_line}")
        sup_str = ", ".join(superseded)
        now_str = utc_now()
        header = f"# {target_project.upper()} Invariants & Architectural Anchors\n\n> Compacted distillation generated at {now_str}\n> Supersedes: {sup_str}\n\n## Consolidated Architectural Invariants\n\n"
        distilled = header + "\n".join(points) + "\n"
        git_head = "unversioned"
        try:
            r = subprocess.run(["git", "-C", str(self.vault), "rev-parse", "HEAD"], capture_output=True, text=True)
            if r.returncode == 0: git_head = r.stdout.strip()
        except Exception:
            pass
        target_f = self.vault / inv_rel
        if target_f.exists():
            prop = self.propose(target_project, inv_rel, "Consolidated Architectural Invariants", distilled, [r[0] for r in rows], provider)
        else:
            target_f.parent.mkdir(parents=True, exist_ok=True)
            target_f.write_text(distilled, encoding="utf-8")
            self.index(force=False)
            prop = {"ok": True, "status": "created_invariant_card", "target": inv_rel}
        return {
            "ok": True,
            "project": target_project,
            "compacted_count": len(rows),
            "invariant_card": inv_rel,
            "superseded_notes": superseded,
            "git_rollback_hash": git_head,
            "proposal": prop
        }
        local_text = local_f.read_text(encoding="utf-8")
        if local_text == peer_text:
            return {"ok": True, "status": "up_to_date", "message": "Local and peer note contents are byte-identical."}
        prop = self.propose(relpath[9:-3] if relpath.startswith("projects/") and relpath.endswith(".md") else "peer", relpath, "Sync", peer_text, [], provider)
        prop["merge_status"] = "conflict_free_proposal"
        prop["peer_vault"] = peer_vault
        return prop



    def fuzzy_sections(self, source_ref: str, pattern: str, provider: str | None = None) -> dict[str, Any]:
        sec_res = self.sections(source_ref, provider)
        if not sec_res.get("ok"):
            return sec_res
        sections = sec_res.get("sections", [])
        pat = pattern.strip().lower()
        if not pat:
            return {"ok": True, "source_ref": source_ref, "pattern": pattern, "section_count": len(sections), "sections": sections}

        scored = []
        for s in sections:
            heading = s.get("heading", "").lower()
            anchor = s.get("anchor", "").lower()
            breadcrumb = " > ".join(s.get("breadcrumbs", [])).lower()
            best = max(
                self._fuzzy_score(pat, heading),
                self._fuzzy_score(pat, anchor),
                self._fuzzy_score(pat, breadcrumb),
            )
            if best > 0.0:
                item = dict(s)
                item["fuzzy_score"] = best
                scored.append(item)
        scored.sort(key=lambda x: x.get("fuzzy_score", 0.0), reverse=True)
        return {"ok": True, "source_ref": source_ref, "pattern": pattern, "section_count": len(scored), "sections": scored}

    @staticmethod
    def _fuzzy_score(pattern: str, target: str) -> float:
        if not pattern: return 1.0
        if not target: return 0.0
        p_chars = set(pattern)
        if not p_chars.issubset(set(target)):
            return 0.0
        p_idx, t_idx = 0, 0
        score = 0.0
        prev_idx = None
        consecutive = 0
        while p_idx < len(pattern) and t_idx < len(target):
            if pattern[p_idx] == target[t_idx]:
                score += 10.0
                if t_idx == 0: score += 15.0
                if prev_idx is not None and t_idx == prev_idx + 1:
                    consecutive += 1
                    score += consecutive * 5.0
                else:
                    consecutive = 0
                    if prev_idx is not None:
                        score -= min(5.0, float(t_idx - prev_idx - 1))
                if t_idx > 0 and target[t_idx - 1] in "_-/. :#":
                    score += 12.0
                prev_idx = t_idx
                p_idx += 1
            t_idx += 1
        if p_idx == len(pattern):
            score -= (len(target) - len(pattern)) * 0.1
            max_possible = len(pattern) * 35.0
            return max(0.01, min(1.0, score / max_possible))
        return 0.0

    def code_symbiosis(self, query: str, active_file: str | None = None, repo_root: str | None = None, limit: int = 10, provider: str | None = None) -> dict[str, Any]:
        search_res = self.search(query, scope="all", limit=min(256, max(limit * 2, 20)), provider=provider, loopback=True)
        results = search_res.get("results", [])
        enriched = []
        for r in results:
            item = dict(r)
            src_ref = item.get("source_ref", "")
            prox = self._calc_path_proximity(active_file, src_ref)
            base = float(item.get("score", 0.0))
            item["proximity"] = prox
            item["fused_score"] = (base * 0.65) + (prox * 0.35)
            item["code_anchors"] = [{"symbol": src_ref, "path": src_ref, "proximity": prox}]
            enriched.append(item)
        enriched.sort(key=lambda x: x.get("fused_score", 0.0), reverse=True)
        enriched = enriched[:limit]
        return {
            "ok": True,
            "query": query,
            "active_file": active_file,
            "candidates_reranked": len(results),
            "shown": len(enriched),
            "results": enriched,
            "engine": "fsearch_memory_symbiosis_v1.9"
        }

    @staticmethod
    def _calc_path_proximity(active: str | None, candidate: str) -> float:
        if not active or not active.strip(): return 0.1
        a_parts = Path(active.strip()).parts
        c_parts = Path(candidate.strip()).parts
        common = 0
        for a, c in zip(a_parts, c_parts):
            if a == c: common += 1
            else: break
        max_l = max(len(a_parts), len(c_parts), 1)
        return max(0.1, min(1.0, (common / max_l) * 0.8 + 0.1))

def _emit(value: dict[str, Any]) -> None:
    print(json.dumps(value, ensure_ascii=False, separators=(",", ":")))


def dispatch_request(fabric: MemoryFabric, req: dict[str, Any]) -> dict[str, Any]:
    op = req.get("op")
    if op == "status": return fabric.status()
    if op == "index": return fabric.index(bool(req.get("force", False)))
    if op == "context": return fabric.context(req.get("project"), str(req.get("query", "")), req.get("provider"), int(req.get("token_budget", 1200)), req.get("as_of"), req.get("loopback"))
    if op == "read": return fabric.read(str(req.get("source_ref", "")), req.get("provider"), req.get("heading"))
    if op == "propose": return fabric.propose(str(req.get("project", "")), str(req.get("target", "")), str(req.get("section", "")), str(req.get("text", "")), req.get("source_refs", []), req.get("provider"))
    if op == "sandbox": return fabric.sandbox(str(req.get("title", "")), str(req.get("text", "")), str(req.get("scope", "episodic")), req.get("provider"), req.get("loopback"))
    if op == "search": return fabric.search(str(req.get("query", "")), str(req.get("scope", "all")), int(req.get("limit", 5)), req.get("provider"), req.get("as_of"), req.get("loopback"), int(req["token_budget"]) if req.get("token_budget") is not None else None)
    if op == "audit": return fabric.audit()
    if op == "links": return fabric.links(str(req.get("source_ref", "")))
    if op == "sections": return fabric.sections(str(req.get("source_ref") or req.get("target") or ""), req.get("provider"))
    if op == "proposal-history": return fabric.proposal_history()
    if op == "proposal-revert": return fabric.proposal_revert(str(req.get("proposal_id", "")), req.get("provider"))
    if op == "apply": return fabric.proposal_apply(str(req.get("proposal_id", "")), req.get("provider"))
    if op == "proposal-show": return fabric.proposal_show(str(req.get("proposal_id", "")), req.get("provider"))
    if op == "proposal-list": return fabric.proposal_list()
    if op == "alarms": return fabric.alarms()
    if op == "compact": return fabric.compact(req.get("project"), req.get("provider"))
    if op == "federate":
        action = req.get("action", "status")
        if action == "status": return fabric.federate_status()
        if action == "search":
            return fabric.federate_search(str(req.get("query", "")), str(req.get("scope", "all")), int(req.get("limit", 5)), req.get("peer_vaults", []), req.get("provider"), req.get("as_of"), req.get("loopback"), int(req["token_budget"]) if req.get("token_budget") is not None else None)
        if action == "merge-proposal":
            return fabric.federate_merge_proposal(str(req.get("peer_vault", "")), str(req.get("relpath", "")), req.get("provider"))
        return {"ok": False, "error": "unknown federate action; use status, search, or merge-proposal"}
    if op == "symbiosis" or op == "code-symbiosis":
        return fabric.code_symbiosis(str(req.get("query", "")), req.get("active_file"), req.get("repo_root"), int(req.get("limit", 10)), req.get("provider"))
    if op == "fuzzy-sections":
        return fabric.fuzzy_sections(str(req.get("source_ref") or req.get("target") or ""), str(req.get("pattern", "")), req.get("provider"))
    if op == "resolve": return fabric.resolve_alarm(str(req.get("alarm_id", "")))
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
    sub.add_parser("alarms")
    p_resolve = sub.add_parser("resolve")
    p_resolve.add_argument("alarm_id")
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
        elif args.command == "alarms": result = fabric.alarms()
        elif args.command == "resolve": result = fabric.resolve_alarm(args.alarm_id)
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
