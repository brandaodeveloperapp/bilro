use once_cell::sync::Lazy;
use regex::Regex;

use crate::learn::is_severe;
use crate::shapes::contract::{Compressed, Shape};

static CASE_PASS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\s*PASS\s+\S|^\s*[✓✔]\s|^\s*ok\s+\d+\b|^\s*---\s*PASS:|^\S+::\S+.*\bPASSED\b")
        .unwrap()
});
static CASE_FAIL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^\s*FAIL\s+\S|^\s*[✕✗×]\s|^\s*not\s+ok\s+\d+\b|^\s*---\s*FAIL:|^\S+::\S+.*\bFAILED\b",
    )
    .unwrap()
});
static PROGRESS: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^[.EFsx]{3,}(\s*\[\s*\d{1,3}%\])?$").unwrap());
static SUMMARY: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)^\s*(Test (Suites|Files)\b:?|Tests\b:?|Snapshots\b:?|Time\b:?|Ran all test suites|Start at\b:?|Duration\b:?)|^\d+ (passed|failed|examples?|tests?)\b|^(OK|FAILURES!)\b|\d+ examples?,\s*\d+ failures?|assertions?\)\s*$|^(ok|FAIL)\s+\S+\s+[\d.]+s$|^\d+ (errors?|passed|failed) in [\d.]+s$|={3,}\s*(FAILURES|ERRORS|short test summary)",
    )
    .unwrap()
});

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Pass,
    Fail,
    Progress,
    Summary,
    Plain,
}

fn classify(line: &str) -> Kind {
    if CASE_FAIL.is_match(line) {
        Kind::Fail
    } else if CASE_PASS.is_match(line) {
        Kind::Pass
    } else if PROGRESS.is_match(line) {
        Kind::Progress
    } else if SUMMARY.is_match(line) {
        Kind::Summary
    } else {
        Kind::Plain
    }
}

/// Stable identifier for this shape.
pub fn name() -> &'static str {
    "test-report"
}

/// Confidence that `lines` is a test runner's own output: jest, vitest,
/// pytest, phpunit, pest, rspec, playwright, go test. A flood of per-case
/// marks plus a summary count are the tell.
pub fn detect(lines: &[&str]) -> f64 {
    let body: Vec<&&str> = lines.iter().filter(|l| !l.trim().is_empty()).collect();
    if body.is_empty() {
        return 0.0;
    }
    let case_hits = body.iter().filter(|l| CASE_PASS.is_match(l) || CASE_FAIL.is_match(l)).count();
    let progress_hits = body.iter().filter(|l| PROGRESS.is_match(l)).count();
    let summary_hit = body.iter().any(|l| SUMMARY.is_match(l));
    let case_ratio = case_hits as f64 / body.len() as f64;

    let mut confidence = 0.0;
    if summary_hit {
        confidence += 0.55;
    }
    if case_hits > 0 {
        confidence += (case_ratio * 6.0 + 0.15).min(0.45);
    }
    if progress_hits > 0 {
        confidence += 0.15;
    }
    confidence.min(1.0)
}

/// Compresses a test-report style output: a passing case mark opens a block
/// that runs until the next mark, and the whole block is dropped unless a
/// line in it is `is_severe`. A failing mark, a progress line and a summary
/// line are never touched, so a failure keeps its whole stack and diff.
pub fn compress(lines: &[&str]) -> Compressed {
    let n = lines.len();
    let kinds: Vec<Kind> = lines.iter().map(|l| classify(l)).collect();
    let mut out: Vec<&str> = Vec::with_capacity(n);
    let mut dropped = 0usize;
    let mut collapsed_blocks = 0usize;
    let mut i = 0;
    while i < n {
        if kinds[i] == Kind::Pass {
            let start = i;
            let mut end = i + 1;
            while end < n && kinds[end] == Kind::Plain {
                end += 1;
            }
            let before = dropped;
            for line in lines.iter().take(end).skip(start) {
                if is_severe(line) {
                    out.push(line);
                } else {
                    dropped += 1;
                }
            }
            if dropped > before {
                collapsed_blocks += 1;
            }
            i = end;
        } else {
            out.push(lines[i]);
            i += 1;
        }
    }

    let text = out.join("\n");
    let note = if dropped > 0 {
        format!("{dropped} linha(s) de caso passando cortada(s) ({collapsed_blocks} bloco(s) de sucesso resumido(s))")
    } else {
        String::new()
    };
    Compressed { text, dropped, note }
}

