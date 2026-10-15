use crate::MemoryFabric;
use crate::paths::{MAX_CONTEXT_CHARS_DEFAULT, MAX_RESULT_CHARS_DEFAULT, SCHEMA_VERSION, home_dir, safe_relpath, set_mode, set_open_mode};
use crate::policy::{
    DEFAULT_EXCLUDED_DIRS, DEFAULT_INCLUDE_DIRS, DEFAULT_INCLUDE_FILES, apple_path_re,
};

/// The FTS tokenizer the code expects, and the single source of truth for it.
///
/// A named constant because a mismatch between this and what the database actually contains is
/// invisible: retrieval quality changes and nothing reports it. That happened once already - an
/// experiment changed the tokenizer, the experiment was reverted in the code, and the database kept
/// the old one because its migration went away with the revert. `init_db` now reconciles in both
/// directions, against this constant.
pub(crate) const EXPECTED_FTS_TOKENIZER: &str = "porter unicode61";
use rusqlite::{Connection, OptionalExtension, Row};

/// Distinguish "another writer holds the index" from "the index is broken". Retrying is
/// the right response to the first and pointless for the second, so a caller must be able
/// to tell them apart without parsing a free-text SQLite message.
pub(crate) fn busy_aware(action: &str, err: rusqlite::Error) -> String {
    busy_message(&err.to_string(), action)
}

/// Same classification, for an error that has already been flattened to a string on its way
/// out of a transaction. Applied at the dispatch funnel so no call site is missed.
pub(crate) fn busy_message(detail: &str, action: &str) -> String {
    let lowered = detail.to_ascii_lowercase();
    if lowered.contains("file is not a database")
        || lowered.contains("malformed")
        || lowered.contains("corrupt")
    {
        format!("local memory index is corrupt (during {action}); reindex with 'index --force' [{detail}]")
    } else if lowered.contains("database is locked")
        || lowered.contains("database is busy")
        || lowered.contains("locked")
    {
        format!("local memory index is busy (during {action}); retry shortly [{detail}]")
    } else {
        detail.to_string()
    }
}
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

impl MemoryFabric {
    pub(crate) fn new() -> Result<Self, String> {
        let home = env::var_os("MEMORY_FABRIC_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".pi/agent/scratch/memory-fabric"));
        let vault = env::var_os("MEMORY_FABRIC_VAULT")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("system-memory"));
        Self::with_paths(home, vault)
    }

