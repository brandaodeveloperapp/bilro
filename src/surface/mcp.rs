use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL: &str = "2024-11-05";

fn tools() -> Value {
    json!([
        {
            "name": "bilro_run",
            "description": "Runs a shell command, holds its output in a local index instead of the conversation, and returns only the passages matching the queries. Use when output size cannot be predicted: recursive finds, repo-wide greps, build logs, test runs.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to run" },
                    "find": { "type": "array", "items": { "type": "string" }, "description": "What to look for in the output. Omit to get only the size report." },
                    "cwd": { "type": "string", "description": "Working directory" }
                },
                "required": ["command"]
            }
        },
        {
            "name": "bilro_script",
            "description": "Runs a snippet in its own interpreter and returns only what it printed. Use it to DERIVE an answer from data — filter, count, parse, aggregate — so the raw bytes never enter the conversation. Output larger than a few kilobytes is indexed instead of returned; pass find to say what you are looking for.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "language": { "type": "string", "description": "shell, javascript, python, ruby or perl" },
                    "code": { "type": "string" },
                    "find": { "type": "array", "items": { "type": "string" }, "description": "What to look for, when the output is too large to return whole" },
                    "cwd": { "type": "string" },
                    "timeout_ms": { "type": "integer" }
                },
                "required": ["language", "code"]
            }
        },
        {
            "name": "bilro_recall",
            "description": "Recalls what already happened in this project: what was asked for, and which commands failed. Use it when resuming or after the conversation has been compacted, BEFORE asking the user to repeat themselves. With no query it returns the most recent events.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Omit to get the timeline" },
                    "cwd": { "type": "string", "description": "Project directory. Defaults to the current one." },
                    "limit": { "type": "integer" }
                },
                "required": []
            }
        },
        {
            "name": "bilro_fetch",
            "description": "Fetches a web page, keeps its readable text in the index instead of the conversation, and returns only the passages matching what you ask for. Use it for documentation and reference pages, where the markup is most of the bytes and almost none of the meaning.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "http or https only" },
                    "find": { "type": "array", "items": { "type": "string" }, "description": "What to look for on the page" }
                },
                "required": ["url"]
            }
        },
        {
            "name": "bilro_remember",
            "description": "Files something decided, rejected or discovered so a later session does not have to rediscover it. Call it the moment it happens — when the user settles a question, vetoes an approach, or a constraint of this machine or project comes to light. A hook cannot infer any of these from a tool call. Use kind: decision for what was settled, rejected for an approach ruled out (say why), constraint for a fact about the environment that will bite again.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": { "type": "string", "enum": ["decision", "rejected", "constraint"] },
                    "subject": { "type": "string", "description": "One line, the thing itself" },
                    "body": { "type": "string", "description": "Why, and what it means for later" },
                    "cwd": { "type": "string" }
                },
                "required": ["kind", "subject"]
            }
        },
        crate::run::batch::mcp_tool_schema(),
        {
            "name": "bilro_find",
            "description": "Searches everything already indexed by bilro_run, without running anything again.",
            "inputSchema": {
                "type": "object",
                "properties": { "query": { "type": "string" }, "limit": { "type": "integer" } },
                "required": ["query"]
            }
        },
        {
            "name": "bilro_filter",
            "description": "Runs a command and returns it compressed: repeated lines this command always prints are dropped, structure is collapsed, and credentials are redacted. A line reporting a failure is never dropped.",
            "inputSchema": {
                "type": "object",
                "properties": { "command": { "type": "string" }, "cwd": { "type": "string" } },
                "required": ["command"]
            }
        },
        {
            "name": "bilro_read",
            "description": "Reads a file compressed. Outline mode keeps declarations and replaces bodies with a marker naming the exact elided line range - use it to understand a file, never to edit one.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "outline": { "type": "boolean", "description": "Structure only. Lossy." }
                },
                "required": ["path"]
            }
        },
        {
            "name": "bilro_grep",
            "description": "Searches a tree and returns hits grouped by file with repeated match text collapsed into a count. The matched content is never truncated.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" },
                    "paths": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["pattern"]
            }
        }
    ])
}

const RETURN_WHOLE_UNDER: usize = 4096;

