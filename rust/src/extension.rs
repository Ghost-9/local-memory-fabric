//! Policy and session decisions used by the Pi transport extension.
//!
//! The extension sends observations and arguments to these functions; it does not decide what
//! memory may be read, written, injected, scrubbed, or retained in a session.

use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use url::Url;

pub(crate) const MAX_TRACKED_TOOL_IDS: usize = 4096;
pub(crate) const HANDOFF_THRESHOLD_TOKENS: u64 = 150_000;
pub(crate) const REMINDER_INTERVAL_TURNS: u64 = 25;
pub(crate) const HANDOFF_STALE_AFTER_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub(crate) const DEFAULT_SCRUB_NOTICE_INTERVAL_MS: i64 = 10 * 60 * 1000;

const GENERIC: &[&str] = &[
    "the", "and", "for", "with", "that", "this", "from", "have", "has", "had", "are", "was",
    "were", "not", "but", "you", "your", "our", "its", "it", "we", "us", "all", "any", "can",
    "will", "would", "should", "could", "please", "now", "then", "there", "here", "when", "what",
    "which", "who", "how", "why", "do", "does", "did", "done", "make", "made", "get", "got",
    "give", "one", "two", "also", "just", "only", "more", "most", "much", "many", "some", "such",
    "than", "too", "very", "into", "onto", "over", "under", "about", "after", "before", "again",
    "still", "need", "needs", "want", "work", "working", "fix", "fixes", "fixed", "issue",
    "issues", "thing", "found", "things", "check", "checks", "checked", "run", "runs", "use",
    "used", "uses", "let", "lets",
];

fn strings_from_env(name: &str) -> HashSet<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

pub(crate) fn remote_providers() -> HashSet<String> {
    strings_from_env("PI_MEMORY_REMOTE_ALLOWED_PROVIDERS")
}