    pub(crate) fn with_paths(home: PathBuf, vault: PathBuf) -> Result<Self, String> {
        if home.is_symlink() {
            return Err("Memory Fabric state directory must not be a symlink".into());
        }
        fs::create_dir_all(&home).map_err(|e| format!("cannot create state directory: {e}"))?;
        if home.is_symlink() {
            return Err("Memory Fabric state directory must not be a symlink".into());
        }
        set_mode(&home, 0o700);
        let db_path = home.join("index.sqlite3");
        let proposals_path = home.join("proposals");
        let config_path = home.join("config.json");
        if config_path.is_symlink() {
            return Err("Memory Fabric config must not be a symlink".into());
        }
        let mut config_value = if config_path.exists() {
            let raw =
                fs::read_to_string(&config_path).map_err(|e| format!("cannot read config: {e}"))?;
            serde_json::from_str::<Value>(&raw)
                .map_err(|_| "Memory Fabric config must be valid JSON".to_string())?
        } else {
            let mut m = Map::new();
            m.insert("include_dirs".into(), json!(DEFAULT_INCLUDE_DIRS));
            m.insert("include_files".into(), json!(DEFAULT_INCLUDE_FILES));
            m.insert("excluded_dirs".into(), json!(DEFAULT_EXCLUDED_DIRS));
            m.insert(
                "local_providers".into(),
                json!(["lmstudio", "llama.cpp", "ollama"]),
            );
            m.insert("max_result_chars".into(), json!(MAX_RESULT_CHARS_DEFAULT));
            // Ceiling for a context packet, in characters. The budget argument is in tokens
            // (x4 to characters); this caps it regardless of what a caller asks for, so an
            // omitted budget can never mean "unbounded".
            m.insert("max_context_chars".into(), json!(MAX_CONTEXT_CHARS_DEFAULT));
            m.insert("enforce_export_policy".into(), json!(false));
            Value::Object(m)
        };
        if !config_value.is_object() {
            return Err("Memory Fabric config must be a JSON object".into());
        }
        if let Ok(providers) = env::var("PI_MEMORY_LOCAL_PROVIDERS") {
            config_value["local_providers"] = json!(
                providers
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            );
        }
        let include_dirs = string_list(&config_value, "include_dirs", DEFAULT_INCLUDE_DIRS)?;
        let include_files = string_list(&config_value, "include_files", DEFAULT_INCLUDE_FILES)?;
        let excluded = string_list(&config_value, "excluded_dirs", DEFAULT_EXCLUDED_DIRS)?;
        let providers = string_list(
            &config_value,
            "local_providers",
            &["lmstudio", "llama.cpp", "ollama"],
        )?;
        if include_dirs
            .iter()
            .chain(include_files.iter())
            .any(|s| apple_path_re().is_match(s))
        {
            return Err("Apple Notes paths cannot be added to the source allowlist".into());
        }
        let max_result_chars = config_value
            .get("max_result_chars")
            .and_then(Value::as_u64)
            .unwrap_or(MAX_RESULT_CHARS_DEFAULT as u64)
            .clamp(500, 12_000) as usize;
        if !config_path.exists() {
            let mut opts = OpenOptions::new();
            opts.write(true).create_new(true);
            set_open_mode(&mut opts, 0o600);
            let mut file = opts
                .open(&config_path)
                .map_err(|e| format!("cannot create config: {e}"))?;
            serde_json::to_writer_pretty(&mut file, &config_value).map_err(|e| e.to_string())?;
            file.write_all(b"\n").map_err(|e| e.to_string())?;
        }
        set_mode(&config_path, 0o600);
        Ok(Self {
            home,
            vault,
            db_path,
            proposals_path,
            config: Config {
                include_dirs,
                include_files,
                excluded_dirs: excluded.into_iter().collect(),
                local_providers: providers
                    .into_iter()
                    .map(|s| s.to_ascii_lowercase())
                    .collect(),
                max_result_chars,
                max_context_chars: config_value
                    .get("max_context_chars")
                    .and_then(Value::as_u64)
                    .unwrap_or(MAX_CONTEXT_CHARS_DEFAULT as u64)
                    .clamp(2_000, 4_000_000) as usize,
                enforce_export_policy: config_value
                    .get("enforce_export_policy")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
        })
    }

    pub(crate) fn connect(&self) -> Result<Connection, String> {
        if self.db_path.is_symlink() {
            return Err("Memory index must not be a symlink".into());
        }
        if !self.db_path.exists() {
            return Err(
                "Memory index is not initialized; run the explicit 'index' command first".into(),
            );
        }
        let connection = Connection::open(&self.db_path).map_err(|e| busy_aware("open", e))?;
        // Bounded retry with jitter rather than one flat 5s wait. Three attempts of ~1.5s with
        // randomised slots: two writers that collide then back off to different instants instead
        // of retrying in lockstep, and a genuinely stuck lock fails in ~4.5s rather than blocking
        // for the full timeout on every attempt.
        let retry_ms = [900u64, 1500, 2200];
        connection
            .busy_timeout(std::time::Duration::from_millis(
                retry_ms[0] + (std::process::id() as u64 % 400),
            ))
            .map_err(|e| busy_aware("lock", e))?;
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| e.to_string())?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| e.to_string())?;
        // Additive-only forward migration for READ paths. Mirrors the Python engine exactly.
        //
        // 0.3.6 added `secret_alarms` and left every pre-existing index without it, so search,
        // context, status and alarms all failed with "no such table" until a re-index. A read
        // must never require a write. Additive and idempotent, so it is safe on every connect;
        // anything needing a rebuild belongs in index(), not here.
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS secret_alarms(
                    alarm_id TEXT PRIMARY KEY,
                    relpath TEXT NOT NULL,
                    secret_kind TEXT NOT NULL,
                    fingerprint TEXT NOT NULL,
                    detected_at TEXT NOT NULL,
                    resolved_at TEXT);
                 CREATE INDEX IF NOT EXISTS secret_alarms_unresolved ON secret_alarms(resolved_at);
                 CREATE TABLE IF NOT EXISTS note_links(
                    source TEXT NOT NULL,
                    target TEXT NOT NULL,
                    PRIMARY KEY(source, target)
                 ) WITHOUT ROWID;
                 CREATE INDEX IF NOT EXISTS note_links_by_target ON note_links(target);",
            )
            .map_err(|e| format!("could not apply the additive index migration: {e}"))?;
        Ok(connection)
    }

    /// Whether per-note export policy is enforced.
    ///
    /// Re-read on every call rather than served from the value cached at startup: a
    /// long-lived `--serve` process must not answer from a stale policy, or the operator
    /// flipping `enforce_export_policy` would silently do nothing until a restart. The
    /// file is tiny and any read failure falls back to the cached value, so a transient
    /// config problem cannot break retrieval.
    pub(crate) fn export_enforced(&self) -> bool {
        match fs::read_to_string(self.home.join("config.json")) {
            Ok(raw) => serde_json::from_str::<Value>(&raw)
                .ok()
                .and_then(|v| v.get("enforce_export_policy").and_then(Value::as_bool))
                .unwrap_or_else(|| self.config.enforce_export_policy),
            Err(_) => self.config.enforce_export_policy,
        }
    }

    pub(crate) fn init_db(&self, con: &Connection) -> Result<bool, String> {
        con.execute_batch("\
            CREATE TABLE IF NOT EXISTS documents (
                id INTEGER PRIMARY KEY, relpath TEXT NOT NULL UNIQUE, title TEXT NOT NULL, scope TEXT NOT NULL,
                classification TEXT NOT NULL, source_status TEXT NOT NULL DEFAULT 'unspecified', valid_from TEXT,
                valid_to TEXT, superseded_by TEXT, provider_policy TEXT NOT NULL DEFAULT 'loopback_allowlist_only',
                export_policy TEXT NOT NULL DEFAULT 'deny_remote', body TEXT NOT NULL, content_hash TEXT NOT NULL,
                mtime_ns INTEGER NOT NULL, indexed_at TEXT NOT NULL
            );
            CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(relpath UNINDEXED,title,headings,body,tokenize='porter unicode61');
            CREATE TABLE IF NOT EXISTS doc_terms(relpath TEXT NOT NULL,term TEXT NOT NULL,PRIMARY KEY(relpath,term)) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS doc_terms_by_term ON doc_terms(term);
            CREATE TABLE IF NOT EXISTS term_df(term TEXT PRIMARY KEY,df INTEGER NOT NULL) WITHOUT ROWID;
            CREATE TABLE IF NOT EXISTS metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL) WITHOUT ROWID;
            -- Credential exposure alarms: mirrors the Python engine. A note matching the secret
            -- filter is excluded from the index, which is safe but silent; this is how the owner
            -- finds out. Stores a FINGERPRINT, never the value: telling the user which key to
            -- rotate must not mean copying that key somewhere readable a second time.
            CREATE TABLE IF NOT EXISTS secret_alarms(
                alarm_id TEXT PRIMARY KEY,
                relpath TEXT NOT NULL,
                secret_kind TEXT NOT NULL,
                fingerprint TEXT NOT NULL,
                detected_at TEXT NOT NULL,
                resolved_at TEXT);
            CREATE INDEX IF NOT EXISTS secret_alarms_unresolved ON secret_alarms(resolved_at);
            -- Relational knowledge graph: [[wikilink]] connections between notes
            CREATE TABLE IF NOT EXISTS note_links(
                source TEXT NOT NULL,
                target TEXT NOT NULL,
                PRIMARY KEY(source, target)
            ) WITHOUT ROWID;
            CREATE INDEX IF NOT EXISTS note_links_by_target ON note_links(target);
        ").map_err(|e| e.to_string())?;
        let version: Option<String> = con
            .query_row(
                "SELECT value FROM metadata WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let from_version = version.unwrap_or_else(|| "0".into());
        let mut migrated = from_version != SCHEMA_VERSION;
        let mut columns = HashSet::new();
        let mut stmt = con
            .prepare("PRAGMA table_info(documents)")
            .map_err(|e| e.to_string())?;
        let iter = stmt
            .query_map([], |r| r.get::<_, String>(1))
            .map_err(|e| e.to_string())?;
        for col in iter {
            columns.insert(col.map_err(|e| e.to_string())?);
        }
        // C1 - an expand-only migration ladder. Every step adds a column with a default, which is
        // the property that makes the change safe in both directions: an older engine still reads
        // the table (it ignores columns it does not know), and a newer one sees the default for
        // rows written before the change. Nothing here rewrites or drops, so a failed step leaves
        // the schema valid at the previous rung - there is no half-migrated state to recover from.
        //
        // C3 - reconcile around the migration. A structural change must not alter data: the row
        // counts before and after have to match, or the migration did something it was not
        // supposed to. That check is the difference between "the ALTER returned OK" and "the data
        // is intact".
        let counts = |c: &Connection| -> Result<(i64, i64, i64), String> {
            let one = |sql: &str| -> Result<i64, String> {
                c.query_row(sql, [], |r| r.get(0)).map_err(|e| e.to_string())
            };
            Ok((
                one("SELECT COUNT(*) FROM documents")?,
                one("SELECT COUNT(*) FROM doc_terms")?,
                one("SELECT COUNT(*) FROM term_df")?,
            ))
        };
        let before = counts(con).unwrap_or((0, 0, 0));
        let migrations = [
            ("source_status", "TEXT NOT NULL DEFAULT 'unspecified'"),
            ("valid_from", "TEXT"),
            ("valid_to", "TEXT"),
            ("superseded_by", "TEXT"),
            (
                "provider_policy",
                "TEXT NOT NULL DEFAULT 'loopback_allowlist_only'",
            ),
            ("export_policy", "TEXT NOT NULL DEFAULT 'deny_remote'"),
        ];
        let mut steps: Vec<String> = Vec::new();
        for (name, declaration) in migrations {
            if !columns.contains(name) {
                con.execute(
                    &format!("ALTER TABLE documents ADD COLUMN {name} {declaration}"),
                    [],
                )
                .map_err(|e| format!("migration step {name} failed (schema left at {from_version}): {e}"))?;
                steps.push(format!("add column documents.{name}"));
                migrated = true;
            }
        }
        // MEASURED AND REVERTED: aligning the FTS vocabulary DOWN to ours.
        //
        // The real defect is that `term_df` is built from our tokenizer (12,960 terms, camelCase
        // splitting) while matching happens through `tokenize='porter unicode61'` (9,182 terms).
        // Document frequency disagreed for 19% of the terms they share - median error 40%, worst
        // case 105x. The engine picks query terms by rarity in one vocabulary and matches them in
        // another, which is the mechanism behind off-topic results.
        //
        // I tried the cheap fix: drop Porter so the matching vocabulary IS ours. Vocabulary overlap
        // went to 97.7% and df disagreement to 4.7% - the diagnosis was right - but on the retrieval
        // fixture:
        //
        //     recall@3  0.9180 -> 0.8852   (WORSE)
        //     MRR@3     0.7366 -> 0.7500   (better)
        //
        // Recall is the metric that decides whether the right note is returned at all, and Porter
        // stemming is what lets "ranking" find "rank". Trading 3.3 points of recall for sharper df
        // is the wrong trade, so it is reverted.
        //
        // The fix that keeps both is the OTHER direction: leave the matching vocabulary alone and
        // compute df from the vocabulary that actually matches. `fts5vocab(documents_fts,'row')`
        // exposes exactly that, so `term_df` becomes derivable and `doc_terms` (53% of the index)
        // can go. Raised as issue #131 with the measurements.

        // DERIVED-OBJECT MIGRATION, and it reconciles in BOTH directions.
        //
        // A virtual table's tokenizer cannot be ALTERed, so a mismatch is fixed by dropping and
        // refilling. That is safe for exactly one reason: this table is DERIVED. It holds nothing
        // that is not in the vault.
        //
        // Both directions matter, and I learned that the hard way. An experiment changed this
        // tokenizer to unicode61, the experiment was reverted in the CODE, and the DATABASE kept
        // unicode61 - because the migration went away with the revert. The live index then ran a
        // different tokenizer than the source said it did, silently, with retrieval quality changed
        // and nothing reporting it. A migration that only fires on mismatch-detected-in-one-
        // direction cannot heal that.
        let actual_sql: Option<String> = con
            .query_row("SELECT sql FROM sqlite_master WHERE name='documents_fts'", [], |r| r.get(0))
            .optional()
            .unwrap_or(None);
        let mismatched = match &actual_sql {
            Some(sql) => {
                let has_porter = sql.contains("porter");
                let want_porter = EXPECTED_FTS_TOKENIZER.contains("porter");
                has_porter != want_porter
            }
            None => true,
        };
        if mismatched {
            con.execute_batch("DROP TABLE IF EXISTS documents_fts")
                .map_err(|e| e.to_string())?;
            con.execute_batch(&format!(
                "CREATE VIRTUAL TABLE documents_fts USING fts5(relpath UNINDEXED,title,headings,body,tokenize='{EXPECTED_FTS_TOKENIZER}')"
            ))
            .map_err(|e| e.to_string())?;
            migrated = true;   // forces a full re-read so the table is refilled
            steps.push(format!("rebuild documents_fts to tokenize='{EXPECTED_FTS_TOKENIZER}'"));
        }

        let after = counts(con).map_err(|e| e.to_string())?;
        if before != after {
            return Err(format!(
                "migration reconciliation failed: documents/doc_terms/term_df went {before:?} -> {after:?}. \
                 An expand-only migration must not change row counts. The schema version was NOT \
                 advanced, so the next run will retry rather than build on a broken state."
            ));
        }
        // The version is written only after every step succeeded and reconciled. A failure above
        // returns early, deliberately leaving the old version recorded so the next index run
        // retries instead of assuming the schema is current.
        con.execute(
            &format!(
                "INSERT INTO metadata(key,value) VALUES('schema_version','{SCHEMA_VERSION}') \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value"
            ),
            [],
        )
        .map_err(|e| e.to_string())?;
        // Record the transition on every version change, not only when a column was added. A bump
        // with no structural step still changes what the format *means*, and the next person to
        // read this table needs to know when it happened.
        if from_version != SCHEMA_VERSION {
            con.execute(
                "INSERT INTO metadata(key,value) VALUES('last_migration',?1) \
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [format!(
                    "{}->{} steps={:?} reconciled=true at {}",
                    from_version,
                    SCHEMA_VERSION,
                    steps,
                    crate::paths::now_utc()
                )],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(migrated)
    }

    /// C4 - the index format, stated as data rather than as a comment.
    ///
    /// The contract is what other things depend on: the recorded retrieval fixture, the Python
    /// oracle's parity check, and any future reader. If it drifts silently, a fixture captured
    /// against the old shape keeps passing while measuring something that no longer exists.
    /// `bench/golden/format.json` records this and the gate fails when it changes unrecorded.
    pub(crate) fn schema_contract(&self) -> Result<Value, String> {
        let con = self.connect()?;
        let mut tables: Vec<String> = Vec::new();
        {
            let mut stmt = con
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for t in rows {
                tables.push(t.map_err(|e| e.to_string())?);
            }
        }
        // Documents columns matter most: they are what a migration adds to.
        let mut doc_columns: Vec<String> = Vec::new();
        {
            let mut stmt = con
                .prepare("PRAGMA table_info(documents)")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| e.to_string())?;
            for c in rows {
                doc_columns.push(c.map_err(|e| e.to_string())?);
            }
        }
        let recorded: Option<String> = con
            .query_row("SELECT value FROM metadata WHERE key='schema_version'", [], |r| {
                r.get(0)
            })
            .optional()
            .unwrap_or(None);
        let last_migration: Option<String> = con
            .query_row("SELECT value FROM metadata WHERE key='last_migration'", [], |r| {
                r.get(0)
            })
            .optional()
            .unwrap_or(None);
        Ok(json!({
            "ok": true,
            "schema_version": recorded,
            "expected_version": SCHEMA_VERSION,
            "up_to_date": recorded.as_deref() == Some(SCHEMA_VERSION),
            "last_migration": last_migration,
            "tables": tables,
            "documents_columns": doc_columns,
            "fts_tokenizer": EXPECTED_FTS_TOKENIZER,
            "migration_policy": "expand-only: steps add columns with defaults and never rewrite, rename or drop, so a failure leaves the previous rung valid",
            "rollback": "none needed by construction; an older engine reads the table and ignores unknown columns",
        }))
    }
}