/// Returns short output as it is, and holds a long one in the index so the
/// conversation gets the passages asked for instead of every byte. This is the
/// whole reason to run a snippet through bilro rather than a plain shell.
fn hold_if_large(label: &str, output: &str, queries: &[String], failed: bool) -> String {
    let head = if failed { "failed\n" } else { "" };
    if output.len() <= RETURN_WHOLE_UNDER {
        return format!("{head}{output}");
    }
    let Ok(conn) = crate::store::sandbox::open_default() else {
        return format!("{head}{}", &output[..RETURN_WHOLE_UNDER.min(output.len())]);
    };
    let chunks = crate::store::sandbox::index(&conn, label, output, "script").unwrap_or(0);
    let mut out = format!(
        "{head}{} lines of output held in the index across {chunks} chunks, out of context.\n",
        output.lines().count()
    );
    for q in queries {
        if let Ok(hits) = crate::store::sandbox::search(&conn, q, 3) {
            for h in hits {
                out.push_str(&format!("\n## {q}\n{}\n", h.body));
            }
        }
    }
    if queries.is_empty() {
        out.push_str("\nUse bilro_find to search it, or pass find on the next call.");
    }
    out
}

fn text_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }] })
}

fn error_result(text: String) -> Value {
    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
}

fn arg_str(args: &Value, key: &str) -> String {
    args.get(key).and_then(|v| v.as_str()).unwrap_or("").to_string()
}

