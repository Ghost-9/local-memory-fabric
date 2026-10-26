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
                if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("md") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        for line in content.lines() {
                            let trimmed = line.trim();
                            if (trimmed.starts_with("- ") || trimmed.starts_with("* ")) && trimmed.len() > 15 {
                                acc.push((path.file_name().unwrap_or_default().to_string_lossy().to_string(), trimmed.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }

    /// Does `a` assert one side of a mutually exclusive tooling pair while `b` asserts the other?
    ///
    /// Kept as a separate one-directional predicate so it can be applied in both orders; see the
    /// call site for why order must not matter.
    fn tooling_pair_matches(a_low: &str, b_low: &str, tech_1: &str, tech_2: &str) -> bool {
        (a_low.contains(tech_1) && a_low.contains("always") && b_low.contains(tech_2) && b_low.contains("always"))
            || (a_low.contains(tech_1) && a_low.contains("only") && b_low.contains(tech_2) && b_low.contains("only"))
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

        // Both orders are checked on purpose. `collect_rules_from_dir` fills `rules` in
        // `fs::read_dir` order, which is filesystem-dependent: APFS and ext4 return the same two
        // files in different orders. A one-directional check therefore reported this conflict on
        // macOS and silently missed it on Linux for byte-identical notes. A conflict detector whose
        // answer depends on directory iteration order is wrong on one of the two platforms.
        for (tech_1, tech_2) in &technology_pairs {
            let forward = Self::tooling_pair_matches(&a_low, &b_low, tech_1, tech_2);
            let reverse = Self::tooling_pair_matches(&a_low, &b_low, tech_2, tech_1);

            if forward || reverse {
                // Keep file_a/rule_a pointing at whichever side asserted `primary`, so the reported
                // pair reads in the same order regardless of which entry was read first.
                let (primary, secondary, primary_file, primary_rule, secondary_file, secondary_rule) =
                    if forward {
                        (*tech_1, *tech_2, file_a, a, file_b, b)
                    } else {
                        (*tech_2, *tech_1, file_b, b, file_a, a)
                    };

                return Some(ConflictItem {
                    file_a: primary_file.to_string(),
                    rule_a: primary_rule.to_string(),
                    file_b: secondary_file.to_string(),
                    rule_b: secondary_rule.to_string(),
                    conflict_type: format!(
                        "Tooling preference conflict between {} and {}",
                        primary, secondary
                    ),
                    suggested_resolution: format!(
                        "Designate primary standard ({}) or specify project-specific boundaries.",
                        primary
                    ),
                });
            }
        }

        // Direct negation check: "always use X" vs "never use X" / "do not use X".
        // Also evaluated in both orders, for the same iteration-order reason as above.
        let affirmative_tokens = ["always", "must", "prefer", "require"];
        let negative_tokens = ["never", "do not", "avoid", "forbidden", "disallow"];

        let polarity_conflict = |aff_text: &str, aff_low: &str, neg_text: &str, neg_low: &str| {
            for aff in &affirmative_tokens {
                for neg in &negative_tokens {
                    if aff_low.contains(aff) && neg_low.contains(neg) {
                        // Check if they share significant keywords (len >= 6)
                        let words: Vec<&str> = aff_low.split_whitespace().filter(|w| w.len() >= 6).collect();
                        for w in words {
                            if neg_low.contains(w) {
                                return Some(ConflictItem {
                                    file_a: file_a.to_string(),
                                    rule_a: aff_text.to_string(),
                                    file_b: file_b.to_string(),
                                    rule_b: neg_text.to_string(),
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
        };

        polarity_conflict(a, &a_low, b, &b_low).or_else(|| polarity_conflict(b, &b_low, a, &a_low))
    }
}

#[cfg(test)]
mod tests {
    use super::ConflictDetector;

    const VITEST_RULE: &str = "- We must always use vitest for fast testing.";
    const JEST_RULE: &str = "- We must always use jest for enterprise coverage.";

    /// The regression this module was fixed for.
    ///
    /// `collect_rules_from_dir` pushes rules in `fs::read_dir` order, which differs between APFS
    /// and ext4. The tooling-pair check used to test only "a asserts tech_1, b asserts tech_2", so
    /// byte-identical notes produced a conflict on macOS and none on Linux. Both argument orders are
    /// pinned here so detection cannot become order-dependent again.
    #[test]
    fn tooling_pair_is_detected_in_either_argument_order() {
        let forward =
            ConflictDetector::compare_rule_pair("rule_a.md", VITEST_RULE, "rule_b.md", JEST_RULE);
        let reverse =
            ConflictDetector::compare_rule_pair("rule_b.md", JEST_RULE, "rule_a.md", VITEST_RULE);

        assert!(forward.is_some(), "vitest-then-jest must be reported");
        assert!(reverse.is_some(), "jest-then-vitest must also be reported");
    }

    /// Whichever side asserted the first-listed technology is reported as `file_a`, so the pair
    /// reads in a stable order instead of flipping with directory iteration order.
    #[test]
    fn tooling_pair_report_order_does_not_depend_on_argument_order() {
        let forward =
            ConflictDetector::compare_rule_pair("rule_a.md", VITEST_RULE, "rule_b.md", JEST_RULE)
                .expect("forward conflict");
        let reverse =
            ConflictDetector::compare_rule_pair("rule_b.md", JEST_RULE, "rule_a.md", VITEST_RULE)
                .expect("reverse conflict");

        assert!(forward.rule_a.contains("vitest"));
        assert!(reverse.rule_a.contains("vitest"), "reversed input must still put vitest first");
        assert!(forward.conflict_type.contains("vitest"));
        assert!(reverse.conflict_type.contains("vitest"));
    }

    #[test]
    fn polarity_contradiction_is_detected_in_either_argument_order() {
        let aff = "- Always prefer postgres for the primary datastore.";
        let neg = "- Never use postgres for anything in this project.";

        assert!(ConflictDetector::compare_rule_pair("a.md", aff, "b.md", neg).is_some());
        assert!(ConflictDetector::compare_rule_pair("b.md", neg, "a.md", aff).is_some());
    }

    /// Detection must not fire on a single note, or on two notes that agree.
    #[test]
    fn agreeing_rules_are_not_a_conflict() {
        let a = "- We must always use vitest for fast testing.";
        let b = "- We must always use vitest for unit tests.";
        assert!(ConflictDetector::compare_rule_pair("a.md", a, "b.md", b).is_none());
    }
}
