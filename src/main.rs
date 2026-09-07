mod exec;
mod filters;
mod graph;
mod grep;
mod install;
mod journal;
mod mcp;
mod ledger;
mod learn;
mod memory;
mod propose;
mod read;
mod redact;
mod ready;
mod sandbox;
mod web;
mod script;
mod shapes;
mod style;
mod weigh;

use shapes::contract::Shape;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_OBSERVED: usize = 512 * 1024;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn learn_db() -> PathBuf {
    home().join(".claude").join("bilro").join("learn.db")
}

fn all_shapes() -> Vec<Shape> {
    vec![
        shapes::table::shape(),
        shapes::diff::shape(),
        shapes::listing::shape(),
        shapes::install_log::shape(),
        shapes::keyvalue::shape(),
        shapes::test_report::shape(),
        shapes::diagnostics::shape(),
    ]
}

/// Structure first, then history: a shape works on a command never seen before,
/// while denoise needs several runs before it may judge anything.
fn squeeze(command: &str, output: &str) -> (String, String) {
    let shaped = shapes::apply(&all_shapes(), output);
    let mut note = shaped.shape.map(|s| s.to_string()).unwrap_or_default();
    let Ok(db) = learn::open(&learn_db()) else {
        return (shaped.text, note);
    };
    match learn::denoise(&db, command, &shaped.text, 3, 0.8) {
        Ok(d) if d.learned && d.dropped > 0 => {
            if !note.is_empty() {
                note.push_str(" + ");
            }
            note.push_str(&format!("{} linhas repetidas", d.dropped));
            (d.text, note)
        }
        _ => (shaped.text, note),
    }
}

fn hook_shadow() {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return;
    }
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
    if data["tool_name"].as_str() != Some("Bash") {
        return;
    }
    let command = data["tool_input"]["command"].as_str().unwrap_or("");
    let response = &data["tool_response"];
    let output = response
        .as_str()
        .or_else(|| response["stdout"].as_str())
        .or_else(|| response["output"].as_str())
        .unwrap_or("");
    if command.is_empty() || output.trim().is_empty() {
        return;
    }
    let safe = redact::redact(output);
    let clipped: &str = if safe.len() > MAX_OBSERVED {
        match safe.char_indices().nth(MAX_OBSERVED) {
            Some((i, _)) => &safe[..i],
            None => &safe,
        }
    } else {
        &safe
    };
    if let Ok(mut db) = learn::open(&learn_db()) {
        let _ = learn::observe(&mut db, command, clipped);
    }

    let falhas: Vec<&str> = clipped.lines().filter(|l| learn::is_severe(l)).take(6).collect();
    if !falhas.is_empty() {
        let cwd = std::env::current_dir().unwrap_or_default();
        if let Ok(db) = journal::open_default() {
            let _ = journal::record(
                &db,
                "falha",
                command,
                &falhas.join("\n"),
                data["session_id"].as_str().unwrap_or("desconhecida"),
                &journal::project_of(&cwd),
            );
        }
    }
}

/// Runs a command and returns its compressed output, the shape that did the
/// compressing, and how much smaller it got. Shared by the command line and the
/// MCP tool so both answer identically.
pub fn filtered(command: &str) -> Result<(String, String, usize), String> {
    let out = Command::new("sh").arg("-c").arg(command).output().map_err(|e| e.to_string())?;
    let mut captured = String::from_utf8_lossy(&out.stdout).to_string();
    captured.push_str(&String::from_utf8_lossy(&out.stderr));
    let raw = redact::redact(&captured);
    let before = raw.len();
    let (text, note) = squeeze(command, &raw);
    if let Ok(mut db) = learn::open(&learn_db()) {
        let _ = learn::observe(&mut db, command, &raw);
    }
    let saved = if before > text.len() { 100 - text.len() * 100 / before.max(1) } else { 0 };
    let text = match suppressed_notice(&raw, &text) {
        Some(msg) => msg,
        None => text,
    };
    Ok((text, note, saved))
}

