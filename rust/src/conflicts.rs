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

    /// Order-independent pair comparison.
    ///
    /// `fs::read_dir` yields entries in an order the filesystem chooses, so a
    /// comparison that only asks "is `a` the affirmative/vitest side and `b` the
    /// negative/jest side" detects a conflict on one platform and silently misses
    /// it on another. Measured 2026-10-23: the same fixture found a conflict on
    /// macOS (APFS) and zero on the Ubuntu CI runner, because the directory order
    /// was reversed. Both orientations are therefore evaluated, and the labels are
    /// swapped with them so the report still names the file each rule came from.
    fn compare_rule_pair(file_a: &str, a: &str, file_b: &str, b: &str) -> Option<ConflictItem> {
        if let Some(conflict) = Self::compare_directed(file_a, a, file_b, b) {
            return Some(conflict);
        }
        Self::compare_directed(file_b, b, file_a, a)
    }

    fn compare_directed(file_a: &str, a: &str, file_b: &str, b: &str) -> Option<ConflictItem> {
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

#[cfg(test)]
mod tests {
    use super::ConflictDetector;

    const TOOLING_A: (&str, &str) = ("rule_a.md", "- We must always use vitest for fast testing.");
    const TOOLING_B: (&str, &str) = (
        "rule_b.md",
        "- We must always use jest for enterprise coverage.",
    );
    const POLARITY_A: (&str, &str) = (
        "rule_c.md",
        "- You must always require pull request review before merging.",
    );
    const POLARITY_B: (&str, &str) = (
        "rule_d.md",
        "- Never require pull request review before merging.",
    );

    /// The regression this test exists for: the detector used to depend on the
    /// order `fs::read_dir` returned, so CI (Linux) and the reference machine
    /// (macOS) disagreed about whether the same two notes conflict.
    #[test]
    fn conflict_detection_is_independent_of_pair_order() {
        for ((fa, ra), (fb, rb)) in [
            (TOOLING_A, TOOLING_B),
            (TOOLING_B, TOOLING_A),
            (POLARITY_A, POLARITY_B),
            (POLARITY_B, POLARITY_A),
        ] {
            let conflict = ConflictDetector::compare_rule_pair(fa, ra, fb, rb);
            assert!(
                conflict.is_some(),
                "no conflict detected for {fa} vs {fb} — order dependence is back"
            );
            let conflict = conflict.unwrap();
            // Labels may swap with the orientation; what must not change is which two
            // files and which two rules were paired, or the report would name the
            // wrong note for its own rule.
            let mut files = [conflict.file_a.clone(), conflict.file_b.clone()];
            files.sort();
            let mut expected_files = [fa.to_string(), fb.to_string()];
            expected_files.sort();
            assert_eq!(
                files, expected_files,
                "the pair must name exactly these two files"
            );

            let mut rules = [conflict.rule_a.clone(), conflict.rule_b.clone()];
            rules.sort();
            let mut expected_rules = [ra.to_string(), rb.to_string()];
            expected_rules.sort();
            assert_eq!(
                rules, expected_rules,
                "no rule text may be dropped or duplicated"
            );
        }
    }

    /// A single rule is not a conflict with itself, and unrelated rules stay silent.
    #[test]
    fn unrelated_rules_are_not_reported_as_conflicts() {
        assert!(
            ConflictDetector::compare_rule_pair(
                "rule_a.md",
                "- We must always use vitest for fast testing.",
                "rule_e.md",
                "- Deployment notes live in the operations handbook.",
            )
            .is_none()
        );
        assert!(
            ConflictDetector::compare_rule_pair(TOOLING_A.0, TOOLING_A.1, TOOLING_A.0, TOOLING_A.1)
                .is_none()
        );
    }
}
