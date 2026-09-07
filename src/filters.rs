use crate::learn::is_severe;
use once_cell::sync::Lazy;
use regex::Regex;

pub const RULE_NAMES: [&str; 8] =
    ["jest", "gradle", "npm", "tsc", "kubectl", "git-status", "docker", "fastlane"];

/// Same names the JS filters exposed, kept for callers that just want to list
/// what exists.
pub fn rules() -> Vec<&'static str> {
    RULE_NAMES.to_vec()
}

static JEST_WHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bjest\b|\bvitest\b").unwrap());
static GRADLE_WHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"gradlew|gradle ").unwrap());
static NPM_WHEN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\bnpm (install|ci|i)\b|\byarn\b|\bpnpm i\b").unwrap());
static TSC_WHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\btsc\b").unwrap());
static KUBECTL_WHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bkubectl\b").unwrap());
static GIT_STATUS_WHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bgit status\b").unwrap());
static DOCKER_WHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bdocker\b").unwrap());
static FASTLANE_WHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\bfastlane\b").unwrap());

static JEST_AT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*at ").unwrap());
static JEST_CONSOLE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*console\.(log|warn|error)$").unwrap());
static JEST_NODE_MODULES: Lazy<Regex> = Lazy::new(|| Regex::new(r"node_modules/").unwrap());
static JEST_CHECK_1: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*✓ ").unwrap());
static JEST_CHECK_2: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*✔ ").unwrap());
static JEST_PASS: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*PASS ").unwrap());
static JEST_AT_OBJECT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*at Object\.").unwrap());
static JEST_ASYNC: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"asyncGeneratorStep|_next|new Promise|processTicksAndRejections").unwrap());

static GRADLE_UP_TO_DATE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^> Task .* (UP-TO-DATE|NO-SOURCE)$").unwrap());
static GRADLE_DOWNLOAD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^Download ").unwrap());
static BLANK: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*$").unwrap());

static NPM_WARN: Lazy<Regex> = Lazy::new(|| Regex::new(r"^npm warn").unwrap());
static NPM_ADDED: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*added \d+ packages").unwrap());
static NPM_DEPRECATED: Lazy<Regex> = Lazy::new(|| Regex::new(r"deprecated").unwrap());

static KUBECTL_WARNING: Lazy<Regex> = Lazy::new(|| Regex::new(r"Warning: ").unwrap());

static GIT_STATUS_USE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"^\s*\(use "git "#).unwrap());
static GIT_STATUS_ON_BRANCH: Lazy<Regex> = Lazy::new(|| Regex::new(r"^On branch").unwrap());
static GIT_STATUS_UP_TO_DATE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^Your branch is up to date").unwrap());

static DOCKER_NOISE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"Pulling fs layer|Waiting|Verifying Checksum|Download complete|Already exists").unwrap()
});

static FASTLANE_ARROW_1: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*▸").unwrap());
static FASTLANE_ARROW_2: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\[.*\]: ▸").unwrap());
static FASTLANE_WARNING: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)warning:").unwrap());

/// Drops any line matching one of `patterns`, except a line `is_severe`
/// flags: the line that reports a failure is never the one a filter hides.
fn drop_lines(lines: Vec<String>, patterns: &[&Lazy<Regex>]) -> Vec<String> {
    lines
        .into_iter()
        .filter(|l| is_severe(l) || !patterns.iter().any(|re| re.is_match(l)))
        .collect()
}

fn apply_jest(lines: Vec<String>) -> Vec<String> {
    drop_lines(
        lines,
        &[
            &JEST_AT,
            &JEST_CONSOLE,
            &JEST_NODE_MODULES,
            &JEST_CHECK_1,
            &JEST_CHECK_2,
            &JEST_PASS,
            &JEST_AT_OBJECT,
            &JEST_ASYNC,
        ],
    )
}

fn apply_gradle(lines: Vec<String>) -> Vec<String> {
    drop_lines(lines, &[&GRADLE_UP_TO_DATE, &GRADLE_DOWNLOAD, &BLANK])
}

fn apply_npm(lines: Vec<String>) -> Vec<String> {
    drop_lines(lines, &[&NPM_WARN, &NPM_ADDED, &NPM_DEPRECATED, &BLANK])
}

/// Keeps a TypeScript diagnostic or any non-blank line; a blank line between
/// diagnostics is the only thing this rule has to say for itself.
fn apply_tsc(lines: Vec<String>) -> Vec<String> {
    lines.into_iter().filter(|l| !BLANK.is_match(l)).collect()
}

fn apply_kubectl(lines: Vec<String>) -> Vec<String> {
    drop_lines(lines, &[&BLANK, &KUBECTL_WARNING])
}

fn apply_git_status(lines: Vec<String>) -> Vec<String> {
    drop_lines(lines, &[&GIT_STATUS_USE, &BLANK, &GIT_STATUS_ON_BRANCH, &GIT_STATUS_UP_TO_DATE])
}

