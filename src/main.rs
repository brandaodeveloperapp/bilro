mod exec;
mod filters;
mod graph;
mod grep;
mod ledger;
mod learn;
mod memory;
mod propose;
mod read;
mod ready;
mod sandbox;
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
    let clipped = if output.len() > MAX_OBSERVED { &output[..MAX_OBSERVED] } else { output };
    if let Ok(mut db) = learn::open(&learn_db()) {
        let _ = learn::observe(&mut db, command, clipped);
    }
}

fn run_filtered(argv: &[String]) {
    if argv.is_empty() {
        return eprintln!("  uso: bilro filter <comando>");
    }
    let command = argv.join(" ");
    let out = Command::new("sh").arg("-c").arg(&command).output();
    let Ok(out) = out else { return eprintln!("  falhou ao executar") };
    let mut raw = String::from_utf8_lossy(&out.stdout).to_string();
    raw.push_str(&String::from_utf8_lossy(&out.stderr));

    let before = raw.len();
    let (text, note) = squeeze(&command, &raw);
    if let Ok(mut db) = learn::open(&learn_db()) {
        let _ = learn::observe(&mut db, &command, &raw);
    }
    if let Some(msg) = suppressed_notice(&raw, &text) {
        return eprintln!("  \x1b[2m{msg}\x1b[0m");
    }
    println!("{text}");
    if before > text.len() {
        let pct = 100 - text.len() * 100 / before.max(1);
        eprintln!("\n  \x1b[2m{pct}% menor{}\x1b[0m", if note.is_empty() { String::new() } else { format!(" ({note})") });
    }
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
        "identico ao que este comando ja imprimiu antes: {lines} linhas suprimidas, nenhuma delas relatando falha"
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
            println!("{}", r.text);
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
    let r = grep::compress(&raw);
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
         \x20   bilro sessions         agentes despachados por sessao\n"
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
            _ => {}
        },
        Some("filter") => run_filtered(&rest),
        Some("read") => cmd_read(&rest),
        Some("grep") => cmd_grep(&rest),
        Some("ready") => cmd_ready(),
        Some("lint") => cmd_lint(&std::env::current_dir().unwrap_or_default()),
        Some("verify") => cmd_verify(&std::env::current_dir().unwrap_or_default()),
        Some("propose") => cmd_propose(),
        Some("bill") => cmd_bill(&std::env::current_dir().unwrap_or_default()),
        Some("sessions") => cmd_sessions(),
        _ => usage(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saida_vazia_nunca_sai_calada() {
        let aviso = suppressed_notice("alpha\nbeta\ngamma", "").expect("deveria avisar");
        assert!(aviso.contains("3 linhas suprimidas"));
        assert!(aviso.contains("nenhuma delas relatando falha"));
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