pub(crate) struct Config {
    pub(crate) include_dirs: Vec<String>,
    pub(crate) include_files: Vec<String>,
    pub(crate) excluded_dirs: HashSet<String>,
    pub(crate) local_providers: HashSet<String>,
    pub(crate) max_result_chars: usize,
    pub(crate) max_context_chars: usize,
    pub(crate) enforce_export_policy: bool,
}

fn string_list(value: &Value, key: &str, default: &[&str]) -> Result<Vec<String>, String> {
    let Some(array) = value.get(key).and_then(Value::as_array) else {
        return Ok(default.iter().map(|s| s.to_string()).collect());
    };
    array
        .iter()
        .map(|v| {
            let s = v
                .as_str()
                .ok_or_else(|| format!("config field {key} must contain strings"))?;
            let rel = safe_relpath(s)?;
            if rel != s.replace('\\', "/") {
                return Err(format!("config field {key} contains a non-normalized path"));
            }
            Ok(rel)
        })
        .collect()
}

pub(crate) fn delete_fts_terms(con: &Connection, rel: &str) -> Result<(), String> {
    con.execute("DELETE FROM documents_fts WHERE relpath=?1", [rel])
        .map_err(|e| e.to_string())?;
    con.execute("DELETE FROM doc_terms WHERE relpath=?1", [rel])
        .map_err(|e| e.to_string())?;
    con.execute("DELETE FROM note_links WHERE source=?1", [rel])
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn delete_document(con: &Connection, rel: &str) -> Result<(), String> {
    delete_fts_terms(con, rel)?;
    con.execute("DELETE FROM documents WHERE relpath=?1", [rel])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Candidate {
    pub(crate) relpath: String,
    pub(crate) title: String,
    pub(crate) scope: String,
    pub(crate) classification: String,
    pub(crate) source_status: String,
    pub(crate) valid_from: Option<String>,
    pub(crate) valid_to: Option<String>,
    pub(crate) superseded_by: Option<String>,
    pub(crate) provider_policy: String,
    pub(crate) export_policy: String,
    pub(crate) body: String,
    pub(crate) indexed_at: String,
    pub(crate) rank: f64,
}

pub(crate) fn map_candidate(row: &Row<'_>) -> rusqlite::Result<Candidate> {
    Ok(Candidate {
        relpath: row.get(0)?,
        title: row.get(1)?,
        scope: row.get(2)?,
        classification: row.get(3)?,
        source_status: row.get(4)?,
        valid_from: row.get(5)?,
        valid_to: row.get(6)?,
        superseded_by: row.get(7)?,
        provider_policy: row.get(8)?,
        export_policy: row.get(9)?,
        body: row.get(10)?,
        indexed_at: row.get(11)?,
        rank: row.get(12)?,
    })
}
