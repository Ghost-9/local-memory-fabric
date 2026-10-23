use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// Semantic Contradiction & Superseded-Rule Detector.
/// Compares active invariant rules to detect opposing guidance and suggest resolution.
pub struct ConflictDetector;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConflictItem {
    pub file_a: String,
    pub rule_a: String,
    pub file_b: String,
    pub rule_b: String,
    pub conflict_type: String,
    pub suggested_resolution: String,
}

impl ConflictDetector {
    /// Scans the vault for conflicting invariants.
    pub fn scan_vault_conflicts(vault_path: &Path) -> Result<Value, String> {
        let mut rules: Vec<(String, String)> = Vec::new();
        let inv_dir = vault_path.join("invariants");
        let proj_dir = vault_path.join("projects");

        Self::collect_rules_from_dir(&inv_dir, &mut rules);
        Self::collect_rules_from_dir(&proj_dir, &mut rules);

        let mut detected_conflicts = Vec::new();

        for i in 0..rules.len() {
            for j in (i + 1)..rules.len() {
                let (file_a, text_a) = &rules[i];
                let (file_b, text_b) = &rules[j];
                if file_a == file_b {
                    continue;
                }

                if let Some(conflict) = Self::compare_rule_pair(file_a, text_a, file_b, text_b) {
                    detected_conflicts.push(conflict);
                }
            }
        }

        Ok(json!({
            "ok": true,
            "conflicts_found": detected_conflicts.len(),
            "conflicts": detected_conflicts
        }))
    }

    /// Evaluates a candidate proposal string against existing vault rules.
    #[allow(dead_code)]
    pub fn check_proposal_conflict(vault_path: &Path, candidate: &str) -> Vec<ConflictItem> {
        let mut rules = Vec::new();
        let inv_dir = vault_path.join("invariants");
        Self::collect_rules_from_dir(&inv_dir, &mut rules);

        let mut conflicts = Vec::new();
        for (file, existing_rule) in &rules {
            if let Some(c) = Self::compare_rule_pair("proposal", candidate, file, existing_rule) {
                conflicts.push(c);
            }
        }
        conflicts
    }

    fn collect_rules_from_dir(dir: &Path, acc: &mut Vec<(String, String)>) {
        if !dir.exists() || !dir.is_dir() {
            return;
        }
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && path.extension().and_then(|s| s.to_str()) == Some("md")
                    && let Ok(content) = fs::read_to_string(&path)
                {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if (trimmed.starts_with("- ") || trimmed.starts_with("* "))
                            && trimmed.len() > 15
                        {
                            acc.push((
                                path.file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_string(),
                                trimmed.to_string(),
                            ));
                        }
                    }
                }
            }
        }
    }

    fn compare_rule_pair(file_a: &str, a: &str, file_b: &str, b: &str) -> Option<ConflictItem> {
        let a_low = a.to_lowercase();
        let b_low = b.to_lowercase();

        let technology_pairs = [
            ("vitest", "jest"),
            ("pnpm", "npm"),
            ("yarn", "npm"),
            ("rust", "python"),
            ("zero-mcp", "mcp-required"),
        ];

        for (tech_1, tech_2) in &technology_pairs {
            if (a_low.contains(tech_1)
                && a_low.contains("always")
                && b_low.contains(tech_2)
                && b_low.contains("always"))
                || (a_low.contains(tech_1)
                    && a_low.contains("only")
                    && b_low.contains(tech_2)
                    && b_low.contains("only"))
            {
                return Some(ConflictItem {
                    file_a: file_a.to_string(),
                    rule_a: a.to_string(),
                    file_b: file_b.to_string(),
                    rule_b: b.to_string(),
                    conflict_type: format!(
                        "Tooling preference conflict between {} and {}",
                        tech_1, tech_2
                    ),
                    suggested_resolution: format!(
                        "Designate primary standard ({}) or specify project-specific boundaries.",
                        tech_1
                    ),
                });
            }
        }

        // Direct negation check: "always use X" vs "never use X" / "do not use X"
        let affirmative_tokens = ["always", "must", "prefer", "require"];
        let negative_tokens = ["never", "do not", "avoid", "forbidden", "disallow"];

        for aff in &affirmative_tokens {
            for neg in &negative_tokens {
                if a_low.contains(aff) && b_low.contains(neg) {
                    // Check if they share significant keywords (len >= 6)
                    let words_a: Vec<&str> =
                        a_low.split_whitespace().filter(|w| w.len() >= 6).collect();
                    for w in words_a {
                        if b_low.contains(w) {
                            return Some(ConflictItem {
                                file_a: file_a.to_string(),
                                rule_a: a.to_string(),
                                file_b: file_b.to_string(),
                                rule_b: b.to_string(),
                                conflict_type: format!(
                                    "Direct polarity contradiction on keyword '{}'",
                                    w
                                ),
                                suggested_resolution: format!(
                                    "Clarify condition or deprecate earlier rule in {}",
                                    file_b
                                ),
                            });
                        }
                    }
                }
            }
        }

        None
    }
}
