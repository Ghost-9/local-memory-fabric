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
    regex(r"[A-Za-z0-9]{3,}")
        .find_iter(text)
        .map(|m| m.as_str().to_ascii_lowercase())
        .filter(|term| !stop.contains(term.as_str()))
        .collect()
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
