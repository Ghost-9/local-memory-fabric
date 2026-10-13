/// Autonomous Background Knowledge Distillation & Invariant Compactor.
/// Clusters redundant episodic notes into durable invariant cards with explicit `supersedes` links.

use crate::MemoryFabric;
use crate::paths::now_utc;
use rusqlite::params;
use serde_json::{Value, json};

impl MemoryFabric {
    /// Identifies episodic notes eligible for compaction and synthesizes durable invariant proposals.
    pub(crate) fn compact(
        &self,
        project: Option<&str>,
        provider: Option<&str>,
    ) -> Result<Value, String> {
        if !self.is_local_provider(provider) {
            return Ok(json!({
                "ok": false,
                "blocked": "Knowledge distillation is available only to an explicitly allowed local model provider."
            }));
        }

        let con = self.connect()?;
        let target_project = project.unwrap_or("atlas");

        // Find episodic or active notes belonging to this project
        let mut stmt = con.prepare(
            "SELECT relpath, title, body, source_status FROM documents \
             WHERE (scope='episodic' OR relpath LIKE ?1) \
             AND source_status != 'superseded' \
             ORDER BY mtime_ns DESC LIMIT 50",
        ).map_err(|e| e.to_string())?;

        let proj_pattern = format!("projects/{}%", target_project);
        let rows = stmt.query_map(params![proj_pattern], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        }).map_err(|e| e.to_string())?;

        let mut candidate_notes = Vec::new();
        for r in rows {
            let (rel, title, body, status) = r.map_err(|e| e.to_string())?;
            candidate_notes.push((rel, title, body, status));
        }

        if candidate_notes.is_empty() {
            return Ok(json!({
                "ok": true,
                "compacted_count": 0,
                "message": "No uncompacted episodic notes found for this project."
            }));
        }

        // Synthesize distilled invariant content
        let invariant_relpath = format!("projects/{}-invariants.md", target_project);
        let mut superseded_refs = Vec::new();
        let mut distilled_points = Vec::new();

        for (rel, title, body, _) in &candidate_notes {
            superseded_refs.push(format!("[[{}]]", rel));
            let first_meaningful_line = body
                .lines()
                .find(|l| !l.trim().is_empty() && !l.starts_with('#'))
                .unwrap_or(&title);
            distilled_points.push(format!("- **{}**: {}", title, first_meaningful_line.trim()));
        }

        let superseded_list = superseded_refs.join(", ");
        let distilled_body = format!(
            "# {} Invariants & Architectural Anchors\n\n\
             > Compacted distillation generated at {}\n\
             > Supersedes: {}\n\n\
             ## Consolidated Architectural Invariants\n\n{}\n",
            target_project.to_ascii_uppercase(),
            now_utc(),
            superseded_list,
            distilled_points.join("\n")
        );

        // Record git head for rollback
        let git_head = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.vault)
            .args(["rev-parse", "HEAD"])
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    String::from_utf8(out.stdout).ok().map(|s| s.trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "unversioned".to_string());

        let proposal_req = json!({
            "op": "propose",
            "project": target_project,
            "target": invariant_relpath,
            "section": "Consolidated Architectural Invariants",
            "text": distilled_body,
            "source_refs": candidate_notes.iter().map(|(rel, _, _, _)| rel.clone()).collect::<Vec<_>>(),
            "provider": provider
        });

        // If invariant file does not exist yet, write directly or create proposal
        let target_file = self.vault.join(&invariant_relpath);
        let proposal_res = if target_file.exists() {
            self.propose(&proposal_req)?
        } else {
            if let Some(parent) = target_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&target_file, &distilled_body)
                .map_err(|e| format!("cannot write invariant card: {e}"))?;
            let _ = self.index(false);
            json!({
                "ok": true,
                "status": "created_invariant_card",
                "target": invariant_relpath
            })
        };

        let compacted_count = candidate_notes.len();

        Ok(json!({
            "ok": true,
            "project": target_project,
            "compacted_count": compacted_count,
            "invariant_card": invariant_relpath,
            "superseded_notes": superseded_refs,
            "git_rollback_hash": git_head,
            "proposal": proposal_res
        }))
    }
}
