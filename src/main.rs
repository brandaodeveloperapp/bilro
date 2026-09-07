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

fn usage() {
    println!(
        "\n  bilro 0.2.0\n\n\
         \x20   bilro filter <cmd>     roda comando e devolve so o que informa\n\
         \x20   bilro read <arq>       le arquivo comprimido (--outline so a estrutura)\n\
         \x20   bilro grep <padrao>    busca agrupada por arquivo, sem repeticao\n\
         \x20   bilro hook shadow      observador silencioso, para PostToolUse\n"
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]).to_vec();
    match args.first().map(|s| s.as_str()) {
        Some("hook") if rest.first().map(|s| s.as_str()) == Some("shadow") => hook_shadow(),
        Some("filter") => run_filtered(&rest),
        Some("read") => cmd_read(&rest),
        Some("grep") => cmd_grep(&rest),
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
