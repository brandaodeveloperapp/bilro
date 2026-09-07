use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;

use crate::learn::is_severe_text;
use crate::shapes::contract::{Compressed, Shape};

static COLON_LOC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^(?P<path>[^\s:()][^:()]*[./][^\s:()]*):(?P<line>\d+):(?P<col>\d+):?\s*[-–]?\s*(?P<msg>.+)$",
    )
    .unwrap()
});
static MYPY_LOC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)^(?P<path>[^\s:()][^:()]*[./][^\s:()]*):(?P<line>\d+):\s*(?:error|warning|note):\s*(?P<msg>.+)$",
    )
    .unwrap()
});
static PAREN_LOC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(?P<path>[^\s():]+)\((?P<line>\d+),(?P<col>\d+)\):\s*(?P<msg>.+)$").unwrap()
});
static HEADER_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\.?[./][^\s:]*\.[a-zA-Z]{1,10}$|^[^\s:]+\.[a-zA-Z]{1,10}$").unwrap()
});
static INDENT_LOC: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s*(?P<line>\d+):(?P<col>\d+)\s+(?:Warning|Error)\b:?\s*(?P<msg>.+)$").unwrap()
});
static SUMMARY_LINE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^(Found \d+ error|\d+ problems?\s*\(|\[\*\]|BUILD (SUCCESS|FAILURE))").unwrap()
});
static CODE_IN_MSG: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b([A-Z]{1,10}\d{3,5})\b").unwrap());
static TRAILING_CODE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([\w-]+(?:/[\w-]+)+)\s*$").unwrap());
static BRACKET_CODE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\*?\]?\s*([\w-]+(?:/[\w-]+)*)\s*$").unwrap());

fn looks_like_stack_frame(line: &str) -> bool {
    line.trim_start().starts_with("at ")
}

struct LocMatch {
    path: String,
    line: String,
    col: Option<String>,
    msg: String,
}

fn match_any(line: &str) -> Option<LocMatch> {
    if looks_like_stack_frame(line) {
        return None;
    }
    if let Some(c) = COLON_LOC.captures(line) {
        return Some(LocMatch {
            path: c["path"].to_string(),
            line: c["line"].to_string(),
            col: Some(c["col"].to_string()),
            msg: c["msg"].to_string(),
        });
    }
    if let Some(c) = MYPY_LOC.captures(line) {
        return Some(LocMatch {
            path: c["path"].to_string(),
            line: c["line"].to_string(),
            col: None,
            msg: c["msg"].to_string(),
        });
    }
    if let Some(c) = PAREN_LOC.captures(line) {
        return Some(LocMatch {
            path: c["path"].to_string(),
            line: c["line"].to_string(),
            col: Some(c["col"].to_string()),
            msg: c["msg"].to_string(),
        });
    }
    None
}

fn rule_code_of(line: &str, msg: &str) -> String {
    if let Some(c) = CODE_IN_MSG.captures(msg) {
        return c[1].to_string();
    }
    if let Some(c) = TRAILING_CODE.captures(msg) {
        return c[1].to_string();
    }
    if let Some(c) = BRACKET_CODE.captures(line) {
        return c[1].to_string();
    }
    msg.chars().take(40).collect::<String>().trim().to_string()
}

struct Entry {
    loc: String,
    code: String,
    severe: bool,
    header_index: Option<usize>,
}

struct Parsed {
    entries: HashMap<usize, Entry>,
    order: Vec<usize>,
    header_indices: HashSet<usize>,
}

fn parse_entries(lines: &[&str]) -> Parsed {
    let mut entries = HashMap::new();
    let mut order = Vec::new();
    let mut header_indices = HashSet::new();
    let mut current_header: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        if let Some(m) = match_any(line) {
            let loc = match &m.col {
                Some(col) => format!("{}:{}:{}", m.path, m.line, col),
                None => format!("{}:{}", m.path, m.line),
            };
            let code = rule_code_of(line, &m.msg);
            entries.insert(
                i,
                Entry { loc, code, severe: is_severe_text(line), header_index: None },
            );
            order.push(i);
            continue;
        }
        if let Some(c) = INDENT_LOC.captures(line) {
            let path = match current_header {
                Some(h) => lines[h].trim().to_string(),
                None => "?".to_string(),
            };
            let loc = format!("{}:{}:{}", path, &c["line"], &c["col"]);
            let code = rule_code_of(line, &c["msg"]);
            entries.insert(
                i,
                Entry { loc, code, severe: is_severe_text(line), header_index: current_header },
            );
            order.push(i);
            continue;
        }
        if HEADER_PATH.is_match(line.trim()) {
            current_header = Some(i);
            header_indices.insert(i);
        }
    }

    Parsed { entries, order, header_indices }
}

