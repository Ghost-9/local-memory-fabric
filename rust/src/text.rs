use crate::policy::{STOPWORDS, apple_path_re};
use regex::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};

pub(crate) fn regex(pattern: &str) -> Regex {
    static CACHE: OnceLock<Mutex<HashMap<String, Regex>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(compiled) = guard.get(pattern) {
            return compiled.clone();
        }
    }
    let compiled = Regex::new(pattern).expect("regular expression must compile");
    if pattern.len() <= 512 {
        if let Ok(mut guard) = cache.lock() {
            if guard.len() < 256 {
                guard
                    .entry(pattern.to_string())
                    .or_insert_with(|| compiled.clone());
            }
        }
    }
    compiled
}

pub(crate) fn tokenize(text: &str) -> BTreeSet<String> {
    let stop: HashSet<&str> = STOPWORDS.split_whitespace().collect();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for m in regex(r"[A-Za-z0-9]{2,}").find_iter(text) {
        let raw = m.as_str();
        if raw.len() < 2 {
            continue;
        }
        let low = raw.to_ascii_lowercase();
        if stop.contains(low.as_str()) {
            continue;
        }
        out.insert(low);
        // Split identifiers on case boundaries, keeping the original token too. Measured on the
        // live vault: `CliffCompaction` matched 2 documents and returned 0, because FTS5 never
        // derived `compaction` — the part the notes actually contain. Acronyms survive whole.
        //
        // Hand-rolled rather than a regex: the `regex` crate has no lookahead, which the obvious
        // `[A-Z]+(?![a-z])` pattern needs. Doing it by hand also keeps the behaviour explicit.
        let parts = split_identifier(raw);
        // Filter FIRST, then decide whether this is a compound identifier. Matches Python's
        // `[p for p in CAMEL_SPLIT_RE.findall(raw) if len(p) >= 3]` followed by `len(parts) > 1`.
        // Order matters: `SQLite` splits to [sq, lite], and after dropping "sq" only one part
        // remains, so it is NOT compound and no partial token is added. Filtering the other way
        // round would index "lite" out of "SQLite" and diverge from the Python engine.
        // Two characters minimum, mirroring the Python engine: `KV cache` must tokenize to
        // two terms, not one. Sub-3-char SPLIT parts are still dropped below, which is a
        // separate rule about decomposing an identifier rather than about word length.
        let parts: Vec<String> = parts.into_iter().filter(|p| p.len() >= 3).collect();
        if parts.len() > 1 {
            for p in parts {
                if !stop.contains(p.as_str()) {
                    out.insert(p);
                }
            }
        }
    }
    out
}

/// Split an identifier into lowercase parts at case boundaries.
///
/// `CliffCompaction` -> [cliff, compaction]; `parseNotes` -> [parse, notes];
/// `BM25` -> [bm25] (an acronym stays whole); `BM25Cache` -> [bm, cache].
pub(crate) fn split_identifier(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        let prev_upper = i > 0 && chars[i - 1].is_uppercase();
        let next_lower = i + 1 < chars.len() && chars[i + 1].is_lowercase();
        // Mirrors Python's `[A-Z]+(?![a-z])|[A-Z][a-z]+|[a-z]+|[0-9]+` exactly. Verified against it
        // by `camel_split_matches_the_python_engine`, which pins the cases that are easy to get
        // subtly wrong:
        //   lower -> Upper        "parseNotes"  -> parse | Notes
        //   digit starts a part   "BM25"       -> BM     | 25      (NOT bm25)
        //   upper run + lower    "FTS5-Lite"  -> FTS|5   | Lite
        // Because a digit run always begins a new part, "BM25" yields bm + 25 but the caller
        // drops parts shorter than 3, so "25" never reaches the index and BM25 still matches.
        let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
        let boundary = if c.is_ascii_digit() {
            !prev_digit
        } else {
            c.is_uppercase() && (prev_lower || (prev_upper && next_lower) || prev_digit)
        } || !c.is_alphanumeric();

        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if boundary && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

pub(crate) fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    if !text.starts_with("---\n") {
        return None;
    }
    let end = text[4..].find("\n---")? + 4;
    let header = &text[4..end];
    let pattern = format!(r#"(?im)^{}\s*:\s*[\"']?([^\"'\n#]+)"#, regex::escape(key));
    regex(&pattern)
        .captures(header)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

pub(crate) fn title_for(relpath: &str, text: &str) -> String {
    for line in text.lines().take(80) {
        if let Some(c) = regex(r"^#\s+(.+?)\s*#*\s*$").captures(line) {
            return c[1].trim().to_string();
        }
        if let Some(c) = regex(r#"(?i)^title:\s*[\"']?(.+?)[\"']?\s*$"#).captures(line) {
            return c[1].trim().to_string();
        }
    }
    Path::new(relpath)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace('-', " ")
        .replace('_', " ")
}

pub(crate) fn headings_for(text: &str) -> String {
    text.lines()
        .filter(|line| regex(r"^#{1,6}\s+").is_match(line))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn scope_for(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    match parts.first().copied().unwrap_or("") {
        "projects" => {
            let project = if parts.len() > 2 {
                parts[1].to_string()
            } else if parts.len() == 2 {
                Path::new(parts[1])
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            } else {
                "unknown".into()
            };
            format!("project:{project}")
        }
        "company" => "company".into(),
        "evidence" => "evidence".into(),
        "daily" => "episodic".into(),
        "pi-harness" => "system:pi-harness".into(),
        _ => "system".into(),
    }
}

pub(crate) fn is_skipped_name(name: &str, excluded: &HashSet<String>) -> bool {
    name.starts_with('.')
        || excluded.iter().any(|item| item.eq_ignore_ascii_case(name))
        || apple_path_re().is_match(name)
}

pub(crate) fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub(crate) fn json_error(message: impl Into<String>) -> Value {
    json!({"ok": false, "error": message.into()})
}

pub(crate) fn extract_wikilinks(text: &str) -> Vec<String> {
    let re = regex(r"\[\[([^\]|#]+)(?:[|#][^\]]*)?\]\]");
    let mut links = Vec::new();
    for cap in re.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            let target = m.as_str().trim();
            if !target.is_empty() {
                links.push(target.to_string());
            }
        }
    }
    links.sort();
    links.dedup();
    links
}
