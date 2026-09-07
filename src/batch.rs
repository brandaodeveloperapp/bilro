use crate::exec::spawn_with_timeout;
use crate::redact::redact;
use crate::sandbox::{index, open_default, search, QueryHit};
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

const DEFAULT_BATCH_TIMEOUT_MS: u64 = 5_000;
const MAX_CONCURRENCY: usize = 8;

/// One command to run as part of a batch, with a label used as the index
/// chunk title so search results can be traced back to it.
#[derive(Debug, Clone)]
pub struct Command {
    pub label: String,
    pub command: String,
}

/// What happened to a single command in the batch: whether it failed, how
/// much raw output it produced, and how many chunks it left in the index.
#[derive(Debug)]
pub struct CommandOutcome {
    pub label: String,
    pub failed: bool,
    pub timed_out: bool,
    pub raw_bytes: usize,
    pub chunks: usize,
}

/// Result of a batch run: per-command outcomes in input order, plus whatever
/// the given queries turned up in the freshly indexed output.
pub struct BatchResult {
    pub outcomes: Vec<CommandOutcome>,
    pub hits: Vec<QueryHit>,
}

fn run_one(command: &Command, cwd: Option<&Path>, timeout_ms: u64) -> (CommandOutcome, String) {
    let mut sh = ProcessCommand::new("/bin/sh");
    sh.arg("-c").arg(&command.command);
    if let Some(dir) = cwd {
        sh.current_dir(dir);
    }

    let (failed, timed_out, raw) = match spawn_with_timeout(sh, timeout_ms) {
        Ok(o) if o.timed_out => {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            let err = String::from_utf8_lossy(&o.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            text.push_str(&format!("\n[bilro: interrompido em {}ms]", timeout_ms));
            (true, true, text)
        }
        Ok(o) => {
            let ok = o.status.map(|s| s.success()).unwrap_or(false);
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            let err = String::from_utf8_lossy(&o.stderr);
            if !ok && !err.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            (!ok, false, text)
        }
        Err(e) => (true, false, e.to_string()),
    };

    let outcome = CommandOutcome {
        label: command.label.clone(),
        failed,
        timed_out,
        raw_bytes: raw.len(),
        chunks: 0,
    };
    (outcome, raw)
}

/// Runs `commands` with at most `concurrency` (clamped to 1..=8) in flight at
/// once, indexes every command's redacted output under its label, then runs
/// `queries` against the freshly indexed chunks. Output order in the result
/// matches input order regardless of which command finished first.
pub fn run(commands: &[Command], queries: &[String], concurrency: usize, cwd: Option<&Path>) -> BatchResult {
    let concurrency = concurrency.clamp(1, MAX_CONCURRENCY);
    let cwd_owned = cwd.map(Path::to_path_buf);
    let commands_arc: Arc<Vec<Command>> = Arc::new(commands.to_vec());
    let results: Arc<Mutex<Vec<Option<CommandOutcome>>>> =
        Arc::new(Mutex::new((0..commands.len()).map(|_| None).collect()));
    let next = Arc::new(AtomicUsize::new(0));

    let workers = concurrency.min(commands.len());
    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let next = Arc::clone(&next);
        let results = Arc::clone(&results);
        let commands_arc = Arc::clone(&commands_arc);
        let cwd_owned = cwd_owned.clone();
        handles.push(thread::spawn(move || loop {
            let i = next.fetch_add(1, Ordering::SeqCst);
            if i >= commands_arc.len() {
                break;
            }
            let command = &commands_arc[i];
            let (partial, raw) = run_one(command, cwd_owned.as_deref(), DEFAULT_BATCH_TIMEOUT_MS);
            let sanitized = redact(&raw);
            let chunks = open_default()
                .and_then(|conn| index(&conn, &command.label, &sanitized, "batch"))
                .unwrap_or(0);
            let outcome = CommandOutcome { chunks, ..partial };
            results.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(outcome);
        }));
    }
    for h in handles {
        let _ = h.join();
    }

    let outcomes: Vec<CommandOutcome> = Arc::try_unwrap(results)
        .map(|m| m.into_inner().unwrap_or_default())
        .unwrap_or_default()
        .into_iter()
        .flatten()
        .collect();

    let mut hits = Vec::new();
    if !queries.is_empty() {
        if let Ok(conn) = open_default() {
            for q in queries {
                if let Ok(found) = search(&conn, q, 5) {
                    hits.extend(found.into_iter().map(|hit| QueryHit { query: q.clone(), hit }));
                }
            }
        }
    }

    BatchResult { outcomes, hits }
}

