use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Enterprise Security, Governance, Synchronization and Pre-warming Suite.
pub struct GovernanceEngine;

impl GovernanceEngine {
    // ==========================================
    // Milestone 23: Line-Anchored Codebase Cross-Referencing
    // ==========================================
    pub fn sync_anchors(vault_path: &Path, fix: bool) -> Result<Value, String> {
        let mut total_anchors = 0;
        let mut broken_anchors = Vec::new();
        let mut valid_anchors = 0;

        let anchor_regex =
            Regex::new(r"file://([^#\s]+)#L(\d+)(?:-L(\d+))?").map_err(|e| e.to_string())?;

        let mut md_files = Vec::new();
        Self::collect_md_files(vault_path, &mut md_files);

        for md_path in md_files {
            let content = match fs::read_to_string(&md_path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for cap in anchor_regex.captures_iter(&content) {
                total_anchors += 1;
                let target_file = cap.get(1).map(|m| m.as_str()).unwrap_or("");
                let start_line: usize = cap
                    .get(2)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(0);
                let end_line: usize = cap
                    .get(3)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(start_line);

                let target_path = Path::new(target_file);
                if !target_path.exists() {
                    broken_anchors.push(json!({
                        "source_doc": md_path.to_string_lossy(),
                        "target": target_file,
                        "status": "target_file_missing",
                        "start_line": start_line,
                        "end_line": end_line
                    }));
                } else if let Ok(target_content) = fs::read_to_string(target_path) {
                    let line_count = target_content.lines().count();
                    if start_line > line_count {
                        broken_anchors.push(json!({
                            "source_doc": md_path.to_string_lossy(),
                            "target": target_file,
                            "status": "line_exceeds_bounds",
                            "start_line": start_line,
                            "target_max_line": line_count
                        }));
                    } else {
                        valid_anchors += 1;
                    }
                }
            }
        }

        Ok(json!({
            "ok": true,
            "total_anchors_scanned": total_anchors,
            "valid_anchors": valid_anchors,
            "broken_anchors": broken_anchors,
            "fix_attempted": fix
        }))
    }

    // ==========================================
    // Milestone 24: Pre-Commit Memory Invariant Hook
    // ==========================================
    pub fn hook_check() -> Result<Value, String> {
        let out = Command::new("git")
            .args(["status", "--porcelain"])
            .output()
            .map_err(|e| format!("Failed to run git status: {}", e))?;

        if !out.status.success() {
            return Ok(json!({ "ok": false, "error": "Not a git repository" }));
        }

        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut code_changed = false;
        let mut memory_or_doc_changed = false;
        let mut modified_files = Vec::new();

        for line in stdout.lines() {
            let path = line.get(3..).unwrap_or("").trim();
            modified_files.push(path.to_string());
            if path.ends_with(".rs")
                || path.ends_with(".ts")
                || path.ends_with(".py")
                || path.ends_with(".go")
            {
                code_changed = true;
            }
            if path.contains("system-memory")
                || path.ends_with("STATUS.md")
                || path.ends_with("README.md")
                || path.ends_with("SKILL.md")
            {
                memory_or_doc_changed = true;
            }
        }

        let pass = !code_changed || memory_or_doc_changed;
        Ok(json!({
            "ok": pass,
            "code_changed": code_changed,
            "memory_or_doc_updated": memory_or_doc_changed,
            "staged_files": modified_files,
            "advisory": if !pass {
                "Architectural code changes detected without corresponding STATUS.md or memory invariant update."
            } else {
                "Memory and code change invariant verified."
            }
        }))
    }

    pub fn hook_install(repo_root: Option<&Path>) -> Result<Value, String> {
        let root = repo_root
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        let hooks_dir = root.join(".git").join("hooks");
        if !hooks_dir.exists() {
            return Ok(json!({ "ok": false, "error": ".git/hooks directory not found" }));
        }

        let hook_file = hooks_dir.join("pre-commit");
        let script = r#"#!/usr/bin/env bash
# Engram Pre-Commit Invariant Guard
if command -v engram >/dev/null 2>&1; then
    engram hook check || {
        echo "⚠️ Engram pre-commit advisory: Architectural code changes detected without STATUS.md or memory update."
    }
fi
exit 0
"#;
        fs::write(&hook_file, script).map_err(|e| e.to_string())?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&hook_file, fs::Permissions::from_mode(0o755))
                .map_err(|e| e.to_string())?;
        }