/// Stable identifier for this shape.
pub fn name() -> &'static str {
    "diagnostics"
}

/// Confidence that `lines` is a static checker's own output: tsc, mypy, ruff,
/// rubocop, phpstan, golangci-lint, eslint, cargo, mvn, gradlew, next build.
/// `path:line:col: message` repeated, sometimes with the path on its own
/// header line above indented `line:col` entries, is the tell.
pub fn detect(lines: &[&str]) -> f64 {
    let body: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    if body.is_empty() {
        return 0.0;
    }

    let mut loc_hits = 0usize;
    let mut indent_hits = 0usize;
    let mut header_count = 0usize;
    let mut summary_hit = false;
    for line in &body {
        if match_any(line).is_some() {
            loc_hits += 1;
        } else if INDENT_LOC.is_match(line) {
            indent_hits += 1;
        } else if HEADER_PATH.is_match(line.trim()) {
            header_count += 1;
        }
        if SUMMARY_LINE.is_match(line) {
            summary_hit = true;
        }
    }

    let total_hits = loc_hits + indent_hits;
    if total_hits == 0 {
        return 0.0;
    }

    let ratio = total_hits as f64 / body.len() as f64;
    let mut confidence = (ratio * 3.0 + 0.2).min(0.85);
    if indent_hits > 0 && header_count > 0 {
        confidence = (confidence + 0.15).min(1.0);
    }
    if summary_hit {
        confidence = (confidence + 0.1).min(1.0);
    }
    confidence.min(1.0)
}

