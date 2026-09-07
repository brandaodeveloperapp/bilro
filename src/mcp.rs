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
    let head = if failed { "falhou\n" } else { "" };
    if output.len() <= RETURN_WHOLE_UNDER {
        return format!("{head}{output}");
    }
    let Ok(conn) = crate::sandbox::open_default() else {
        return format!("{head}{}", &output[..RETURN_WHOLE_UNDER.min(output.len())]);
    };
    let chunks = crate::sandbox::index(&conn, label, output, "script").unwrap_or(0);
    let mut out = format!(
        "{head}{} linhas de saida guardadas no indice em {chunks} trechos, fora do contexto.\n",
        output.lines().count()
    );
    for q in queries {
        if let Ok(hits) = crate::sandbox::search(&conn, q, 3) {
            for h in hits {
                out.push_str(&format!("\n## {q}\n{}\n", h.body));
            }
        }
    }
    if queries.is_empty() {
        out.push_str("\nUse bilro_find para buscar nela, ou passe find na proxima chamada.");
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
            let r = crate::sandbox::run(&command, None, &queries, dir);
            let mut out = format!(
                "{} — {} trechos indexados, {} tokens ficaram fora do contexto\n",
                if r.failed { "falhou" } else { "ok" },
                r.chunks,
                r.withheld_tokens
            );
            for h in &r.hits {
                out.push_str(&format!("\n## {}\n{}\n", h.query, crate::redact::redact(&h.hit.body)));
            }
            if r.hits.is_empty() && !queries.is_empty() {
                out.push_str("\nnenhum trecho casou com o que foi pedido");
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
            match crate::script::run(&language, &code, dir, timeout) {
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
            let Ok(db) = crate::journal::open_default() else {
                return error_result("sem diario ainda".into());
            };
            let cwd = arg_str(args, "cwd");
            let dir = if cwd.is_empty() { std::env::current_dir().unwrap_or_default() } else { PathBuf::from(cwd) };
            let project = crate::journal::project_of(&dir);
            let query = arg_str(args, "query");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(12) as usize;
            let events = if query.trim().is_empty() {
                crate::journal::timeline(&db, Some(&project), limit)
            } else {
                crate::journal::search(&db, &query, Some(&project), limit)
            };
            match events {
                Err(e) => error_result(format!("recall falhou: {e}")),
                Ok(list) if list.is_empty() => text_result("nada registrado para este projeto ainda".into()),
                Ok(list) => {
                    let mut out = format!("{} eventos\n", list.len());
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
        "bilro_find" => {
            let query = arg_str(args, "query");
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(8) as usize;
            let Ok(conn) = crate::sandbox::open_default() else {
                return error_result("sem indice ainda".into());
            };
            match crate::sandbox::search(&conn, &query, limit) {
                Err(e) => error_result(format!("busca falhou: {e}")),
                Ok(hits) if hits.is_empty() => text_result(format!("nada indexado casa com {query}")),
                Ok(hits) => {
                    let mut out = format!("{} trechos\n", hits.len());
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
            match crate::filtered(&full) {
                Ok((text, note, saved)) => {
                    let mut out = text;
                    if saved > 0 {
                        out.push_str(&format!("\n\n[bilro: {saved}% menor{}]", if note.is_empty() { String::new() } else { format!(", {note}") }));
                    }
                    text_result(out)
                }
                Err(e) => error_result(e),
            }
        }
        "bilro_read" => {
            let path = arg_str(args, "path");
            let outline = args.get("outline").and_then(|v| v.as_bool()).unwrap_or(false);
            match crate::read::read(Path::new(&path), if outline { "outline" } else { "safe" }) {
                Err(e) => error_result(format!("{path}: {e}")),
                Ok(r) => {
                    let mut out = crate::redact::redact(&r.text);
                    if r.lossy {
                        out.push_str("\n\n[bilro: esboco. Corpo de funcao elidido, faixa de linha marcada. Nao edite a partir daqui.]");
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
            let Ok(out) = out else { return error_result("grep indisponivel".into()) };
            let raw = String::from_utf8_lossy(&out.stdout).to_string();
            if raw.trim().is_empty() {
                return text_result("sem resultado".into());
            }
            let r = crate::grep::compress(&crate::redact::redact(&raw));
            text_result(format!("{}\n\n[{} ocorrencias em {} arquivos]", r.text, r.hits, r.files))
        }
        other => error_result(format!("ferramenta desconhecida: {other}")),
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
                "error": { "code": -32601, "message": format!("metodo nao suportado: {other}") }
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
    fn initialize_anuncia_protocolo_e_nome() {
        let r = handle(&json!({"jsonrpc":"2.0","id":1,"method":"initialize"})).unwrap();
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL);
        assert_eq!(r["result"]["serverInfo"]["name"], "bilro");
        assert_eq!(r["id"], 1);
    }

    #[test]
    fn notificacao_nao_recebe_resposta() {
        assert!(handle(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).is_none());
    }

    #[test]
    fn tools_list_traz_todas_com_schema_bem_formado() {
        let r = handle(&json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).unwrap();
        let t = r["result"]["tools"].as_array().unwrap();
        assert_eq!(t.len(), 7);
        for tool in t {
            assert!(tool["name"].as_str().unwrap().starts_with("bilro_"));
            assert!(!tool["description"].as_str().unwrap().is_empty());
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["inputSchema"]["required"].is_array());
        }
    }

    #[test]
    fn metodo_desconhecido_devolve_erro_nao_panico() {
        let r = handle(&json!({"jsonrpc":"2.0","id":3,"method":"inventado"})).unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn ferramenta_desconhecida_e_erro_de_conteudo_nao_de_protocolo() {
        let r = handle(&json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"nao_existe","arguments":{}}})).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert!(r.get("error").is_none());
    }

    #[test]
    fn argumento_faltando_nao_derruba_o_servidor() {
        for name in ["bilro_run", "bilro_filter", "bilro_grep"] {
            let r = handle(&json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":name,"arguments":{}}})).unwrap();
            assert_eq!(r["result"]["isError"], true, "{name}");
        }
    }

    #[test]
    fn read_devolve_conteudo_e_avisa_quando_e_esboco() {
        let r = handle(&json!({"jsonrpc":"2.0","id":6,"method":"tools/call",
            "params":{"name":"bilro_read","arguments":{"path":"Cargo.toml"}}})).unwrap();
        let txt = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(txt.contains("bilro"));
        assert!(!txt.contains("esboco"), "modo seguro nao deve avisar de esboco");
    }

    #[test]
    fn script_deriva_resposta_sem_trazer_os_dados() {
        let r = handle(&json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{
            "name":"bilro_script",
            "arguments":{"language":"shell","code":"seq 1 100000"}
        }})).unwrap();
        let txt = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(txt.contains("indice"), "saida grande deveria ir para o indice: {}", &txt[..80.min(txt.len())]);
        assert!(txt.len() < 4000, "voltou grande demais: {} bytes", txt.len());
    }

    #[test]
    fn script_curto_volta_inteiro() {
        let r = handle(&json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{
            "name":"bilro_script","arguments":{"language":"shell","code":"echo 42"}
        }})).unwrap();
        assert!(r["result"]["content"][0]["text"].as_str().unwrap().contains("42"));
    }

    #[test]
    fn script_sem_argumento_e_erro_de_conteudo() {
        let r = handle(&json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{
            "name":"bilro_script","arguments":{"language":"shell"}
        }})).unwrap();
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn recall_sem_termo_nao_e_erro_mesmo_vazio() {
        let r = handle(&json!({"jsonrpc":"2.0","id":11,"method":"tools/call","params":{
            "name":"bilro_recall","arguments":{"cwd":"/caminho/que/nao/existe/em/lugar/nenhum"}
        }})).unwrap();
        assert!(r["result"]["isError"].as_bool() != Some(true), "recall vazio nao deveria ser erro");
    }

    #[test]
    fn segredo_nao_atravessa_a_ferramenta() {
        let r = handle(&json!({"jsonrpc":"2.0","id":7,"method":"tools/call",
            "params":{"name":"bilro_filter","arguments":{"command":"echo 'URL=postgres://u:S3GR3DO@db:5432/x'"}}})).unwrap();
        let txt = r["result"]["content"][0]["text"].as_str().unwrap_or("");
        assert!(!txt.contains("S3GR3DO"), "vazou segredo pela ferramenta MCP");
    }
}
