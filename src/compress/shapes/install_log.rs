use crate::compress::learn::is_severe;
use crate::compress::shapes::contract::{Compressed, Shape};
use once_cell::sync::Lazy;
use regex::{Regex, RegexBuilder};

static PROGRESS_RE: Lazy<Regex> = Lazy::new(|| {
    RegexBuilder::new(
        r"^npm (verbose|info|http|timing)\b|^\s*(Collecting|Downloading|Using cached|Requirement already satisfied|Building wheel|Preparing metadata|Resolving|Fetching)\b|^\s*[+-]\s+\S+==\S+|^\s*\[notice\]",
    )
    .case_insensitive(true)
    .build()
    .unwrap()
});
static WARN_RE: Lazy<Regex> =
    Lazy::new(|| RegexBuilder::new(r"^npm warn\b|^\s*warning:").case_insensitive(true).build().unwrap());
static FUNDING_RE: Lazy<Regex> =
    Lazy::new(|| RegexBuilder::new(r"looking for funding|run `npm fund`").case_insensitive(true).build().unwrap());
static SUMMARY_RE: Lazy<Regex> = Lazy::new(|| {
    RegexBuilder::new(
        r"\b(added|removed|changed)\s+\d+\s+packages?\b|\baudited\s+\d+\s+packages?\b|\bup to date in\b|Successfully installed|\bResolved\s+\d+\s+packages?\b|\bPrepared\s+\d+\s+packages?\b|\bInstalled\s+\d+\s+packages?\b|found\s+\d+\s+vulnerabilit",
    )
    .case_insensitive(true)
    .build()
    .unwrap()
});

fn strip_carriage_return(line: &str) -> &str {
    if !line.contains('\r') {
        return line;
    }
    line.rsplit('\r').next().unwrap_or(line)
}

fn classify(lines: &[&str]) -> f64 {
    let non_empty: Vec<&str> =
        lines.iter().map(|l| strip_carriage_return(l)).filter(|l| !l.trim().is_empty()).collect();
    let n = non_empty.len();
    if n < 2 {
        return 0.0;
    }
    let progress = non_empty.iter().filter(|l| PROGRESS_RE.is_match(l)).count();
    let warn = non_empty.iter().filter(|l| WARN_RE.is_match(l)).count();
    let summary = non_empty.iter().filter(|l| SUMMARY_RE.is_match(l)).count();
    if summary == 0 {
        return 0.0;
    }
    (progress + warn + summary) as f64 / n as f64
}

/// Stable identifier for this shape.
pub fn name() -> &'static str {
    "install-log"
}

/// Confidence, 0..1, that these lines are a package manager install log.
pub fn detect(lines: &[&str]) -> f64 {
    classify(lines)
}

/// Drops progress and funding noise, keeping every warning, summary, and severe line.
pub fn compress(lines: &[&str]) -> Compressed {
    let mut kept: Vec<String> = Vec::new();
    let mut dropped_progress = 0usize;
    let mut dropped_funding = 0usize;
    let mut dropped_blank = 0usize;

    for raw in lines {
        let line = strip_carriage_return(raw);
        if line.trim().is_empty() {
            dropped_blank += 1;
            continue;
        }
        if is_severe(line) || SUMMARY_RE.is_match(line) || WARN_RE.is_match(line) {
            kept.push(line.to_string());
            continue;
        }
        if FUNDING_RE.is_match(line) {
            dropped_funding += 1;
            continue;
        }
        if PROGRESS_RE.is_match(line) {
            dropped_progress += 1;
            continue;
        }
        kept.push(line.to_string());
    }

    let text = kept.join("\n");
    let dropped = lines.len().saturating_sub(kept.len());
    let note = format!(
        "install-log: {} progress line(s), {} funding and {} blank cut",
        dropped_progress, dropped_funding, dropped_blank
    );
    Compressed { text, dropped, note }
}

