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
    if let Ok(guard) = cache.lock()
        && let Some(compiled) = guard.get(pattern)
    {
        return compiled.clone();
    }
    let compiled = Regex::new(pattern).expect("regular expression must compile");
    if pattern.len() <= 512
        && let Ok(mut guard) = cache.lock()
        && guard.len() < 256
    {
        guard
            .entry(pattern.to_string())
            .or_insert_with(|| compiled.clone());
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
        let parts = split_identifier(raw);
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

pub(crate) fn split_identifier(raw: &str) -> Vec<String> {
    let chars: Vec<char> = raw.chars().collect();
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev_lower = i > 0 && chars[i - 1].is_lowercase();
        let prev_upper = i > 0 && chars[i - 1].is_uppercase();
        let next_lower = i + 1 < chars.len() && chars[i + 1].is_lowercase();
        let prev_digit = i > 0 && chars[i - 1].is_ascii_digit();
        let boundary = if c.is_ascii_digit() {
            !prev_digit
        } else {
            c.is_uppercase() && (prev_lower || (prev_upper && next_lower) || prev_digit)
        } || !c.is_alphanumeric();

        if !c.is_alphanumeric() {
            if !cur.empty_or_whitespace() {
                parts.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if boundary && !cur.empty_or_whitespace() {
            parts.push(std::mem::take(&mut cur));
        }
        cur.push(c.to_ascii_lowercase());
    }
    if !cur.empty_or_whitespace() {
        parts.push(cur);
    }
    parts
}

trait EmptyCheck {
    fn empty_or_whitespace(&self) -> bool;
}

impl EmptyCheck for String {
    fn empty_or_whitespace(&self) -> bool {
        self.is_empty()
    }
}

pub(crate) fn frontmatter_value(text: &str, key: &str) -> Option<String> {
    if !text.starts_with("---\n") {
        return None;
    }
    let end = text[4..].find("\n---")? + 4;
    let header = &text[4..end];
    let pattern = format!(r#"(?im)^{}\s*:\s*["']?([^"'\n#]+)"#, regex::escape(key));
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
        if let Some(c) = regex(r#"(?i)^title:\s*["']?(.+?)["']?\s*$"#).captures(line) {
            return c[1].trim().to_string();
        }
    }
    Path::new(relpath)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .replace(['-', '_'], " ")
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct MarkdownSection {
    pub heading: String,
    pub level: usize,
    pub anchor: String,
    pub breadcrumbs: Vec<String>,
    pub line_start: usize,
    pub line_end: usize,
    pub char_start: usize,
    pub char_end: usize,
}

pub(crate) fn parse_sections(text: &str, title: &str) -> Vec<MarkdownSection> {
    let mut sections = Vec::new();
    let lines: Vec<&str> = text.lines().collect();
    let heading_re = regex(r"^(#{1,6})\s+(.+?)\s*#*$");

    let mut current_heading = title.to_string();
    let mut current_level = 0;
    let mut current_anchor = "overview".to_string();
    let mut current_breadcrumbs = vec![title.to_string()];
    let mut current_start_line = 1;
    let mut current_start_char = 0;
    let mut running_char = 0;
    let mut stack: Vec<(usize, String)> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;
        let line_len = line.chars().count() + 1;
        if let Some(caps) = heading_re.captures(line) {
            if running_char > current_start_char || current_level > 0 {
                sections.push(MarkdownSection {
                    heading: current_heading.clone(),
                    level: current_level,
                    anchor: current_anchor.clone(),
                    breadcrumbs: current_breadcrumbs.clone(),
                    line_start: current_start_line,
                    line_end: line_num.saturating_sub(1),
                    char_start: current_start_char,
                    char_end: running_char,
                });
            }
            let hashes = caps.get(1).map(|m| m.as_str().len()).unwrap_or(1);
            let h_text = caps
                .get(2)
                .map(|m| m.as_str().trim())
                .unwrap_or("")
                .to_string();
            current_heading = h_text.clone();
            current_level = hashes;
            current_anchor = h_text
                .to_ascii_lowercase()
                .replace(' ', "-")
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-')
                .collect();
            current_start_line = line_num;
            current_start_char = running_char;

            while let Some((lvl, _)) = stack.last() {
                if *lvl >= hashes {
                    stack.pop();
                } else {
                    break;
                }
            }
            stack.push((hashes, h_text.clone()));
            current_breadcrumbs = std::iter::once(title.to_string())
                .chain(stack.iter().map(|(_, name)| name.clone()))
                .collect();
        }
        running_char += line_len;
    }

    if running_char > current_start_char || current_level > 0 {
        sections.push(MarkdownSection {
            heading: current_heading,
            level: current_level,
            anchor: current_anchor,
            breadcrumbs: current_breadcrumbs,
            line_start: current_start_line,
            line_end: lines.len(),
            char_start: current_start_char,
            char_end: running_char,
        });
    }

    sections
}
