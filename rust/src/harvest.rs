use serde_json::{Value, json};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Session-End Autonomous Harvester.
/// Scans agent session transcripts, identifies command failures and user corrections,
/// and stages structured invariant drafts into `~/system-memory/invariants/drafts/`.
pub struct SessionHarvester;

impl SessionHarvester {
    /// Discovers candidate sessions and extracts invariant candidates.
    pub fn harvest_all(
        sessions_dir: Option<&Path>,
        vault_path: &Path,
        dry_run: bool,
    ) -> Result<Value, String> {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let default_sessions = PathBuf::from(&home).join(".pi/agent/sessions");
        let search_dir = sessions_dir.unwrap_or(&default_sessions);

        let mut processed_files = 0;
        let mut candidates_found = Vec::new();

        if search_dir.exists() && search_dir.is_dir() {
            if let Ok(entries) = fs::read_dir(search_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                        processed_files += 1;
                        if let Ok(mut extracted) = Self::harvest_single_transcript(&path) {
                            candidates_found.append(&mut extracted);
                        }
                    }
                }
            }
        }

        let drafts_dir = vault_path.join("invariants").join("drafts");
        let mut written_drafts = Vec::new();

        if !dry_run && !candidates_found.is_empty() {
            fs::create_dir_all(&drafts_dir).map_err(|e| e.to_string())?;
            for (idx, draft) in candidates_found.iter().enumerate() {
                let filename = format!("draft-{}-{}.md", crate::paths::now_utc().replace([':', '-'], "").replace('T', "_"), idx + 1);
                let filepath = drafts_dir.join(&filename);
                let content = format!(
                    "---\ntitle: \"{}\"\nscope: invariants\ntype: invariant_draft\nextracted_at: \"{}\"\nsource: \"{}\"\nstatus: draft\n---\n\n### Observation & Context\n{}\n\n### Invariant Rule Proposal\n{}\n",
                    draft["title"].as_str().unwrap_or("Candidate Invariant"),
                    crate::paths::now_utc(),
                    draft["source_file"].as_str().unwrap_or("session"),
                    draft["context"].as_str().unwrap_or(""),
                    draft["proposal"].as_str().unwrap_or("")
                );
                if fs::write(&filepath, content).is_ok() {
                    written_drafts.push(filepath.to_string_lossy().to_string());
                }
            }
        }

        Ok(json!({
            "ok": true,
            "sessions_scanned": processed_files,
            "candidates_extracted": candidates_found.len(),
            "drafts_written": written_drafts,
            "dry_run": dry_run,
            "candidates": candidates_found
        }))
    }

    /// Parses a single JSONL session transcript to locate failures, corrections, or tool gotchas.
    pub fn harvest_single_transcript(path: &Path) -> Result<Vec<Value>, String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let reader = BufReader::new(file);
        let mut candidates = Vec::new();
        let session_name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("session").to_string();

        let mut prev_user_msg = String::new();

        for line_res in reader.lines() {
            let line = match line_res {
                Ok(l) => l,
                Err(_) => continue,
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<Value>(&line) {
                let role = val.get("role").or_else(|| val.get("source")).and_then(|v| v.as_str()).unwrap_or("");
                let content = val.get("content").and_then(|v| v.as_str()).unwrap_or("");

                if role == "user" || role == "USER_EXPLICIT" {
                    // Detect corrections
                    let lower = content.to_lowercase();
                    if lower.contains("never") || lower.contains("do not") || lower.contains("always") || lower.contains("remember to") || lower.contains("prefer") {
                        if content.len() > 15 && content.len() < 400 {
                            candidates.push(json!({
                                "title": format!("Correction from {}", session_name),
                                "source_file": session_name,
                                "context": format!("User correction detected: \"{}\"", content.trim()),
                                "proposal": format!("Enforce constraint: {}", content.trim())
                            }));
                        }
                    }
                    prev_user_msg = content.to_string();
                } else if role == "tool" || val.get("tool_calls").is_some() {
                    // Check for command error output
                    if content.contains("exited with code") && !content.contains("exited with code 0") {
                        candidates.push(json!({
                            "title": format!("Tool failure in {}", session_name),
                            "source_file": session_name,
                            "context": format!("Tool execution failed under prompt: \"{}\"", prev_user_msg.chars().take(120).collect::<String>()),
                            "proposal": format!("Investigate command failure and record remediation invariant for error: {}", content.chars().take(200).collect::<String>())
                        }));
                    }
                }
            }
        }

        Ok(candidates)
    }
}
