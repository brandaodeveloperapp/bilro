use once_cell::sync::Lazy;
use regex::Regex;

use crate::learn::is_severe;
use crate::shapes::contract::{Compressed, Shape};

static FILE_HEADER: Lazy<Regex> = Lazy::new(|| Regex::new(r"^diff --(git|cc) ").unwrap());
static HUNK: Lazy<Regex> = Lazy::new(|| Regex::new(r"^@@[@ ]").unwrap());
static META: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^(old mode|new mode|deleted file mode|new file mode|similarity index|dissimilarity index|rename from|rename to|copy from|copy to|Binary files|index |--- |\+\+\+ )",
    )
    .unwrap()
});
static CHANGE_LINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[+-]").unwrap());
static STATUS_ENTRY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^(modified|deleted|new file|renamed|copied|both modified|both added|added by us|deleted by us|deleted by them|added by them):\s*(.+)$",
    )
    .unwrap()
});

const BUDGET_CHARS: usize = 10_000;

fn status_section(line: &str) -> Option<&'static str> {
    match line {
        "Changes to be committed:" => Some("staged"),
        "Changes not staged for commit:" => Some("unstaged"),
        "Untracked files:" => Some("untracked"),
        "Unmerged paths:" => Some("conflict"),
        _ => None,
    }
}

/// Stable identifier for this shape.
pub fn name() -> &'static str {
    "diff"
}

struct Structure {
    file_headers: usize,
    hunks: usize,
    plus_minus: usize,
    status_markers: usize,
}

fn count_structure(lines: &[&str]) -> Structure {
    let mut s = Structure { file_headers: 0, hunks: 0, plus_minus: 0, status_markers: 0 };
    for &l in lines {
        let trimmed = l.trim();
        if FILE_HEADER.is_match(l) {
            s.file_headers += 1;
        } else if HUNK.is_match(l) {
            s.hunks += 1;
        } else if CHANGE_LINE.is_match(l) && !META.is_match(l) {
            s.plus_minus += 1;
        } else if STATUS_ENTRY.is_match(trimmed) {
            s.status_markers += 1;
        } else if status_section(trimmed).is_some() {
            s.status_markers += 2;
        }
    }
    s
}

/// Confidence 0..1 that lines are a unified diff or a git-status change list.
pub fn detect(lines: &[&str]) -> f64 {
    let s = count_structure(lines);
    if s.file_headers > 0 || s.hunks > 0 {
        return (0.6 + s.hunks.min(5) as f64 * 0.06 + s.plus_minus.min(10) as f64 * 0.01).min(1.0);
    }
    if s.status_markers >= 2 {
        return (0.65 + s.status_markers as f64 * 0.03).min(1.0);
    }
    0.0
}

fn flush_pending(out: &mut Vec<String>, pending_drop: &mut usize, dropped_total: &mut usize) {
    if *pending_drop == 0 {
        return;
    }
    let plural = if *pending_drop > 1 { "s" } else { "" };
    out.push(format!("      \u{2026} {pending_drop} linha{plural} de contexto omitida{plural}"));
    *dropped_total += *pending_drop;
    *pending_drop = 0;
}

fn compress_unified_diff(lines: &[&str]) -> Compressed {
    let mut out: Vec<String> = Vec::new();
    let mut file_block_starts: Vec<usize> = Vec::new();
    let mut in_hunk = false;
    let mut pending_drop: usize = 0;
    let mut dropped_total: usize = 0;
    let mut file_count: usize = 0;
    let mut hunk_count: usize = 0;

    for &line in lines {
        if FILE_HEADER.is_match(line) {
            flush_pending(&mut out, &mut pending_drop, &mut dropped_total);
            file_block_starts.push(out.len());
            file_count += 1;
            in_hunk = false;
            out.push(line.to_string());
            continue;
        }
        if !in_hunk && META.is_match(line) {
            out.push(line.to_string());
            continue;
        }
        if HUNK.is_match(line) {
            flush_pending(&mut out, &mut pending_drop, &mut dropped_total);
            hunk_count += 1;
            in_hunk = true;
            out.push(line.to_string());
            continue;
        }
        if in_hunk {
            if CHANGE_LINE.is_match(line) || is_severe(line) {
                flush_pending(&mut out, &mut pending_drop, &mut dropped_total);
                out.push(line.to_string());
            } else {
                pending_drop += 1;
            }
            continue;
        }
        out.push(line.to_string());
    }
    flush_pending(&mut out, &mut pending_drop, &mut dropped_total);

    let mut text = out.join("\n");
    let mut note =
        format!("{dropped_total} linhas de contexto removidas em {hunk_count} hunks ({file_count} arquivos)");

    if text.len() > BUDGET_CHARS && file_block_starts.len() > 1 {
        let mut shown = file_block_starts.len();
        for k in (1..file_block_starts.len()).rev() {
            let slice = out[..file_block_starts[k]].join("\n");
            if slice.len() <= BUDGET_CHARS {
                text = slice;
                shown = k;
                break;
            }
            shown = 0;
        }
        let omitted = file_count - shown;
        if omitted > 0 {
            note.push_str(&format!(
                " | orcamento estourado: mostrando {shown} de {file_count} arquivos, {omitted} arquivos omitidos por corte de orcamento"
            ));
        }
    }

    Compressed { text, dropped: dropped_total, note }
}