pub(crate) fn remote_full_access_enabled() -> bool {
    matches!(
        std::env::var("PI_MEMORY_REMOTE_FULL_ACCESS")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub(crate) fn remote_context_scrubbing_enabled() -> bool {
    !matches!(
        std::env::var("PI_MEMORY_REMOTE_CONTEXT_SCRUB")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "0" | "false" | "no" | "off"
    )
}

fn endpoint(model: &Value) -> Option<Url> {
    let raw = model
        .get("base_url")
        .or_else(|| model.get("baseUrl"))?
        .as_str()?;
    let url = Url::parse(raw).ok()?;
    matches!(url.scheme(), "http" | "https").then_some(url)
}

fn model_name(model: &Value) -> Option<String> {
    model
        .get("provider")?
        .as_str()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
}

fn endpoint_is_loopback(url: &Url) -> bool {
    matches!(
        url.host_str()
            .unwrap_or("")
            .trim_matches(['[', ']'])
            .to_ascii_lowercase()
            .as_str(),
        "localhost" | "127.0.0.1" | "::1"
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProviderDecision {
    pub(crate) allowed: bool,
    pub(crate) loopback: bool,
    pub(crate) remote_allowlisted: bool,
    pub(crate) scrub_context: bool,
    pub(crate) may_write: bool,
}

pub(crate) fn provider_decision(
    model: &Value,
    local: &HashSet<String>,
    remote: &HashSet<String>,
    full_access: bool,
    scrub_remote: bool,
) -> ProviderDecision {
    let name = model_name(model);
    let url = endpoint(model);
    let loopback = name.as_ref().is_some_and(|n| local.contains(n))
        && url.as_ref().is_some_and(endpoint_is_loopback);
    let remote_allowlisted = name.as_ref().is_some_and(|n| remote.contains(n)) && url.is_some();
    ProviderDecision {
        allowed: loopback || remote_allowlisted,
        loopback,
        remote_allowlisted,
        // An unapproved endpoint is always scrubbed. The opt-out applies only to a model that
        // passed the explicit remote allowlist and can therefore read memory.
        scrub_context: !loopback && (!remote_allowlisted || scrub_remote),
        may_write: loopback || (remote_allowlisted && full_access),
    }
}

pub(crate) fn provider_decision_json(model: &Value, local: &HashSet<String>) -> Value {
    let d = provider_decision(
        model,
        local,
        &remote_providers(),
        remote_full_access_enabled(),
        remote_context_scrubbing_enabled(),
    );
    json!({
        "allowed": d.allowed,
        "loopback": d.loopback,
        "remote_allowlisted": d.remote_allowlisted,
        "scrub_context": d.scrub_context,
        "may_write": d.may_write,
        "write_mode": if d.may_write { "write" } else if d.allowed { "propose_only" } else { "denied" },
    })
}

pub(crate) fn authorized_request(
    provider: Option<&str>,
    base_url: Option<&str>,
    loopback_hint: Option<bool>,
    local: &HashSet<String>,
) -> ProviderDecision {
    let model = json!({"provider": provider, "base_url": base_url});
    let mut d = provider_decision(
        &model,
        local,
        &remote_providers(),
        remote_full_access_enabled(),
        remote_context_scrubbing_enabled(),
    );
    // Direct CLI callers are an explicit local operation. Extension/MCP requests must provide a
    // usable endpoint; the boolean hint alone can never turn an unknown host into loopback.
    if base_url.is_none()
        && loopback_hint == Some(true)
        && provider.is_some_and(|p| local.contains(&p.trim().to_ascii_lowercase()))
    {
        d = ProviderDecision {
            allowed: true,
            loopback: true,
            remote_allowlisted: false,
            scrub_context: false,
            may_write: true,
        };
    }
    d
}

pub(crate) fn may_apply(
    provider: Option<&str>,
    base_url: Option<&str>,
    loopback_hint: Option<bool>,
    local: &HashSet<String>,
) -> bool {
    authorized_request(provider, base_url, loopback_hint, local).may_write
}

pub(crate) fn content_words(text: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut words = Vec::new();
    let mut current = String::new();
    for c in text.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            current.push(c);
        } else if !current.is_empty() {
            if current.len() >= 4
                && !GENERIC.contains(&current.as_str())
                && seen.insert(current.clone())
            {
                words.push(current.clone());
            }
            current.clear();
        }
    }
    if !current.is_empty()
        && current.len() >= 4
        && !GENERIC.contains(&current.as_str())
        && seen.insert(current.clone())
    {
        words.push(current);
    }
    words
}

pub(crate) fn decide_injection(prompt: &str, response: &Value) -> Value {
    let content = content_words(prompt);
    let results = response
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let none = |reason: &str| json!({"inject": false, "reason": reason, "coverage": 0.0, "matched": 0, "content": content.len()});
    if results.is_empty() {
        return none("no results");
    }
    if response.get("low_discrimination").and_then(Value::as_bool) == Some(true) {
        return none("search flagged low discrimination");
    }
    if response
        .get("abstention")
        .is_some_and(|v| !v.is_null() && !v.as_str().unwrap_or("").is_empty())
    {
        return none("search abstained");
    }
    if content.len() < 2 {
        return none("prompt has too few content words");
    }
    let content_set: HashSet<&str> = content.iter().map(String::as_str).collect();
    let matched = results[0]
        .get("matched_terms")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .filter(|s| content_set.contains(s.as_str()))
        .count();
    let coverage = matched as f64 / content.len() as f64;
    if matched < 2 {
        return json!({"inject": false, "reason": format!("best result matched {matched} content word(s), need 2"), "coverage": coverage, "matched": matched, "content": content.len()});
    }
    if coverage < 0.5 {
        return json!({"inject": false, "reason": format!("coverage {:.0}% is below 50%", coverage * 100.0), "coverage": coverage, "matched": matched, "content": content.len()});
    }
    json!({"inject": true, "reason": format!("matched {matched}/{} content words", content.len()), "coverage": coverage, "matched": matched, "content": content.len()})
}

fn filename_shaped(query: &str) -> bool {
    let q = query.trim();
    if q.is_empty() || q.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((_, ext)) = q.rsplit_once('.') else {
        return false;
    };
    (1..=8).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric())
}

pub(crate) fn decide_abstention(query: &str, evidence: &Value) -> Value {
    let engine_reason = evidence
        .get("abstention")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if !engine_reason.is_empty() {
        return json!({"abstained": true, "reason": engine_reason});
    }
    if evidence.get("low_discrimination").and_then(Value::as_bool) == Some(true) {
        return json!({"abstained": true, "reason": "the engine flagged these terms as non-discriminating; the results are unranked candidates, not a ranked answer"});
    }
    let content = content_words(query);
    if content.is_empty() {
        return json!({"abstained": true, "reason": "the query has no discriminative words; every term is a function word or shorter than four characters"});
    }
    let results = evidence
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let top = results.first();
    let matched: HashSet<String> = top
        .and_then(|v| v.get("matched_terms"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect();
    if filename_shaped(query) && !results.is_empty() {
        let literal = query.trim().to_ascii_lowercase();
        let excerpts: Vec<&str> = results
            .iter()
            .filter_map(|r| r.get("excerpt").and_then(Value::as_str))
            .collect();
        if !excerpts.is_empty()
            && !excerpts
                .iter()
                .any(|e| e.to_ascii_lowercase().contains(&literal))
        {
            return json!({"abstained": true, "reason": format!("no indexed source contains the literal `{}`; the matches are on its segments, which is a vocabulary collision rather than the file asked for", query.trim())});
        }
    }
    if !matched.is_empty() && content.iter().all(|w| !matched.contains(w)) {
        return json!({"abstained": true, "reason": format!("the best result matched none of the query's {} discriminative word(s)", content.len())});
    }
    json!({"abstained": false, "reason": ""})
}

pub(crate) fn with_abstention(query: &str, response: &Value) -> Value {
    let mut out = response.clone();
    if !out.is_object() {
        out = json!({});
    }
    let verdict = decide_abstention(query, response);
    let abstained = verdict["abstained"].as_bool().unwrap_or(false);
    out["abstained"] = json!(abstained);
    if abstained {
        out["abstention"] = verdict["reason"].clone();
    } else if out.get("abstention").is_none() {
        out["abstention"] = Value::Null;
    }
    out
}

pub(crate) fn project_key(cwd: Option<&str>) -> Option<String> {
    let cwd = cwd?.trim().trim_end_matches('/');
    if cwd.is_empty() || cwd == "/" || cwd == "." {
        None
    } else {
        Some(cwd.to_string())
    }
}

pub(crate) fn workspace_project(workspace_path: &str) -> String {
    let name = Path::new(workspace_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .trim();
    let slug = name.to_ascii_lowercase().replace([' ', '_'], "-");
    if slug.is_empty() {
        "workspace".into()
    } else {
        slug
    }
}

pub(crate) fn t3_system_instruction(project: &str) -> String {
    format!(
        "<!-- LOCAL-MEMORY-FABRIC-T3-CODE -->\n[PROJECT: {project}]\nThis thread is linked to the Local Memory Fabric index.\nQuery memories using `memory_fabric_context` or `memory_fabric_search`.\nAll writes must be submitted through `memory_fabric_propose`.\n<!-- /LOCAL-MEMORY-FABRIC-T3-CODE -->"
    )
}

pub(crate) fn tier_of(relpath: &str) -> &'static str {
    let p = relpath.trim_start_matches('/');
    if p.starts_with("projects/") {
        return "project";
    }
    if [
        "user/",
        "invariants/",
        "pi-harness/",
        "company/",
        "agents/",
        "evidence/",
    ]
    .iter()
    .any(|prefix| p.starts_with(prefix))
    {
        return "global";
    }
    "episodic"
}

pub(crate) fn project_note_matches(relpath: &str, cwd: Option<&str>) -> bool {
    let Some(key) = project_key(cwd) else {
        return false;
    };
    let Some(name) = relpath
        .strip_prefix("projects/")
        .and_then(|s| s.strip_suffix(".md"))
    else {
        return false;
    };
    if name.contains('/') {
        return false;
    }
    let base = key.rsplit('/').next().unwrap_or("").to_ascii_lowercase();
    let slug = name.to_ascii_lowercase();
    base.len() >= 5
        && (slug == base
            || slug.starts_with(&format!("{base}-"))
            || base.starts_with(&format!("{slug}-")))
}

pub(crate) fn may_inject_handoff(handoff: &Value, cwd: Option<&str>) -> bool {
    let Some(key) = project_key(cwd) else {
        return false;
    };
    let owner = handoff
        .get("cwd")
        .and_then(Value::as_str)
        .and_then(|s| project_key(Some(s)));
    owner.as_deref() == Some(key.as_str())
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CostState {
    turns: u64,
    peak_tokens: u64,
    crossed_at_turn: u64,
    last_reminder_turn: u64,
    reminders: u64,
}

fn positive_tokens(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

pub(crate) fn prompt_tokens(usage: &Value) -> u64 {
    positive_tokens(usage.get("input"))
        .saturating_add(positive_tokens(usage.get("cacheRead")))
        .saturating_add(positive_tokens(usage.get("cacheWrite")))
}

pub(crate) fn record_turn(
    state: &mut CostState,
    usage: &Value,
    threshold: u64,
    interval: u64,
) -> Value {
    let tokens = prompt_tokens(usage);
    let cache_read = positive_tokens(usage.get("cacheRead"));
    state.turns += 1;
    state.peak_tokens = state.peak_tokens.max(tokens);
    let mut advise = false;
    let mut reason = "";
    if tokens >= threshold {
        if state.crossed_at_turn == 0 {
            state.crossed_at_turn = state.turns;
            state.last_reminder_turn = state.turns;
            state.reminders += 1;
            advise = true;
            reason = "crossed";
        } else if state.turns.saturating_sub(state.last_reminder_turn) >= interval {
            state.last_reminder_turn = state.turns;
            state.reminders += 1;
            advise = true;
            reason = "interval";
        }
    }
    json!({"state":{"turns":state.turns,"peakTokens":state.peak_tokens,"crossedAtTurn":state.crossed_at_turn,"lastReminderTurn":state.last_reminder_turn,"reminders":state.reminders},"advise":advise,"peakTokens":state.peak_tokens,"cacheReadTokens":cache_read,"reason":reason})
}

pub(crate) fn cost_notice(tokens: u64) -> String {
    let k = (tokens as f64 / 1000.0).round() as u64;
    let equivalent = (tokens as f64 * 0.1 / 1000.0).round() as u64;
    format!(
        "[MEMORY: SESSION COST] This session's prompt is {k}K tokens; every turn re-reads it at the cache rate (~{equivalent}K token-equivalents) before doing any work. At a task boundary, call memory_fabric_handoff and start a fresh session: a handoff is ~3K against this, and a resume from it is automatic."
    )
}

pub(crate) fn resume_pointer(handoff: &Value, now_ms: i64) -> Option<String> {
    let mut parts = Vec::new();
    for field in ["done", "in_flight", "blocked", "next"] {
        if let Some(s) = handoff
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            parts.push(format!("{}: {}", field.replace('_', "-"), s));
        }
    }
    if parts.is_empty() {
        return None;
    }
    let age = handoff
        .get("at")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|then| {
            let mins = ((now_ms - then.timestamp_millis()).max(0) as f64 / 60_000.0).round() as u64;
            if mins < 60 {
                format!("{mins}m ago")
            } else if mins < 2880 {
                format!("{}h ago", (mins as f64 / 60.0).round() as u64)
            } else {
                format!("{}d ago", (mins as f64 / 1440.0).round() as u64)
            }
        })
        .unwrap_or_default();
    let transcript = handoff
        .get("transcript")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(|p| format!("Full handoff: inbox/RESUME.md | transcript: {p}"))
        .unwrap_or_else(|| "Full handoff: inbox/RESUME.md".into());
    Some(format!(
        "[RESUME] A previous session handed off{}{}. {}. {transcript}",
        if age.is_empty() { "" } else { " " },
        age,
        parts.join(" | ")
    ))
}

pub(crate) fn is_resumable(handoff: &Value, now_ms: i64) -> bool {
    let Some(at) = handoff
        .get("at")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
    else {
        return false;
    };
    let age = now_ms - at.timestamp_millis();
    (0..=HANDOFF_STALE_AFTER_MS).contains(&age)
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HealthState {
    pub(crate) consecutive_failures: u64,
    pub(crate) breaker_opened_at: Option<i64>,
    pub(crate) respawn_times: Vec<i64>,
    pub(crate) refusals: u64,
    pub(crate) respawn_refusals: u64,
}

impl HealthState {
    pub(crate) fn record(&mut self, ok: bool, now: i64, threshold: u64) {
        if ok {
            self.consecutive_failures = 0;
            self.breaker_opened_at = None;
            return;
        }
        self.consecutive_failures += 1;
        if self.consecutive_failures >= threshold && self.breaker_opened_at.is_none() {
            self.breaker_opened_at = Some(now);
        }
    }
    pub(crate) fn allow(&mut self, now: i64, window_ms: i64) -> bool {
        let Some(opened) = self.breaker_opened_at else {
            return true;
        };
        if now - opened < window_ms {
            self.refusals += 1;
            return false;
        }
        self.breaker_opened_at = None;
        self.consecutive_failures = 0;
        true
    }
    pub(crate) fn allow_respawn(&mut self, now: i64, limit: usize, window_ms: i64) -> bool {
        self.respawn_times.retain(|t| now - *t < window_ms);
        if self.respawn_times.len() >= limit {
            self.respawn_refusals += 1;
            return false;
        }
        self.respawn_times.push(now);
        true
    }
    pub(crate) fn refusal_message(&self, now: i64, window_ms: i64) -> String {
        let remaining = self
            .breaker_opened_at
            .map(|at| ((window_ms - (now - at)).max(0) as f64 / 1000.0).ceil() as u64)
            .unwrap_or(0);
        format!(
            "Local memory is unavailable: the engine failed {} times in a row; retrying in {remaining}s. Run /memory-fabric status to inspect it.",
            self.consecutive_failures
        )
    }
}

#[derive(Default)]
struct ScrubState {
    tracked: HashSet<String>,
    hashes: HashSet<String>,
    overflowed: bool,
    last_notified_at: Option<i64>,
}

static SCRUB_STATES: OnceLock<Mutex<HashMap<String, ScrubState>>> = OnceLock::new();
static COST_STATES: OnceLock<Mutex<HashMap<String, CostState>>> = OnceLock::new();
static HEALTH_STATES: OnceLock<Mutex<HashMap<String, HealthState>>> = OnceLock::new();
static INJECTION_ACCOUNTS: OnceLock<Mutex<HashMap<String, InjectionAccount>>> = OnceLock::new();

#[derive(Default)]
pub(crate) struct InjectionState {
    refs: Option<Vec<String>>,
    injected: bool,
    resume_checked: bool,
}

static INJECTION_STATES: OnceLock<Mutex<HashMap<String, InjectionState>>> = OnceLock::new();
static SESSION_WRITES: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

pub(crate) fn cached_refs(session: &str) -> Option<Vec<String>> {
    INJECTION_STATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .ok()?
        .get(session)
        .and_then(|s| s.refs.clone())
}

pub(crate) fn set_cached_refs(session: &str, refs: Vec<String>) {
    let states = INJECTION_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut states) = states.lock() {
        states.entry(session.to_string()).or_default().refs = Some(refs);
    }
}

pub(crate) fn reserve_injection(session: &str) -> bool {
    let states = INJECTION_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut states) = states.lock() else {
        return false;
    };
    let state = states.entry(session.to_string()).or_default();
    if state.injected {
        return false;
    }
    state.injected = true;
    true
}

pub(crate) fn reserve_resume_check(session: &str) -> bool {
    let states = INJECTION_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut states) = states.lock() else {
        return false;
    };
    let state = states.entry(session.to_string()).or_default();
    if state.resume_checked {
        return false;
    }
    state.resume_checked = true;
    true
}

pub(crate) fn record_session_write(session: &str, budget: u64) -> bool {
    let states = SESSION_WRITES.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut states) = states.lock() else {
        return false;
    };
    let used = states.entry(session.to_string()).or_default();
    *used += 1;
    *used <= budget
}

