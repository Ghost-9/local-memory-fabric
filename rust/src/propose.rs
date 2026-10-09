use crate::MemoryFabric;
use crate::paths::{now_utc, safe_relpath, set_mode, set_open_mode};
use crate::policy::secret_signature;
use crate::text::{json_error, regex};
use serde_json::{Value, json};
use similar::TextDiff;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use uuid::Uuid;

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
}