fn run_filtered(argv: &[String]) -> i32 {
    if argv.is_empty() {
        eprintln!("  uso: bilro filter <comando>");
        return 2;
    }
    let command = argv.join(" ");
    let out = Command::new("sh").arg("-c").arg(&command).output();
    let Ok(out) = out else {
        eprintln!("  falhou ao executar");
        return 127;
    };
    let status = out.status.code().unwrap_or(1);
    let mut captured = String::from_utf8_lossy(&out.stdout).to_string();
    captured.push_str(&String::from_utf8_lossy(&out.stderr));
    let raw = redact::redact(&captured);

    let before = raw.len();
    let (text, note) = squeeze(&command, &raw);
    if let Ok(mut db) = learn::open(&learn_db()) {
        let _ = learn::observe(&mut db, &command, &raw);
    }
    if let Some(msg) = suppressed_notice(&raw, &text) {
        eprintln!("  \x1b[2m{msg}\x1b[0m");
        return status;
    }
    println!("{text}");
    if before > text.len() {
        let pct = 100 - text.len() * 100 / before.max(1);
        eprintln!("\n  \x1b[2m{pct}% menor{}\x1b[0m", if note.is_empty() { String::new() } else { format!(" ({note})") });
    }
    status
}

/// Says what happened when compression leaves nothing to print. Printing an
/// empty result would look like the command produced no output at all, which is
/// the silent loss this tool exists to avoid.
fn suppressed_notice(raw: &str, text: &str) -> Option<String> {
    if !text.trim().is_empty() || raw.trim().is_empty() {
        return None;
    }
    let lines = raw.lines().filter(|l| !l.trim().is_empty()).count();
    Some(format!(
        "identico ao que este comando ja imprimiu antes: {lines} linhas suprimidas. bilro nao viu falha entre elas, mas so reconhece as que sabe nomear — rode sem bilro se o resultado importa"
    ))
}

fn words(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .filter(|w| w.chars().count() > 3)
        .map(|w| w.to_string())
        .collect()
}

fn overlap(a: &std::collections::HashSet<String>, b: &std::collections::HashSet<String>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let hits = a.iter().filter(|w| b.contains(*w)).count();
    hits as f64 / a.len().min(b.len()) as f64
}

/// Prices a subagent before it is dispatched. The fixed cost is paid whatever
/// the agent then does, and a second agent on a subject already covered pays it
/// twice for one answer.
fn hook_task(cwd: &Path) {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return;
    }
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
    let tool = data["tool_name"].as_str().unwrap_or("");
    if tool != "Task" && tool != "Agent" {
        return;
    }
    let input = &data["tool_input"];
    let kind = input["subagent_type"].as_str().unwrap_or("general-purpose");
    let prompt = input["prompt"].as_str().unwrap_or("");
    let description = input["description"].as_str().unwrap_or("");
    let session = data["session_id"].as_str().unwrap_or("unknown");

    let claude = home().join(".claude");
    let agents = weigh::weigh_agents(&[claude.join("agents"), cwd.join(".claude").join("agents")]);
    let catalogue: i64 = agents.iter().map(|a| a.catalogue_tokens).sum();
    let inherits = agents.iter().find(|a| a.name == kind).map(|a| a.inherits_everything).unwrap_or(true);
    let floor = 18000 + catalogue;
    let prompt_tokens = weigh::tokens_of(prompt);

    let subject = words(&format!("{description} {prompt}").chars().take(600).collect::<String>());
    let state = ledger::read(session);
    let near: Vec<String> = state["dispatches"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|d| {
                    let prev: std::collections::HashSet<String> = d["words"]
                        .as_array()
                        .map(|w| w.iter().filter_map(|x| x.as_str()).map(|s| s.to_string()).collect())
                        .unwrap_or_default();
                    overlap(&subject, &prev) > 0.45
                })
                .filter_map(|d| d["type"].as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    println!(
        "bilro: ~{}k de custo fixo + {prompt_tokens} tok deste prompt, antes de qualquer trabalho.",
        floor / 1000
    );
    if inherits {
        println!("  {kind} nao declara tools: herda o catalogo inteiro de ferramentas neste despacho.");
    }
    if !near.is_empty() {
        println!(
            "  {}o agente sobre o mesmo assunto nesta sessao ({}). Da pra medir com ctx_execute?",
            near.len() + 1,
            near.join(", ")
        );
    }
    let total = state["dispatches"].as_array().map(|a| a.len()).unwrap_or(0);
    if total >= 8 {
        println!("  {total} agentes ja despachados nesta sessao.");
    }

    ledger::record(
        session,
        "dispatches",
        serde_json::json!({
            "type": kind,
            "words": subject.iter().take(40).collect::<Vec<_>>(),
            "cost": floor + prompt_tokens,
            "repeat": !near.is_empty(),
        }),
    );
}