fn touches_vault(value: &Value, vault: &str) -> bool {
    match value {
        Value::String(s) => {
            let normalized = s.replace('\\', "/");
            let lower = normalized.to_ascii_lowercase();
            normalized.contains(&vault.replace('\\', "/"))
                || lower.contains("system-memory/")
                || lower.contains("com.apple.notes")
                || lower.contains("notestore.sqlite")
                || crate::policy::apple_path_re().is_match(s)
        }
        Value::Array(xs) => xs.iter().any(|x| touches_vault(x, vault)),
        Value::Object(m) => m.values().any(|x| touches_vault(x, vault)),
        _ => false,
    }
}

fn is_local_provider(provider: &Value, local: &HashSet<String>) -> bool {
    provider
        .as_str()
        .is_some_and(|p| local.contains(&p.to_ascii_lowercase()))
}

fn message_fingerprint(message: &Value) -> String {
    let mut normalized = message.clone();
    if let Some(object) = normalized.as_object_mut() {
        object.remove("provider");
    }
    crate::text::sha256(&serde_json::to_string(&normalized).unwrap_or_default())
}

pub(crate) fn remember_loopback_turn(session: &str, message: &Value) {
    let states = SCRUB_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut states) = states.lock() {
        let state = states.entry(session.to_string()).or_default();
        state.hashes.insert(message_fingerprint(message));
        if state.hashes.len() > MAX_TRACKED_TOOL_IDS {
            state.overflowed = true;
        }
    }
}

pub(crate) fn parse_scrub_notice_interval(value: Option<&str>) -> i64 {
    let Some(value) = value else {
        return DEFAULT_SCRUB_NOTICE_INTERVAL_MS;
    };
    let value = value.trim();
    if value.is_empty()
        || value.trim_start_matches(['+', '-']).is_empty()
        || !value
            .trim_start_matches(['+', '-'])
            .bytes()
            .all(|b| b.is_ascii_digit())
    {
        return DEFAULT_SCRUB_NOTICE_INTERVAL_MS;
    }
    value
        .parse::<i64>()
        .unwrap_or(DEFAULT_SCRUB_NOTICE_INTERVAL_MS)
}

