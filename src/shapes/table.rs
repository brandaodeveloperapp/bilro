use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::learn::is_severe;
use crate::shapes::contract::{Compressed, Shape};

static HEALTHY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(Up|Running|Ready|Active|Completed|Succeeded|Bound|healthy)\b").unwrap()
});
static RATIO: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d+)/(\d+)$").unwrap());
static STATUS_STATE_TOKEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(STATUS|STATE)$").unwrap());
static STATE_COL_TOKEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^(STATUS|STATE|READY)$").unwrap());
static HEADER_WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^[A-Z0-9][A-Z0-9 _/.%-]*$").unwrap());
static SPLIT_COLS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s{2,}").unwrap());

/// Stable identifier for this shape.
pub fn name() -> &'static str {
    "table"
}

fn column_is_healthy(token: &str, value: &str) -> bool {
    if let Some(caps) = RATIO.captures(value) {
        return caps[1] == caps[2];
    }
    if STATUS_STATE_TOKEN.is_match(token) {
        return HEALTHY.is_match(value);
    }
    true
}

fn split_columns(line: &str) -> Vec<String> {
    SPLIT_COLS.split(line).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

fn is_header_line(line: &str) -> bool {
    let cols = split_columns(line.trim());
    cols.len() >= 2 && cols.iter().all(|c| HEADER_WORD.is_match(c))
}

fn char_index_of(haystack: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(haystack.len()));
    }
    if from >= haystack.len() {
        return None;
    }
    haystack[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn column_starts(line: &str, tokens: &[String]) -> Vec<usize> {
    let line_chars: Vec<char> = line.chars().collect();
    let mut starts = Vec::with_capacity(tokens.len());
    let mut from = 0usize;
    for t in tokens {
        let t_chars: Vec<char> = t.chars().collect();
        let idx = char_index_of(&line_chars, &t_chars, from).unwrap_or(from);
        starts.push(idx);
        from = idx + t_chars.len();
    }
    starts
}

fn find_header(lines: &[&str]) -> Option<usize> {
    let limit = lines.len().min(6);
    (0..limit).find(|&i| is_header_line(lines[i]))
}

fn boundary_ok(chars: &[char], starts: &[usize]) -> bool {
    for &start in starts.iter().skip(1) {
        if chars.len() <= start {
            continue;
        }
        match start.checked_sub(1) {
            Some(p) if chars[p] == ' ' => {}
            _ => return false,
        }
    }
    true
}

fn slice_column(chars: &[char], start: usize, end: Option<usize>) -> String {
    let len = chars.len();
    let s = start.min(len);
    let e = end.map_or(len, |e| e.min(len));
    let raw: String = if e > s { chars[s..e].iter().collect() } else { String::new() };
    raw.trim().to_string()
}

/// Confidence 0..1 that lines form a columnar table with an all-caps header.
pub fn detect(lines: &[&str]) -> f64 {
    let Some(idx) = find_header(lines) else { return 0.0 };
    let header = lines[idx];
    let tokens = split_columns(header.trim());
    let starts = column_starts(header, &tokens);
    let data: Vec<&&str> = lines[idx + 1..].iter().filter(|l| !l.trim().is_empty()).collect();
    if data.is_empty() {
        return 0.5;
    }
    let mut ok = 0usize;
    for l in &data {
        let chars: Vec<char> = l.chars().collect();
        if boundary_ok(&chars, &starts) {
            ok += 1;
        }
    }
    (0.35 + (ok as f64 / data.len() as f64) * 0.65).min(1.0)
}

struct Group {
    slot: usize,
    names: Vec<String>,
}

/// Drops columns whose value never varies across rows, collapses padding to a
/// single space, and leaves any row whose status column is not a healthy one
/// (or whose line trips `is_severe`) untouched and complete.
pub fn compress(lines: &[&str]) -> Compressed {
    let Some(idx) = find_header(lines) else { return Compressed::unchanged(lines) };
    let original = lines.join("\n");

    let before = &lines[..idx];
    let header = lines[idx];
    let tokens = split_columns(header.trim());
    let starts = column_starts(header, &tokens);
    let ends: Vec<Option<usize>> =
        (0..tokens.len()).map(|c| if c + 1 < starts.len() { Some(starts[c + 1]) } else { None }).collect();

    let mut rows: Vec<&str> = Vec::new();
    let mut after: &[&str] = &[];
    let mut i = idx + 1;
    while i < lines.len() {
        let line = lines[i];
        let chars: Vec<char> = line.chars().collect();
        if line.trim().is_empty() || (!rows.is_empty() && !boundary_ok(&chars, &starts)) {
            after = &lines[i..];
            break;
        }
        rows.push(line);
        i += 1;
    }

    let state_cols: Vec<usize> =
        tokens.iter().enumerate().filter(|(_, t)| STATE_COL_TOKEN.is_match(t)).map(|(c, _)| c).collect();

    let row_chars: Vec<Vec<char>> = rows.iter().map(|l| l.chars().collect()).collect();

    let full_survive: Vec<bool> = rows
        .iter()
        .enumerate()
        .map(|(r, line)| {
            if is_severe(line) {
                return true;
            }
            if state_cols.is_empty() {
                return false;
            }
            state_cols
                .iter()
                .any(|&c| !column_is_healthy(&tokens[c], &slice_column(&row_chars[r], starts[c], ends[c])))
        })
        .collect();

    let healthy_idx: Vec<usize> = (0..rows.len()).filter(|&r| !full_survive[r]).collect();

    let constant_value: Vec<Option<String>> = (0..tokens.len())
        .map(|c| {
            if healthy_idx.len() < 3 {
                return None;
            }
            let mut set = HashSet::new();
            for &r in &healthy_idx {
                set.insert(slice_column(&row_chars[r], starts[c], ends[c]));
            }
            if set.len() == 1 {
                set.into_iter().next()
            } else {
                None
            }
        })
        .collect();

    let mut surviving_cols: Vec<usize> = (0..tokens.len()).filter(|&c| constant_value[c].is_none()).collect();
    if surviving_cols.is_empty() {
        surviving_cols = vec![0];
    }
    let dropped_cols: Vec<usize> = (0..tokens.len()).filter(|c| !surviving_cols.contains(c)).collect();

    let format_note_item = |c: usize| format!("{}={}", tokens[c], constant_value[c].as_deref().unwrap_or(""));
    let constant_note = dropped_cols.iter().map(|&c| format_note_item(c)).collect::<Vec<_>>().join(" ");

    let out_header = {
        let head = surviving_cols.iter().map(|&c| tokens[c].as_str()).collect::<Vec<_>>().join(" ");
        if constant_note.is_empty() { head } else { format!("{head}   [iguais em todas: {constant_note}]") }
    };

    let reduce = |chars: &[char]| -> String {
        surviving_cols.iter().map(|&c| slice_column(chars, starts[c], ends[c])).collect::<Vec<_>>().join(" ")
    };

    let rest: Vec<usize> = surviving_cols[1..].to_vec();

    let mut out_rows: Vec<Option<String>> = Vec::new();
    let mut groups: HashMap<String, Group> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for r in 0..rows.len() {
        if full_survive[r] {
            out_rows.push(Some(rows[r].to_string()));
            continue;
        }
        let chars = &row_chars[r];
        let key = rest.iter().map(|&c| slice_column(chars, starts[c], ends[c])).collect::<Vec<_>>().join(" ");
        let name = slice_column(chars, starts[surviving_cols[0]], ends[surviving_cols[0]]);

        if !groups.contains_key(&key) {
            order.push(key.clone());
            let slot = out_rows.len();
            out_rows.push(None);
            groups.insert(key.clone(), Group { slot, names: Vec::new() });
        }
        let g = groups.get_mut(&key).unwrap();
        g.names.push(name);
        if g.names.len() == 1 {
            out_rows[g.slot] = Some(reduce(chars));
        }
    }

    for key in &order {
        let g = groups.get(key).unwrap();
        if g.names.len() < 4 {
            for (offset, n) in g.names[1..].iter().enumerate() {
                out_rows.insert(g.slot + 1 + offset, Some(format!("{n} {key}")));
            }
        } else {
            out_rows[g.slot] = Some(format!("{}x {}  {}", g.names.len(), key, g.names.join(" ")));
        }
    }

    let out_rows: Vec<String> = out_rows.into_iter().map(Option::unwrap_or_default).collect();

    let mut text_lines: Vec<String> = Vec::new();
    text_lines.extend(before.iter().map(|s| (*s).to_string()));
    text_lines.push(out_header);
    text_lines.extend(out_rows);
    text_lines.extend(after.iter().map(|s| (*s).to_string()));
    let text = text_lines.join("\n");

    if text.len() >= original.len() {
        return Compressed::unchanged(lines);
    }

    let note = if dropped_cols.is_empty() {
        "padding colapsado".to_string()
    } else {
        format!(
            "colunas sem variacao removidas ({}); padding colapsado",
            dropped_cols.iter().map(|&c| format_note_item(c)).collect::<Vec<_>>().join(", ")
        )
    };

    Compressed { text, dropped: dropped_cols.len(), note }
}

/// Assembles the `table` shape for registration alongside the other shapes.
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

    #[test]
    fn nome_estavel() {
        assert_eq!(name(), "table");
    }

    #[test]
    fn detecta_docker_ps_real_como_tabela() {
        let raw = fixture("docker-ps.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_docker_ps_com_container_up_e_exited_misturados() {
        let raw = fixture("docker-ps-mixed.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_docker_images_real_ignorando_linha_de_warning_antes_do_header() {
        let raw = fixture("docker-images.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn nao_detecta_diff_unificado_como_tabela() {
        let raw = fixture("git-diff-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn nao_detecta_git_status_como_tabela() {
        let raw = fixture("git-status-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn nao_detecta_git_log_oneline_como_tabela() {
        let raw = fixture("git-log.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn texto_sem_header_nao_e_tabela() {
        assert_eq!(detect(&["algum texto qualquer", "outra linha solta"]), 0.0);
    }

    #[test]
    fn comprime_docker_images_real_remove_coluna_extra_constante_e_colapsa_padding() {
        let raw = fixture("docker-images.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.len() < raw.len());
        let reduction = 1.0 - (r.text.len() as f64 / raw.len() as f64);
        assert!(reduction > 0.3, "esperava corte real > 30%, obteve {:.1}%", reduction * 100.0);
        assert!(r.note.contains("EXTRA=U"));
        let extra_word = Regex::new(r"\bEXTRA\b").unwrap();
        assert_eq!(extra_word.find_iter(&r.text).count(), 1);
        for line in r.text.split('\n').skip(2) {
            assert!(!extra_word.is_match(line));
        }
    }

    #[test]
    fn comprime_docker_ps_misturado_linha_up_encolhe_linhas_exited_sobrevivem_inteiras() {
        let raw = fixture("docker-ps-mixed.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.len() < raw.len());

        let exited_lines: Vec<&str> = lines.iter().copied().filter(|l| l.contains("Exited")).collect();
        for original in exited_lines {
            assert!(r.text.contains(original), "linha unhealthy foi alterada: {original}");
        }

        let up_line_out = r.text.split('\n').find(|l| l.contains("bilro-fixture-tmp"));
        assert!(up_line_out.is_some(), "linha do container saudavel sumiu");
        let up_line_in = lines.iter().find(|l| l.contains("bilro-fixture-tmp")).unwrap();
        assert!(up_line_out.unwrap().len() < up_line_in.len(), "linha saudavel deveria ter encolhido");
    }

    #[test]
    fn linha_is_severe_sobrevive_mesmo_sem_coluna_de_status_reconhecida() {
        let lines = ["NAME  VALUE", "a     ok", "b     connection refused", "c     ok"];
        let r = compress(&lines);
        assert!(r.text.contains("connection refused"));
        assert!(r.text.split('\n').any(|l| l == lines[2]));
    }

    #[test]
    fn linha_com_status_nao_saudavel_sobrevive_inteira_mesmo_sem_is_severe() {
        let lines = ["NAME  STATUS", "pod-a Running", "pod-b CrashLoopBackOff", "pod-c Running"];
        let r = compress(&lines);
        assert!(r.text.split('\n').any(|l| l == "pod-b CrashLoopBackOff"));
    }

    #[test]
    fn sem_header_reconhecivel_compress_devolve_entrada_intacta() {
        let lines = ["so texto", "mais texto", "linha final"];
        let r = compress(&lines);
        assert_eq!(r.text, lines.join("\n"));
        assert_eq!(r.dropped, 0);
        assert_eq!(r.note, "");
    }

    #[test]
    fn ordem_das_linhas_sobreviventes_nunca_e_alterada() {
        let raw = fixture("docker-ps-mixed.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let names_in: Vec<&str> = lines[1..]
            .iter()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.split_whitespace().last().unwrap())
            .collect();
        let names_out: Vec<&str> = r
            .text
            .split('\n')
            .skip(1)
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.split_whitespace().last().unwrap())
            .collect();
        assert_eq!(names_out, names_in);
    }
}