/// The one line a session opens with: what context costs before anything is
/// asked for. A hook that says more than this spends the budget it is reporting.
fn hook_session(cwd: &Path) {
    let claude = home().join(".claude");
    let slug = cwd.to_string_lossy().replace('/', "-");
    let fixed: i64 = weigh::weigh_always_on(&claude).iter().map(|a| a.tokens).sum::<i64>()
        + weigh::weigh_memory(&claude.join("projects")).iter().filter(|m| m.project == slug).map(|m| m.tokens).sum::<i64>()
        + weigh::weigh_agents(&[claude.join("agents"), cwd.join(".claude").join("agents")])
            .iter()
            .map(|a| a.catalogue_tokens)
            .sum::<i64>();
    if fixed == 0 {
        return;
    }
    let agents = weigh::weigh_agents(&[claude.join("agents"), cwd.join(".claude").join("agents")]);
    let herdam = agents.iter().filter(|a| a.inherits_everything).count();
    print!("bilro: {fixed} tok de custo fixo por request neste projeto.");
    if herdam > 0 {
        print!(" {herdam} agentes sem tools: herdam o catalogo inteiro quando despachados.");
    }
    println!(" Antes de despachar agente, pergunte se da pra medir com ctx_execute.");
}

fn cmd_read(argv: &[String]) {
    let outline = argv.iter().any(|a| a == "--outline");
    let Some(file) = argv.iter().find(|a| !a.starts_with("--")) else {
        return eprintln!("  uso: bilro read <arquivo> [--outline]");
    };
    match read::read(Path::new(file), if outline { "outline" } else { "safe" }) {
        Ok(r) => {
            println!("{}", redact::redact(&r.text));
            if r.lossy {
                eprintln!(
                    "\n  \x1b[33mesboco: corpo de funcao elidido, faixa de linha marcada. Nao use para editar.\x1b[0m \x1b[2m{}% menor\x1b[0m",
                    (r.saved * 100.0).round()
                );
            }
        }
        Err(e) => eprintln!("  {e}"),
    }
}

fn cmd_grep(argv: &[String]) {
    if argv.is_empty() {
        return eprintln!("  uso: bilro grep <padrao> [caminho...]");
    }
    let out = Command::new("grep").arg("-rn").args(argv).output();
    let Ok(out) = out else { return eprintln!("  grep nao disponivel") };
    let raw = String::from_utf8_lossy(&out.stdout).to_string();
    if raw.trim().is_empty() {
        return eprintln!("  \x1b[2msem resultado\x1b[0m");
    }
    let r = grep::compress(&redact::redact(&raw));
    println!("{}", r.text);
    eprintln!("\n  \x1b[2m{} ocorrencias em {} arquivos\x1b[0m", r.hits, r.files);
}

fn memory_dir(cwd: &Path) -> PathBuf {
    let slug = cwd.to_string_lossy().replace('/', "-");
    home().join(".claude").join("projects").join(slug).join("memory")
}

const DIM: &str = "\x1b[2m";
const OFF: &str = "\x1b[0m";
const WARN: &str = "\x1b[33m";

fn cmd_ready() {
    let Ok(m) = ready::evaluate_default() else { return eprintln!("  sem banco ainda") };
    let pct = |x: f64| format!("{}%", (x * 100.0).round());
    let mark = |ok: bool| if ok { "\x1b[32mok\x1b[0m".to_string() } else { format!("{WARN}falta{OFF}") };
    println!("\n  pode aposentar as ferramentas que o bilro substitui?\n");
    println!("  historico   {} comandos aprendidos  {}", m.total, mark(m.total >= 40));
    println!("  cobertura   {} com 3+ execucoes = {}  {}", m.learned, pct(m.coverage), mark(m.coverage >= 0.8));
    println!("  economia    {} do output cortado  {}", pct(m.savings), mark(m.savings >= 0.5));
    println!("  sinal       {} linhas de falha perdidas  {}", m.lost.len(), mark(m.lost.is_empty()));
    println!("  auditoria   {} red-team, {} criticos  {}", m.audits, m.criticals, mark(m.audits >= 2 && m.criticals == 0));
    println!();
    for v in ready::verdicts(&m) {
        if v.missing.is_empty() {
            println!("  \x1b[32mPODE APOSENTAR\x1b[0m  {}", v.tool);
        } else {
            println!("  {DIM}ainda nao{OFF}       {}{DIM}  — falta {}; se errar: {}{OFF}", v.tool, v.missing.join(", "), v.risk);
        }
    }
    println!();
}