pub(crate) fn scrub_context(
    session: &str,
    messages: &Value,
    vault: &str,
    local: &HashSet<String>,
    now: i64,
    notice_interval: i64,
) -> Value {
    let state_map = SCRUB_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = state_map.lock().expect("scrub state mutex poisoned");
    let state = states.entry(session.to_string()).or_default();
    let messages = messages.as_array().cloned().unwrap_or_default();
    for message in &messages {
        if message.get("role").and_then(Value::as_str) != Some("assistant")
            || !is_local_provider(&message["provider"], local)
        {
            continue;
        }
        state.hashes.insert(message_fingerprint(message));
        let Some(parts) = message.get("content").and_then(Value::as_array) else {
            continue;
        };
        for part in parts {
            if part.get("type").and_then(Value::as_str) != Some("toolCall") {
                continue;
            }
            let Some(id) = part.get("id").and_then(Value::as_str) else {
                continue;
            };
            let name = part.get("name").and_then(Value::as_str).unwrap_or("");
            let args = part
                .get("arguments")
                .or_else(|| part.get("input"))
                .unwrap_or(&Value::Null);
            if name.starts_with("memory_fabric_") || touches_vault(args, vault) {
                state.tracked.insert(id.to_string());
                if state.tracked.len() > MAX_TRACKED_TOOL_IDS {
                    state.overflowed = true;
                }
            }
        }
    }
    let filtered: Vec<Value> = messages
        .iter()
        .filter(|message| {
            let custom = message
                .get("customType")
                .and_then(Value::as_str)
                .unwrap_or("");
            if custom.starts_with("vault-context") {
                return false;
            }
            if message.get("role").and_then(Value::as_str) == Some("assistant")
                && is_local_provider(&message["provider"], local)
            {
                return false;
            }
            if message.get("role").and_then(Value::as_str) == Some("toolResult") {
                let name = message
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let id = message
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                if name.starts_with("memory_fabric_")
                    || state.tracked.contains(id)
                    || state.overflowed
                {
                    return false;
                }
            }
            if message.get("role").and_then(Value::as_str) == Some("assistant")
                && state.hashes.contains(&message_fingerprint(message))
            {
                return false;
            }
            true
        })
        .cloned()
        .collect();
    let removed = filtered.len() != messages.len();
    let notify = if !removed {
        false
    } else if state.last_notified_at.is_none() || notice_interval < 0 {
        state.last_notified_at = Some(now);
        true
    } else if notice_interval == 0 {
        false
    } else if now - state.last_notified_at.unwrap_or(now) >= notice_interval {
        state.last_notified_at = Some(now);
        true
    } else {
        false
    };
    json!({"messages":filtered,"removed_anything":removed,"notice":notify,"tracked_ids":state.tracked.len(),"overflowed":state.overflowed})
}

pub(crate) fn record_session_turn(
    session: &str,
    usage: &Value,
    threshold: u64,
    interval: u64,
) -> Value {
    let states = COST_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = states.lock().expect("cost state mutex poisoned");
    let state = states.entry(session.to_string()).or_default();
    record_turn(state, usage, threshold, interval)
}

#[derive(Clone, Debug, Default)]
pub(crate) struct InjectionAccount {
    baseline_chars: Option<u64>,
    turns: u64,
    session_chars: u64,
    prefix_breaks: u64,
}

impl InjectionAccount {
    pub(crate) fn record(&mut self, chars: u64, refs: u64) -> Value {
        self.turns += 1;
        self.session_chars = self.session_chars.saturating_add(chars);
        let prefix_break = match self.baseline_chars {
            None => {
                self.baseline_chars = Some(chars);
                false
            }
            Some(baseline) if baseline != chars => {
                self.prefix_breaks += 1;
                true
            }
            Some(_) => false,
        };
        json!({"turn":self.turns,"chars":chars,"refs":refs,"sessionChars":self.session_chars,"prefixBreak":prefix_break})
    }

    pub(crate) fn state(&self) -> Value {
        json!({"turns":self.turns,"sessionChars":self.session_chars,"baselineChars":self.baseline_chars,"prefixBreaks":self.prefix_breaks})
    }
}

pub(crate) fn record_injection_account(session: &str, chars: u64, refs: u64) -> Value {
    let accounts = INJECTION_ACCOUNTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut accounts = accounts.lock().expect("injection account mutex poisoned");
    accounts
        .entry(session.to_string())
        .or_default()
        .record(chars, refs)
}

fn injection_account_state(session: &str) -> Value {
    let accounts = INJECTION_ACCOUNTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut accounts = accounts.lock().expect("injection account mutex poisoned");
    accounts.entry(session.to_string()).or_default().state()
}