fn flush_section(
    out: &mut Vec<String>,
    section: Option<&str>,
    bucket: &mut Vec<(String, Vec<String>)>,
    raw_entries: &mut Vec<String>,
) {
    let Some(sec) = section else { return };
    if sec == "conflict" {
        for e in raw_entries.iter() {
            out.push(format!("  {e}"));
        }
    } else {
        for (t, entries) in bucket.iter() {
            out.push(format!("  {} ({}): {}", t, entries.len(), entries.join(", ")));
        }
    }
    bucket.clear();
    raw_entries.clear();
}

fn compress_status(lines: &[&str]) -> Compressed {
    let mut out: Vec<String> = Vec::new();
    let mut section: Option<&'static str> = None;
    let mut bucket: Vec<(String, Vec<String>)> = Vec::new();
    let mut raw_entries: Vec<String> = Vec::new();
    let mut dropped_total: usize = 0;

    for &line in lines {
        let trimmed = line.trim();
        if let Some(sec) = status_section(trimmed) {
            flush_section(&mut out, section, &mut bucket, &mut raw_entries);
            section = Some(sec);
            out.push(trimmed.to_string());
            continue;
        }
        if trimmed.is_empty() {
            dropped_total += 1;
            continue;
        }
        if trimmed.starts_with('(') {
            dropped_total += 1;
            continue;
        }
        if section.is_some() && line.starts_with('\t') {
            if section == Some("conflict") {
                raw_entries.push(trimmed.to_string());
                continue;
            }
            let (ty, path) = if let Some(caps) = STATUS_ENTRY.captures(trimmed) {
                (caps[1].to_string(), caps[2].to_string())
            } else {
                ("untracked".to_string(), trimmed.to_string())
            };
            if let Some(entry) = bucket.iter_mut().find(|(t, _)| *t == ty) {
                entry.1.push(path);
            } else {
                bucket.push((ty, vec![path]));
            }
            continue;
        }
        flush_section(&mut out, section, &mut bucket, &mut raw_entries);
        section = None;
        out.push(line.to_string());
    }
    flush_section(&mut out, section, &mut bucket, &mut raw_entries);

    Compressed {
        text: out.join("\n"),
        dropped: dropped_total,
        note: format!("{dropped_total} linhas de aviso/em branco removidas"),
    }
}

/// Keeps every file header and every +/- line; drops unchanged context lines
/// (or collapses git-status hint lines), grouping status entries by change
/// type. A merge conflict or `is_severe` line is never among what is dropped.
/// If the result still overruns the budget, whole file blocks are cut from
/// the end and the note says exactly how many files and hunks were left out.
pub fn compress(lines: &[&str]) -> Compressed {
    let original = lines.join("\n");
    let s = count_structure(lines);
    let r =
        if s.file_headers > 0 || s.hunks > 0 { compress_unified_diff(lines) } else { compress_status(lines) };
    if r.text.len() >= original.len() {
        return Compressed::unchanged(lines);
    }
    r
}