/// Assembles the shape descriptor for the install-log dispatcher.
pub fn shape() -> Shape {
    Shape { name: name(), detect, compress }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compress::shapes::keyvalue::detect as detect_keyvalue;
    use crate::compress::shapes::listing::detect as detect_listing;

    fn load(fixture: &str) -> String {
        let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), fixture);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {} missing: {}", fixture, e))
    }

    fn lines_of(text: &str) -> Vec<&str> {
        text.split('\n').collect()
    }

    #[test]
    fn stable_name() {
        assert_eq!(name(), "install-log");
    }

    #[test]
    fn detects_a_real_npm_install_with_high_confidence() {
        let text = load("install-npm-real.txt");
        assert!(detect(&lines_of(&text)) >= 0.6);
    }

    #[test]
    fn detects_a_real_pip_install_with_high_confidence() {
        let text = load("install-pip-real.txt");
        assert!(detect(&lines_of(&text)) >= 0.6);
    }

    #[test]
    fn detects_a_real_uv_add_with_high_confidence() {
        let text = load("install-uv-real.txt");
        assert!(detect(&lines_of(&text)) >= 0.6);
    }

    #[test]
    fn does_not_detect_a_file_listing_as_an_install_log() {
        let text = load("listing-find.txt");
        assert!(detect(&lines_of(&text)) < 0.6);
        let grep_text = load("listing-grep.txt");
        assert!(detect(&lines_of(&grep_text)) < 0.6);
    }

    #[test]
    fn does_not_detect_json_keyvalue_as_an_install_log() {
        let text = load("keyvalue-curl-real.json");
        assert!(detect(&lines_of(&text)) < 0.6);
    }

    #[test]
    fn does_not_detect_a_git_diff_as_an_install_log() {
        let text = load("git-diff-real.txt");
        assert!(detect(&lines_of(&text)) < 0.6);
    }

    #[test]
    fn listing_and_keyvalue_are_not_confused_with_install_log_in_reverse() {
        let text = load("install-npm-real.txt");
        assert!(detect_listing(&lines_of(&text)) < 0.6);
        assert!(detect_keyvalue(&lines_of(&text)) < 0.6);
    }

    #[test]
    fn compressing_a_real_npm_install_keeps_summary_and_every_warning_cuts_progress_and_funding() {
        let text = load("install-npm-real.txt");
        let before = text.len();
        let r = compress(&lines_of(&text));
        assert!(r.text.len() < before, "expected a reduction: before={} after={}", before, r.text.len());
        assert!(r.text.contains("added 126 packages"));
        assert!(r.text.contains("found 0 vulnerabilities"));
        for dep in ["inflight@1.0.6", "rimraf@3.0.2", "glob@7.2.3"] {
            assert!(r.text.contains(dep), "warning for {} must not be cut", dep);
        }
        assert!(!r.text.contains("looking for funding"));
        assert!(!r.text.contains("run `npm fund`"));
    }

    #[test]
    fn compressing_a_real_pip_install_keeps_successfully_installed_and_cuts_collecting_downloading() {
        let text = load("install-pip-real.txt");
        let before = text.len();
        let r = compress(&lines_of(&text));
        let cut = 1.0 - (r.text.len() as f64 / before as f64);
        assert!(cut > 0.3, "expected a reasonable cut, got {:.1}%", cut * 100.0);
        assert!(r.text.contains("Successfully installed"));
        assert!(!r.text.contains("Collecting requests"));
        assert!(!r.text.contains("Downloading requests"));
    }

    #[test]
    fn compressing_a_real_uv_add_keeps_the_package_count_and_cuts_the_resolved_list() {
        let text = load("install-uv-real.txt");
        let r = compress(&lines_of(&text));
        assert!(r.text.contains("Installed 12 packages in 9ms"));
        assert!(!r.text.contains("+ blinker==1.9.0"));
        assert!(r.dropped >= 10);
    }

    #[test]
    fn a_progress_bar_with_carriage_returns_never_survives_as_garbage() {
        let lines = vec![
            "Collecting bigpkg",
            "Downloading bigpkg-1.0.0.whl (900 MB)\r 10%|#         | 90/900 MB\r 55%|#####     | 495/900 MB\r 100%|##########| 900/900 MB",
            "Successfully installed bigpkg-1.0.0",
        ];
        let r = compress(&lines);
        assert!(!r.text.contains('\r'));
        assert!(!r.text.contains("10%|"));
        assert!(r.text.contains("Successfully installed bigpkg-1.0.0"));
    }

    #[test]
    fn peer_dependency_and_audit_failures_are_never_cut_even_amid_a_flood() {
        let flood: Vec<String> = (0..40).map(|i| format!("npm verbose fetch manifest pkg{}@1.0.0", i)).collect();
        let mut owned: Vec<String> = flood;
        owned.push("npm error ERESOLVE unable to resolve dependency tree".to_string());
        owned.push("npm error peer react@\">=18\" from some-lib@2.0.0".to_string());
        owned.push("found 3 high severity vulnerabilities, run `npm audit fix`".to_string());
        owned.push("added 40 packages in 2s".to_string());
        let lines: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
        let r = compress(&lines);
        assert!(r.text.contains("ERESOLVE unable to resolve dependency tree"));
        assert!(r.text.contains("3 high severity vulnerabilities"));
        assert!(r.dropped >= 30);
    }

    #[test]
    fn compress_returns_a_string_even_with_no_recognized_summary_safe_fallback() {
        let lines = vec!["something or other", "another random line"];
        let r = compress(&lines);
        assert!(r.text.contains("something or other"));
    }
}

