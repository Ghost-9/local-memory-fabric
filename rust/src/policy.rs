use crate::MemoryFabric;
use crate::text::regex;
use regex::Regex;

pub(crate) const DEFAULT_INCLUDE_DIRS: &[&str] = &[
    "agents",
    "company",
    "daily",
    "evidence",
    "pi-harness",
    "projects",
];
pub(crate) const DEFAULT_INCLUDE_FILES: &[&str] =
    &["README.md", "ROADMAP.md", "STATUS.md", "Welcome.md"];

pub(crate) const DEFAULT_EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".obsidian",
    ".trash",
    "archive",
    "archives",
    "templates",
    "apple-notes",
    "apple_notes",
    "notes-export",
    "notes_export",
];

pub(crate) const STOPWORDS: &str = "a an and are as at be been being but by can could did do does for from had has have how i in into is it its may might of on or our should so than that the their then there these this those to was we were what when where which who why will with would you your";

pub(crate) fn apple_path_re() -> Regex {
    regex(r"(?i)(?:apple[ _-]*notes|notes[ _-]*export)")
}

pub(crate) fn secret_signature(text: &str) -> Option<&'static str> {
    let patterns: [(&str, &str); 9] = [
        (
            "private_key",
            r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----",
        ),
        ("aws_access_key", r"\bAKIA[0-9A-Z]{16}\b"),
        (
            "github_token",
            r"\b(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})\b",
        ),
        ("slack_token", r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b"),
        ("stripe_live_key", r"\b(?:sk|rk)_live_[A-Za-z0-9]{16,}\b"),
        ("google_api_key", r"\bAIza[0-9A-Za-z_-]{30,}\b"),
        (
            "openai_api_key",
            r"(?:^|[^A-Za-z0-9])sk-(?:proj-)?[A-Za-z0-9_-]{20,}(?:$|[^A-Za-z0-9])",
        ),
        (
            "jwt",
            r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{8,}\b",
        ),
        (
            "bearer_credential",
            r"(?i)\bBearer\s+[A-Za-z0-9._~+/-]{24,}={0,2}",
        ),
    ];
    for (kind, pattern) in patterns {
        if regex(pattern).is_match(text) {
            return Some(kind);
        }
    }
    let assignment = regex(
        r#"(?i)(?:^|[^A-Za-z0-9])([A-Za-z0-9_-]*(?:api[_-]?key|client[_-]?secret|service[_-]?role[_-]?key|access[_-]?key|private[_-]?key|secret|password|passwd|token)[A-Za-z0-9_-]*)\s*[:=]\s*['\"]?([^\s'\"]{12,})"#,
    );
    let placeholders = [
        "placeholder",
        "redacted",
        "example",
        "sample",
        "change-me",
        "changeme",
        "your_",
        "your-",
        "insert_",
        "insert-",
        "replace_",
        "replace-",
        "not-a-real",
        "not_a_real",
        "dummy",
        "fake",
    ];
    if let Some(captures) = assignment.captures(text) {
        let value = captures
            .get(2)
            .map(|m| {
                m.as_str()
                    .trim_matches(['\'', '"', ' '])
                    .to_ascii_lowercase()
            })
            .unwrap_or_default();
        if !placeholders.iter().any(|marker| value.contains(marker))
            && !regex(r"^<[^>]+>$").is_match(&value)
        {
            return Some("credential_assignment");
        }
    }
    if regex(r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/:@]+:[^\s/@]+@").is_match(text) {
        return Some("credential_url");
    }
    None
}

