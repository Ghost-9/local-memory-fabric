use crate::MemoryFabric;
use crate::db::{busy_aware, delete_document, delete_fts_terms};
use crate::paths::{VERSION, now_utc, safe_relpath, set_mode};
use crate::policy::{apple_path_re, secret_fingerprint, secret_signature};
use crate::text::{
    extract_wikilinks, frontmatter_value, headings_for, is_skipped_name, scope_for, sha256,
    title_for, tokenize,
};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};


/// Whether the knowledge base is under version control, and how stale its baseline is.
///
/// Read-only: shells out to `git` in the vault and mutates nothing. Used by `status` so that
/// recoverability is *reported* rather than assumed - a vault with history but nine uncommitted
/// files is not fully recoverable, and status should say so rather than imply it is.
fn vault_history(vault: &std::path::Path) -> serde_json::Value {
    use std::process::Command;
    let run = |args: &[&str]| -> Option<String> {
        let out = Command::new("git").arg("-C").arg(vault).args(args).output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let head = run(&["log", "-1", "--format=%H %cI"]);
    let dirty = run(&["status", "--porcelain"])
        .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count());
    json!({
        "under_version_control": head.is_some(),
        "head": head.as_ref().and_then(|h| h.split_whitespace().next())
            .map(|h| h.chars().take(8).collect::<String>()),
        "head_committed_at": head.as_ref().and_then(|h| h.split_whitespace().nth(1)),
        "uncommitted_files": dirty,
        "fully_recoverable": head.is_some() && dirty == Some(0),
    })
}

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
        let start_time = Instant::now();
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
        let existing: HashMap<String, (String, i64)> = {
            let mut stmt = con
                .prepare("SELECT relpath,content_hash,mtime_ns FROM documents")
                .map_err(|e| e.to_string())?;
            let mut out = HashMap::new();
            let rows = stmt
                .query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))
                .map_err(|e| e.to_string())?;
            for row in rows {
                let (p, data) = row.map_err(|e| e.to_string())?;
                out.insert(p, data);
            }
            out
        };
        let canonical_vault = self.vault.canonicalize().map_err(|e| e.to_string())?;
        let tx = con.unchecked_transaction().map_err(|e| e.to_string())?;
        let mut found = HashSet::new();
        let (mut added, mut updated, mut unchanged, mut excluded_sensitive) =
            (0usize, 0usize, 0usize, 0usize);
        for path in self.sources()? {
            let Ok(canonical_path) = path.canonicalize() else {
                continue;
            };
            if !canonical_path.starts_with(&canonical_vault) {
                continue;
            }
            let rel = path
                .strip_prefix(&self.vault)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            found.insert(rel.clone());

            let mtime_ns = fs::symlink_metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
                .unwrap_or(0);

            // Fast incremental skip: if mtime_ns is identical and not forced/migrated, skip reading disk!
            if !force && !migrated && mtime_ns > 0 {
                if let Some((_, prev_mtime)) = existing.get(&rel) {
                    if *prev_mtime == mtime_ns {
                        unchanged += 1;
                        continue;
                    }
                }
            }

            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let classification = frontmatter_value(&text, "classification")
                .unwrap_or_else(|| "local_only".into())
                .to_ascii_lowercase();
            if ["secret", "restricted", "excluded"].contains(&classification.as_str())
                || secret_signature(&text).is_some()
            {
                excluded_sensitive += 1;
                // Excluding it is correct; saying nothing is not. Record an alarm (fingerprint
                // only, never the value) so the owner learns a credential is in their vault.
                let kind = secret_signature(&text)
                    .map(|k| k.to_string())
                    .unwrap_or_else(|| format!("classification:{}", classification));
                let fingerprint = secret_fingerprint(&text).unwrap_or_else(|| "n/a".into());
                let alarm_id: String =
                    sha256(&format!("{}:{}", rel, fingerprint))[..12].to_string();
                tx.execute(
                    "INSERT INTO secret_alarms(alarm_id,relpath,secret_kind,fingerprint,detected_at,resolved_at)
                     VALUES(?1,?2,?3,?4,?5,NULL)
                     ON CONFLICT(alarm_id) DO UPDATE SET relpath=excluded.relpath,
                        secret_kind=excluded.secret_kind, detected_at=excluded.detected_at, resolved_at=NULL",
                    rusqlite::params![alarm_id, rel, kind, fingerprint, now_utc()],
                )
                .map_err(|e| e.to_string())?;
                if existing.contains_key(&rel) {
                    delete_document(&tx, &rel)?;
                }
                continue;
            }
            let digest = sha256(&text);
            if !force && !migrated && existing.get(&rel).map(|(h, _)| h == &digest).unwrap_or(false) {
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
            if existing.contains_key(&rel) {
                updated += 1;
                delete_fts_terms(&tx, &rel)?;
            } else {
                added += 1;
            }
            tx.execute("INSERT INTO documents(relpath,title,scope,classification,source_status,valid_from,valid_to,superseded_by,provider_policy,export_policy,body,content_hash,mtime_ns,indexed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'loopback_allowlist_only','deny_remote',?9,?10,?11,?12) ON CONFLICT(relpath) DO UPDATE SET title=excluded.title,scope=excluded.scope,classification=excluded.classification,source_status=excluded.source_status,valid_from=excluded.valid_from,valid_to=excluded.valid_to,superseded_by=excluded.superseded_by,provider_policy=excluded.provider_policy,export_policy=excluded.export_policy,body=excluded.body,content_hash=excluded.content_hash,mtime_ns=excluded.mtime_ns,indexed_at=excluded.indexed_at",
                params![rel,title,scope,classification,status,valid_from,valid_to,superseded_by,text,digest,mtime_ns,now_utc()]).map_err(|e| e.to_string())?;
            let slug_clean = rel.trim_end_matches(".md").replace('/', " ").replace('-', " ").replace('_', " ");
            let full_headings = format!("{headings}\n{slug_clean}");
            tx.execute(
                "INSERT INTO documents_fts(relpath,title,headings,body) VALUES(?1,?2,?3,?4)",
                params![rel, title, full_headings, text],
            )
            .map_err(|e| e.to_string())?;
            let all_terms = tokenize(&format!("{title}\n{full_headings}\n{text}"));
            for term in all_terms {
                tx.execute(
                    "INSERT OR IGNORE INTO doc_terms(relpath,term) VALUES(?1,?2)",
                    params![rel, term],
                )
                .map_err(|e| e.to_string())?;
            }
            // Index relational wikilinks
            let links = extract_wikilinks(&text);
            tx.execute("DELETE FROM note_links WHERE source=?1", [&rel])
                .map_err(|e| e.to_string())?;
            for target in links {
                tx.execute(
                    "INSERT OR IGNORE INTO note_links(source,target) VALUES(?1,?2)",
                    params![rel, target],
                )
                .map_err(|e| e.to_string())?;
            }
        }
        let stale: Vec<String> = existing
            .keys()
            .filter(|p| !found.contains(*p))
            // Sandboxed payloads are not in the canonical memory, so the vault scan cannot see
            // them. Without this they would be swept as stale on every reindex.
            .filter(|p| !p.starts_with(crate::propose::SANDBOX_PREFIX))
            .cloned()
            .collect();
        for rel in &stale {
            delete_document(&tx, rel)?;
        }
        // Incremental delta skip: only rebuild term_df if terms actually changed
        if added > 0 || updated > 0 || !stale.is_empty() || force || migrated {
            tx.execute("DELETE FROM term_df", [])
                .map_err(|e| e.to_string())?;
            tx.execute(
                "INSERT INTO term_df(term,df) SELECT term,COUNT(*) FROM doc_terms GROUP BY term",
                [],
            )
            .map_err(|e| e.to_string())?;
        }
        tx.execute("INSERT INTO metadata(key,value) VALUES('last_indexed_at',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [now_utc()]).map_err(|e| e.to_string())?;
        let canonical = canonical_vault.to_string_lossy().to_string();
        tx.execute("INSERT INTO metadata(key,value) VALUES('vault_path',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [canonical]).map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        set_mode(&self.db_path, 0o600);
        let docs: i64 = con
            .query_row("SELECT COUNT(*) FROM documents", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let elapsed_ms = (start_time.elapsed().as_micros() as f64) / 1000.0;
        Ok(
            json!({"ok":true,"version":VERSION,"added":added,"updated":updated,"unchanged":unchanged,"removed_from_index":stale.len(),"excluded_sensitive":excluded_sensitive,"documents":docs,"secret_alarms":self.unresolved_alarms()?.len(),"elapsed_ms":elapsed_ms}),
        )
    }

    // --- credential exposure alarms (mirrors the Python engine) ------------------------
    // The secret filter excludes a note silently. That is safe but leaves the owner unaware,
    // and only they can rotate a key. Detect -> remind on every fetch -> resolve on their word.

    pub(crate) fn unresolved_alarms(&self) -> Result<Vec<Value>, String> {
        if !self.db_path.exists() {
            return Ok(Vec::new());
        }
        let con = self.connect().map_err(|e| e.to_string())?;
        let mut stmt = con
            .prepare(
                "SELECT alarm_id,relpath,secret_kind,fingerprint,detected_at FROM secret_alarms \
                 WHERE resolved_at IS NULL ORDER BY detected_at",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok(json!({
                    "alarm_id": r.get::<_, String>(0)?,
                    "relpath": r.get::<_, String>(1)?,
                    "secret_kind": r.get::<_, String>(2)?,
                    "fingerprint": r.get::<_, String>(3)?,
                    "detected_at": r.get::<_, String>(4)?,
                }))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| e.to_string())?);
        }
        Ok(out)
    }

    /// One line appended to any memory payload while an alarm is open. Names the note and the
    /// KIND, never the value — a reminder that repeated the key would be its own leak. `None`
    /// when nothing is open, so the common payload is unchanged.
    pub(crate) fn secret_alert_reminder(&self) -> Result<Option<String>, String> {
        let alarms = self.unresolved_alarms()?;
        if alarms.is_empty() {
            return Ok(None);
        }
        if alarms.len() == 1 {
            let a = &alarms[0];
            return Ok(Some(format!(
                concat!(
                    "[memory: CREDENTIAL ALARM] A key of type '{}' was found in {} on {}. ",
                    "That note is excluded from the index and the value was never copied anywhere. ",
                    "Rotate it, then run 'memory-fabric resolve {}' to stop this reminder."
                ),
                a["secret_kind"].as_str().unwrap_or("unknown"),
                a["relpath"].as_str().unwrap_or("?"),
                a["detected_at"].as_str().unwrap_or("?"),
                a["alarm_id"].as_str().unwrap_or("?")
            )));
        }
        let mut kinds: Vec<&str> = alarms
            .iter()
            .filter_map(|a| a["secret_kind"].as_str())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        let ids: Vec<&str> = alarms
            .iter()
            .take(3)
            .filter_map(|a| a["alarm_id"].as_str())
            .collect();
        let more = if alarms.len() > 3 { "..." } else { "" };
        Ok(Some(format!(
            concat!(
                "[memory: CREDENTIAL ALARM x{}] {} vault notes contain credentials (types: {}) and are ",
                "excluded from the index. Values were never copied. Rotate them, then run ",
                "'memory-fabric resolve <alarm_id>' per alarm ({}{})."
            ),
            alarms.len(),
            alarms.len(),
            kinds.join(", "),
            ids.join(", "),
            more
        )))
    }

    pub(crate) fn alarms(&self) -> Result<Value, String> {
        Ok(json!({
            "ok": true,
            "alarms": self.unresolved_alarms()?,
            "note": "Values are never stored here - only a fingerprint. Rotate the key, then resolve."
        }))
    }

    pub(crate) fn resolve_alarm(&self, alarm_id: &str) -> Result<Value, String> {
        let id = alarm_id.trim();
        if !id.chars().all(|c| c.is_ascii_hexdigit()) || id.is_empty() || id.len() > 16 {
            return Ok(json!({"ok": false, "error": "alarm id must be a short hex id"}));
        }
        let con = self.connect().map_err(|e| e.to_string())?;
        let existing: Option<(String, String, Option<String>)> = con
            .query_row(
                "SELECT alarm_id,relpath,resolved_at FROM secret_alarms WHERE alarm_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        let Some((aid, relpath, resolved_at)) = existing else {
            return Ok(json!({"ok": false, "error": "no such alarm; run 'memory-fabric alarms' to list them"}));
        };
        let already = resolved_at.is_some();
        if !already {
            con.execute(
                "UPDATE secret_alarms SET resolved_at=?1 WHERE alarm_id=?2 AND resolved_at IS NULL",
                rusqlite::params![now_utc(), id],
            )
            .map_err(|e| e.to_string())?;
        }
        let remaining = self.unresolved_alarms()?.len();
        Ok(json!({"ok": true, "alarm_id": aid, "relpath": relpath, "already_resolved": already, "unresolved_remaining": remaining}))
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
        let unresolved: i64 = con
            .query_row("SELECT COUNT(*) FROM secret_alarms WHERE resolved_at IS NULL", [], |r| r.get(0))
            .unwrap_or(0);
        let oldest_alarm: Option<String> = con
            .query_row(
                "SELECT MIN(detected_at) FROM secret_alarms WHERE resolved_at IS NULL",
                [], |r| r.get(0),
            )
            .optional()
            .unwrap_or(None);
        let index_bytes = std::fs::metadata(&self.db_path).map(|m| m.len()).unwrap_or(0);
        let bytes_per_doc = if docs > 0 { index_bytes / docs as u64 } else { 0 };
        let growth_warn_at_docs = if bytes_per_doc > 0 { (512 * 1024 * 1024u64 / bytes_per_doc) as i64 } else { 0 };
        let growth_warning = growth_warn_at_docs > 0 && docs >= growth_warn_at_docs;
        let git = vault_history(&self.vault);
        Ok(
            json!({"ok":true,"version":VERSION,"indexed":true,"documents":docs,"last_indexed_at":last,
                   "index_bytes":index_bytes,"bytes_per_document":bytes_per_doc,
                   "growth_warn_at_documents":growth_warn_at_docs,"growth_warning":growth_warning,
                   "unresolved_alarms":unresolved,"oldest_unresolved_alarm":oldest_alarm,
                   "vault_history":git,
                   // Reported so the operator can see the knob without reading the config:
                   // both the scrub and the export gate change what a session receives.
                   "enforce_export_policy":self.export_enforced(),
                   // Non-zero means a credential is sitting in the vault and needs rotating.
                   "secret_alarms":self.unresolved_alarms()?.len()}),
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

    pub(crate) fn audit(&self) -> Result<Value, String> {
        let con = self.connect()?;
        let now_date = chrono::Utc::now().format("%Y-%m-%d").to_string();

        let mut stmt = con.prepare("SELECT relpath, title, valid_to FROM documents WHERE valid_to IS NOT NULL AND valid_to < ?1 ORDER BY relpath")
            .map_err(|e| e.to_string())?;
        let stale: Vec<Value> = stmt.query_map([&now_date], |r| {
            Ok(json!({
                "relpath": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "valid_to": r.get::<_, String>(2)?
            }))
        }).map_err(|e| e.to_string())?
        .filter_map(Result::ok).collect();

        let mut stmt = con.prepare("SELECT relpath, title, source_status, superseded_by FROM documents WHERE superseded_by IS NOT NULL AND source_status = 'current' ORDER BY relpath")
            .map_err(|e| e.to_string())?;
        let contradictions: Vec<Value> = stmt.query_map([], |r| {
            Ok(json!({
                "relpath": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "status": r.get::<_, String>(2)?,
                "superseded_by": r.get::<_, String>(3)?
            }))
        }).map_err(|e| e.to_string())?
        .filter_map(Result::ok).collect();

        let mut stmt = con.prepare("SELECT relpath, title, content_hash FROM documents WHERE content_hash IN (SELECT content_hash FROM documents GROUP BY content_hash HAVING COUNT(*) > 1) ORDER BY content_hash, relpath")
            .map_err(|e| e.to_string())?;
        let duplicates: Vec<Value> = stmt.query_map([], |r| {
            Ok(json!({
                "relpath": r.get::<_, String>(0)?,
                "title": r.get::<_, String>(1)?,
                "content_hash": r.get::<_, String>(2)?
            }))
        }).map_err(|e| e.to_string())?
        .filter_map(Result::ok).collect();

        let mut stmt = con.prepare("SELECT source, target FROM note_links WHERE NOT EXISTS (SELECT 1 FROM documents WHERE relpath = target OR title = target OR relpath = target || '.md') ORDER BY source, target")
            .map_err(|e| e.to_string())?;
        let broken_links: Vec<Value> = stmt.query_map([], |r| {
            Ok(json!({
                "source": r.get::<_, String>(0)?,
                "target": r.get::<_, String>(1)?
            }))
        }).map_err(|e| e.to_string())?
        .filter_map(Result::ok).collect();

        Ok(json!({
            "ok": true,
            "stale_count": stale.len(),
            "stale_notes": stale,
            "contradiction_count": contradictions.len(),
            "contradictions": contradictions,
            "duplicate_count": duplicates.len(),
            "duplicates": duplicates,
            "broken_link_count": broken_links.len(),
            "broken_links": broken_links,
            "note": "Audit is advisory. Use proposals to apply any updates."
        }))
    }
}
