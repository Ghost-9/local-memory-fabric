use chrono::{NaiveDate, SecondsFormat, Utc};
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use similar::TextDiff;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::UNIX_EPOCH;
use uuid::Uuid;

const VERSION: &str = "0.2.0";
const MAX_REQUEST_BYTES: usize = 131_072;
const MAX_RESULT_CHARS_DEFAULT: usize = 6_000;
const DEFAULT_INCLUDE_DIRS: &[&str] = &[
    "agents",
    "company",
    "daily",
    "evidence",
    "pi-harness",
    "projects",
];
const DEFAULT_INCLUDE_FILES: &[&str] = &["README.md", "ROADMAP.md", "STATUS.md", "Welcome.md"];
const DEFAULT_EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".obsidian",
    ".trash",
    "archive",
    "archives",
    "templates",
    "apple-notes",
    "apple_notes",
    "notes-export",
    "notes_export",
];
const STOPWORDS: &str = "a an and are as at be been being but by can could did do does for from had has have how i in into is it its may might of on or our should so than that the their then there these this those to was we were what when where which who why will with would you your";

fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn regex(pattern: &str) -> Regex {
    static CACHE: OnceLock<Mutex<HashMap<String, Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(compiled) = guard.get(pattern) {
            return compiled.clone();
        }
    }
    let compiled = Regex::new(pattern).expect("regular expression must compile");
    if pattern.len() <= 512 {
        if let Ok(mut guard) = cache.lock() {
            if guard.len() < 256 {
                guard
                    .entry(pattern.to_string())
                    .or_insert_with(|| compiled.clone());
            }
        }
    }
    compiled
}

fn apple_path_re() -> Regex {
    regex(r"(?i)(?:apple[ _-]*notes|notes[ _-]*export)")
}

fn secret_signature(text: &str) -> Option<&'static str> {
    let patterns: [(&str, &str); 9] = [
        (
            "private_key",
            r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----",
        ),
        ("aws_access_key", r"\bAKIA[0-9A-Z]{16}\b"),
        (
            "github_token",
            r"\b(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})\b",
        ),
        ("slack_token", r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b"),
        ("stripe_live_key", r"\b(?:sk|rk)_live_[A-Za-z0-9]{16,}\b"),
        ("google_api_key", r"\bAIza[0-9A-Za-z_-]{30,}\b"),
        (
            "openai_api_key",
            r"(?:^|[^A-Za-z0-9])sk-(?:proj-)?[A-Za-z0-9_-]{20,}(?:$|[^A-Za-z0-9])",
        ),
        (
            "jwt",
            r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{8,}\b",
        ),
        (
            "bearer_credential",
            r"(?i)\bBearer\s+[A-Za-z0-9._~+/-]{24,}={0,2}",
        ),
    ];
    for (kind, pattern) in patterns {
        if regex(pattern).is_match(text) {
            return Some(kind);
        }
    }
    let assignment = regex(
        r#"(?i)(?:^|[^A-Za-z0-9])([A-Za-z0-9_-]*(?:api[_-]?key|client[_-]?secret|service[_-]?role[_-]?key|access[_-]?key|private[_-]?key|secret|password|passwd|token)[A-Za-z0-9_-]*)\s*[:=]\s*['\"]?([^\s'\"]{12,})"#,
    );
    let placeholders = [
        "placeholder",
        "redacted",
        "example",
        "sample",
        "change-me",
        "changeme",
        "your_",
        "your-",
        "insert_",
        "insert-",
        "replace_",
        "replace-",
        "not-a-real",
        "not_a_real",
        "dummy",
        "fake",
    ];
    if let Some(captures) = assignment.captures(text) {
        let value = captures
            .get(2)
            .map(|m| {
                m.as_str()
                    .trim_matches(['\'', '"', ' '])
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        if !placeholders.iter().any(|marker| value.contains(marker))
            && !regex(r"^<[^>]+>$").is_match(&value)
        {
            return Some("credential_assignment");
        }
    }
    if regex(r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/:@]+:[^\s/@]+@").is_match(text) {
        return Some("credential_url");
    }
    None
}

fn tokenize(text: &str) -> BTreeSet<String> {
    let stop: HashSet<&str> = STOPWORDS.split_whitespace().collect();
    regex(r"[A-Za-z0-9]{3,}")
        .find_iter(text)
        .map(|m| m.as_str().to_ascii_lowercase())
        .filter(|term| !stop.contains(term.as_str()))
        .collect()
}

fn safe_relpath(value: &str) -> Result<String, String> {
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err("source reference is not an allowed relative path".into());
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().to_string()),
            _ => return Err("source reference is not an allowed relative path".into()),
        }
    }
    if parts.is_empty() {
        return Err("source reference is not an allowed relative path".into());
    }
    Ok(parts.join("/"))
}

fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    if !text.starts_with("---\n") {
        return None;
    }
    let end = text[4..].find("\n---")? + 4;
    let header = &text[4..end];
    let pattern = format!(r#"(?im)^{}\s*:\s*[\"']?([^\"'\n#]+)"#, regex::escape(key));
    regex(&pattern)
        .captures(header)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn title_for(relpath: &str, text: &str) -> String {
    for line in text.lines().take(80) {
        if let Some(c) = regex(r"^#\s+(.+?)\s*#*\s*$").captures(line) {
            return c[1].trim().to_string();
        }
        if let Some(c) = regex(r#"(?i)^title:\s*[\"']?(.+?)[\"']?\s*$"#).captures(line) {
            return c[1].trim().to_string();
        }
    }
    Path::new(relpath)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace('-', " ")
        .replace('_', " ")
}

fn headings_for(text: &str) -> String {
    text.lines()
        .filter(|line| regex(r"^#{1,6}\s+").is_match(line))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
}

fn scope_for(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.first().copied().unwrap_or("") {
        "projects" => {
            let project = if parts.len() > 2 {
                parts[1].to_string()
            } else if parts.len() == 2 {
                Path::new(parts[1])
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            } else {
                "unknown".into()
            };
            format!("project:{project}")
        }
        "company" => "company".into(),
        "evidence" => "evidence".into(),
        "daily" => "episodic".into(),
        "pi-harness" => "system:pi-harness".into(),
        _ => "system".into(),
    }
}

fn is_skipped_name(name: &str, excluded: &HashSet<String>) -> bool {
    name.starts_with('.')
        || excluded.iter().any(|item| item.eq_ignore_ascii_case(name))
        || apple_path_re().is_match(name)
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn json_error(message: impl Into<String>) -> Value {
    json!({"ok": false, "error": message.into()})
}

struct Config {
    include_dirs: Vec<String>,
    include_files: Vec<String>,
    excluded_dirs: HashSet<String>,
    local_providers: HashSet<String>,
    max_result_chars: usize,
}

struct MemoryFabric {
    home: PathBuf,
    vault: PathBuf,
    db_path: PathBuf,
    proposals_path: PathBuf,
    config: Config,
}

impl MemoryFabric {
    fn new() -> Result<Self, String> {
        let home = env::var_os("MEMORY_FABRIC_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".pi/agent/scratch/memory-fabric"));
        let vault = env::var_os("MEMORY_FABRIC_VAULT")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("system-memory"));
        Self::with_paths(home, vault)
    }

    fn with_paths(home: PathBuf, vault: PathBuf) -> Result<Self, String> {
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
            file.write_all(b"\\n").map_err(|e| e.to_string())?;
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
            },
        })
    }

    fn is_local_provider(&self, provider: Option<&str>) -> bool {
        provider
            .map(|p| {
                self.config
                    .local_providers
                    .contains(&p.trim().to_ascii_lowercase())
            })
            .unwrap_or(false)
    }

    fn connect(&self) -> Result<Connection, String> {
        if self.db_path.is_symlink() {
            return Err("Memory index must not be a symlink".into());
        }
        if !self.db_path.exists() {
            return Err(
                "Memory index is not initialized; run the explicit 'index' command first".into(),
            );
        }
        let connection = Connection::open(&self.db_path).map_err(|e| e.to_string())?;
        connection
            .busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        connection
            .pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| e.to_string())?;
        Ok(connection)
    }

    fn init_db(&self, con: &Connection) -> Result<bool, String> {
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
        ").map_err(|e| e.to_string())?;
        let version: Option<String> = con
            .query_row(
                "SELECT value FROM metadata WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let mut migrated = version.as_deref() != Some("2");
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
        for (name, declaration) in migrations {
            if !columns.contains(name) {
                con.execute(
                    &format!("ALTER TABLE documents ADD COLUMN {name} {declaration}"),
                    [],
                )
                .map_err(|e| e.to_string())?;
                migrated = true;
            }
        }
        con.execute("INSERT INTO metadata(key,value) VALUES('schema_version','2') ON CONFLICT(key) DO UPDATE SET value=excluded.value", []).map_err(|e| e.to_string())?;
        Ok(migrated)
    }

    fn sources(&self) -> Result<Vec<PathBuf>, String> {
        let meta = fs::symlink_metadata(&self.vault)
            .map_err(|_| "Configured vault root is unavailable".to_string())?;
        if meta.file_type().is_symlink() {
            return Err("Configured vault root must not be a symlink".into());
        }
        let mut found = Vec::new();
        for rel in &self.config.include_dirs {
            if is_skipped_name(rel, &self.config.excluded_dirs) {
                continue;
            }
            let candidate = self.vault.join(rel);
            if candidate
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true)
            {
                continue;
            }
            if !candidate.is_dir() {
                continue;
            }
            self.walk(&candidate, &mut found);
        }
        for rel in &self.config.include_files {
            if is_skipped_name(rel, &self.config.excluded_dirs) {
                continue;
            }
            let candidate = self.vault.join(rel);
            if candidate
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true)
            {
                continue;
            }
            if candidate.is_file()
                && candidate
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|s| s.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            {
                found.push(candidate);
            }
        }
        found.sort();
        found.dedup();
        Ok(found)
    }

    fn walk(&self, root: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(root) else {
            return;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().to_string();
            if is_skipped_name(&name, &self.config.excluded_dirs) {
                continue;
            }
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                self.walk(&path, found);
            } else if meta.is_file()
                && path
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|s| s.eq_ignore_ascii_case("md"))
                    .unwrap_or(false)
            {
                found.push(path);
            }
        }
    }

    fn index(&self, force: bool) -> Result<Value, String> {
        if self.home.is_symlink() {
            return Err("Memory Fabric state directory must not be a symlink".into());
        }
        fs::create_dir_all(&self.home).map_err(|e| e.to_string())?;
        if self.db_path.is_symlink() {
            return Err("Memory index must not be a symlink".into());
        }
        let con = Connection::open(&self.db_path).map_err(|e| e.to_string())?;
        con.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        con.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        let migrated = self.init_db(&con)?;
        let existing: HashMap<String, String> = {
            let mut stmt = con
                .prepare("SELECT relpath,content_hash FROM documents")
                .map_err(|e| e.to_string())?;
            let mut out = HashMap::new();
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (p, h) = row.map_err(|e| e.to_string())?;
                out.insert(p, h);
            }
            out
        };
        let tx = con.unchecked_transaction().map_err(|e| e.to_string())?;
        let mut found = HashSet::new();
        let (mut added, mut updated, mut unchanged, mut excluded_sensitive) =
            (0usize, 0usize, 0usize, 0usize);
        for path in self.sources()? {
            let canonical_vault = self.vault.canonicalize().map_err(|e| e.to_string())?;
            let Ok(canonical_path) = path.canonicalize() else {
                continue;
            };
            if !canonical_path.starts_with(&canonical_vault) {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let rel = path
                .strip_prefix(&self.vault)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            found.insert(rel.clone());
            let classification = frontmatter_value(&text, "classification")
                .unwrap_or_else(|| "local_only".into())
                .to_ascii_lowercase();
            if ["secret", "restricted", "excluded"].contains(&classification.as_str())
                || secret_signature(&text).is_some()
            {
                excluded_sensitive += 1;
                if existing.contains_key(&rel) {
                    delete_document(&tx, &rel)?;
                }
                continue;
            }
            let digest = sha256(&text);
            if !force && !migrated && existing.get(&rel) == Some(&digest) {
                unchanged += 1;
                continue;
            }
            let title = title_for(&rel, &text);
            let headings = headings_for(&text);
            let scope = scope_for(&rel);
            let status = frontmatter_value(&text, "status")
                .unwrap_or_else(|| "unspecified".into())
                .trim()
                .to_ascii_lowercase();
            let valid_from = frontmatter_value(&text, "valid_from");
            let valid_to = frontmatter_value(&text, "valid_to");
            let superseded_by = frontmatter_value(&text, "superseded_by");
            let mtime_ns = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
                .unwrap_or(0);
            if existing.contains_key(&rel) {
                updated += 1;
                delete_fts_terms(&tx, &rel)?;
            } else {
                added += 1;
            }
            tx.execute("INSERT INTO documents(relpath,title,scope,classification,source_status,valid_from,valid_to,superseded_by,provider_policy,export_policy,body,content_hash,mtime_ns,indexed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'loopback_allowlist_only','deny_remote',?9,?10,?11,?12) ON CONFLICT(relpath) DO UPDATE SET title=excluded.title,scope=excluded.scope,classification=excluded.classification,source_status=excluded.source_status,valid_from=excluded.valid_from,valid_to=excluded.valid_to,superseded_by=excluded.superseded_by,provider_policy=excluded.provider_policy,export_policy=excluded.export_policy,body=excluded.body,content_hash=excluded.content_hash,mtime_ns=excluded.mtime_ns,indexed_at=excluded.indexed_at",
                params![rel,title,scope,classification,status,valid_from,valid_to,superseded_by,text,digest,mtime_ns,now_utc()]).map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO documents_fts(relpath,title,headings,body) VALUES(?1,?2,?3,?4)",
                params![rel, title, headings, text],
            )
            .map_err(|e| e.to_string())?;
            let all_terms = tokenize(&format!("{title}\n{headings}\n{text}"));
            for term in all_terms {
                tx.execute(
                    "INSERT OR IGNORE INTO doc_terms(relpath,term) VALUES(?1,?2)",
                    params![rel, term],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        let stale: Vec<String> = existing
            .keys()
            .filter(|p| !found.contains(*p))
            .cloned()
            .collect();
        for rel in &stale {
            delete_document(&tx, rel)?;
        }
        tx.execute("DELETE FROM term_df", [])
            .map_err(|e| e.to_string())?;
        tx.execute(
            "INSERT INTO term_df(term,df) SELECT term,COUNT(*) FROM doc_terms GROUP BY term",
            [],
        )
        .map_err(|e| e.to_string())?;
        tx.execute("INSERT INTO metadata(key,value) VALUES('last_indexed_at',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [now_utc()]).map_err(|e| e.to_string())?;
        let canonical = self
            .vault
            .canonicalize()
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .to_string();
        tx.execute("INSERT INTO metadata(key,value) VALUES('vault_path',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [canonical]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        set_mode(&self.db_path, 0o600);
        let docs: i64 = con
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        Ok(
            json!({"ok":true,"version":VERSION,"added":added,"updated":updated,"unchanged":unchanged,"removed_from_index":stale.len(),"excluded_sensitive":excluded_sensitive,"documents":docs}),
        )
    }

    fn status(&self) -> Result<Value, String> {
        if !self.db_path.exists() {
            return Ok(
                json!({"ok":true,"version":VERSION,"indexed":false,"documents":0,"message":"Run the explicit index command to initialize."}),
            );
        }
        let con = self.connect()?;
        let docs: i64 = con
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let last: Option<String> = con
            .query_row(
                "SELECT value FROM metadata WHERE key='last_indexed_at'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        Ok(
            json!({"ok":true,"version":VERSION,"indexed":true,"documents":docs,"last_indexed_at":last}),
        )
    }

    fn search(
        &self,
        query: &str,
        scope: &str,
        limit: usize,
        provider: Option<&str>,
        as_of: Option<&str>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Memory content is available only to an explicitly allowed local model provider."}),
            );
        }
        if query.trim().is_empty() {
            return Ok(json_error("query is required"));
        }
        let as_of_date = match as_of {
            Some(s) => match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                Ok(d) => Some(d),
                Err(_) => return Ok(json_error("as_of must be an ISO date (YYYY-MM-DD)")),
            },
            None => None,
        };
        let terms = tokenize(query);
        if terms.is_empty() {
            return Ok(json!({"ok":true,"results":[],"abstention":"no searchable terms"}));
        }
        let con = self.connect()?;
        let n_docs: i64 = con
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let mut df = HashMap::new();
        {
            let mut stmt = con
                .prepare("SELECT term,df FROM term_df")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (t, n) = row.map_err(|e| e.to_string())?;
                df.insert(t, n);
            }
        }
        let max_df = std::cmp::max(2, (n_docs as f64 * 0.25) as i64);
        let mut useful: Vec<String> = terms
            .into_iter()
            .filter(|t| {
                df.get(t).copied().unwrap_or(0) > 0 && df.get(t).copied().unwrap_or(0) <= max_df
            })
            .collect();
        useful.sort_by(|a, b| {
            df[a]
                .cmp(&df[b])
                .then_with(|| b.len().cmp(&a.len()))
                .then_with(|| a.cmp(b))
        });
        useful.truncate(16);
        if useful.is_empty() {
            return Ok(
                json!({"ok":true,"results":[],"abstention":"no discriminative indexed terms"}),
            );
        }
        let match_query = useful
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        let mut stmt=con.prepare("SELECT d.relpath,d.title,d.scope,d.classification,d.source_status,d.valid_from,d.valid_to,d.superseded_by,d.provider_policy,d.export_policy,d.body,d.indexed_at,bm25(documents_fts,0.0,4.0,1.5,1.0) AS rank FROM documents_fts JOIN documents d ON d.relpath=documents_fts.relpath WHERE documents_fts MATCH ?1 AND (?2='all' OR ?2='' OR d.scope=?2) ORDER BY rank LIMIT ?3").map_err(|e|e.to_string())?;
        let raw = stmt
            .query_map(
                params![match_query, scope, (limit.clamp(1, 20) * 10) as i64],
                map_candidate,
            )
            .map_err(|e| e.to_string())?;
        let mut candidates = Vec::new();
        for row in raw {
            candidates.push(row.map_err(|e| e.to_string())?);
        }
        let mut results = Vec::new();
        let mut excluded_temporal = 0;
        let mut temporal_without_as_of = false;
        for row in candidates {
            let valid_from = match parse_optional_date(row.valid_from.as_deref()) {
                Ok(x) => x,
                Err(()) => {
                    excluded_temporal += 1;
                    continue;
                }
            };
            let valid_to = match parse_optional_date(row.valid_to.as_deref()) {
                Ok(x) => x,
                Err(()) => {
                    excluded_temporal += 1;
                    continue;
                }
            };
            let status = row.source_status.trim().to_ascii_lowercase();
            if as_of_date.is_none() && (valid_from.is_some() || valid_to.is_some()) {
                temporal_without_as_of = true;
            }
            if let Some(d) = as_of_date {
                if valid_from.map(|v| d < v).unwrap_or(false)
                    || valid_to.map(|v| d > v).unwrap_or(false)
                {
                    continue;
                }
            }
            if ["superseded", "archived", "deprecated", "historical"].contains(&status.as_str())
                && (as_of_date.is_none()
                    || valid_to.is_none()
                    || as_of_date.unwrap() > valid_to.unwrap())
            {
                continue;
            }
            let mut doc_terms = HashSet::new();
            {
                let mut q = con
                    .prepare("SELECT term FROM doc_terms WHERE relpath=?1")
                    .map_err(|e| e.to_string())?;
                let iter = q
                    .query_map([&row.relpath], |r| r.get::<_, String>(0))
                    .map_err(|e| e.to_string())?;
                for x in iter {
                    doc_terms.insert(x.map_err(|e| e.to_string())?);
                }
            }
            let matched: Vec<String> = useful
                .iter()
                .filter(|t| doc_terms.contains(t.as_str()))
                .cloned()
                .collect();
            let distinctive = matched
                .iter()
                .any(|t| t.len() >= 9 && df.get(t).copied() == Some(1));
            if matched.len() < 2 && !distinctive {
                continue;
            }
            let folded = row.body.to_lowercase();
            let byte_start = matched
                .iter()
                .filter_map(|t| folded.find(t))
                .min()
                .unwrap_or(0);
            let start = row
                .body
                .get(..byte_start)
                .map(|prefix| prefix.chars().count())
                .unwrap_or(0)
                .saturating_sub(240);
            let body_chars = row.body.chars().count();
            let mut excerpt = row
                .body
                .chars()
                .skip(start)
                .take(1000)
                .collect::<String>()
                .trim()
                .to_string();
            if start > 0 {
                excerpt = format!("…{excerpt}");
            }
            if start + 1000 < body_chars {
                excerpt.push('…');
            }
            results.push(json!({"source_ref":row.relpath,"title":row.title,"scope":row.scope,"classification":row.classification,"source_status":row.source_status,"valid_from":row.valid_from,"valid_to":row.valid_to,"superseded_by":row.superseded_by,"provider_policy":row.provider_policy,"export_policy":row.export_policy,"excerpt":excerpt,"matched_terms":matched,"bm25_rank":(row.rank*1_000_000.0).round()/1_000_000.0,"indexed_at":row.indexed_at}));
            if results.len() >= limit.clamp(1, 20) {
                break;
            }
        }
        let mut warning: Option<String> = None;
        'outer: for i in 0..results.len() {
            for j in i + 1..results.len() {
                let a = &results[i];
                let b = &results[j];
                if a["scope"] != b["scope"] || a["source_status"] == b["source_status"] {
                    continue;
                }
                let a0 = a["valid_from"].as_str().unwrap_or("0001-01-01");
                let a1 = a["valid_to"].as_str().unwrap_or("9999-12-31");
                let b0 = b["valid_from"].as_str().unwrap_or("0001-01-01");
                let b1 = b["valid_to"].as_str().unwrap_or("9999-12-31");
                if a0 <= b1 && b0 <= a1 {
                    warning=Some("Overlapping source lifecycle records; inspect provenance and do not auto-resolve the conflict.".into());
                    break 'outer;
                }
            }
        }
        if excluded_temporal > 0 {
            warning =
                Some("Some candidates had invalid temporal metadata and were excluded.".into());
        } else if temporal_without_as_of && as_of.is_none() {
            warning=Some("No as_of date supplied; validity bounds were not evaluated against the host clock.".into());
        }
        Ok(
            json!({"ok":true,"results":results,"abstention":if results.is_empty(){Some("insufficient rare-term overlap or no source valid at the requested date")}else{None},"temporal_warning":warning,"as_of":as_of,"local_only":true,"note":"Lexical candidates are not verified facts; inspect cited source passages."}),
        )
    }

    fn context(
        &self,
        project: Option<&str>,
        query: &str,
        provider: Option<&str>,
        budget: usize,
        as_of: Option<&str>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Memory content is available only to an explicitly allowed local model provider."}),
            );
        }
        let scope = project
            .map(|p| format!("project:{p}"))
            .unwrap_or_else(|| "all".into());
        let hits = self.search(query, &scope, 10, provider, as_of)?;
        if !hits["ok"].as_bool().unwrap_or(false) {
            return Ok(hits);
        }
        let char_budget = budget.clamp(100, 6000) * 4;
        let mut sections = Vec::new();
        let mut used = 0usize;
        if let Some(items) = hits["results"].as_array() {
            for item in items {
                let temporal = format!(
                    "; valid {} to {}",
                    item["valid_from"].as_str().unwrap_or("unspecified"),
                    item["valid_to"].as_str().unwrap_or("open")
                );
                let block = format!(
                    "### {} — {}\nstatus={}; class={}{}; provider={}; export={}\n{}\n",
                    item["title"].as_str().unwrap_or(""),
                    item["source_ref"].as_str().unwrap_or(""),
                    item["source_status"].as_str().unwrap_or("unspecified"),
                    item["classification"].as_str().unwrap_or("local_only"),
                    temporal,
                    item["provider_policy"]
                        .as_str()
                        .unwrap_or("loopback_allowlist_only"),
                    item["export_policy"].as_str().unwrap_or("deny_remote"),
                    item["excerpt"].as_str().unwrap_or("")
                );
                if used + block.chars().count() > char_budget {
                    let remain = char_budget.saturating_sub(used);
                    if remain > 100 {
                        sections.push(block.chars().take(remain).collect::<String>());
                        used += remain;
                    }
                    break;
                }
                used += block.chars().count();
                sections.push(block);
            }
        }
        Ok(
            json!({"ok":true,"project":project,"source_count":sections.len(),"estimated_characters":used,"char_budget":char_budget,"context":sections.join(""),"abstention":hits["abstention"],"as_of":hits["as_of"],"temporal_warning":hits["temporal_warning"],"instruction":"Treat excerpts as source evidence, not verified conclusions; cite source_ref and distinguish inference."}),
        )
    }

    fn indexed_doc(&self, rel: &str, con: &Connection) -> Result<Value, String> {
        let rel = safe_relpath(rel)?;
        if apple_path_re().is_match(&rel) || self.vault.is_symlink() {
            return Err("source path is excluded or unsafe".into());
        }
        let row:Option<Value>=con.query_row("SELECT relpath,title,scope,classification,source_status,valid_from,valid_to,superseded_by,provider_policy,export_policy,body,content_hash FROM documents WHERE relpath=?1",[&rel],|r|Ok(json!({"relpath":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"scope":r.get::<_,String>(2)?,"classification":r.get::<_,String>(3)?,"source_status":r.get::<_,String>(4)?,"valid_from":r.get::<_,Option<String>>(5)?,"valid_to":r.get::<_,Option<String>>(6)?,"superseded_by":r.get::<_,Option<String>>(7)?,"provider_policy":r.get::<_,String>(8)?,"export_policy":r.get::<_,String>(9)?,"body":r.get::<_,String>(10)?,"content_hash":r.get::<_,String>(11)?}))).optional().map_err(|e|e.to_string())?;
        let row = row.ok_or_else(|| "source reference is not in the approved index".to_string())?;
        let mut path = self.vault.clone();
        for part in rel.split('/') {
            path.push(part);
            if path
                .symlink_metadata()
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(true)
            {
                return Err("source path is no longer safe".into());
            }
        }
        let canonical = path
            .canonicalize()
            .map_err(|_| "source path is no longer available".to_string())?;
        let root = self
            .vault
            .canonicalize()
            .map_err(|_| "configured vault root is unavailable".to_string())?;
        if !canonical.starts_with(root) {
            return Err("source path is no longer inside the approved vault".into());
        }
        Ok(row)
    }

    fn read(
        &self,
        source_ref: &str,
        provider: Option<&str>,
        heading: Option<&str>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Memory content is available only to an explicitly allowed local model provider."}),
            );
        }
        let con = self.connect()?;
        let row = match self.indexed_doc(source_ref, &con) {
            Ok(v) => v,
            Err(e) => return Ok(json_error(e)),
        };
        let body = row["body"].as_str().unwrap_or("");
        let current = fs::read_to_string(self.vault.join(safe_relpath(source_ref)?));
        if current
            .as_ref()
            .map(|s| sha256(s) != row["content_hash"].as_str().unwrap_or(""))
            .unwrap_or(true)
        {
            return Ok(
                json!({"ok":false,"stale":true,"message":"Source changed or disappeared; run explicit reindex before reading."}),
            );
        }
        let mut output = body.to_string();
        if let Some(wanted) = heading {
            let lines: Vec<&str> = body.lines().collect();
            let start = lines.iter().position(|line| {
                regex(&format!(
                    r"(?i)^#{{1,6}}\s+{}\s*#*\s*$",
                    regex::escape(wanted)
                ))
                .is_match(line.trim())
            });
            let Some(start) = start else {
                return Ok(json_error("requested heading was not found"));
            };
            let level = lines[start].chars().take_while(|c| *c == '#').count();
            let mut end = lines.len();
            for (i, line) in lines.iter().enumerate().skip(start + 1) {
                let n = line.chars().take_while(|c| *c == '#').count();
                if n > 0
                    && n <= level
                    && line
                        .chars()
                        .nth(n)
                        .map(char::is_whitespace)
                        .unwrap_or(false)
                {
                    end = i;
                    break;
                }
            }
            output = lines[start..end].join("\n");
        }
        let cap = self.config.max_result_chars;
        let truncated = output.chars().count() > cap;
        output = output.chars().take(cap).collect();
        Ok(
            json!({"ok":true,"source_ref":row["relpath"],"title":row["title"],"scope":row["scope"],"classification":row["classification"],"source_status":row["source_status"],"valid_from":row["valid_from"],"valid_to":row["valid_to"],"superseded_by":row["superseded_by"],"provider_policy":row["provider_policy"],"export_policy":row["export_policy"],"content":output,"truncated":truncated,"local_only":true}),
        )
    }

    fn propose(&self, req: &Value) -> Result<Value, String> {
        let project = req["project"].as_str().unwrap_or("");
        let target = req["target"].as_str().unwrap_or("");
        let section = req["section"].as_str().unwrap_or("");
        let text = req["text"].as_str().unwrap_or("");
        let provider = req["provider"].as_str();
        if !regex(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,79}$").is_match(project) {
            return Ok(json_error("invalid project identifier"));
        }
        let rel = match safe_relpath(target) {
            Ok(r) => r,
            Err(e) => return Ok(json_error(e)),
        };
        if !(rel == format!("projects/{project}.md")
            || rel.starts_with(&format!("projects/{project}/")))
        {
            return Ok(json_error(
                "proposal target must be inside the selected project",
            ));
        }
        if section.is_empty()
            || section.chars().count() > 120
            || text.trim().is_empty()
            || text.chars().count() > 20_000
        {
            return Ok(json_error(
                "section and bounded non-empty proposal text are required",
            ));
        }
        if let Some(kind) = secret_signature(text) {
            return Ok(json_error(format!(
                "proposal rejected by local secret filter ({kind}); no content was stored"
            )));
        }
        let con = self.connect()?;
        let target_row = match self.indexed_doc(&rel, &con) {
            Ok(r) => r,
            Err(e) => return Ok(json_error(e)),
        };
        let mut refs = Vec::new();
        if let Some(source_refs) = req["source_refs"].as_array() {
            for r in source_refs {
                let Some(s) = r.as_str() else {
                    return Ok(json_error("source reference must be text"));
                };
                match self.indexed_doc(s, &con) {
                    Ok(item) => refs.push(item["relpath"].clone()),
                    Err(e) => return Ok(json_error(e)),
                }
            }
        }
        let base = target_row["body"].as_str().unwrap_or("");
        let lines: Vec<&str> = base.split_inclusive('\n').collect();
        let start = lines.iter().position(|line| {
            regex(&format!(
                r"(?i)^#{{1,6}}\s+{}\s*#*\s*$",
                regex::escape(section)
            ))
            .is_match(line.trim())
        });
        let Some(start) = start else {
            return Ok(json_error("proposal section heading must already exist"));
        };
        let level = lines[start].chars().take_while(|c| *c == '#').count();
        let mut end = lines.len();
        for (i, line) in lines.iter().enumerate().skip(start + 1) {
            let n = line.chars().take_while(|c| *c == '#').count();
            if n > 0
                && n <= level
                && line
                    .chars()
                    .nth(n)
                    .map(char::is_whitespace)
                    .unwrap_or(false)
            {
                end = i;
                break;
            }
        }
        let mut insert_at = end;
        while insert_at > start + 1 && lines[insert_at - 1].trim().is_empty() {
            insert_at -= 1;
        }
        let mut updated = String::new();
        for line in &lines[..insert_at] {
            updated.push_str(line);
        }
        if insert_at > 0 && !lines[insert_at - 1].trim().is_empty() {
            updated.push('\n');
        }
        updated.push_str(text.trim_end());
        updated.push('\n');
        if end < lines.len() {
            updated.push('\n');
        }
        for line in &lines[end..] {
            updated.push_str(line);
        }
        let diff = TextDiff::from_lines(base, &updated)
            .unified_diff()
            .header(&format!("a/{rel}"), &format!("proposal/{rel}"))
            .to_string();
        fs::create_dir_all(&self.proposals_path).map_err(|e| e.to_string())?;
        if self.proposals_path.is_symlink() {
            return Ok(json_error("Proposal directory must not be a symlink"));
        }
        set_mode(&self.proposals_path, 0o700);
        let id = Uuid::new_v4().simple().to_string()[..12].to_string();
        let record = json!({"id":id,"created_at":now_utc(),"status":"proposed_review_required","classification":"local_only","provider_policy":"loopback_allowlist_only","export_policy":"deny_remote","project":project,"target":rel,"section":section,"source_refs":refs,"proposal_text":text,"diff":diff});
        let path = self.proposals_path.join(format!("{id}.json"));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        set_open_mode(&mut opts, 0o600);
        let mut file = opts.open(path).map_err(|e| e.to_string())?;
        serde_json::to_writer_pretty(&mut file, &record).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        let mut out = json!({"ok":true,"proposal_id":id,"status":"proposed_review_required","writeback":"none; proposal is stored outside the vault for human review"});
        if self.is_local_provider(provider) {
            out["diff"] = json!(diff);
        }
        Ok(out)
    }

    fn proposal_show(&self, id: &str, provider: Option<&str>) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Proposal contents are available only to an explicitly allowed local model provider."}),
            );
        }
        if !regex(r"^[a-f0-9]{12}$").is_match(id) {
            return Ok(json_error("invalid proposal id"));
        }
        let path = self.proposals_path.join(format!("{id}.json"));
        if path.is_symlink() {
            return Ok(json_error("proposal path is unsafe"));
        }
        let record: Value = match fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => return Ok(json_error("proposal not found")),
        };
        Ok(
            json!({"ok":true,"proposal_id":id,"status":record["status"],"target":record["target"],"diff":record["diff"].as_str().unwrap_or("")}),
        )
    }

    fn proposal_list(&self) -> Result<Value, String> {
        if !self.proposals_path.exists() {
            return Ok(json!({"ok":true,"proposals":[]}));
        }
        if self.proposals_path.is_symlink() {
            return Ok(json_error("proposal directory is unsafe"));
        }
        let mut items = Vec::new();
        let Ok(entries) = fs::read_dir(&self.proposals_path) else {
            return Ok(json!({"ok":true,"proposals":items}));
        };
        let mut paths: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            if path.is_symlink() || path.extension().and_then(|x| x.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = fs::read_to_string(path) else {
                continue;
            };
            let Ok(record) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            if record["id"]
                .as_str()
                .map(|s| regex(r"^[a-f0-9]{12}$").is_match(s))
                .unwrap_or(false)
            {
                items.push(json!({"proposal_id":record["id"],"created_at":record["created_at"],"status":record["status"],"project":record["project"]}));
            }
        }
        Ok(json!({"ok":true,"proposals":items}))
    }
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
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
fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::symlink_metadata(path) {
            if !meta.file_type().is_symlink() {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}
fn set_open_mode(options: &mut OpenOptions, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(mode);
    }
    #[cfg(not(unix))]
    {
        let _ = (options, mode);
    }
}
fn parse_optional_date(value: Option<&str>) -> Result<Option<NaiveDate>, ()> {
    match value {
        None => Ok(None),
        Some(s) => NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map(Some)
            .map_err(|_| ()),
    }
}
fn delete_fts_terms(con: &Connection, rel: &str) -> Result<(), String> {
    con.execute("DELETE FROM documents_fts WHERE relpath=?1", [rel])
        .map_err(|e| e.to_string())?;
    con.execute("DELETE FROM doc_terms WHERE relpath=?1", [rel])
        .map_err(|e| e.to_string())?;
    Ok(())
}
fn delete_document(con: &Connection, rel: &str) -> Result<(), String> {
    delete_fts_terms(con, rel)?;
    con.execute("DELETE FROM documents WHERE relpath=?1", [rel])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug)]