fn cmd_lint(cwd: &Path) {
    let r = graph::lint(&memory_dir(cwd));
    println!("\n  {} memorias", r.total);
    if !r.broken.is_empty() {
        println!("\n  {WARN}{} links quebrados{OFF}", r.broken.len());
        for b in r.broken.iter().take(10) {
            println!("    {} {DIM}aponta para{OFF} {}", b.from, b.to);
        }
    }
    if !r.orphans.is_empty() {
        println!("\n  {} orfas {DIM}(sem link entrando nem saindo){OFF}", r.orphans.len());
        for o in r.orphans.iter().take(8) {
            println!("    {o}");
        }
    }
    if !r.hubs.is_empty() {
        println!("\n  mais citadas");
        for h in &r.hubs {
            println!("    {:<44} {DIM}{} entradas{OFF}", h.name, h.incoming);
        }
    }
    println!();
}

fn cmd_propose() {
    let Ok(db) = learn::open(&learn_db()) else { return eprintln!("  sem banco ainda") };
    let opts = propose::ProposalOptions { min_runs: 5, min_failure_runs: 2 };
    let Ok(list) = propose::proposals(&db, &opts) else { return };
    if list.is_empty() {
        return println!("\n  {DIM}nada a propor ainda{OFF}\n");
    }
    println!("\n  {} memorias que valeria escrever\n", list.len());
    for p in &list {
        println!("  {:<10} {}", p.kind, p.subject);
        println!("             {DIM}{}{OFF}", p.why);
    }
    println!();
}

fn cmd_verify(cwd: &Path) {
    let dir = memory_dir(cwd);
    let memories = memory::load_memories(&dir);
    if memories.is_empty() {
        return println!("\n  {DIM}este projeto nao tem memoria em {}{OFF}\n", dir.display());
    }
    let armed = std::env::args().any(|a| a == "--run");
    let mut refused = Vec::new();
    let mut checked = 0usize;
    println!();
    for m in &memories {
        let Some(v) = m.verify.as_deref() else { continue };
        if !exec::is_safe(v) || !exec::is_allowed_program(v) {
            refused.push((m.name.clone(), exec::program_of(v).unwrap_or_default()));
            continue;
        }
        if !armed {
            println!("  {DIM}?{OFF} {}  {DIM}{v}{OFF}", m.name);
            checked += 1;
            continue;
        }
        let r = exec::run_declared(v);
        let ok = r.ok && m.expect.as_deref().map(|e| r.output.contains(e)).unwrap_or(true);
        println!("  {} {}", if ok { "\x1b[32mok\x1b[0m" } else { "\x1b[31mfalhou\x1b[0m" }, m.name);
        checked += 1;
    }
    if !refused.is_empty() {
        println!("\n  {WARN}{} recusadas: so programa permitido roda em verify{OFF}", refused.len());
        for (name, prog) in &refused {
            println!("    {name}  {DIM}{prog}{OFF}");
        }
    }
    if !armed && checked > 0 {
        println!("\n  {DIM}nada foi executado. bilro verify --run executa{OFF}");
    }
    println!();
}

