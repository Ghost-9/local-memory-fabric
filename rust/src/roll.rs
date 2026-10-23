use serde_json::{Value, json};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

/// Automated Memory File Rolling.
/// Archives completed checkboxes and stale status sections into quarterly archive files
/// to keep STATUS.md and project notes concise and token-efficient.
pub struct MemoryRoller;

impl MemoryRoller {
    pub fn roll_file(
        vault_path: &Path,
        rel_target: &str,
        max_lines: usize,
        dry_run: bool,
    ) -> Result<Value, String> {
        let safe_rel = match crate::paths::safe_relpath(rel_target) {
            Ok(r) => r,
            Err(e) => {
                return Ok(crate::text::json_error(format!(
                    "unsafe target path: {}",
                    e
                )));
            }
        };
        let target_path = vault_path.join(&safe_rel);
        if let Ok(canon_vault) = vault_path.canonicalize()
            && let Ok(canon_target) = target_path.canonicalize()
            && !canon_target.starts_with(&canon_vault)
        {
            return Ok(crate::text::json_error(
                "target path escapes vault boundary",
            ));
        }
        if !target_path.exists() {
            return Ok(json!({
                "ok": false,
                "error": format!("Target file {} does not exist", rel_target)
            }));
        }

        let content = fs::read_to_string(&target_path).map_err(|e| e.to_string())?;
        let lines: Vec<&str> = content.lines().collect();

        if lines.len() <= max_lines {
            return Ok(json!({
                "ok": true,
                "rolled": false,
                "lines_count": lines.len(),
                "max_lines": max_lines,
                "message": format!("File {} has {} lines (below cap of {}). No rotation needed.", rel_target, lines.len(), max_lines)
            }));
        }

        let mut active_lines = Vec::new();
        let mut completed_items = Vec::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.starts_with("- [x]")
                || trimmed.starts_with("* [x]")
                || trimmed.contains("[DONE]")
            {
                completed_items.push(*line);
            } else {
                active_lines.push(*line);
            }
        }

        if completed_items.is_empty() {
            return Ok(json!({
                "ok": true,
                "rolled": false,
                "lines_count": lines.len(),
                "completed_count": 0,
                "message": "File exceeds line cap, but contains no completed items (- [x]) to archive."
            }));
        }

        // Determine quarter
        let now = chrono::Utc::now();
        let year = now.format("%Y").to_string();
        let month: u32 = now.format("%m").to_string().parse().unwrap_or(1);
        let quarter = (month - 1) / 3 + 1;
        let archive_rel = format!("archive/status-{}-Q{}.md", year, quarter);
        let archive_path = vault_path.join(&archive_rel);

        if !dry_run {
            if let Some(parent) = archive_path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }

            let mut archive_file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&archive_path)
                .map_err(|e| e.to_string())?;

            writeln!(
                archive_file,
                "\n## Archived on {} from {}\n",
                now.to_rfc3339(),
                rel_target
            )
            .map_err(|e| e.to_string())?;

            for item in &completed_items {
                writeln!(archive_file, "{}", item).map_err(|e| e.to_string())?;
            }

            // Write pruned active file
            let new_active_content = active_lines.join("\n") + "\n";
            fs::write(&target_path, new_active_content).map_err(|e| e.to_string())?;
        }

        Ok(json!({
            "ok": true,
            "rolled": true,
            "original_lines": lines.len(),
            "retained_lines": active_lines.len(),
            "archived_items": completed_items.len(),
            "archive_destination": archive_rel,
            "dry_run": dry_run
        }))
    }
}