/// A stable, non-reversible id for the secret in `text`, or None if there is none.
///
/// Tells the owner WHICH key to rotate without copying that key anywhere. Derived from the
/// matched VALUE, so the same key in two notes produces one fingerprint (one problem to
/// rotate) while two different keys never collide. Mirrors the Python engine exactly:
/// `sha256(value)[:12]`, with the same placeholder suppression.
pub(crate) fn secret_fingerprint(text: &str) -> Option<String> {
    let patterns: [(&str, &str); 11] = [
        ("private_key", r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----"),
        ("aws_access_key", r"\bAKIA[0-9A-Z]{16}\b"),
        ("github_token", r"\b(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})\b"),
        ("slack_token", r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b"),
        ("stripe_live_key", r"\b(?:sk|rk)_live_[A-Za-z0-9]{16,}\b"),
        ("google_api_key", r"\bAIza[0-9A-Za-z_-]{30,}\b"),
        ("openai_api_key", r"(?:^|[^A-Za-z0-9])sk-(?:proj-)?[A-Za-z0-9_-]{20,}(?:$|[^A-Za-z0-9])"),
        ("jwt", r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{8,}\b"),
        ("bearer_credential", r"(?i)\bBearer\s+[A-Za-z0-9._~+/-]{24,}={0,2}"),
        ("credential_assignment", r#"(?i)(?:^|[^A-Za-z0-9])([A-Za-z0-9_-]*(?:api[_-]?key|client[_-]?secret|service[_-]?role[_-]?key|access[_-]?key|private[_-]?key|secret|password|passwd|token)[A-Za-z0-9_-]*)\s*[:=]\s*['"]?([^\s'"]{12,})"#),
        ("credential_url", r"(?i)\b[a-z][a-z0-9+.-]*://[^\s/:@]+:[^\s/@]+@"),
    ];
    let placeholders = [
        "placeholder", "redacted", "example", "sample", "change-me", "changeme", "your_",
        "your-", "insert_", "replace_", "not-a-real", "not_a_real", "dummy", "fake",
    ];
    for (kind, pat) in patterns {
        let re = regex(pat);
        let Some(caps) = re.captures(text) else { continue };
        let value = caps.get(caps.len() - 1).map(|m| m.as_str()).unwrap_or("").to_string();
        if kind == "credential_assignment" {
            // Lowercase ONLY for the placeholder test, and hash the ORIGINAL value. Lowercasing
            // before hashing would make the fingerprint case-insensitive and diverge from the
            // Python engine, which hashes the value verbatim — two engines would then hand the
            // owner two different ids for the same key.
            let lowered = value
                .trim_matches(['\'', '"', ' '])
                .to_ascii_lowercase();
            if placeholders.iter().any(|m| lowered.contains(m))
                || regex(r"^<[^>]+>$").is_match(&lowered)
            {
                continue;
            }
        }
        return Some(crate::text::sha256(&value)[..12].to_string());
    }
    None
}


/// Dynamic streaming Data Loss Prevention (DLP) scanner for outbound excerpts in remote mode.
/// Redacts confidential tokens and credential strings in-place.
pub(crate) fn sanitize_remote_excerpt(text: &str) -> String {
    let mut out = text.to_string();
    let redact_patterns: [(&str, &str); 9] = [
        (r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----", "[REDACTED_PRIVATE_KEY]"),
        (r"AKIA[0-9A-Z]{16}", "[REDACTED_AWS_KEY]"),
        (r"(?:gh[pousr]_[A-Za-z0-9_]{30,}|github_pat_[A-Za-z0-9_]{30,})", "[REDACTED_GITHUB_TOKEN]"),
        (r"xox[baprs]-[A-Za-z0-9-]{20,}", "[REDACTED_SLACK_TOKEN]"),
        (r"(?:sk|rk)_live_[A-Za-z0-9]{16,}", "[REDACTED_STRIPE_KEY]"),
        (r"AIza[0-9A-Za-z_-]{30,}", "[REDACTED_GOOGLE_KEY]"),
        (r"(?:^|[^A-Za-z0-9])sk-(?:proj-)?[A-Za-z0-9_-]{20,}(?:$|[^A-Za-z0-9])", " [REDACTED_OPENAI_KEY] "),
        (r"(?i)Bearer\s+[A-Za-z0-9._~+/-]{24,}={0,2}", "Bearer [REDACTED_BEARER_TOKEN]"),
        (r"(?i)[a-z][a-z0-9+.-]*://[^\s/:@]+:[^\s/@]+@", "protocol://[REDACTED_USER_PASS]@"),
    ];

    for (pat, replacement) in redact_patterns {
        let re = regex(pat);
        out = re.replace_all(&out, replacement).to_string();
    }

    // Generic key assignments: KEY = "secret..."
    let assign_re = regex(r#"(?i)([A-Za-z0-9_-]*(?:api[_-]?key|client[_-]?secret|service[_-]?role[_-]?key|access[_-]?key|private[_-]?key|secret|password|passwd|token)[A-Za-z0-9_-]*\s*[:=]\s*['"]?)[^\s'"]{12,}(['"]?)"#);
    out = assign_re.replace_all(&out, "${1}[REDACTED_CREDENTIAL]${2}").to_string();

    out
}

impl MemoryFabric {
    pub(crate) fn is_local_provider(&self, provider: Option<&str>) -> bool {
        provider
            .map(|p| {
                self.config
                    .local_providers
                    .contains(&p.trim().to_ascii_lowercase())
            })
            .unwrap_or(false)
    }
}