/// Assembles the `diff` shape for registration alongside the other shapes.
pub fn shape() -> Shape {
    Shape { name: name(), detect, compress }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shapes::contract::MIN_CONFIDENCE;

    fn fixture(name: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
        std::fs::read_to_string(path).unwrap()
    }

    fn is_structural(l: &str) -> bool {
        l.starts_with("diff --git ") || ((l.starts_with('+') || l.starts_with('-')) && !(l.starts_with("++") || l.starts_with("--")))
    }

    #[test]
    fn nome_estavel() {
        assert_eq!(name(), "diff");
    }

    #[test]
    fn detecta_git_diff_real_multi_arquivo_como_diff() {
        let raw = fixture("git-diff-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_git_show_p_real_como_diff() {
        let raw = fixture("git-show-code.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_diff_cc_de_merge_conflict_real_como_diff() {
        let raw = fixture("git-diff-conflict.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_git_status_real_como_diff() {
        let raw = fixture("git-status-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_git_status_de_merge_conflito_real() {
        let raw = fixture("git-status-conflict.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn nao_detecta_tabela_docker_ps_como_diff() {
        let raw = fixture("docker-ps.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn nao_detecta_tabela_docker_images_como_diff() {
        let raw = fixture("docker-images.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn texto_solto_sem_marcador_de_diff_ou_status_confidence_zero() {
        assert_eq!(detect(&["so um texto qualquer", "sem estrutura nenhuma"]), 0.0);
    }

    #[test]
    fn comprime_git_show_p_real_corta_contexto_mantem_marcadores_e_headers() {
        let raw = fixture("git-show-code.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.len() < raw.len());
        let reduction = 1.0 - (r.text.len() as f64 / raw.len() as f64);
        assert!(reduction > 0.1, "esperava corte real > 10%, obteve {:.1}%", reduction * 100.0);
        assert!(r.dropped > 0);

        for &l in &lines {
            if is_structural(l) {
                assert!(r.text.contains(l), "linha estrutural perdida: {l}");
            }
        }
    }

    #[test]
    fn diff_grande_de_verdade_estoura_orcamento_e_avisa_explicitamente() {
        let raw = fixture("git-diff-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let total_files = lines.iter().filter(|l| l.starts_with("diff --git ")).count();
        assert!(total_files > 1);
        assert!(r.note.contains("orcamento estourado"));
        assert!(r.note.contains("arquivos omitidos"));

        let shown_files = r.text.split('\n').filter(|l| l.starts_with("diff --git ")).count();
        assert!(shown_files < total_files, "deveria ter cortado pelo menos um arquivo");
        assert!(shown_files > 0, "nunca deveria zerar tudo silenciosamente");
    }

    #[test]
    fn conflito_de_merge_diff_cc_marcadores_sobrevivem_ao_corte() {
        let raw = fixture("git-diff-conflict.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.contains("<<<<<<< HEAD"));
        assert!(r.text.contains("======="));
        assert!(r.text.contains(">>>>>>> b2"));
    }

    #[test]
    fn conflito_de_merge_git_status_both_modified_sobrevive_por_completo() {
        let raw = fixture("git-status-conflict.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let re = Regex::new(r"both modified:\s+f\.txt").unwrap();
        assert!(re.is_match(&r.text));
    }

    #[test]
    fn git_status_real_agrupa_por_tipo_com_contagem_e_derruba_boilerplate() {
        let raw = fixture("git-status-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.len() < raw.len());
        let hint = Regex::new(r#"(?m)^\s+\(use "git"#).unwrap();
        assert!(!hint.is_match(&r.text));

        let modified_count = lines.iter().filter(|l| l.trim().starts_with("modified:")).count();
        let expect = Regex::new(&format!(r"modified \({modified_count}\):")).unwrap();
        assert!(expect.is_match(&r.text));

        let entry = Regex::new(r"^\t(?:modified|deleted|new file|renamed):\s*(.+)$").unwrap();
        for &l in &lines {
            if let Some(caps) = entry.captures(l) {
                let path = caps[1].trim();
                assert!(r.text.contains(path), "arquivo sumiu do agrupamento: {path}");
            }
        }
    }

    #[test]
    fn linha_de_contexto_com_is_severe_nunca_e_cortada_mesmo_dentro_de_hunk() {
        let lines = [
            "diff --git a/x.txt b/x.txt",
            "index 111..222 100644",
            "--- a/x.txt",
            "+++ b/x.txt",
            "@@ -1,6 +1,6 @@",
            " linha de contexto comum",
            " linha de contexto comum tambem",
            " connection refused ao conectar no banco",
            "-valor antigo",
            "+valor novo",
            " mais uma linha de contexto sem importancia",
        ];
        let r = compress(&lines);
        assert!(r.text.contains(" connection refused ao conectar no banco"));
    }

    #[test]
    fn shape_nao_muda_nem_reordena_linhas_mais_menos_e_de_header() {
        let raw = fixture("git-show-code.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let change_lines_in: Vec<&str> = lines.iter().copied().filter(|l| is_structural(l)).collect();
        let text_owned = r.text.clone();
        let change_lines_out: Vec<&str> = text_owned.split('\n').filter(|l| is_structural(l)).collect();
        assert_eq!(change_lines_out, change_lines_in);
    }

    #[test]
    fn sem_estrutura_de_diff_ou_status_compress_devolve_entrada_intacta() {
        let lines = ["texto qualquer", "sem hunk sem status"];
        let r = compress(&lines);
        assert_eq!(r.text, lines.join("\n"));
        assert_eq!(r.dropped, 0);
        assert_eq!(r.note, "");
    }
}