fn cmd_bill(cwd: &Path) {
    let claude = home().join(".claude");
    let slug = cwd.to_string_lossy().replace('/', "-");
    let always = weigh::weigh_always_on(&claude);
    let mems: Vec<_> = weigh::weigh_memory(&claude.join("projects"))
        .into_iter()
        .filter(|m| m.project == slug)
        .collect();
    let agents = weigh::weigh_agents(&[claude.join("agents"), cwd.join(".claude").join("agents")]);
    let mcp = weigh::weigh_mcp(&home());
    let fixed: i64 = always.iter().map(|a| a.tokens).sum::<i64>()
        + mems.iter().map(|m| m.tokens).sum::<i64>()
        + agents.iter().map(|a| a.catalogue_tokens).sum::<i64>();
    println!("\n  custo fixo por request: {fixed} tokens\n");
    for a in &always {
        println!("  {:<34} {:>6}", a.name, a.tokens);
    }
    for m in &mems {
        println!("  {:<34} {:>6} {DIM}({} memorias){OFF}", m.project, m.tokens, m.entries);
    }
    println!("  {:<34} {:>6} {DIM}({} agentes){OFF}", "catalogo de agentes", agents.iter().map(|a| a.catalogue_tokens).sum::<i64>(), agents.len());
    let sem_tools = agents.iter().filter(|a| a.inherits_everything).count();
    if sem_tools > 0 {
        println!("\n  {WARN}{sem_tools} agentes sem tools: herdam o catalogo inteiro quando despachados{OFF}");
    }
    println!("  {DIM}{} servidores MCP{OFF}\n", mcp.len());
}

fn cmd_sessions() {
    let list = ledger::sessions();
    if list.is_empty() {
        return println!("\n  {DIM}nenhuma sessao registrada{OFF}\n");
    }
    println!("\n  {} sessoes\n", list.len());
    for s in list.iter().take(12) {
        let sum = ledger::summarize(&s.id);
        println!("  {:<40} {DIM}{} agentes, {} repetidos{OFF}", s.id, sum.dispatches, sum.repeats);
    }
    println!();
}


const WRAPPABLE: &[(&str, &[&str])] = &[
    ("git", &["log", "status", "diff", "show", "branch", "blame", "shortlog", "ls-files"]),
    ("ls", &[]),
    ("find", &[]),
    ("tree", &[]),
    ("env", &[]),
    ("printenv", &[]),
    ("npm", &["install", "ci", "ls", "audit", "outdated", "test"]),
    ("pnpm", &["install", "ls", "audit", "outdated", "test"]),
    ("yarn", &["install", "list", "audit", "test"]),
    ("pip", &["list", "install", "freeze"]),
    ("uv", &["pip", "sync", "lock"]),
    ("cargo", &["build", "test", "clippy", "check", "tree"]),
    ("jest", &[]),
    ("vitest", &[]),
    ("pytest", &[]),
    ("playwright", &[]),
    ("tsc", &[]),
    ("eslint", &[]),
    ("ruff", &[]),
    ("mypy", &[]),
    ("gradlew", &[]),
    ("kubectl", &["get", "describe", "logs", "top", "explain"]),
    ("docker", &["ps", "images", "logs", "stats"]),
];

const NEVER_WRAP: &[&str] = &[
    ">", "<", "|", "&&", "||", ";", "`", "$(", "&",
];

/// Decides whether a shell command is worth routing through the filter. A
/// compound command is left alone: its parts may mutate state, and merging the
/// output of several would misrepresent which one spoke. A single verbose
/// read-only command is the case worth compressing.
fn wrappable(command: &str) -> bool {
    let body = command.strip_prefix("cd ").and_then(|rest| rest.split_once("&&")).map(|(_, r)| r.trim()).unwrap_or(command);
    if NEVER_WRAP.iter().any(|m| body.contains(m)) {
        return false;
    }
    let mut parts = body.split_whitespace();
    let Some(program) = parts.next() else { return false };
    let program = program.rsplit('/').next().unwrap_or(program);
    let Some((_, subs)) = WRAPPABLE.iter().find(|(p, _)| *p == program) else { return false };
    if subs.is_empty() {
        return true;
    }
    parts.find(|a| !a.starts_with('-')).map(|sub| subs.contains(&sub)).unwrap_or(false)
}

/// Rewrites a Bash call to run through the filter, using the same PreToolUse
/// contract the harness offers. Printing nothing leaves the command untouched,
/// which is the answer for everything not clearly safe to compress.
fn hook_bash() {
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return;
    }
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
    if data["tool_name"].as_str() != Some("Bash") {
        return;
    }
    let command = data["tool_input"]["command"].as_str().unwrap_or("");
    if command.is_empty() || command.starts_with("bilro ") || command.contains("/bilro ") || !wrappable(command) {
        return;
    }
    let me = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "bilro".into());
    let updated = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "bilro filter",
            "updatedInput": { "command": format!("{me} filter {}", shell_quote(command)) }
        }
    });
    println!("{updated}");
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}