fn call_tool(name: &str, args: &Value) -> Value {
    match name {
        "bilro_run" => {
            let command = arg_str(args, "command");
            if command.is_empty() {
                return error_result("command is required".into());
            }
            let queries: Vec<String> = args
                .get("find")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|q| q.as_str()).map(String::from).collect())
                .unwrap_or_default();
            let cwd = arg_str(args, "cwd");
            let dir = if cwd.is_empty() { None } else { Some(Path::new(cwd.as_str())) };
            let r = crate::store::sandbox::run(&command, None, &queries, dir);
            let mut out = format!(
                "{} — {} chunks indexed, {} tokens stayed out of context\n",
                if r.failed { "failed" } else { "ok" },
                r.chunks,
                r.withheld_tokens
            );
            for h in &r.hits {
                out.push_str(&format!("\n## {}\n{}\n", h.query, crate::redact::redact(&h.hit.body)));
            }
            if r.hits.is_empty() && !queries.is_empty() {
                out.push_str("\nno chunk matched what was asked for");
            }
            text_result(out)
        }
        "bilro_script" => {
            let language = arg_str(args, "language");
            let code = arg_str(args, "code");
            if language.is_empty() || code.is_empty() {
                return error_result("language and code are required".into());
            }
            let cwd = arg_str(args, "cwd");
            let dir = if cwd.is_empty() { None } else { Some(Path::new(cwd.as_str())) };
            let timeout = args.get("timeout_ms").and_then(|v| v.as_u64());
            match crate::run::script::run(&language, &code, dir, timeout) {
                Err(e) => error_result(e),
                Ok(r) => {
                    let queries: Vec<String> = args
                        .get("find")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|q| q.as_str()).map(String::from).collect())
                        .unwrap_or_default();
                    text_result(hold_if_large(&format!("{language} snippet"), &r.output, &queries, r.failed))
                }
            }
        }
        "bilro_recall" => {
            let Ok(db) = crate::store::journal::open_default() else {
                return error_result("no journal yet".into());
            };
            let cwd = arg_str(args, "cwd");
            let dir = if cwd.is_empty() { std::env::current_dir().unwrap_or_default() } else { PathBuf::from(cwd) };
            let project = crate::store::journal::project_of(&dir);
            let query = arg_str(args, "query");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(12) as usize;
            let events = if query.trim().is_empty() {
                crate::store::journal::timeline(&db, Some(&project), limit)
            } else {
                crate::store::journal::search(&db, &query, Some(&project), limit)
            };
            match events {
                Err(e) => error_result(format!("recall failed: {e}")),
                Ok(list) if list.is_empty() => text_result("nothing recorded for this project yet".into()),
                Ok(list) => {
                    let mut out = format!("{} events\n", list.len());
                    for e in &list {
                        out.push_str(&format!("\n[{}] {}\n", e.kind, e.subject));
                        if !e.body.trim().is_empty() {
                            out.push_str(&format!("{}\n", e.body));
                        }
                    }
                    text_result(out)
                }
            }
        }
        "bilro_fetch" => {
            let url = arg_str(args, "url");
            if url.is_empty() {
                return error_result("url is required".into());
            }
            match crate::run::web::fetch(&url) {
                Err(e) => error_result(e),
                Ok(page) => {
                    let queries: Vec<String> = args
                        .get("find")
                        .and_then(|v| v.as_array())
                        .map(|a| a.iter().filter_map(|q| q.as_str()).map(String::from).collect())
                        .unwrap_or_default();
                    let label = page.title.clone().unwrap_or_else(|| page.url.clone());
                    let header = format!(
                        "{}\n{} — {} bytes of page became {} of text\n",
                        label,
                        page.url,
                        page.bytes,
                        page.text.len()
                    );
                    text_result(format!("{header}{}", hold_if_large(&label, &page.text, &queries, false)))
                }
            }
        }
        "bilro_remember" => {
            let kind = arg_str(args, "kind");
            let subject = arg_str(args, "subject");
            if subject.trim().is_empty() {
                return error_result("subject is required".into());
            }
            if !crate::store::journal::is_known_kind(&kind) {
                return error_result(format!(
                    "unknown kind: {kind}. Use decision, rejected or constraint"
                ));
            }
            let cwd = arg_str(args, "cwd");
            let dir = if cwd.is_empty() { std::env::current_dir().unwrap_or_default() } else { PathBuf::from(cwd) };
            let Ok(db) = crate::store::journal::open_default() else {
                return error_result("no journal yet".into());
            };
            match crate::store::journal::record(
                &db,
                &kind,
                &subject,
                &arg_str(args, "body"),
                "deliberate",
                &crate::store::journal::project_of(&dir),
            ) {
                Err(e) => error_result(format!("could not write: {e}")),
                Ok(()) => text_result(format!("stored as {kind}: {subject}")),
            }
        }
        "bilro_batch" => match crate::run::batch::mcp_call(args) {
            Ok(text) => text_result(text),
            Err(e) => error_result(e),
        },
        "bilro_find" => {
            let query = arg_str(args, "query");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
            let Ok(conn) = crate::store::sandbox::open_default() else {
                return error_result("no index yet".into());
            };
            match crate::store::sandbox::search(&conn, &query, limit) {
                Err(e) => error_result(format!("search failed: {e}")),
                Ok(hits) if hits.is_empty() => text_result(format!("nothing indexed matches {query}")),
                Ok(hits) => {
                    let mut out = format!("{} chunks\n", hits.len());
                    for h in &hits {
                        out.push_str(&format!("\n## {}\n{}\n", h.label, crate::redact::redact(&h.body)));
                    }
                    text_result(out)
                }
            }
        }
        "bilro_filter" => {
            let command = arg_str(args, "command");
            if command.is_empty() {
                return error_result("command is required".into());
            }
            let cwd = arg_str(args, "cwd");
            let full = if cwd.is_empty() { command.clone() } else { format!("cd {cwd} && {command}") };
            match crate::compress::pipeline::filtered(&full) {
                Ok((text, note, saved)) => {
                    let mut out = text;
                    if saved > 0 {
                        out.push_str(&format!("\n\n[bilro: {saved}% smaller{}]", if note.is_empty() { String::new() } else { format!(", {note}") }));
                    }
                    text_result(out)
                }
                Err(e) => error_result(e),
            }
        }
        "bilro_read" => {
            let path = arg_str(args, "path");
            let outline = args.get("outline").and_then(|v| v.as_bool()).unwrap_or(false);
            match crate::compress::read::read(Path::new(&path), if outline { "outline" } else { "safe" }) {
                Err(e) => error_result(format!("{path}: {e}")),
                Ok(r) => {
                    let mut out = crate::redact::redact(&r.text);
                    if r.lossy {
                        out.push_str("\n\n[bilro: outline. Function bodies elided, line range marked. Do not edit from this.]");
                    }
                    text_result(out)
                }
            }
        }
        "bilro_grep" => {
            let pattern = arg_str(args, "pattern");
            if pattern.is_empty() {
                return error_result("pattern is required".into());
            }
            let mut argv = vec!["-rn".to_string(), pattern];
            if let Some(paths) = args.get("paths").and_then(|v| v.as_array()) {
                argv.extend(paths.iter().filter_map(|p| p.as_str()).map(String::from));
            }
            let out = std::process::Command::new("grep").args(&argv).output();
            let Ok(out) = out else { return error_result("grep unavailable".into()) };
            let raw = String::from_utf8_lossy(&out.stdout).to_string();
            if raw.trim().is_empty() {
                return text_result("no result".into());
            }
            let r = crate::compress::grep::compress(&crate::redact::redact(&raw));
            text_result(format!("{}\n\n[{} matches in {} files]", r.text, r.hits, r.files))
        }
        other => error_result(format!("unknown tool: {other}")),
    }
}

/// Answers one JSON-RPC request. Returns None for a notification, which by the
/// protocol gets no reply at all — sending one is what makes a client hang.
pub fn handle(req: &Value) -> Option<Value> {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = req.get("id").cloned()?;

    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "bilro", "version": env!("CARGO_PKG_VERSION") }
        }),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
            let name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            call_tool(name, &args)
        }
        "ping" => json!({}),
        other => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("unsupported method: {other}") }
            }))
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

