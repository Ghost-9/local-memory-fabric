mod db;
mod index;
mod paths;
mod policy;
mod propose;
mod retrieval;
mod text;
mod shm;

#[cfg(test)]
mod tests;

use crate::db::Config;
use crate::paths::MAX_REQUEST_BYTES;
use crate::text::json_error;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};

static CACHE_GEN: AtomicU64 = AtomicU64::new(1);
static QUERY_CACHE: OnceLock<Mutex<HashMap<String, (u64, Value)>>> = OnceLock::new();

fn invalidate_cache() {
    CACHE_GEN.fetch_add(1, Ordering::Release);
}

pub(crate) struct MemoryFabric {
    pub(crate) home: PathBuf,
    pub(crate) vault: PathBuf,
    pub(crate) db_path: PathBuf,
    pub(crate) proposals_path: PathBuf,
    pub(crate) config: Config,
}

pub(crate) fn dispatch(fabric: &MemoryFabric, req: &Value) -> Result<Value, String> {
    let op = req["op"].as_str().unwrap_or("");
    if ["index", "propose", "apply", "sandbox", "resolve", "proposal-revert"].contains(&op) {
        invalidate_cache();
    }
    match op {
        "status" => fabric.status(),
        "index" => fabric.index(req["force"].as_bool().unwrap_or(false)),
        "search" => {
            let key = format!(
                "search:{}:{}:{}:{}:{}:{}:{}",
                req["query"].as_str().unwrap_or(""),
                req["scope"].as_str().unwrap_or("all"),
                req["limit"].as_u64().unwrap_or(5),
                req["provider"].as_str().unwrap_or(""),
                req["as_of"].as_str().unwrap_or(""),
                req["loopback"].as_bool().unwrap_or(false),
                req["token_budget"].as_u64().unwrap_or(0),
            );
            let cur_gen = CACHE_GEN.load(Ordering::Acquire);
            if let Some(cache_map) = QUERY_CACHE.get() {
                if let Ok(guard) = cache_map.lock() {
                    if let Some((cached_gen, val)) = guard.get(&key) {
                        if *cached_gen == cur_gen {
                            return Ok(val.clone());
                        }
                    }
                }
            }
            let res = fabric.search(
                req["query"].as_str().unwrap_or(""),
                req["scope"].as_str().unwrap_or("all"),
                req["limit"].as_u64().unwrap_or(5) as usize,
                req["provider"].as_str(),
                req["as_of"].as_str(),
                req["loopback"].as_bool(),
                req["token_budget"].as_u64().map(|b| b as usize),
            )?;
            let cache_map = QUERY_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
            if let Ok(mut guard) = cache_map.lock() {
                if guard.len() >= 1024 {
                    guard.clear();
                }
                guard.insert(key, (cur_gen, res.clone()));
            }
            Ok(res)
        }
        "audit" => fabric.audit(),
        "links" => fabric.links(req["source_ref"].as_str().unwrap_or("")),
        "sections" => fabric.sections(
            req["source_ref"].as_str().or_else(|| req["target"].as_str()).unwrap_or(""),
            req["provider"].as_str(),
            req["loopback"].as_bool(),
        ),
        "proposal-history" => fabric.proposal_history(),
        "proposal-revert" => fabric.proposal_revert(
            req["proposal_id"].as_str().unwrap_or(""),
            req["provider"].as_str(),
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

fn serve_mcp() -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    let fabric = MemoryFabric::new()?;
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let req: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let id = req.get("id").cloned().unwrap_or(Value::Null);
        let method = req["method"].as_str().unwrap_or("");
        match method {
            "initialize" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": {} },
                        "serverInfo": {
                            "name": "local-memory-fabric",
                            "version": crate::paths::VERSION
                        }
                    }
                });
                serde_json::to_writer(&mut stdout, &resp).map_err(|e| e.to_string())?;
                stdout.write_all(b"\n").map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
            "notifications/initialized" => {}
            "tools/list" => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": [
                            {
                                "name": "memory_fabric_search",
                                "description": "Search local memory index with BM25 lexical retrieval and optional temporal filtering.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "query": { "type": "string" },
                                        "scope": { "type": "string", "default": "all" },
                                        "limit": { "type": "integer", "default": 5 }
                                    },
                                    "required": ["query"]
                                }
                            },
                            {
                                "name": "memory_fabric_context",
                                "description": "Retrieve bounded context packet from local memory.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "query": { "type": "string" },
                                        "project": { "type": "string" },
                                        "token_budget": { "type": "integer", "default": 1200 }
                                    },
                                    "required": ["query"]
                                }
                            },
                            {
                                "name": "memory_fabric_status",
                                "description": "Check memory index status, document count, and alarm counts.",
                                "inputSchema": { "type": "object", "properties": {} }
                            },
                            {
                                "name": "memory_fabric_audit",
                                "description": "Audit memory vault for stale notes, contradictions, duplicates, and broken links.",
                                "inputSchema": { "type": "object", "properties": {} }
                            },
                            {
                                "name": "memory_fabric_sections",
                                "description": "Extract hierarchical Markdown sections with breadcrumbs and line bounds.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {
                                        "source_ref": { "type": "string" }
                                    },
                                    "required": ["source_ref"]
                                }
                            }
                        ]
                    }
                });
                serde_json::to_writer(&mut stdout, &resp).map_err(|e| e.to_string())?;
                stdout.write_all(b"\n").map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
            "tools/call" => {
                let name = req["params"]["name"].as_str().unwrap_or("");
                let args = &req["params"]["arguments"];
                let mut op_req = json!({});
                if name == "memory_fabric_search" {
                    op_req = json!({
                        "op": "search",
                        "query": args["query"].as_str().unwrap_or(""),
                        "scope": args["scope"].as_str().unwrap_or("all"),
                        "limit": args["limit"].as_u64().unwrap_or(5),
                        "provider": "ollama"
                    });
                } else if name == "memory_fabric_context" {
                    op_req = json!({
                        "op": "context",
                        "query": args["query"].as_str().unwrap_or(""),
                        "project": args["project"].as_str(),
                        "token_budget": args["token_budget"].as_u64().unwrap_or(1200),
                        "provider": "ollama"
                    });
                } else if name == "memory_fabric_status" {
                    op_req = json!({ "op": "status" });
                } else if name == "memory_fabric_audit" {
                    op_req = json!({ "op": "audit" });
                } else if name == "memory_fabric_sections" {
                    op_req = json!({
                        "op": "sections",
                        "source_ref": args["source_ref"].as_str().unwrap_or(""),
                        "provider": "ollama"
                    });
                }
                let res = dispatch(&fabric, &op_req).unwrap_or_else(|e| json_error(&e));
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": serde_json::to_string_pretty(&res).unwrap_or_default()
                            }
                        ]
                    }
                });
                serde_json::to_writer(&mut stdout, &resp).map_err(|e| e.to_string())?;
                stdout.write_all(b"\n").map_err(|e| e.to_string())?;
                stdout.flush().map_err(|e| e.to_string())?;
            }
            _ => {
                let resp = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": "Method not found" }
                });
                let _ = serde_json::to_writer(&mut stdout, &resp);
                let _ = stdout.write_all(b"\n");
                let _ = stdout.flush();
            }
        }
    }
    Ok(())
}

