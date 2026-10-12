mod db;
mod index;
mod paths;
mod policy;
mod propose;
mod retrieval;
mod text;

#[cfg(test)]
mod tests;

use crate::db::Config;
use crate::paths::MAX_REQUEST_BYTES;
use crate::text::json_error;
use serde_json::Value;
use std::env;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;

pub(crate) struct MemoryFabric {
    pub(crate) home: PathBuf,
    pub(crate) vault: PathBuf,
    pub(crate) db_path: PathBuf,
    pub(crate) proposals_path: PathBuf,
    pub(crate) config: Config,
}

fn dispatch(fabric: &MemoryFabric, req: &Value) -> Result<Value, String> {
    match req["op"].as_str().unwrap_or("") {
        "status" => fabric.status(),
        "index" => fabric.index(req["force"].as_bool().unwrap_or(false)),
        "search" => fabric.search(
            req["query"].as_str().unwrap_or(""),
            req["scope"].as_str().unwrap_or("all"),
            req["limit"].as_u64().unwrap_or(5) as usize,
            req["provider"].as_str(),
            req["as_of"].as_str(),
            req["loopback"].as_bool(),
        ),
        "context" => fabric.context(
            req["project"].as_str(),
            req["query"].as_str().unwrap_or(""),
            req["provider"].as_str(),
            req["token_budget"].as_u64().unwrap_or(1200) as usize,
            req["as_of"].as_str(),
            req["loopback"].as_bool(),
        ),
        "read" => fabric.read(
            req["source_ref"].as_str().unwrap_or(""),
            req["provider"].as_str(),
            req["heading"].as_str(),
            req["loopback"].as_bool(),
        ),
        "propose" => fabric.propose(req),
        "sandbox" => fabric.sandbox(
            req["title"].as_str().unwrap_or(""),
            req["text"].as_str().unwrap_or(""),
            req["scope"].as_str().unwrap_or("episodic"),
            req["provider"].as_str(),
            req["loopback"].as_bool(),
        ),
        "proposal-show" => fabric.proposal_show(
            req["proposal_id"].as_str().unwrap_or(""),
            req["provider"].as_str(),
        ),
        "proposal-list" => fabric.proposal_list(),
        "apply" => fabric.proposal_apply(
            req["proposal_id"].as_str().unwrap_or(""),
            req["provider"].as_str(),
        ),
        "alarms" => fabric.alarms(),
        "resolve" => fabric.resolve_alarm(req["alarm_id"].as_str().unwrap_or("")),
        _ => Ok(json_error("unsupported memory operation")),
    }
}

fn serve() -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let mut fabric: Option<MemoryFabric> = None;
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.len() > MAX_REQUEST_BYTES {
            writeln!(
                stdout,
                "{{\"ok\":false,\"error\":\"request exceeds size limit\"}}"
            )
            .map_err(|e| e.to_string())?;
            stdout.flush().map_err(|e| e.to_string())?;
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Err(_) => json_error("request must be valid JSON"),
            Ok(req) => {
                if !req.is_object() {
                    json_error("request must be a JSON object")
                } else {
                if fabric.is_none() {
                    let outcome = MemoryFabric::new();
                    match outcome {
                        Ok(f) => fabric = Some(f),
                        // Surface the real cause instead of a bare "could not initialize":
                        // that message hid a one-character config-writing bug for weeks.
                        Err(e) => {
                            let response = json_error(&e);
                            serde_json::to_writer(&mut stdout, &response)
                                .map_err(|e| e.to_string())?;
                            stdout.write_all(b"\n").map_err(|e| e.to_string())?;
                            stdout.flush().map_err(|e| e.to_string())?;
                            continue;
                        }
                    }
                }
                    match fabric.as_ref() {
                        Some(f) => dispatch(f, &req)
                            .unwrap_or_else(|e| json_error(&crate::db::busy_message(&e, "dispatch"))),
                        None => json_error("Memory Fabric could not initialize"),
                    }
                }
            }
        };
        serde_json::to_writer(&mut stdout, &response).map_err(|e| e.to_string())?;
        stdout.write_all(b"\n").map_err(|e| e.to_string())?;
        stdout.flush().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Exit-code taxonomy for one-shot use, so a caller can branch without parsing prose.
///
/// 0  succeeded and returned content
/// 1  succeeded, but there is nothing to return (no match, or an abstention)
/// 2  refused by policy (the response carries `blocked`)
/// 3  error: invalid request, engine failure, or a rejected proposal
///
/// The engine never guesses between these: an empty result is 1, not an error, and
/// an abstention is never reported as a success. `--serve` does not use this, because
/// a long-running protocol cannot exit on a per-request outcome.
fn exit_code_for(response: &Value) -> i32 {
    if response.get("blocked").is_some() {
        return 2;
    }
    if !response.get("ok").and_then(Value::as_bool).unwrap_or(false) {
        return 3;
    }
    let content_bearing = response.get("results").is_some()
        || response.get("abstention").is_some()
        || response.get("content").is_some();
    if content_bearing {
        let has_results = response
            .get("results")
            .and_then(Value::as_array)
            .map(|items| !items.is_empty())
            .unwrap_or(false);
        let has_content = response
            .get("content")
            .and_then(Value::as_str)
            .map(|text| !text.is_empty())
            .unwrap_or(false);
        if !has_results && !has_content {
            return 1;
        }
    }
    0
}

fn one_shot(quiet: bool) -> Result<i32, String> {
    let mut input = String::new();
    io::stdin()
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_string(&mut input)
        .map_err(|e| e.to_string())?;
    let response = if input.len() > MAX_REQUEST_BYTES {
        json_error("request exceeds size limit")
    } else {
        match serde_json::from_str::<Value>(&input) {
            Err(_) => json_error("request must be valid JSON"),
            Ok(req) if !req.is_object() => json_error("request must be a JSON object"),
            Ok(req) => MemoryFabric::new()
                .and_then(|f| dispatch(&f, &req))
                .unwrap_or_else(|e| json_error(&crate::db::busy_message(&e, "dispatch"))),
        }
    };
    if !quiet {
        println!("{}", response);
    }
    Ok(exit_code_for(&response))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = if args.iter().any(|a| a == "--serve") {
        serve().map(|_| 0)
    } else {
        one_shot(args.iter().any(|a| a == "--quiet"))
    };
    match result {
        Ok(0) => {}
        Ok(code) => std::process::exit(code),
        Err(err) => {
            eprintln!("memory-fabric: {err}");
            std::process::exit(3);
        }
    }
}
