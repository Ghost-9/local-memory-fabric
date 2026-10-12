use crate::MemoryFabric;
use crate::paths::{now_utc, safe_relpath, set_mode, set_open_mode};
use crate::policy::secret_signature;
use crate::text::{json_error, regex, sha256, tokenize};
use rusqlite::params;
use serde_json::{Value, json};
use similar::TextDiff;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use uuid::Uuid;

/// Largest payload accepted into the sandbox. Tool output is the target, so anything
/// materially larger than a screenful is refused rather than silently truncated.
pub(crate) const MAX_SANDBOX_CHARS: usize = 256 * 1024;
/// How much of the payload is returned with the handle. The point of the sandbox is that the
/// rest is retrieved deliberately, so this stays small.
pub(crate) const SANDBOX_HEAD_CHARS: usize = 480;
/// Relpath prefix for sandboxed payloads. `index()` protects it from the stale sweep.
pub(crate) const SANDBOX_PREFIX: &str = "sandbox/";

impl MemoryFabric {
    pub(crate) fn propose(&self, req: &Value) -> Result<Value, String> {
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
                regex::escape(&section)
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

    /// Store a large payload outside the canonical vault and return only a bounded head plus a
/// handle, so raw bytes never enter the context window.
///
/// This is the piece worth borrowing from the context-window tooling: a 315 KB tool result
/// costs the same tokens whether or not it is useful, and pushing it through the retriever is
/// strictly better than pushing it through the prompt. Retrieval then uses the ordinary
/// search/read path, so there is no second ranking policy to maintain.
pub(crate) fn sandbox(
    &self,
    title: &str,
    text: &str,
    scope: &str,
    provider: Option<&str>,
    loopback: Option<bool>,
) -> Result<Value, String> {
    // `loopback` is accepted for symmetry with the other ops and echoed back, but it
    // deliberately does not gate this payload: a sanitised copy goes to the caller either
    // way, and the bytes here came from the agent's own tool output rather than the
    // canonical memory, which is what export policy protects.
    let _ = loopback;
    if !self.is_local_provider(provider) {
        return Ok(
            json!({"ok":false,"blocked":"Memory content is available only to an explicitly allowed local model provider."}),
        );
    }
    if text.trim().is_empty() {
        return Ok(json_error("sandbox payload is required"));
    }
    if text.chars().count() > MAX_SANDBOX_CHARS {
        return Ok(json_error(format!(
            "sandbox payload exceeds {MAX_SANDBOX_CHARS} characters; store a slice instead"
        )));
    }
    if let Some(kind) = secret_signature(text) {
        // Local filter, before anything is written.
        return Ok(json_error(format!(
            "sandbox rejected by local secret filter ({kind}); no content was stored"
        )));
    }
    let digest = sha256(text);
    let rel = format!("{SANDBOX_PREFIX}{digest}.md");
    let sandbox_path = self.home.join("sandbox");
    fs::create_dir_all(&sandbox_path).map_err(|e| e.to_string())?;
    let destination = sandbox_path.join(format!("{digest}.md"));
    if !destination.exists() {
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        set_open_mode(&mut opts, 0o600);
        let mut file = opts.open(&destination).map_err(|e| e.to_string())?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        set_mode(&destination, 0o600);
    }
    let con = self.connect()?;
    let now = now_utc();
    let head: String = text.chars().take(SANDBOX_HEAD_CHARS).collect();
    con.execute(
        "INSERT INTO documents(relpath,title,scope,classification,source_status,valid_from,valid_to,superseded_by,provider_policy,export_policy,body,content_hash,mtime_ns,indexed_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,'loopback_allowlist_only','deny_remote',?9,?10,?11,?12) ON CONFLICT(relpath) DO UPDATE SET title=excluded.title,body=excluded.body,content_hash=excluded.content_hash,indexed_at=excluded.indexed_at",
        params![rel, if title.trim().is_empty() { "sandbox".to_string() } else { title.to_string() },
                if scope.trim().is_empty() { "episodic".to_string() } else { scope.trim().to_string() },
                "local_only", "current", None::<String>, None::<String>, None::<String>,
                text, &digest, now, now],
    )
    .map_err(|e| e.to_string())?;
    con.execute("DELETE FROM doc_terms WHERE relpath=?1", [&rel])
        .map_err(|e| e.to_string())?;
    con.execute("DELETE FROM documents_fts WHERE relpath=?1", [&rel])
        .map_err(|e| e.to_string())?;
    con.execute("INSERT INTO documents_fts(relpath,title,headings,body) VALUES(?1,?2,?3,?4)",
                params![rel, if title.trim().is_empty() { "sandbox" } else { title }, "", text])
        .map_err(|e| e.to_string())?;
    // Mirror index.rs exactly: doc_terms is (relpath,term) with no df column, and term_df is
    // derived from it. Writing a df here would be a schema drift that only shows up on search.
    for term in tokenize(text) {
        con.execute("INSERT OR IGNORE INTO doc_terms(relpath,term) VALUES(?1,?2)", params![rel, term])
            .map_err(|e| e.to_string())?;
    }
    con.execute("INSERT INTO term_df(term,df) SELECT term,COUNT(*) FROM doc_terms GROUP BY term ON CONFLICT(term) DO UPDATE SET df=excluded.df", [])
        .map_err(|e| e.to_string())?;
    Ok(
        json!({"ok":true,"handle":rel,"title":if title.trim().is_empty() {"sandbox"} else {title},
               "scope":if scope.trim().is_empty() {"episodic"} else {scope.trim()},
               "chars":text.chars().count(),"head":head,
               "note":"Raw payload stored outside the canonical memory at mode 0600; retrieve deliberately with search or read rather than pasting it into context."}),
    )
}

pub(crate) fn proposal_show(&self, id: &str, provider: Option<&str>) -> Result<Value, String> {
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

    pub(crate) fn proposal_list(&self) -> Result<Value, String> {
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

    pub(crate) fn proposal_apply(&self, id: &str, provider: Option<&str>) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(
                json!({"ok":false,"blocked":"Proposal apply is available only to an explicitly allowed local model provider."}),
            );
        }
        if !regex(r"^[a-f0-9]{12}$").is_match(id) {
            return Ok(json_error("invalid proposal id"));
        }
        let path = self.proposals_path.join(format!("{id}.json"));
        if path.is_symlink() {
            return Ok(json_error("proposal path is unsafe"));
        }
        let mut record: Value = match fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => return Ok(json_error("proposal not found")),
        };
        if record["status"].as_str() == Some("applied") {
            return Ok(json!({
                "ok": true,
                "proposal_id": id,
                "status": "already_applied",
                "target": record["target"],
                "note": "Proposal was already applied previously"
            }));
        }
        let target = record["target"].as_str().unwrap_or("").to_string();
        let section = record["section"].as_str().unwrap_or("").to_string();
        let text = record["proposal_text"].as_str().unwrap_or("").to_string();
        let target_path = self.vault.join(&target);
        if target_path.is_symlink() || !target_path.exists() {
            return Ok(json_error("target note does not exist or is a symlink"));
        }
        let base = fs::read_to_string(&target_path).map_err(|e| e.to_string())?;
        let lines: Vec<&str> = base.split_inclusive('\n').collect();
        let start = lines.iter().position(|line| {
            regex(&format!(
                r"(?i)^#{{1,6}}\s+{}\s*#*\s*$",
                regex::escape(&section)
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
            if n > 0 && n <= level && line.chars().nth(n).map(char::is_whitespace).unwrap_or(false) {
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
        let tmp_path = self.vault.join(format!("{}.tmp.{}", target.replace('/', "_"), id));
        fs::write(&tmp_path, &updated).map_err(|e| e.to_string())?;
        fs::rename(&tmp_path, &target_path).map_err(|e| e.to_string())?;

        let mut git_commit: Option<String> = None;
        if self.vault.join(".git").exists() {
            let _ = std::process::Command::new("git")
                .args(["-C", self.vault.to_str().unwrap(), "add", &target])
                .output();
            let commit_msg = format!("memory: apply proposal {id} to {target}");
            if let Ok(c) = std::process::Command::new("git")
                .args(["-C", self.vault.to_str().unwrap(), "commit", "-m", &commit_msg])
                .output()
            {
                if c.status.success() {
                    git_commit = String::from_utf8(c.stdout).ok().map(|s| s.lines().next().unwrap_or("").to_string());
                }
            }
        }

        record["status"] = json!("applied");
        record["applied_at"] = json!(now_utc());
        if let Some(ref c) = git_commit {
            record["git_commit"] = json!(c);
        }
        if let Ok(mut f) = fs::File::create(&path) {
            let _ = serde_json::to_writer_pretty(&mut f, &record);
        }

        Ok(json!({
            "ok": true,
            "proposal_id": id,
            "status": "applied",
            "target": target,
            "git_commit": git_commit,
            "note": "Proposal applied cleanly to vault. Changes recorded in git."
        }))
    }
}