fn dirs_or_home_socket_path() -> PathBuf {
    if let Ok(cache) = env::var("XDG_CACHE_HOME") {
        PathBuf::from(cache).join("memory-fabric.sock")
    } else if let Ok(home) = env::var("HOME") {
        PathBuf::from(home).join(".cache").join("memory-fabric.sock")
    } else {
        PathBuf::from("/tmp/memory-fabric.sock")
    }
}

#[cfg(unix)]
fn serve_socket(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::net::UnixListener;
    if path.exists() {
        let _ = std::fs::remove_file(path);
    }
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener = UnixListener::bind(path)
        .map_err(|e| format!("failed to bind UNIX domain socket {}: {}", path.display(), e))?;
    let fabric = MemoryFabric::new()?;
    for stream in listener.incoming() {
        if let Ok(stream) = stream {
            let mut reader = io::BufReader::new(&stream);
            let mut writer = io::BufWriter::new(&stream);
            let mut line = String::new();
            if reader.read_line(&mut line).is_ok() && !line.trim().is_empty() {
                let resp = match serde_json::from_str::<Value>(&line) {
                    Ok(req) => dispatch(&fabric, &req).unwrap_or_else(|e| json_error(&e)),
                    Err(_) => json_error("request must be valid JSON"),
                };
                let _ = serde_json::to_writer(&mut writer, &resp);
                let _ = writer.write_all(b"\n");
                let _ = writer.flush();
            }
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn serve_socket(_path: &std::path::Path) -> Result<(), String> {
    Err("UNIX domain sockets are not supported on this platform".into())
}

fn watch_loop() -> Result<(), String> {
    let fabric = MemoryFabric::new()?;
    let mut last_mtime = None;
    println!("Watching vault {} for changes...", fabric.vault.display());
    loop {
        if let Ok(meta) = std::fs::metadata(&fabric.vault) {
            let cur_mtime = meta.modified().ok();
            if cur_mtime != last_mtime {
                last_mtime = cur_mtime;
                let _ = fabric.index(false);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = if args.iter().any(|a| a == "--mcp") {
        serve_mcp().map(|_| 0)
    } else if args.iter().any(|a| a == "--serve") {
        serve().map(|_| 0)
    } else if let Some(shm_idx) = args.iter().position(|a| a == "--shm") {
        let default_shm = PathBuf::from("/tmp/memory-fabric.shm");
        let path = args.get(shm_idx + 1)
            .filter(|p| !p.starts_with("--"))
            .map(PathBuf::from)
            .unwrap_or(default_shm);
        shm::serve_shm(&path).map(|_| 0)
    } else if let Some(call_idx) = args.iter().position(|a| a == "--shm-call") {
        let default_shm = PathBuf::from("/tmp/memory-fabric.shm");
        let path = args.get(call_idx + 1)
            .filter(|p| !p.starts_with("--"))
            .map(PathBuf::from)
            .unwrap_or(default_shm);
        (|| -> Result<i32, String> {
            let mut input = Vec::new();
            io::stdin().read_to_end(&mut input).map_err(|e| e.to_string())?;
            let resp = shm::client_shm_call(&path, &input)?;
            io::stdout().write_all(&resp).map_err(|e| e.to_string())?;
            io::stdout().write_all(b"\n").map_err(|e| e.to_string())?;
            Ok(0)
        })()
    } else if let Some(sock_idx) = args.iter().position(|a| a == "--socket") {
        let default_sock = dirs_or_home_socket_path();
        let path = args.get(sock_idx + 1)
            .filter(|p| !p.starts_with("--"))
            .map(PathBuf::from)
            .unwrap_or(default_sock);
        serve_socket(&path).map(|_| 0)
    } else if args.iter().any(|a| a == "--watch") {
        watch_loop().map(|_| 0)
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