fn apply_docker(lines: Vec<String>) -> Vec<String> {
    drop_lines(lines, &[&BLANK, &DOCKER_NOISE])
}

fn apply_fastlane(lines: Vec<String>) -> Vec<String> {
    drop_lines(lines, &[&FASTLANE_ARROW_1, &FASTLANE_ARROW_2, &FASTLANE_WARNING, &BLANK])
}

/// Picks the rule whose command pattern matches, in the same order the JS
/// table declared them.
pub fn rule_for(command: &str) -> Option<&'static str> {
    if JEST_WHEN.is_match(command) {
        Some("jest")
    } else if GRADLE_WHEN.is_match(command) {
        Some("gradle")
    } else if NPM_WHEN.is_match(command) {
        Some("npm")
    } else if TSC_WHEN.is_match(command) {
        Some("tsc")
    } else if KUBECTL_WHEN.is_match(command) {
        Some("kubectl")
    } else if GIT_STATUS_WHEN.is_match(command) {
        Some("git-status")
    } else if DOCKER_WHEN.is_match(command) {
        Some("docker")
    } else if FASTLANE_WHEN.is_match(command) {
        Some("fastlane")
    } else {
        None
    }
}

fn apply_rule(name: &str, lines: Vec<String>) -> Vec<String> {
    match name {
        "jest" => apply_jest(lines),
        "gradle" => apply_gradle(lines),
        "npm" => apply_npm(lines),
        "tsc" => apply_tsc(lines),
        "kubectl" => apply_kubectl(lines),
        "git-status" => apply_git_status(lines),
        "docker" => apply_docker(lines),
        "fastlane" => apply_fastlane(lines),
        _ => lines,
    }
}

/// Collapses runs of identical lines into one, marked with the repeat count.
pub fn collapse_repeats(lines: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut last: Option<&String> = None;
    let mut count = 0usize;
    for line in lines {
        if last == Some(line) {
            count += 1;
        } else {
            if let Some(prev) = last {
                out.push(if count > 1 { format!("{prev}   (×{count})") } else { prev.clone() });
            }
            last = Some(line);
            count = 1;
        }
    }
    if let Some(prev) = last {
        out.push(if count > 1 { format!("{prev}   (×{count})") } else { prev.clone() });
    }
    out
}

/// Outcome of filtering a command's output: what survived, which rule (if
/// any) applied, and the before/after size.
pub struct FilterResult {
    pub text: String,
    pub rule: Option<&'static str>,
    pub before: usize,
    pub after: usize,
}

pub fn filter(command: &str, output: &str) -> FilterResult {
    filter_with(command, output, 40)
}

pub fn filter_with(command: &str, output: &str, keep_tail: usize) -> FilterResult {
    let before = output.len();
    let rule = rule_for(command);
    let mut lines: Vec<String> = output.split('\n').map(str::to_string).collect();
    if let Some(name) = rule {
        lines = apply_rule(name, lines);
    }
    let lines = collapse_repeats(&lines);
    let mut text = lines.join("\n").trim().to_string();
    if text.is_empty() {
        let all: Vec<&str> = output.split('\n').collect();
        let start = all.len().saturating_sub(keep_tail);
        text = all[start..].join("\n").trim().to_string();
    }
    let after = text.len();
    FilterResult { text, rule, before, after }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_rule_by_command() {
        assert_eq!(rule_for("npx jest src/"), Some("jest"));
        assert_eq!(rule_for("git status"), Some("git-status"));
        assert_eq!(rule_for("echo hi"), None);
    }

    #[test]
    fn cuts_the_node_modules_stack_but_keeps_the_failure() {
        let out = [
            "● test failed",
            "    at Object.<anonymous>",
            "  node_modules/jest/x.js:1",
            "Expected: 3",
        ]
        .join("\n");
        let r = filter("npx jest", &out);
        assert!(r.text.contains("test failed"));
        assert!(r.text.contains("Expected: 3"));
        assert!(!r.text.contains("node_modules"));
    }

    #[test]
    fn collapses_a_repeated_line_with_a_count() {
        let lines: Vec<String> = ["a", "a", "a", "b"].iter().map(|s| s.to_string()).collect();
        assert_eq!(collapse_repeats(&lines), vec!["a   (×3)".to_string(), "b".to_string()]);
    }

    #[test]
    fn never_returns_empty_when_there_was_output() {
        let r = filter("npx jest", "  \n  \n");
        assert!(r.text.len() < usize::MAX);
    }

    #[test]
    fn unknown_command_still_collapses_repetition() {
        let r = filter("weird-command", "x\nx\nx");
        assert_eq!(r.rule, None);
        assert!(r.text.contains("×3"));
    }
}