/// Runs a command, keeps its output in the sandbox index instead of the
/// conversation, and returns only the passages matching what was asked for.
fn cmd_run(argv: &[String]) {
    let split = argv.iter().position(|a| a == "--find");
    let (cmd_parts, queries) = match split {
        Some(i) => (&argv[..i], argv[i + 1..].to_vec()),
        None => (argv, Vec::new()),
    };
    let command = cmd_parts.join(" ");
    if command.is_empty() {
        return eprintln!("  uso: bilro run <comando> [--find <termo>...]");
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let r = sandbox::run(&command, None, &queries, Some(&cwd));
    println!(
        "\n  {}  {} trechos indexados, {DIM}{} tok ficaram fora do contexto{OFF}",
        if r.failed { "\x1b[31mfalhou\x1b[0m" } else { "ok" },
        r.chunks,
        r.withheld_tokens
    );
    for h in &r.hits {
        println!("\n  {DIM}{}{OFF}", h.query);
        for line in h.hit.body.lines().take(12) {
            println!("    {line}");
        }
    }
    println!();
}

/// Searches everything the sandbox has indexed, without re-running anything.
fn cmd_find(argv: &[String]) {
    let query = argv.join(" ");
    if query.is_empty() {
        return eprintln!("  uso: bilro find <termo>");
    }
    let Ok(conn) = sandbox::open_default() else { return eprintln!("  sem indice ainda") };
    let Ok(hits) = sandbox::search(&conn, &query, 8) else { return };
    if hits.is_empty() {
        return println!("\n  {DIM}nada encontrado para {query}{OFF}\n");
    }
    println!("\n  {} trechos\n", hits.len());
    for h in &hits {
        println!("  {DIM}{}{OFF}", h.label);
        for line in h.body.lines().take(8) {
            println!("    {line}");
        }
        println!();
    }
}

/// Injects the writing rules a session should follow, on every prompt, because
/// a rule stated once decays over a long conversation.
fn hook_prompt() {
    let mut raw = String::new();
    let _ = std::io::stdin().read_to_string(&mut raw);
    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&raw) {
        let prompt = data["prompt"].as_str().unwrap_or("");
        if !prompt.trim().is_empty() {
            let cwd = std::env::current_dir().unwrap_or_default();
            if let Ok(db) = journal::open_default() {
                let _ = journal::record(
                    &db,
                    "pedido",
                    prompt,
                    "",
                    data["session_id"].as_str().unwrap_or("desconhecida"),
                    &journal::project_of(&cwd),
                );
            }
        }
    }
    let Some(level) = style::own_level() else { return };
    let rules = style::ruleset(&level);
    if !rules.trim().is_empty() {
        println!("{rules}");
    }
}

fn cmd_install() {
    let binary = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return eprintln!("  nao achei o proprio binario: {e}"),
    };
    let link_dir = home().join(".local").join("bin");
    match install::install(&home(), &binary, Some(&link_dir)) {
        Err(e) => eprintln!("  falhou: {e}"),
        Ok(r) => {
            println!("\n  bilro instalado\n");
            println!("  binario   {}", r.binary.display());
            if let Some(l) = &r.linked {
                println!("  no PATH   {}", l.display());
            }
            for a in &r.added {
                println!("  \x1b[32m+{OFF} {a}");
            }
            for a in &r.replaced {
                println!("  {WARN}~{OFF} {a} {DIM}(caminho atualizado){OFF}");
            }
            for a in &r.already {
                println!("  {DIM}= {a} (ja estava){OFF}");
            }
            if let Some(m) = r.mcp {
                println!("  {DIM}mcp       servidor {m}{OFF}");
            }
            if let Some(b) = &r.backup {
                println!("\n  {DIM}settings anterior em {}{OFF}", b.display());
            }
            println!();
        }
    }
}