/// Compresses a diagnostics-list style output: groups repeats by rule/code,
/// keeps one concrete example (path:line) per group plus every word-severe
/// occurrence, and always says how many others were folded in.
pub fn compress(lines: &[&str]) -> Compressed {
    let parsed = parse_entries(lines);
    if parsed.order.is_empty() {
        return Compressed::unchanged(lines);
    }

    let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
    for &i in &parsed.order {
        groups.entry(parsed.entries[&i].code.as_str()).or_default().push(i);
    }

    let mut keep_line: HashSet<usize> = HashSet::new();
    for idxs in groups.values() {
        let first = idxs[0];
        keep_line.insert(first);
        if let Some(h) = parsed.entries[&first].header_index {
            keep_line.insert(h);
        }
        for &i in idxs {
            if parsed.entries[&i].severe {
                keep_line.insert(i);
                if let Some(h) = parsed.entries[&i].header_index {
                    keep_line.insert(h);
                }
            }
        }
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut dropped = 0usize;
    let mut summarized: HashSet<&str> = HashSet::new();

    for (i, line) in lines.iter().enumerate() {
        if keep_line.contains(&i) {
            out.push(line.to_string());
            continue;
        }
        if let Some(entry) = parsed.entries.get(&i) {
            dropped += 1;
            if summarized.insert(entry.code.as_str()) {
                let group = &groups[entry.code.as_str()];
                let last = &parsed.entries[group.last().unwrap()];
                out.push(format!(
                    "  … {} other occurrence(s) of {} (e.g. {})",
                    group.len() - 1,
                    entry.code,
                    last.loc
                ));
            }
            continue;
        }
        if parsed.header_indices.contains(&i) {
            dropped += 1;
            continue;
        }
        out.push(line.to_string());
    }

    let repeated_groups = groups.values().filter(|g| g.len() > 1).count();
    let text = out.join("\n");
    let note = if dropped > 0 {
        format!(
            "{dropped} line(s) grouped into {repeated_groups} repeated rule(s), {} rule(s) in total",
            groups.len()
        )
    } else {
        String::new()
    };
    Compressed { text, dropped, note }
}

/// Assembles the `diagnostics` shape for registration alongside the other shapes.
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

    fn severe_lines<'a>(lines: &[&'a str]) -> Vec<&'a str> {
        lines.iter().copied().filter(|l| is_severe_text(l)).collect()
    }

    #[test]
    fn stable_name() {
        assert_eq!(name(), "diagnostics");
    }

    #[test]
    fn tsc_with_a_real_failure_preserves_every_severe_line() {
        let raw = fixture("tsc-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        for line in severe_lines(&lines) {
            assert!(r.text.contains(line), "severe line disappeared: {line}");
        }
    }

    #[test]
    fn detects_a_failing_tsc_as_diagnostics() {
        let raw = fixture("tsc-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detects_ruff_as_diagnostics() {
        let raw = fixture("ruff-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detects_eslint_as_diagnostics() {
        let raw = fixture("eslint-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn does_not_detect_a_failing_jest_as_diagnostics() {
        let raw = fixture("jest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn does_not_detect_a_passing_jest_as_diagnostics() {
        let raw = fixture("jest-mobile-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn does_not_detect_a_failing_vitest_as_diagnostics() {
        let raw = fixture("vitest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn does_not_detect_pytest_as_diagnostics() {
        let raw = fixture("pytest-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn empty_output_from_a_clean_build_is_not_diagnostics() {
        assert_eq!(detect(&[""]), 0.0);
    }

    #[test]
    fn ruff_groups_by_code_and_keeps_an_example_of_each_rule() {
        let raw = fixture("ruff-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.contains("F401"));
        assert!(r.text.contains("E402"));
        assert!(r.dropped > 0);
        assert!(r.text.contains("app/api/v1/payments.py:10:5"));
        assert!(r.text.contains("other occurrence(s)"));
        assert!(r.text.contains("Found 5 errors."));
    }

    #[test]
    fn ruff_never_hides_the_existence_of_a_group() {
        let raw = fixture("ruff-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let e402_count_in = lines.iter().filter(|l| l.contains("E402")).count();
        assert!(e402_count_in > 1);
        assert!(r.text.contains(&format!("{} other occurrence(s) of E402", e402_count_in - 1)));
    }

    #[test]
    fn eslint_groups_by_rule_and_preserves_the_header_path() {
        let raw = fixture("eslint-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.contains("@next/next/no-img-element"));
        assert!(r.text.contains("react-hooks/exhaustive-deps"));
        assert!(r.text.contains("CuttingHistoryView.tsx"));
        let reduction = 1.0 - (r.text.len() as f64 / raw.len() as f64);
        assert!(reduction > 0.3, "expected a real cut > 30%, got {:.1}%", reduction * 100.0);
    }

    #[test]
    fn tsc_with_few_unique_errors_does_not_need_grouping_but_preserves_everything() {
        let raw = fixture("tsc-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        for line in &lines {
            if !line.trim().is_empty() {
                assert!(r.text.contains(line), "unique error disappeared: {line}");
            }
        }
    }

    #[test]
    fn order_of_surviving_lines_is_never_changed() {
        let raw = fixture("eslint-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let mut prev_idx = 0usize;
        for out_line in r.text.split('\n') {
            if out_line.trim().is_empty() || out_line.trim_start().starts_with('…') {
                continue;
            }
            let found = lines.iter().skip(prev_idx).position(|l| *l == out_line);
            assert!(found.is_some(), "line out of order: {out_line}");
            prev_idx += found.unwrap();
        }
    }

    #[test]
    fn compress_returns_input_untouched_when_there_is_no_diagnostic() {
        let lines = ["just loose text", "no diagnostic format"];
        let r = compress(&lines);
        assert_eq!(r.text, lines.join("\n"));
        assert_eq!(r.dropped, 0);
        assert_eq!(r.note, "");
    }

    #[test]
    fn severe_line_inside_a_repeated_group_survives_even_when_not_first() {
        let lines = [
            "a.py:1:1: F401 x imported but unused",
            "b.py:2:1: F401 y imported but unused",
            "c.py:3:1: F401 error: cannot resolve z",
        ];
        let r = compress(&lines);
        assert!(r.text.contains(lines[0]));
        assert!(r.text.contains(lines[2]));
    }
}