/// Assembles the `test-report` shape for registration alongside the other shapes.
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
        lines.iter().copied().filter(|l| is_severe(l)).collect()
    }

    #[test]
    fn nome_estavel() {
        assert_eq!(name(), "test-report");
    }

    #[test]
    fn jest_com_falha_real_preserva_toda_linha_severa() {
        let raw = fixture("jest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        for line in severe_lines(&lines) {
            assert!(r.text.contains(line), "linha severa sumiu: {line}");
        }
    }

    #[test]
    fn vitest_com_falha_real_preserva_toda_linha_severa() {
        let raw = fixture("vitest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        for line in severe_lines(&lines) {
            assert!(r.text.contains(line), "linha severa sumiu: {line}");
        }
    }

    #[test]
    fn detecta_jest_falho_como_test_report() {
        let raw = fixture("jest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_jest_verde_mobile_como_test_report() {
        let raw = fixture("jest-mobile-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_vitest_falho_como_test_report() {
        let raw = fixture("vitest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_vitest_verde_como_test_report() {
        let raw = fixture("vitest-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn detecta_pytest_como_test_report() {
        let raw = fixture("pytest-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) >= MIN_CONFIDENCE);
    }

    #[test]
    fn nao_detecta_saida_de_tsc_como_test_report() {
        let raw = fixture("tsc-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn nao_detecta_saida_de_ruff_como_test_report() {
        let raw = fixture("ruff-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn nao_detecta_saida_de_eslint_como_test_report() {
        let raw = fixture("eslint-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        assert!(detect(&lines) < MIN_CONFIDENCE);
    }

    #[test]
    fn suite_100_por_cento_verde_vira_quase_uma_linha() {
        let raw = fixture("jest-mobile-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let reduction = 1.0 - (r.text.len() as f64 / raw.len() as f64);
        assert!(reduction > 0.9, "esperava corte real > 90%, obteve {:.1}%", reduction * 100.0);
        assert!(r.text.contains("Test Suites: 230 passed, 230 total"));
    }

    #[test]
    fn vitest_100_por_cento_verde_encolhe_bastante() {
        let raw = fixture("vitest-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let reduction = 1.0 - (r.text.len() as f64 / raw.len() as f64);
        assert!(reduction > 0.8, "esperava corte real > 80%, obteve {:.1}%", reduction * 100.0);
    }

    #[test]
    fn falha_do_jest_preserva_stack_e_assert_diff() {
        let raw = fixture("jest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.contains("Expected: 3"));
        assert!(r.text.contains("Received: 2"));
        assert!(r.text.contains("at Object.toBe (src/__tests__/bilro_tmp.test.ts:6:19)"));
        assert!(r.text.contains("throw new Error(\"boom from bilro fixture\");"));
    }

    #[test]
    fn falha_do_vitest_preserva_diff_e_localizacao() {
        let raw = fixture("vitest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        assert!(r.text.contains("AssertionError: expected 2 to be 3"));
        assert!(r.text.contains("lib/bilro_tmp.test.ts:8:19"));
    }

    #[test]
    fn ordem_das_linhas_sobreviventes_nunca_e_alterada() {
        let raw = fixture("vitest-fail-real.txt");
        let lines: Vec<&str> = raw.split('\n').collect();
        let r = compress(&lines);
        let mut prev_idx = 0usize;
        for out_line in r.text.split('\n') {
            if out_line.is_empty() {
                continue;
            }
            let found = lines.iter().skip(prev_idx).position(|l| *l == out_line);
            assert!(found.is_some(), "linha fora de ordem: {out_line}");
            prev_idx += found.unwrap();
        }
    }

    #[test]
    fn compress_devolve_entrada_intacta_quando_nao_ha_o_que_cortar() {
        let lines = ["FAIL src/x.test.ts", "  at foo (x.ts:1:1)", "Tests: 1 failed, 1 total"];
        let r = compress(&lines);
        assert_eq!(r.text, lines.join("\n"));
        assert_eq!(r.dropped, 0);
        assert_eq!(r.note, "");
    }

    #[test]
    fn texto_sem_marca_de_caso_nao_e_test_report() {
        assert_eq!(detect(&["algum texto qualquer", "outra linha solta"]), 0.0);
    }
}