/// Runs a snippet from the command line, reading the code from stdin so no
/// quoting has to survive the shell twice.
fn cmd_exec(argv: &[String]) {
    let lang = argv.first().cloned().unwrap_or_else(|| "shell".into());
    if script::runtime_for(&lang).is_none() {
        return eprintln!("  linguagem nao suportada: {lang}. Disponiveis: {}", script::languages().join(", "));
    }
    let mut code = String::new();
    if std::io::stdin().read_to_string(&mut code).is_err() || code.trim().is_empty() {
        return eprintln!("  uso: echo '<codigo>' | bilro exec <linguagem>");
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    match script::run(&lang, &code, Some(&cwd), None) {
        Err(e) => eprintln!("  {e}"),
        Ok(r) => {
            print!("{}", r.output);
            if r.failed {
                eprintln!("\n  {WARN}o script terminou com erro{OFF}");
                std::process::exit(1);
            }
        }
    }
}

/// Answers "where were we". With a term it ranks; without one it simply shows
/// what happened here most recently, which is the question after a break.
fn cmd_recall(argv: &[String]) {
    let query = argv.join(" ");
    let cwd = std::env::current_dir().unwrap_or_default();
    let project = journal::project_of(&cwd);
    let Ok(db) = journal::open_default() else { return eprintln!("  sem diario ainda") };
    let events = if query.trim().is_empty() {
        journal::timeline(&db, Some(&project), 12)
    } else {
        journal::search(&db, &query, Some(&project), 10)
    };
    match events {
        Err(e) => eprintln!("  {e}"),
        Ok(list) if list.is_empty() => println!(
            "\n  {DIM}{}{OFF}\n",
            if query.trim().is_empty() { "nada registrado neste projeto ainda".into() } else { format!("nada sobre {query}") }
        ),
        Ok(list) => {
            println!("\n  {} eventos\n", list.len());
            for e in &list {
                let quando = idade(e.at);
                println!("  {:<8} {DIM}{quando}{OFF}  {}", e.kind, e.subject);
                for l in e.body.lines().take(3) {
                    println!("           {DIM}{l}{OFF}");
                }
            }
            println!();
        }
    }
}

fn idade(at: i64) -> String {
    let agora = ledger::now_ms();
    let min = (agora - at) / 60_000;
    if min < 60 {
        format!("{min}min")
    } else if min < 1440 {
        format!("{}h", min / 60)
    } else {
        format!("{}d", min / 1440)
    }
}

/// Fetches a page and shows only what was asked for, keeping the rest indexed.
fn cmd_fetch(argv: &[String]) {
    let qi = argv.iter().position(|a| a == "--find");
    let url = argv.first().cloned().unwrap_or_default();
    let queries: Vec<String> = match qi {
        Some(i) => argv[i + 1..].to_vec(),
        None => Vec::new(),
    };
    if url.is_empty() {
        return eprintln!("  uso: bilro fetch <url> [--find <termo>...]");
    }
    match web::fetch(&url) {
        Err(e) => eprintln!("  {e}"),
        Ok(page) => {
            println!(
                "\n  {}\n  {DIM}{} bytes de pagina viraram {} de texto{OFF}\n",
                page.title.clone().unwrap_or_else(|| page.url.clone()),
                page.bytes,
                page.text.len()
            );
            if queries.is_empty() {
                for l in page.text.lines().take(40) {
                    println!("  {l}");
                }
                return;
            }
            let Ok(conn) = sandbox::open_default() else {
                return eprintln!("  {WARN}sem indice: mostrando so o cabecalho{OFF}");
            };
            let label = page.title.clone().unwrap_or_else(|| page.url.clone());
            let chunks = sandbox::index(&conn, &label, &page.text, "web").unwrap_or(0);
            let mut achou = 0;
            for q in &queries {
                let hits = sandbox::search(&conn, q, 3).unwrap_or_default();
                if hits.is_empty() {
                    println!("  {DIM}{q}: nada nesta pagina{OFF}");
                    continue;
                }
                achou += hits.len();
                println!("  {DIM}{q}{OFF}");
                for h in hits {
                    for l in h.body.lines().take(10) {
                        println!("    {l}");
                    }
                }
            }
            if achou == 0 {
                println!(
                    "\n  {DIM}pagina guardada em {chunks} trechos. bilro find <outro termo> busca nela{OFF}"
                );
            }
        }
    }
}

fn usage() {
    println!(
        "\n  bilro 0.2.0\n\n\
         \x20   bilro filter <cmd>     roda comando e devolve so o que informa\n\
         \x20   bilro read <arq>       le arquivo comprimido (--outline so a estrutura)\n\
         \x20   bilro grep <padrao>    busca agrupada por arquivo, sem repeticao\n\
         \x20   bilro hook shadow      observador silencioso, para PostToolUse\n\
         \x20   bilro ready            ja da pra aposentar rtk, caveman e context-mode?\n\
         \x20   bilro bill             custo fixo de contexto por request\n\
         \x20   bilro verify [--run]   confere memorias que afirmam fato datado\n\
         \x20   bilro lint             link quebrado, memoria orfa, mais citada\n\
         \x20   bilro propose          memorias que valeria escrever\n\
         \x20   bilro sessions         agentes despachados por sessao\n\
         \x20   bilro run <cmd>        roda e indexa; so o trecho pedido volta\n\
         \x20   bilro find <termo>     busca no que ja foi indexado\n\
         \x20   bilro style [nivel]    regras de escrita da sessao\n\
         \x20   bilro install          registra os hooks e poe o binario no PATH\n\
         \x20   bilro exec <ling>      roda trecho de codigo (stdin), so o impresso volta\n\
         \x20   bilro mcp              servidor MCP por stdio\n"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]).to_vec();
    match args.first().map(|s| s.as_str()) {
        Some("hook") => match rest.first().map(|s| s.as_str()) {
            Some("shadow") => hook_shadow(),
            Some("session") => hook_session(&std::env::current_dir().unwrap_or_default()),
            Some("task") => hook_task(&std::env::current_dir().unwrap_or_default()),
            Some("bash") => hook_bash(),
            Some("prompt") => hook_prompt(),
            _ => {}
        },
        Some("filter") => std::process::exit(run_filtered(&rest)),
        Some("read") => cmd_read(&rest),
        Some("grep") => cmd_grep(&rest),
        Some("ready") => cmd_ready(),
        Some("lint") => cmd_lint(&std::env::current_dir().unwrap_or_default()),
        Some("verify") => cmd_verify(&std::env::current_dir().unwrap_or_default()),
        Some("propose") => cmd_propose(),
        Some("bill") => cmd_bill(&std::env::current_dir().unwrap_or_default()),
        Some("sessions") => cmd_sessions(),
        Some("install") => cmd_install(),
        Some("mcp") => mcp::serve(),
        Some("exec") => cmd_exec(&rest),
        Some("recall") => cmd_recall(&rest),
        Some("fetch") => cmd_fetch(&rest),
        Some("run") => cmd_run(&rest),
        Some("find") => cmd_find(&rest),
        Some("style") => println!("{}", style::ruleset(&rest.first().cloned().unwrap_or_else(style::read_level))),
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_comando_verboso_e_de_leitura_e_reescrito() {
        for c in [
            "git log --oneline -40",
            "git status",
            "ls -laR src",
            "npm install",
            "cargo test",
            "kubectl get pods",
            "cd /tmp && git log",
        ] {
            assert!(wrappable(c), "deveria filtrar: {c}");
        }
    }

    #[test]
    fn comando_que_muta_ou_e_composto_passa_intacto() {
        for c in [
            "git commit -m x",
            "git push origin main",
            "git checkout main",
            "rm -rf build",
            "npm run deploy",
            "npm publish",
            "kubectl apply -f x.yaml",
            "kubectl delete pod x",
            "docker rm -f c",
            "git log > /tmp/out.txt",
            "git status && git push",
            "cat file | head -5",
            "echo oi",
            "make deploy",
        ] {
            assert!(!wrappable(c), "nao deveria filtrar: {c}");
        }
    }

    #[test]
    fn saida_vazia_nunca_sai_calada() {
        let aviso = suppressed_notice("alpha\nbeta\ngamma", "").expect("deveria avisar");
        assert!(aviso.contains("3 linhas suprimidas"));
        assert!(!aviso.contains("nenhuma delas relatando falha"), "nao pode afirmar ausencia de falha");
        assert!(aviso.contains("so reconhece as que sabe nomear"));
    }

    #[test]
    fn saida_com_conteudo_nao_gera_aviso() {
        assert!(suppressed_notice("alpha\nbeta", "alpha").is_none());
    }

    #[test]
    fn comando_que_nao_imprimiu_nada_nao_inventa_aviso() {
        assert!(suppressed_notice("", "").is_none());
        assert!(suppressed_notice("   \n  ", "").is_none());
    }
}