/// JSON Schema for the `bilro_batch` MCP tool.
pub fn mcp_tool_schema() -> serde_json::Value {
    serde_json::json!({
        "name": "bilro_batch",
        "description": "Roda comandos em paralelo (concorrencia limitada), indexa a saida de cada um sob seu rotulo e devolve so os trechos que casam com as queries, em vez do texto bruto inteiro.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "commands": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "label": { "type": "string", "description": "titulo do trecho no indice; rotulo descritivo melhora a busca" },
                            "command": { "type": "string", "description": "comando de shell a rodar" }
                        },
                        "required": ["label", "command"]
                    }
                },
                "find": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "termos buscados no indice depois que todos os comandos rodarem"
                },
                "concurrency": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 8,
                    "description": "quantos comandos em voo ao mesmo tempo; padrao 1"
                },
                "cwd": { "type": "string", "description": "diretorio de trabalho para todos os comandos" }
            },
            "required": ["commands"]
        }
    })
}

fn outcome_to_json(o: &CommandOutcome) -> serde_json::Value {
    serde_json::json!({
        "label": o.label,
        "failed": o.failed,
        "timed_out": o.timed_out,
        "raw_bytes": o.raw_bytes,
        "chunks": o.chunks,
    })
}

fn hit_to_json(h: &QueryHit) -> serde_json::Value {
    serde_json::json!({
        "query": h.query,
        "label": h.hit.label,
        "body": h.hit.body,
        "source": h.hit.source,
        "score": h.hit.score,
    })
}

fn parse_commands(value: &serde_json::Value) -> Result<Vec<Command>, String> {
    let raw = value.as_array().ok_or("commands precisa ser uma lista")?;
    if raw.is_empty() {
        return Err("commands vazio".into());
    }
    raw.iter()
        .map(|c| {
            let label = c.get("label").and_then(|v| v.as_str()).ok_or("comando sem label")?.to_string();
            let command = c.get("command").and_then(|v| v.as_str()).ok_or("comando sem command")?.to_string();
            if command.trim().is_empty() {
                return Err("command vazio".into());
            }
            Ok(Command { label, command })
        })
        .collect()
}