pub(crate) fn health_operation(session: &str, request: &Value) -> Value {
    let states = HEALTH_STATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut states = states.lock().expect("health state mutex poisoned");
    let state = states.entry(session.to_string()).or_default();
    let action = request
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("status");
    let now = request.get("now_ms").and_then(Value::as_i64).unwrap_or(0);
    let threshold = request
        .get("threshold")
        .and_then(Value::as_u64)
        .unwrap_or(3);
    let window = request
        .get("window_ms")
        .and_then(Value::as_i64)
        .unwrap_or(60_000);
    match action {
        "record" => state.record(
            request.get("ok").and_then(Value::as_bool).unwrap_or(false),
            now,
            threshold,
        ),
        "allow" => return json!({"allowed":state.allow(now, window),"state":health_json(state)}),
        "allow-respawn" => {
            let allowed = state.allow_respawn(
                now,
                request
                    .get("respawn_limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(3) as usize,
                request
                    .get("respawn_window_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or(60_000),
            );
            return json!({"allowed":allowed,"state":health_json(state)});
        }
        _ => {}
    }
    json!({"state":health_json(state),"message":if state.breaker_opened_at.is_some(){Value::String(state.refusal_message(now,window))}else{Value::Null}})
}

fn health_json(state: &HealthState) -> Value {
    json!({"consecutiveFailures":state.consecutive_failures,"breakerOpenedAt":state.breaker_opened_at,"respawnTimes":state.respawn_times,"refusals":state.refusals,"respawnRefusals":state.respawn_refusals})
}

pub(crate) fn dispatch_policy_op(
    op: &str,
    req: &Value,
    vault: &str,
    local: &HashSet<String>,
) -> Option<Value> {
    let session = req
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("default");
    let result = match op {
        "provider-policy" => {
            provider_decision_json(&req.get("model").cloned().unwrap_or(Value::Null), local)
        }
        "injection-decision" => decide_injection(
            req.get("prompt").and_then(Value::as_str).unwrap_or(""),
            &req.get("response").cloned().unwrap_or(Value::Null),
        ),
        "abstention-decision" => {
            let query = req.get("query").and_then(Value::as_str).unwrap_or("");
            let evidence = req.get("evidence").cloned().unwrap_or(Value::Null);
            let verdict = decide_abstention(query, &evidence);
            if req.get("merge").and_then(Value::as_bool) == Some(true) {
                with_abstention(query, &evidence)
            } else {
                verdict
            }
        }
        "scope-policy" => {
            let action = req.get("action").and_then(Value::as_str).unwrap_or("tier");
            let path = req.get("relpath").and_then(Value::as_str).unwrap_or("");
            let cwd = req.get("cwd").and_then(Value::as_str);
            match action {
                "tier" => json!({"tier":tier_of(path)}),
                "project-match" => json!({"matches":project_note_matches(path,cwd)}),
                "handoff" => {
                    json!({"project_key":project_key(cwd),"may_inject":may_inject_handoff(&req["handoff"],cwd)})
                }
                _ => json!({"error":"unknown scope-policy action"}),
            }
        }
        "t3-adapter" => {
            let action = req
                .get("action")
                .and_then(Value::as_str)
                .unwrap_or("instruction");
            let project = req.get("project").and_then(Value::as_str).unwrap_or("");
            match action {
                "resolve-workspace" => {
                    json!({"project":workspace_project(req.get("workspace_path").and_then(Value::as_str).unwrap_or(""))})
                }
                "instruction" => json!({"instruction":t3_system_instruction(project)}),
                _ => json!({"error":"unknown T3 adapter action"}),
            }
        }
        "scrub-context" => {
            let model = req.get("model").cloned().unwrap_or(Value::Null);
            let decision = provider_decision(
                &model,
                local,
                &remote_providers(),
                remote_full_access_enabled(),
                remote_context_scrubbing_enabled(),
            );
            if !decision.scrub_context {
                json!({"messages":req.get("messages").and_then(Value::as_array).cloned().unwrap_or_default(),"removed_anything":false,"notice":false,"tracked_ids":0,"overflowed":false})
            } else {
                scrub_context(
                    session,
                    &req["messages"],
                    vault,
                    local,
                    req.get("now_ms").and_then(Value::as_i64).unwrap_or(0),
                    req.get("notice_interval_ms")
                        .and_then(Value::as_i64)
                        .or_else(|| {
                            std::env::var("MEMORY_FABRIC_SCRUB_NOTICE_MS")
                                .ok()
                                .map(|value| parse_scrub_notice_interval(Some(&value)))
                        })
                        .unwrap_or(DEFAULT_SCRUB_NOTICE_INTERVAL_MS),
                )
            }
        }
        "session-cost" => {
            let threshold = req
                .get("threshold")
                .and_then(Value::as_u64)
                .unwrap_or(HANDOFF_THRESHOLD_TOKENS);
            let interval = req
                .get("interval")
                .and_then(Value::as_u64)
                .unwrap_or(REMINDER_INTERVAL_TURNS);
            let mut result = record_session_turn(session, &req["usage"], threshold, interval);
            result["notice"] = if result["advise"].as_bool() == Some(true) {
                json!(cost_notice(result["peakTokens"].as_u64().unwrap_or(0)))
            } else {
                Value::Null
            };
            result
        }
        "resume-pointer" => {
            json!({"pointer":resume_pointer(&req["handoff"],req.get("now_ms").and_then(Value::as_i64).unwrap_or(0)),"resumable":is_resumable(&req["handoff"],req.get("now_ms").and_then(Value::as_i64).unwrap_or(0))})
        }
        "engine-health" => health_operation(session, req),
        "injection-account" => match req.get("action").and_then(Value::as_str).unwrap_or("state") {
            "record" => record_injection_account(
                session,
                req.get("chars").and_then(Value::as_u64).unwrap_or(0),
                req.get("refs").and_then(Value::as_u64).unwrap_or(0),
            ),
            _ => injection_account_state(session),
        },
        "write-policy" => {
            let decision = provider_decision(
                &req.get("model").cloned().unwrap_or(Value::Null),
                local,
                &remote_providers(),
                remote_full_access_enabled(),
                remote_context_scrubbing_enabled(),
            );
            json!({"allowed":decision.allowed,"may_write":decision.may_write,"write_mode":if decision.may_write{"write"}else if decision.allowed{"propose_only"}else{"denied"}})
        }
        "scrub-text" => {
            json!({"text":crate::policy::sanitize_remote_excerpt(req.get("text").and_then(Value::as_str).unwrap_or(""))})
        }
        _ => return None,
    };
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sets(local: &[&str], remote: &[&str]) -> (HashSet<String>, HashSet<String>) {
        (
            local.iter().map(|x| x.to_string()).collect(),
            remote.iter().map(|x| x.to_string()).collect(),
        )
    }

    #[test]
    fn gate_1_loopback_and_allowlisted_remote_are_distinct() {
        let (local, remote) = sets(&["ollama"], &["openai"]);
        let loopback = provider_decision(
            &json!({"provider":"Ollama","base_url":"http://127.0.0.1:11434"}),
            &local,
            &remote,
            false,
            true,
        );
        let remote_ok = provider_decision(
            &json!({"provider":"OPENAI","base_url":"https://api.openai.com/v1"}),
            &local,
            &remote,
            false,
            true,
        );
        let unknown = provider_decision(
            &json!({"provider":"unknown","base_url":"https://example.test"}),
            &local,
            &remote,
            true,
            true,
        );
        assert!(
            loopback.allowed && loopback.loopback && loopback.may_write && !loopback.scrub_context
        );
        assert!(remote_ok.allowed && !remote_ok.loopback && remote_ok.remote_allowlisted);
        assert!(!unknown.allowed && !unknown.may_write);
    }

    #[test]
    fn allowlisted_remote_context_is_scrubbed_by_default_and_can_be_explicitly_opted_out() {
        let (local, remote) = sets(&["ollama"], &["openai"]);
        let model = json!({"provider":"openai","base_url":"https://api.openai.com"});
        assert!(provider_decision(&model, &local, &remote, false, true).scrub_context);
        assert!(!provider_decision(&model, &local, &remote, false, false).scrub_context);
    }

    #[test]
    fn allowlisted_remote_provider_is_propose_only_by_default() {
        let (local, remote) = sets(&["ollama"], &["openai"]);
        let model = json!({"provider":"openai","base_url":"https://api.openai.com"});
        let safe = provider_decision(&model, &local, &remote, false, true);
        let opted_in = provider_decision(&model, &local, &remote, true, true);
        assert!(safe.allowed && !safe.may_write);
        assert!(opted_in.may_write);
    }

    #[test]
    fn local_endpoint_must_be_http_loopback_and_listed() {
        let (local, remote) = sets(&["ollama"], &[]);
        for url in [
            "http://localhost:1",
            "https://127.0.0.1:2",
            "http://[::1]:3",
        ] {
            assert!(
                provider_decision(
                    &json!({"provider":"ollama","base_url":url}),
                    &local,
                    &remote,
                    false,
                    true
                )
                .loopback
            );
        }
        for url in ["http://192.168.1.1:1", "ftp://localhost", "broken"] {
            assert!(
                !provider_decision(
                    &json!({"provider":"ollama","base_url":url}),
                    &local,
                    &remote,
                    false,
                    true
                )
                .allowed
            );
        }
    }

    #[test]
    fn content_words_drop_generic_and_short_terms_and_deduplicate() {
        assert_eq!(
            content_words("Fix the fix; retrieval RETRIEVAL AST v2"),
            vec!["retrieval"]
        );
    }

    #[test]
    fn injection_decision_abstains_on_generic_and_colliding_prompts() {
        let empty = json!({"results":[{"matched_terms":["found"]}]});
        assert_eq!(
            decide_injection("fix all the issues that you found", &empty)["inject"],
            false
        );
        let collision = json!({"results":[{"matched_terms":["retrieval"]}]});
        assert_eq!(
            decide_injection("temporal supersession provenance determinism", &collision)["inject"],
            false
        );
    }

    #[test]
    fn injection_decision_requires_two_matches_and_half_prompt_coverage() {
        let good = json!({"results":[{"matched_terms":["temporal","supersession"]}]});
        assert_eq!(
            decide_injection("temporal supersession", &good)["inject"],
            true
        );
        let low = json!({"results":[{"matched_terms":["temporal","supersession"]}]});
        let verdict = decide_injection(
            "temporal supersession provenance determinism calibration",
            &low,
        );
        assert_eq!(verdict["matched"], 2);
        assert_eq!(verdict["coverage"], 0.4);
        assert_eq!(verdict["inject"], false);
    }

    #[test]
    fn injection_decision_honours_engine_abstention_and_low_discrimination() {
        assert_eq!(
            decide_injection(
                "temporal supersession",
                &json!({"results":[{}],"abstention":"no match"})
            )["reason"],
            "search abstained"
        );
        assert_eq!(
            decide_injection(
                "temporal supersession",
                &json!({"results":[{}],"low_discrimination":true})
            )["reason"],
            "search flagged low discrimination"
        );
    }

    #[test]
    fn abstention_preserves_engine_reason_and_flags_non_discriminative_query() {
        assert_eq!(
            decide_abstention("anything", &json!({"abstention":"  nothing indexed  "}))["reason"],
            "nothing indexed"
        );
        assert_eq!(
            decide_abstention(
                "fix all the issues that you found",
                &json!({"results":[{"matched_terms":["found"]}]})
            )["abstained"],
            true
        );
        assert_eq!(
            decide_abstention("how do we fix this", &json!({}))["abstained"],
            true
        );
    }

    #[test]
    fn abstention_keeps_discriminative_answers_and_honours_low_discrimination() {
        assert_eq!(
            decide_abstention(
                "temporal supersession",
                &json!({"results":[{"matched_terms":["temporal"]}]})
            )["abstained"],
            false
        );
        assert_eq!(
            decide_abstention("temporal supersession", &json!({"low_discrimination":true}))["abstained"],
            true
        );
    }

    #[test]
    fn filename_abstention_requires_excerpt_evidence_and_literal_match() {
        let evidence =
            json!({"results":[{"matched_terms":["t3","json"],"excerpt":"T3 Code configuration"}]});
        assert_eq!(decide_abstention("t3.json", &evidence)["abstained"], true);
        let exact = json!({"results":[{"excerpt":"The t3.json file is here"}]});
        assert_eq!(decide_abstention("t3.json", &exact)["abstained"], false);
        assert_eq!(
            decide_abstention(
                "t3.json",
                &json!({"results":[{"matched_terms":["t3","json"]}]})
            )["abstained"],
            false
        );
        let natural = json!({"results":[{"matched_terms":["ranking","work"],"excerpt":"ranking work notes"}]});
        assert_eq!(
            decide_abstention("how does ranking work", &natural)["abstained"],
            false
        );
    }

    #[test]
    fn with_abstention_adds_signal_without_overwriting_existing_reason() {
        let out = with_abstention(
            "temporal supersession",
            &json!({"results":[{"matched_terms":["temporal"]}],"abstention":"source reason"}),
        );
        assert_eq!(out["abstained"], true);
        assert_eq!(out["abstention"], "source reason");
    }

    #[test]
    fn project_key_and_note_tier_rules_are_exact() {
        assert_eq!(
            project_key(Some(" /tmp/repo/// ")),
            Some("/tmp/repo".into())
        );
        assert_eq!(project_key(Some("/")), None);
        for (path, tier) in [
            ("user/prefs.md", "global"),
            ("invariants/x.md", "global"),
            ("pi-harness/a.md", "global"),
            ("projects/repo.md", "project"),
            ("daily/x.md", "episodic"),
        ] {
            assert_eq!(tier_of(path), tier);
        }
    }

    #[test]
    fn t3_adapter_project_slug_and_system_instruction_are_deterministic() {
        assert_eq!(
            workspace_project("/Users/agent/projects/My Cool_App"),
            "my-cool-app"
        );
        assert_eq!(workspace_project("/"), "workspace");
        let instruction = t3_system_instruction("my-cool-app");
        assert!(instruction.contains("[PROJECT: my-cool-app]"));
        assert!(instruction.contains("LOCAL-MEMORY-FABRIC-T3-CODE"));
        assert!(instruction.contains("memory_fabric_propose"));
    }

    #[test]
    fn project_note_matches_only_distinctive_slug_prefixes() {
        assert!(project_note_matches(
            "projects/local-memory-fabric-extension.md",
            Some("/work/local-memory-fabric")
        ));
        assert!(project_note_matches(
            "projects/local-memory.md",
            Some("/work/local-memory-fabric")
        ));
        assert!(!project_note_matches(
            "projects/fikr.md",
            Some("/work/local-memory-fabric")
        ));
        assert!(!project_note_matches("projects/fikr.md", None));
        assert!(!project_note_matches("projects/abc.md", Some("/work/abc")));
    }

    #[test]
    fn handoff_provenance_fails_closed_and_is_project_scoped() {
        assert!(may_inject_handoff(
            &json!({"cwd":"/work/a"}),
            Some("/work/a/")
        ));
        assert!(!may_inject_handoff(
            &json!({"cwd":"/work/a"}),
            Some("/work/b")
        ));
        assert!(!may_inject_handoff(&json!({}), Some("/work/a")));
        assert!(!may_inject_handoff(&json!({"cwd":"/work/a"}), None));
    }

    #[test]
    fn cost_counts_billed_and_cached_tokens_and_junk_as_zero() {
        assert_eq!(
            prompt_tokens(&json!({"input":100,"cacheRead":200,"cacheWrite":30})),
            330
        );
        assert_eq!(prompt_tokens(&json!({"input":"junk","cacheRead":null})), 0);
    }

    #[test]
    fn cost_notice_fires_on_crossing_then_only_after_interval_and_tracks_peak() {
        let mut state = CostState::default();
        assert_eq!(
            record_turn(&mut state, &json!({"input":10}), 100, 3)["advise"],
            false
        );
        assert_eq!(
            record_turn(&mut state, &json!({"input":100}), 100, 3)["reason"],
            "crossed"
        );
        assert_eq!(
            record_turn(&mut state, &json!({"input":120}), 100, 3)["advise"],
            false
        );
        assert_eq!(
            record_turn(&mut state, &json!({"input":130}), 100, 3)["advise"],
            false
        );
        assert_eq!(
            record_turn(&mut state, &json!({"input":110}), 100, 3)["reason"],
            "interval"
        );
        assert_eq!(state.peak_tokens, 130);
    }

    #[test]
    fn resume_pointer_is_bounded_to_work_and_transcript_pointer() {
        let handoff = json!({"at":"2026-10-24T00:00:00Z","done":"index","next":"run gate","transcript":"/sessions/t.jsonl"});
        let pointer = resume_pointer(&handoff, 1_792_800_000_000).unwrap();
        assert!(pointer.contains("done: index"));
        assert!(pointer.contains("next: run gate"));
        assert!(pointer.contains("/sessions/t.jsonl"));
        assert!(!pointer.contains("summary"));
        assert!(resume_pointer(&json!({}), 0).is_none());
    }

    #[test]
    fn injection_account_tracks_baseline_deviations_and_session_cost() {
        let mut account = InjectionAccount::default();
        assert_eq!(
            account.state(),
            json!({"turns":0,"sessionChars":0,"baselineChars":null,"prefixBreaks":0})
        );
        assert_eq!(account.record(412, 3)["prefixBreak"], false);
        for _ in 0..19 {
            assert_eq!(account.record(412, 3)["prefixBreak"], false);
        }
        assert_eq!(account.record(430, 3)["prefixBreak"], true);
        assert_eq!(account.record(412, 3)["prefixBreak"], false);
        assert_eq!(account.record(413, 3)["prefixBreak"], true);
        assert_eq!(account.state()["turns"], 23);
        assert_eq!(account.state()["sessionChars"], 412 * 20 + 430 + 412 + 413);
        assert_eq!(account.state()["baselineChars"], 412);
        assert_eq!(account.state()["prefixBreaks"], 2);
    }

    #[test]
    fn stale_and_future_handoffs_are_not_resumable() {
        assert!(!is_resumable(
            &json!({"at":"2020-01-01T00:00:00Z"}),
            1_792_800_000_000
        ));
        assert!(!is_resumable(
            &json!({"at":"2030-01-01T00:00:00Z"}),
            1_792_800_000_000
        ));
        assert!(is_resumable(
            &json!({"at":"2026-10-23T00:00:00Z"}),
            1_792_800_000_000
        ));
    }

    #[test]
    fn health_breaker_counts_failures_refuses_and_allows_one_probe_after_window() {
        let mut h = HealthState::default();
        h.record(false, 10, 3);
        h.record(false, 11, 3);
        h.record(false, 12, 3);
        assert_eq!(h.breaker_opened_at, Some(12));
        assert!(!h.allow(20, 100));
        assert_eq!(h.refusals, 1);
        assert!(h.allow(112, 100));
        assert_eq!(h.consecutive_failures, 0);
    }

    #[test]
    fn health_success_resets_breaker_and_respawns_are_window_bounded() {
        let mut h = HealthState::default();
        h.record(false, 1, 1);
        h.record(true, 2, 1);
        assert!(h.breaker_opened_at.is_none());
        assert!(h.allow_respawn(0, 2, 100));
        assert!(h.allow_respawn(5, 2, 100));
        assert!(!h.allow_respawn(10, 2, 100));
        assert_eq!(h.respawn_refusals, 1);
        assert!(h.allow_respawn(101, 2, 100));
    }

    #[test]
    fn scrub_context_withholds_local_turns_vault_results_and_vault_context() {
        let result = scrub_context(
            "scrub-test-1",
            &json!([
                {"role":"assistant","provider":"ollama","content":[{"type":"toolCall","id":"id1","name":"read","arguments":{"path":"/vault/STATUS.md"}}]},
                {"role":"toolResult","toolName":"read","toolCallId":"id1","content":"secret memory"},
                {"role":"assistant","customType":"vault-context","content":"context"},
                {"role":"user","content":"hello"}
            ]),
            "/vault",
            &HashSet::from(["ollama".to_string()]),
            1,
            600_000,
        );
        assert_eq!(result["messages"].as_array().unwrap().len(), 1);
        assert_eq!(result["messages"][0]["content"], "hello");
        assert_eq!(result["notice"], true);
    }

    #[test]
    fn scrub_notice_cadence_is_first_interval_zero_or_explicit_repeat() {
        let messages =
            json!([{"role":"assistant","customType":"vault-context","content":"private"}]);
        assert_eq!(
            scrub_context(
                "notice-interval",
                &messages,
                "/vault",
                &HashSet::new(),
                100,
                60
            )["notice"],
            true
        );
        assert_eq!(
            scrub_context(
                "notice-interval",
                &messages,
                "/vault",
                &HashSet::new(),
                120,
                60
            )["notice"],
            false
        );
        assert_eq!(
            scrub_context(
                "notice-interval",
                &messages,
                "/vault",
                &HashSet::new(),
                160,
                60
            )["notice"],
            true
        );
        assert_eq!(
            scrub_context("notice-zero", &messages, "/vault", &HashSet::new(), 100, 0)["notice"],
            true
        );
        assert_eq!(
            scrub_context("notice-zero", &messages, "/vault", &HashSet::new(), 200, 0)["notice"],
            false
        );
        assert_eq!(
            scrub_context(
                "notice-repeat",
                &messages,
                "/vault",
                &HashSet::new(),
                100,
                -1
            )["notice"],
            true
        );
        assert_eq!(
            scrub_context(
                "notice-repeat",
                &messages,
                "/vault",
                &HashSet::new(),
                101,
                -1
            )["notice"],
            true
        );
    }

    #[test]
    fn scrub_notice_interval_parser_accepts_signed_integers_and_rejects_junk() {
        assert_eq!(
            parse_scrub_notice_interval(None),
            DEFAULT_SCRUB_NOTICE_INTERVAL_MS
        );
        assert_eq!(parse_scrub_notice_interval(Some(" 60000 ")), 60_000);
        assert_eq!(parse_scrub_notice_interval(Some("0")), 0);
        assert_eq!(parse_scrub_notice_interval(Some("-1")), -1);
        for junk in ["", "  ", "abc", "10s", "1.5", "1e3", "NaN", "+"] {
            assert_eq!(
                parse_scrub_notice_interval(Some(junk)),
                DEFAULT_SCRUB_NOTICE_INTERVAL_MS,
                "{junk}"
            );
        }
    }

    #[test]
    fn recorded_loopback_fingerprint_survives_provider_relabeling() {
        let message = json!({"role":"assistant","provider":"ollama","content":"loopback turn"});
        remember_loopback_turn("fingerprint-relabel", &message);
        let relabeled =
            json!({"role":"assistant","provider":"anthropic","content":"loopback turn"});
        let result = scrub_context(
            "fingerprint-relabel",
            &json!([relabeled]),
            "/vault",
            &HashSet::from(["ollama".to_string()]),
            1,
            600_000,
        );
        assert!(result["messages"].as_array().unwrap().is_empty());
    }

    #[test]
    fn scrub_filters_fabric_and_apple_notes_results_but_keeps_unrelated_tool_output() {
        let messages = json!([
            {"role":"assistant","provider":"ollama","content":[
                {"type":"toolCall","id":"fabric","name":"memory_fabric_search","arguments":{}},
                {"type":"toolCall","id":"apple","name":"bash","arguments":{"command":"cat ~/Library/Group Containers/com.apple.notes/notes.sqlite"}},
                {"type":"toolCall","id":"other","name":"bash","arguments":{"command":"ls"}}
            ]},
            {"role":"toolResult","toolName":"memory_fabric_search","toolCallId":"fabric","content":"excerpts"},
            {"role":"toolResult","toolName":"bash","toolCallId":"apple","content":"private notes"},
            {"role":"toolResult","toolName":"bash","toolCallId":"other","content":"directory list"}
        ]);
        let result = scrub_context(
            "scrub-apple-and-fabric",
            &messages,
            "/vault",
            &HashSet::from(["ollama".to_string()]),
            1,
            600_000,
        );
        let kept = result["messages"].as_array().unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["content"], "directory list");
    }

    #[test]
    fn scrub_tracking_overflow_fails_closed_without_re_admitting_evicted_ids() {
        let mut calls = Vec::new();
        for i in 0..=MAX_TRACKED_TOOL_IDS {
            calls.push(json!({"role":"assistant","provider":"ollama","content":[{"type":"toolCall","id":format!("id{i}"),"name":"read","arguments":{"path":"/vault/x"}}]}));
        }
        calls.push(json!({"role":"toolResult","toolName":"unrelated","toolCallId":"unknown","content":"unknown"}));
        let result = scrub_context(
            "scrub-overflow",
            &Value::Array(calls),
            "/vault",
            &HashSet::from(["ollama".to_string()]),
            1,
            600_000,
        );
        assert_eq!(result["overflowed"], true);
        assert_eq!(result["messages"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn outbound_secret_scrubbing_replaces_credentials() {
        let text = crate::policy::sanitize_remote_excerpt(
            "api_key=supersecret123456 and AKIA1234567890ABCDEF",
        );
        assert!(!text.contains("supersecret123456"));
        assert!(!text.contains("AKIA1234567890ABCDEF"));
        assert!(text.contains("REDACTED"));
    }

    #[test]
    fn remote_allowlist_requires_http_endpoint_and_empty_list_allows_nobody() {
        let (local, remote) = sets(&["ollama"], &["openai"]);
        let http_remote = provider_decision(
            &json!({"provider":"openai","base_url":"http://api.example.test/v1"}),
            &local,
            &remote,
            false,
            true,
        );
        assert!(http_remote.allowed);
        assert!(!http_remote.loopback);
        assert!(http_remote.scrub_context);
        assert!(!http_remote.may_write);

        let unlisted_local_alias = provider_decision(
            &json!({"provider":"ollama","base_url":"http://localhost.localdomain:11434"}),
            &local,
            &remote,
            true,
            true,
        );
        assert!(!unlisted_local_alias.allowed);
        assert!(!unlisted_local_alias.may_write);

        let (local, empty_remote) = sets(&["ollama"], &[]);
        for model in [
            json!({"provider":"openai","base_url":"https://api.openai.com/v1"}),
            json!({"provider":"ollama","base_url":"http://192.168.1.10:11434"}),
        ] {
            let denied = provider_decision(&model, &local, &empty_remote, true, true);
            assert!(!denied.allowed, "an empty allowlist must deny {model}");
            assert!(!denied.may_write);
        }
    }

    #[test]
    fn scrub_leaves_unrelated_transcripts_untouched_and_hashing_is_deterministic() {
        let messages = json!([
            {"role":"user","content":"ordinary conversation"},
            {"role":"toolResult","toolName":"bash","toolCallId":"untracked","content":"directory listing"}
        ]);
        let result = scrub_context(
            "scrub-ordinary-transcript",
            &messages,
            "/vault",
            &HashSet::from(["ollama".to_string()]),
            10,
            600_000,
        );
        assert_eq!(result["removed_anything"], false);
        assert_eq!(result["notice"], false);
        assert_eq!(result["messages"], messages);

        let json_message =
            json!({"role":"assistant","content":[{"type":"text","text":"local note"}]});
        assert_eq!(
            message_fingerprint(&json_message),
            message_fingerprint(&json_message),
            "all bridge messages are JSON values, so fingerprinting must be deterministic"
        );
    }

    #[test]
    fn session_cost_notice_names_token_count_and_action_and_pointer_stays_bounded() {
        let notice = cost_notice(200_000);
        assert!(notice.contains("200K tokens"));
        assert!(notice.contains("memory_fabric_handoff"));

        let pointer = resume_pointer(
            &json!({
                "at":"2026-10-23T23:00:00Z",
                "done":"settled the retry policy",
                "in_flight":"the migration test",
                "next":"run the gate",
                "transcript":"/Users/agent/sessions/session.jsonl"
            }),
            1_792_800_000_000,
        )
        .unwrap();
        assert!(pointer.len() < 400, "pointer was {} bytes", pointer.len());
        assert!(pointer.contains("in-flight: the migration test"));
        assert!(pointer.contains("transcript: /Users/agent/sessions/session.jsonl"));

        let without_transcript = resume_pointer(
            &json!({"at":"2026-10-24T00:00:00Z","next":"run the gate"}),
            1_792_800_000_000,
        )
        .unwrap();
        assert!(!without_transcript.contains("transcript:"));
        assert!(without_transcript.contains("inbox/RESUME.md"));
    }

    #[test]
    fn default_session_cost_threshold_and_empty_or_malformed_handoffs_are_safe() {
        let mut state = CostState::default();
        for _ in 0..50 {
            assert_eq!(
                record_turn(
                    &mut state,
                    &json!({"input":10_000,"cacheRead":20_000}),
                    HANDOFF_THRESHOLD_TOKENS,
                    REMINDER_INTERVAL_TURNS,
                )["advise"],
                false
            );
        }
        assert_eq!(state.peak_tokens, 30_000);
        assert_eq!(state.crossed_at_turn, 0);
        for junk in [
            Value::Null,
            json!("x"),
            json!(5),
            json!({}),
            json!({"input":"a"}),
            json!({"input":-10,"cacheRead":null}),
        ] {
            assert_eq!(prompt_tokens(&junk), 0, "junk usage was counted: {junk}");
        }
        assert!(resume_pointer(&json!({"at":"2026-10-24T00:00:00Z"}), 1_792_800_000_000).is_none());
        assert!(
            resume_pointer(
                &json!({"at":"2026-10-24T00:00:00Z","done":"   "}),
                1_792_800_000_000
            )
            .is_none()
        );
        for handoff in [json!({}), json!({"at":"not a date"})] {
            assert!(!is_resumable(&handoff, 1_792_800_000_000));
        }
    }

    #[test]
    fn health_decision_table_and_refusal_guidance_are_stable() {
        let mut healthy = HealthState::default();
        for now in 0..50 {
            healthy.record(true, now, 3);
        }
        assert!(healthy.allow(100, 60_000));
        assert_eq!(healthy.refusals, 0);

        let mut state = HealthState::default();
        state.record(false, 0, 3);
        assert!(state.allow(0, 60_000));
        state.record(false, 1, 3);
        assert!(state.allow(1, 60_000));
        state.record(true, 2, 3);
        state.record(false, 3, 3);
        state.record(false, 4, 3);
        assert!(
            state.allow(5, 60_000),
            "a success resets the consecutive-failure count"
        );

        state.record(true, 6, 3);
        for now in 10..13 {
            state.record(false, now, 3);
        }
        assert!(!state.allow(20, 60_000));
        assert!(!state.allow(10_000, 60_000));
        assert!(
            !state.allow(60_011, 60_000),
            "the probe is refused one millisecond early"
        );
        let message = state.refusal_message(10_000, 60_000);
        assert!(message.contains("failed 3 times in a row"));
        assert!(message.contains("retrying in 51s"));
        assert!(message.contains("/memory-fabric status"));
        assert!(state.allow(60_012, 60_000));
        assert_eq!(state.consecutive_failures, 0);

        state.record(false, 60_013, 3);
        state.record(false, 60_014, 3);
        assert!(state.allow(60_014, 60_000));
        state.record(false, 60_015, 3);
        assert!(
            !state.allow(60_015, 60_000),
            "three fresh failures must trip again"
        );
        assert_eq!(state.refusals, 4);

        let mut respawns = HealthState::default();
        for now in [0, 1_000, 2_000] {
            assert!(respawns.allow_respawn(now, 3, 60_000));
        }
        assert!(!respawns.allow_respawn(3_000, 3, 60_000));
        assert_eq!(respawns.respawn_refusals, 1);
        assert!(respawns.allow_respawn(61_000, 3, 60_000));
    }

    #[test]
    fn abstention_keeps_missing_evidence_honest_and_search_bar_looser_than_injection() {
        let query = "section ranking behaviour";
        for evidence in [
            json!({}),
            json!({"results": []}),
            json!({"results": null}),
            json!({"results": [{"matched_terms": []}]}),
        ] {
            assert_eq!(
                decide_abstention(query, &evidence)["abstained"],
                false,
                "missing evidence is not evidence of an abstention: {evidence}"
            );
        }

        let thin = json!({"results":[{"matched_terms":["ranking"]}]});
        assert_eq!(
            decide_abstention("ranking precision recall", &thin)["abstained"],
            false
        );
        assert_eq!(
            decide_injection("ranking precision recall", &thin)["inject"],
            false
        );
        let collision = json!({"results":[{"matched_terms":["unrelated"]}]});
        assert_eq!(decide_abstention(query, &collision)["abstained"], true);

        let ordinary = with_abstention(
            query,
            &json!({"ok":true,"results":[{"matched_terms":["ranking"]}]}),
        );
        assert_eq!(ordinary["abstained"], false);
        assert_eq!(ordinary["abstention"], Value::Null);
        assert_eq!(ordinary["ok"], true);
    }

    #[test]
    fn injection_rejects_empty_evidence_and_filename_shape_is_narrow() {
        for evidence in [json!({}), json!({"results": []}), json!({"results": null})] {
            assert_eq!(
                decide_injection("section ranking behaviour", &evidence)["inject"],
                false,
                "an absent result cannot be injected: {evidence}"
            );
        }
        for query in ["t3.json", "package.json", "config.toml", "agent.rs"] {
            assert!(filename_shaped(query), "{query} should be filename-shaped");
        }
        for query in [
            "how does ranking work",
            "format grep files",
            "fix all the issues",
        ] {
            assert!(!filename_shaped(query), "{query} is ordinary language");
        }
    }

    #[test]
    fn scope_keeps_cross_cutting_notes_global_and_unclassified_notes_episodic() {
        for path in [
            "user/preferences.md",
            "invariants/tooling.md",
            "pi-harness/runtime.md",
            "company/team.md",
            "agents/rules.md",
            "evidence/experiment.md",
        ] {
            assert_eq!(tier_of(path), "global", "{path} should be global");
        }
        assert_eq!(tier_of("projects/local-memory-fabric.md"), "project");
        assert_eq!(tier_of("daily/2026-10-24.md"), "episodic");
        assert_eq!(tier_of("archive/old.md"), "episodic");
    }
}