struct Candidate {
    relpath: String,
    title: String,
    scope: String,
    classification: String,
    source_status: String,
    valid_from: Option<String>,
    valid_to: Option<String>,
    superseded_by: Option<String>,
    provider_policy: String,
    export_policy: String,
    body: String,
    indexed_at: String,
    rank: f64,
}
fn map_candidate(row: &Row<'_>) -> rusqlite::Result<Candidate> {
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

fn dispatch(fabric: &MemoryFabric, req: &Value) -> Result<Value, String> {
    match req["op"].as_str().unwrap_or("") {
        "status" => fabric.status(),
        "index" => fabric.index(req["force"].as_bool().unwrap_or(false)),
        "search" => fabric.search(
            req["query"].as_str().unwrap_or(""),
            req["scope"].as_str().unwrap_or("all"),
            req["limit"].as_u64().unwrap_or(5) as usize,
            req["provider"].as_str(),
            req["as_of"].as_str(),
        ),
        "context" => fabric.context(
            req["project"].as_str(),
            req["query"].as_str().unwrap_or(""),
            req["provider"].as_str(),
            req["token_budget"].as_u64().unwrap_or(1200) as usize,
            req["as_of"].as_str(),
        ),
        "read" => fabric.read(
            req["source_ref"].as_str().unwrap_or(""),
            req["provider"].as_str(),
            req["heading"].as_str(),
        ),
        "propose" => fabric.propose(req),
        "proposal-show" => fabric.proposal_show(
            req["proposal_id"].as_str().unwrap_or(""),
            req["provider"].as_str(),
        ),
        "proposal-list" => fabric.proposal_list(),
        _ => Ok(json_error("unsupported memory operation")),
    }
}

fn serve() -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut fabric: Option<MemoryFabric> = None;
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.len() > MAX_REQUEST_BYTES {
            writeln!(
                stdout,
                "{{\"ok\":false,\"error\":\"request exceeds size limit\"}}"
            )
            .map_err(|e| e.to_string())?;
            stdout.flush().map_err(|e| e.to_string())?;
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Err(_) => json_error("request must be valid JSON"),
            Ok(req) => {
                if !req.is_object() {
                    json_error("request must be a JSON object")
                } else {
                    if fabric.is_none() {
                        fabric = MemoryFabric::new().ok();
                    }
                    match fabric.as_ref() {
                        Some(f) => dispatch(f, &req).unwrap_or_else(json_error),
                        None => json_error("Memory Fabric could not initialize"),
                    }
                }
            }
        };
        serde_json::to_writer(&mut stdout, &response).map_err(|e| e.to_string())?;
        stdout.write_all(b"\n").map_err(|e| e.to_string())?;
        stdout.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn one_shot() -> Result<(), String> {
    let mut input = String::new();
    io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    if input.len() > MAX_REQUEST_BYTES {
        println!("{{\"ok\":false,\"error\":\"request exceeds size limit\"}}");
        return Ok(());
    }
    let req: Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            println!("{{\"ok\":false,\"error\":\"request must be valid JSON\"}}");
            return Ok(());
        }
    };
    if !req.is_object() {
        println!("{{\"ok\":false,\"error\":\"request must be a JSON object\"}}");
        return Ok(());
    }
    let result = MemoryFabric::new()
        .and_then(|f| dispatch(&f, &req))
        .unwrap_or_else(json_error);
    println!("{}", result);
    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = if args.iter().any(|a| a == "--serve") {
        serve()
    } else {
        one_shot()
    };
    if let Err(err) = result {
        eprintln!("memory-fabric: {err}");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn safe_paths_reject_traversal() {
        assert!(safe_relpath("../secret.md").is_err());
        assert_eq!(safe_relpath("projects/a.md").unwrap(), "projects/a.md");
    }
    #[test]
    fn tokenization_is_casefolded_and_deduplicated() {
        assert_eq!(
            tokenize("SQLite sqlite FTS5 the")
                .into_iter()
                .collect::<Vec<_>>(),
            vec!["fts5", "sqlite"]
        );
    }
    #[test]
    fn secret_filter_catches_underscore_service_role_key() {
        let candidate = format!(
            "SUPABASE_SERVICE_ROLE_KEY={}",
            "CANDIDATE".to_string() + &"X".repeat(24)
        );
        assert_eq!(secret_signature(&candidate), Some("credential_assignment"));
    }
    #[test]
    fn secret_filter_ignores_placeholder() {
        assert_eq!(
            secret_signature("SUPABASE_SERVICE_ROLE_KEY=YOUR_API_KEY_HERE"),
            None
        );
    }
    #[test]
    fn apple_paths_are_rejected() {
        assert!(apple_path_re().is_match("Apple Notes Export"));
    }
}