/// Executes `bilro_batch` from raw MCP tool arguments, returning a JSON
/// string with per-command outcomes and query hits.
pub fn mcp_call(args: &serde_json::Value) -> Result<String, String> {
    let commands_value = args.get("commands").ok_or("commands obrigatorio")?;
    let commands = parse_commands(commands_value)?;

    let queries: Vec<String> = args
        .get("find")
        .or_else(|| args.get("queries"))
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
        .unwrap_or_default();

    let concurrency = args.get("concurrency").and_then(serde_json::Value::as_u64).unwrap_or(1) as usize;
    let cwd = args.get("cwd").and_then(|v| v.as_str()).map(PathBuf::from);

    let result = run(&commands, &queries, concurrency, cwd.as_deref());

    let mut out = String::new();
    for o in &result.outcomes {
        out.push_str(&format!(
            "{} {} — {} bytes em {} trecho(s){}\n",
            if o.failed { "falhou" } else { "ok" },
            o.label,
            o.raw_bytes,
            o.chunks,
            if o.timed_out { ", interrompido no timeout" } else { "" }
        ));
    }
    if result.hits.is_empty() {
        out.push_str(if queries.is_empty() {
            "\nSaida indexada. Passe find para receber os trechos, ou use bilro_find depois."
        } else {
            "\nNenhum trecho casou com o que foi pedido. A saida esta indexada; tente outro termo com bilro_find."
        });
    } else {
        for h in &result.hits {
            out.push_str(&format!("\n## {} — {}\n{}\n", h.query, h.hit.label, h.hit.body));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn cmd(label: &str, command: &str) -> Command {
        Command { label: label.to_string(), command: command.to_string() }
    }

    #[test]
    fn quatro_comandos_rapidos_todos_rodam_em_ordem() {
        let commands = vec![
            cmd("um", "echo 1"),
            cmd("dois", "echo 2"),
            cmd("tres", "echo 3"),
            cmd("quatro", "echo 4"),
        ];
        let result = run(&commands, &[], 4, None);
        assert_eq!(result.outcomes.len(), 4);
        let labels: Vec<&str> = result.outcomes.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, vec!["um", "dois", "tres", "quatro"]);
        assert!(result.outcomes.iter().all(|o| !o.failed));
    }

    #[test]
    fn aceita_find_e_o_nome_antigo_queries() {
        for chave in ["find", "queries"] {
            let args = serde_json::json!({
                "commands": [{"label": "eco", "command": "echo termo-raro-xilofone"}],
                chave: ["xilofone"]
            });
            let saida = mcp_call(&args).expect("deveria rodar");
            assert!(saida.contains("xilofone"), "chave {chave} nao trouxe o trecho: {saida}");
        }
    }

    #[test]
    fn sem_trecho_casando_diz_o_que_fazer_em_vez_de_calar() {
        let args = serde_json::json!({
            "commands": [{"label": "eco", "command": "echo alguma coisa"}],
            "find": ["termo-que-nao-existe-em-lugar-nenhum"]
        });
        let saida = mcp_call(&args).unwrap();
        assert!(saida.contains("bilro_find"), "deveria dizer como continuar: {saida}");
    }

    #[test]
    fn paralelismo_real_e_bem_mais_rapido_que_serial() {
        let commands = vec![
            cmd("a", "sleep 0.4"),
            cmd("b", "sleep 0.4"),
            cmd("c", "sleep 0.4"),
            cmd("d", "sleep 0.4"),
        ];

        let inicio_serial = Instant::now();
        let serial = run(&commands, &[], 1, None);
        let tempo_serial = inicio_serial.elapsed();

        let inicio_paralelo = Instant::now();
        let paralelo = run(&commands, &[], 4, None);
        let tempo_paralelo = inicio_paralelo.elapsed();

        assert_eq!(serial.outcomes.len(), 4);
        assert_eq!(paralelo.outcomes.len(), 4);
        assert!(
            tempo_paralelo < tempo_serial / 2,
            "paralelo ({:?}) deveria ser bem mais rapido que serial ({:?})",
            tempo_paralelo,
            tempo_serial
        );
    }

    #[test]
    fn saida_gigante_nao_trava_e_nao_perde_linha() {
        let commands = vec![cmd("gigante", "seq 1 200000")];
        let inicio = Instant::now();
        let result = run(&commands, &[], 1, None);
        assert!(!result.outcomes[0].failed, "nao deveria falhar nem travar ate o timeout");
        assert!(result.outcomes[0].raw_bytes > 1_000_000, "saida veio truncada demais: {} bytes", result.outcomes[0].raw_bytes);
        assert!(inicio.elapsed().as_secs() < 15, "demorou {}s", inicio.elapsed().as_secs());
    }

    #[test]
    fn um_comando_falhando_nao_impede_os_outros() {
        let commands = vec![cmd("ok-a", "echo a"), cmd("falha", "exit 7"), cmd("ok-b", "echo b")];
        let result = run(&commands, &[], 3, None);
        assert_eq!(result.outcomes.len(), 3);
        assert!(!result.outcomes[0].failed);
        assert!(result.outcomes[1].failed, "deveria marcar o comando que falhou");
        assert!(!result.outcomes[2].failed);
    }

    #[test]
    fn comando_pendurado_e_interrompido_pelo_timeout() {
        let commands = vec![cmd("pendurado", "sleep 30")];
        let inicio = Instant::now();
        let result = run(&commands, &[], 1, None);
        assert!(result.outcomes[0].timed_out, "deveria ter marcado timeout");
        assert!(inicio.elapsed().as_secs() < 10, "lote nao terminou logo apos o timeout do comando");
    }

    #[test]
    fn segredo_nao_aparece_no_indice_nem_no_resultado() {
        let commands = vec![cmd("segredo", "echo 'DB=postgres://u:S3GR3DO@h:5432/d'")];
        let result = run(&commands, &["S3GR3DO".to_string()], 1, None);
        assert!(result.hits.is_empty(), "a busca pelo segredo em si nao deveria achar nada indexado");

        let conn = open_default().unwrap();
        let achados = search(&conn, "postgres", 5).unwrap();
        for h in &achados {
            assert!(!h.body.contains("S3GR3DO"), "vazou no indice: {}", h.body);
        }
    }

    #[test]
    fn query_devolve_trecho_do_comando_certo_pelo_rotulo() {
        let commands = vec![
            cmd("relatorio-alfa", "echo termo-raro-alfa-xyz"),
            cmd("relatorio-beta", "echo outra-coisa-completamente-diferente"),
        ];
        let result = run(&commands, &["termo-raro-alfa-xyz".to_string()], 2, None);
        assert!(!result.hits.is_empty(), "deveria ter achado o termo raro");
        assert!(result.hits.iter().any(|h| h.hit.label == "relatorio-alfa"));
        assert!(result.hits.iter().all(|h| h.hit.label != "relatorio-beta"));
    }

    #[test]
    fn schema_mcp_declara_a_ferramenta_bilro_batch() {
        let schema = mcp_tool_schema();
        assert_eq!(schema["name"], "bilro_batch");
        assert!(schema["inputSchema"]["properties"]["commands"].is_object());
    }

    #[test]
    fn mcp_call_relata_cada_comando_em_texto_legivel() {
        let args = serde_json::json!({
            "commands": [
                {"label": "primeiro", "command": "echo um"},
                {"label": "segundo", "command": "exit 3"}
            ]
        });
        let saida = mcp_call(&args).expect("deveria rodar");
        assert!(saida.contains("primeiro"), "faltou o primeiro rotulo: {saida}");
        assert!(saida.contains("segundo"), "faltou o segundo rotulo: {saida}");
        assert!(saida.contains("falhou"), "deveria marcar o que falhou: {saida}");
        assert!(saida.contains("ok"), "deveria marcar o que passou: {saida}");
    }

    #[test]
    fn mcp_call_sem_commands_e_erro_claro() {
        let err = mcp_call(&serde_json::json!({})).unwrap_err();
        assert!(err.contains("commands"));
    }
}