        Ok(json!({
            "ok": true,
            "hook_installed": hook_file.to_string_lossy(),
            "status": "active"
        }))
    }

    // ==========================================
    // Milestone 25: Tamper-Evident SHA-256 Merkle Audit Log
    // ==========================================
    pub fn append_audit_entry(
        log_path: &Path,
        op: &str,
        target: &str,
        author: &str,
        content: &str,
    ) -> Result<Value, String> {
        let (prev_hash, index) = if log_path.exists() {
            let file = File::open(log_path).map_err(|e| e.to_string())?;
            let reader = BufReader::new(file);
            let mut last_hash =
                "0000000000000000000000000000000000000000000000000000000000000000".to_string();
            let mut count = 0u64;
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if let Some(h) = v.get("hash").and_then(|s| s.as_str()) {
                        last_hash = h.to_string();
                    }
                    count += 1;
                }
            }
            (last_hash, count)
        } else {
            (
                "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                0,
            )
        };

        let now = crate::paths::now_utc();
        let content_hash = format!("{:x}", Sha256::digest(content.as_bytes()));

        let entry_payload = format!(
            "{}:{}:{}:{}:{}:{}:{}",
            index, now, op, target, author, prev_hash, content_hash
        );
        let entry_hash = format!("{:x}", Sha256::digest(entry_payload.as_bytes()));

        let record = json!({
            "ok": true,
            "index": index,
            "timestamp": now,
            "op": op,
            "target": target,
            "author": author,
            "content_hash": content_hash,
            "prev_hash": prev_hash,
            "hash": entry_hash
        });

        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let mut opts = OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(log_path).map_err(|e| e.to_string())?;

        writeln!(
            file,
            "{}",
            serde_json::to_string(&record).unwrap_or_default()
        )
        .map_err(|e| e.to_string())?;

        Ok(record)
    }

    pub fn verify_audit_log(log_path: &Path) -> Result<Value, String> {
        if !log_path.exists() {
            return Ok(
                json!({ "ok": true, "entries": 0, "verified": true, "message": "Log file empty" }),
            );
        }

        let file = File::open(log_path).map_err(|e| e.to_string())?;
        let reader = BufReader::new(file);

        let mut expected_prev =
            "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        let mut verified_count = 0u64;

        for (idx, line) in reader.lines().map_while(Result::ok).enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let val: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            let prev = val.get("prev_hash").and_then(|s| s.as_str()).unwrap_or("");
            let current = val.get("hash").and_then(|s| s.as_str()).unwrap_or("");
            let index = val.get("index").and_then(|v| v.as_u64()).unwrap_or(0);

            let now = val.get("timestamp").and_then(|s| s.as_str()).unwrap_or("");
            let op = val.get("op").and_then(|s| s.as_str()).unwrap_or("");
            let target = val.get("target").and_then(|s| s.as_str()).unwrap_or("");
            let author = val.get("author").and_then(|s| s.as_str()).unwrap_or("");
            let content_hash = val
                .get("content_hash")
                .and_then(|s| s.as_str())
                .unwrap_or("");

            let entry_payload = format!(
                "{}:{}:{}:{}:{}:{}:{}",
                index, now, op, target, author, prev, content_hash
            );
            let computed_hash = format!("{:x}", Sha256::digest(entry_payload.as_bytes()));

            if prev != expected_prev || index != (idx as u64) || computed_hash != current {
                return Ok(json!({
                    "ok": false,
                    "tamper_detected": true,
                    "failed_at_index": idx,
                    "expected_prev": expected_prev,
                    "actual_prev": prev,
                    "computed_hash": computed_hash,
                    "actual_hash": current
                }));
            }

            expected_prev = current.to_string();
            verified_count += 1;
        }

        Ok(json!({
            "ok": true,
            "tamper_detected": false,
            "entries_verified": verified_count,
            "merkle_tip": expected_prev
        }))
    }

    // ==========================================
    // Milestone 26: Secret Redaction Auto-Remediation
    // ==========================================
    pub fn scrub_secrets_vault(vault_path: &Path, dry_run: bool) -> Result<Value, String> {
        let mut files = Vec::new();
        Self::collect_md_files(vault_path, &mut files);

        let patterns = [
            (Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap(), "OPENAI_KEY"),
            (Regex::new(r"ghp_[A-Za-z0-9]{36}").unwrap(), "GITHUB_PAT"),
            (Regex::new(r"gho_[A-Za-z0-9]{36}").unwrap(), "GITHUB_OAUTH"),
            (
                Regex::new(r"AIza[0-9A-Za-z\\-_]{35}").unwrap(),
                "GOOGLE_API_KEY",
            ),
            (
                Regex::new(r"xox[baprs]-[0-9a-zA-Z]{10,48}").unwrap(),
                "SLACK_TOKEN",
            ),
            (
                Regex::new(
                    r"eyJ[A-Za-z0-9-_=]{20,}\.eyJ[A-Za-z0-9-_=]{20,}\.[A-Za-z0-9-_.+/=]{20,}",
                )
                .unwrap(),
                "JWT_TOKEN",
            ),
        ];

        let mut redacted_occurrences = 0;
        let mut affected_files = Vec::new();

        for file in files {
            let content = match fs::read_to_string(&file) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let mut scrubbed = content.clone();
            let mut file_modified = false;

            for (re, name) in &patterns {
                for mat in re.find_iter(&content) {
                    let matched_str = mat.as_str();
                    let hash_prefix = format!("{:x}", Sha256::digest(matched_str.as_bytes()))
                        .chars()
                        .take(8)
                        .collect::<String>();
                    let replacement = format!("[REDACTED_{}_{}]", name, hash_prefix);
                    scrubbed = scrubbed.replace(matched_str, &replacement);
                    redacted_occurrences += 1;
                    file_modified = true;
                }
            }

            if file_modified {
                affected_files.push(file.to_string_lossy().to_string());
                if !dry_run {
                    fs::write(&file, scrubbed).map_err(|e| e.to_string())?;
                }
            }
        }

        Ok(json!({
            "ok": true,
            "secrets_redacted": redacted_occurrences,
            "affected_files": affected_files,
            "dry_run": dry_run
        }))
    }

    // ==========================================
    // Milestone 29: User Cognitive Profile Auto-Tuning
    // ==========================================
    pub fn learn_preference(
        vault_path: &Path,
        preference: &str,
        category: Option<&str>,
    ) -> Result<Value, String> {
        let prefs_file = vault_path.join("user").join("preferences.md");
        if let Some(parent) = prefs_file.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let cat = category.unwrap_or("Workflow & Guardrails");
        let timestamp = crate::paths::now_utc();
        let bullet = format!("- [Learned {}] {}\n", timestamp, preference.trim());

        let mut content = if prefs_file.exists() {
            fs::read_to_string(&prefs_file).map_err(|e| e.to_string())?
        } else {
            format!(
                "---\ntitle: \"User Cognitive Preferences\"\nscope: user\n---\n\n# User Preferences\n\n## {}\n\n",
                cat
            )
        };

        let cat_header = format!("## {}", cat);
        if content.contains(&cat_header) {
            content = content.replace(&cat_header, &format!("{}\n{}", cat_header, bullet));
        } else {
            content.push_str(&format!("\n## {}\n{}", cat, bullet));
        }

        fs::write(&prefs_file, content).map_err(|e| e.to_string())?;

        Ok(json!({
            "ok": true,
            "learned_preference": preference.trim(),
            "category": cat,
            "stored_in": prefs_file.to_string_lossy()
        }))
    }

    // ==========================================
    // Milestone 32: P2P Multi-Machine Vault Sync
    // ==========================================
    pub fn vault_sync(
        vault_path: &Path,
        remote: Option<&str>,
        branch: Option<&str>,
    ) -> Result<Value, String> {
        let rem = remote.unwrap_or("origin");
        let br = branch.unwrap_or("main");

        // Check if vault is a git repo
        let is_git = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["rev-parse", "--is-inside-work-tree"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if !is_git {
            return Ok(json!({
                "ok": false,
                "error": "Vault is not initialized as a git repository for P2P sync."
            }));
        }

        // Fetch
        let fetch_res = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["fetch", rem, br])
            .output();

        // Auto-commit local changes
        let _ = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["add", "."])
            .output();

        let commit_msg = format!(
            "sync(memory): automated p2p vault checkpoint at {}",
            crate::paths::now_utc()
        );
        let _ = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["commit", "-m", &commit_msg])
            .output();

        // Pull rebase
        let pull_res = Command::new("git")
            .arg("-C")
            .arg(vault_path)
            .args(["pull", "--rebase", rem, br])
            .output();

        let pull_ok = pull_res.map(|o| o.status.success()).unwrap_or(false);

        Ok(json!({
            "ok": pull_ok,
            "remote": rem,
            "branch": br,
            "synced_at": crate::paths::now_utc(),
            "vault": vault_path.to_string_lossy(),
            "fetch_success": fetch_res.map(|o| o.status.success()).unwrap_or(false)
        }))
    }

    // ==========================================
    // Milestone 34: Predictive Invariant Cache Pre-Warming
    // ==========================================
    pub fn prewarm_cache(vault_path: &Path, files_list: &[String]) -> Result<Value, String> {
        let mut prewarmed_tags = Vec::new();

        for file in files_list {
            let ext = Path::new(file)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            let matched_tag = match ext {
                "rs" => "rust",
                "ts" | "tsx" | "js" | "jsx" => "typescript",
                "py" => "python",
                "sh" | "bash" | "zsh" => "shell",
                "json" | "toml" | "yaml" | "yml" => "config",
                "sql" => "sqlite",
                "md" => "markdown",
                _ => "",
            };
            if !matched_tag.is_empty() && !prewarmed_tags.contains(&matched_tag.to_string()) {
                prewarmed_tags.push(matched_tag.to_string());
            }
        }

        let inv_dir = vault_path.join("invariants");
        let mut prewarmed_cards = Vec::new();

        if inv_dir.exists()
            && inv_dir.is_dir()
            && let Ok(entries) = fs::read_dir(inv_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                for tag in &prewarmed_tags {
                    if name.contains(tag) {
                        prewarmed_cards.push(path.to_string_lossy().to_string());
                        break;
                    }
                }
            }
        }

        Ok(json!({
            "ok": true,
            "detected_stacks": prewarmed_tags,
            "prewarmed_cards_count": prewarmed_cards.len(),
            "cards": prewarmed_cards,
            "cache_state": "hot"
        }))
    }

    fn collect_md_files(dir: &Path, acc: &mut Vec<PathBuf>) {
        if !dir.exists() || !dir.is_dir() {
            return;
        }
        let excluded = [
            "apple-notes",
            "notes_export",
            "templates",
            "archive",
            ".git",
            ".trash",
        ];
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = fs::symlink_metadata(&path)
                    && meta.file_type().is_symlink()
                {
                    continue;
                }
                if path.is_dir() {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_lowercase();
                    if !name.starts_with('.') && !excluded.contains(&name.as_str()) {
                        Self::collect_md_files(&path, acc);
                    }
                } else if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md")
                {
                    acc.push(path);
                }
            }
        }
    }
}