pub fn serve() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(resp) = handle(&req) {
            let _ = writeln!(stdout, "{resp}");
            let _ = stdout.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_announces_protocol_and_name() {
        let r = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize"})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL);
        assert_eq!(r["result"]["serverInfo"]["name"], "bilro");
        assert_eq!(r["id"], 1);
    }

    #[test]
    fn notification_gets_no_response() {
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
    }

    #[test]
    fn tools_list_brings_all_with_well_formed_schema() {
        let r = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        let t = r["result"]["tools"].as_array().unwrap();
        assert_eq!(t.len(), 10);
        for tool in t {
            assert!(tool["name"].as_str().unwrap().starts_with("bilro_"));
            assert!(!tool["description"].as_str().unwrap().is_empty());
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["inputSchema"]["required"].is_array());
        }
    }

    #[test]
    fn unknown_method_returns_error_not_panic() {
        let r = handle(&json!({"jsonrpc":"2.0","id":3,"method":"madeup"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn unknown_tool_is_content_error_not_protocol_error() {
        let r = handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"does_not_exist","arguments":{}}})).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r.get("error").is_none());
    }

    #[test]
    fn missing_argument_does_not_bring_down_the_server() {
        for name in ["bilro_run", "bilro_filter", "bilro_grep"] {
            let r = handle(&json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":name,"arguments":{}}})).unwrap();
            assert_eq!(r["result"]["isError"], true, "{name}");
        }
    }

    #[test]
    fn read_returns_content_and_warns_when_it_is_an_outline() {
        let r = handle(&json!({"jsonrpc":"2.0","id":6,"method":"tools/call",
            "params":{"name":"bilro_read","arguments":{"path":"Cargo.toml"}}})).unwrap();
        let txt = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(txt.contains("bilro"));
        assert!(!txt.contains("outline"), "safe mode should not warn about an outline");
    }

    #[test]
    fn script_derives_an_answer_without_bringing_the_data() {
        let r = handle(&json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{
            "name":"bilro_script",
            "arguments":{"language":"shell","code":"seq 1 100000"}
        }})).unwrap();
        let txt = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(txt.contains("index"), "large output should go to the index: {}", &txt[..80.min(txt.len())]);
        assert!(txt.len() < 4000, "came back too large: {} bytes", txt.len());
    }

    #[test]
    fn short_script_comes_back_whole() {
        let r = handle(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{
            "name":"bilro_script","arguments":{"language":"shell","code":"echo 42"}
        }})).unwrap();
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("42"));
    }

    #[test]
    fn script_without_argument_is_a_content_error() {
        let r = handle(&json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{
            "name":"bilro_script","arguments":{"language":"shell"}
        }})).unwrap();
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn recall_without_a_term_is_not_an_error_even_when_empty() {
        let r = handle(&json!({"jsonrpc":"2.0","id":11,"method":"tools/call","params":{
            "name":"bilro_recall","arguments":{"cwd":"/path/that/does/not/exist/anywhere"}
        }})).unwrap();
        assert!(r["result"]["isError"].as_bool() != Some(true), "empty recall should not be an error");
    }

    #[test]
    fn fetch_refuses_dangerous_scheme_without_trying_to_fetch() {
        let r = handle(&json!({"jsonrpc":"2.0","id":12,"method":"tools/call","params":{
            "name":"bilro_fetch","arguments":{"url":"file:///etc/passwd"}
        }})).unwrap();
        assert_eq!(r["result"]["isError"], true);
        let txt = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(txt.contains("http"), "should say what it accepts: {txt}");
    }

    #[test]
    fn remember_refuses_a_made_up_kind_and_names_the_valid_ones() {
        let r = handle(&json!({"jsonrpc":"2.0","id":13,"method":"tools/call","params":{
            "name":"bilro_remember","arguments":{"kind":"whatever","subject":"x"}
        }})).unwrap();
        assert_eq!(r["result"]["isError"], true);
        let txt = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(txt.contains("decision") && txt.contains("constraint"), "got: {txt}");
    }

    #[test]
    fn remember_without_a_subject_is_refused() {
        let r = handle(&json!({"jsonrpc":"2.0","id":14,"method":"tools/call","params":{
            "name":"bilro_remember","arguments":{"kind":"decision","subject":"  "}
        }})).unwrap();
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn secret_does_not_cross_the_tool() {
        let r = handle(&json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"bilro_filter","arguments":{"command":"echo 'URL=postgres://u:S3CR3T@db:5432/x'"}}})).unwrap();
        let txt = r["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(!txt.contains("S3CR3T"), "leaked secret through the MCP tool");
    }
}
