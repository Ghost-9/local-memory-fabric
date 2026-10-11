use crate::MemoryFabric;
use crate::db::{busy_aware, delete_document, delete_fts_terms};
use crate::paths::{VERSION, now_utc, safe_relpath, set_mode};
use crate::policy::{apple_path_re, secret_signature};
use crate::text::{
    frontmatter_value, headings_for, is_skipped_name, scope_for, sha256, title_for, tokenize,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

impl MemoryFabric {
    pub(crate) fn sources(&self) -> Result<Vec<PathBuf>, String> {
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

    pub(crate) fn walk(&self, root: &Path, found: &mut Vec<PathBuf>) {
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

    pub(crate) fn index(&self, force: bool) -> Result<Value, String> {
        if self.home.is_symlink() {
            return Err("Memory Fabric state directory must not be a symlink".into());
        }
        fs::create_dir_all(&self.home).map_err(|e| e.to_string())?;
        if self.db_path.is_symlink() {
            return Err("Memory index must not be a symlink".into());
        }
        let con = Connection::open(&self.db_path).map_err(|e| busy_aware("open", e))?;
        con.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        con.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| busy_aware("lock", e))?;
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

    pub(crate) fn status(&self) -> Result<Value, String> {
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
            json!({"ok":true,"version":VERSION,"indexed":true,"documents":docs,"last_indexed_at":last,
                   // Reported so the operator can see the knob without reading the config:
                   // both the scrub and the export gate change what a session receives.
                   "enforce_export_policy":self.export_enforced()}),
        )
    }

    pub(crate) fn indexed_doc(&self, rel: &str, con: &Connection) -> Result<Value, String> {
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
}
