use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{params, Connection};
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::path::Path;

const MAX_LINES: usize = 20_000;

static QUOTED: Lazy<Regex> = Lazy::new(|| Regex::new(r#"["'][^"']*["']"#).unwrap());
static HEXISH: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b[0-9a-f]{7,40}\b").unwrap());
static INTEGER: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b\d+\b").unwrap());
static SPACES: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+").unwrap());
static HEX_RUN: Lazy<Regex> = Lazy::new(|| Regex::new(r"[0-9a-f]{7,}").unwrap());
static NUMERIC: Lazy<Regex> = Lazy::new(|| Regex::new(r"\d+(?:[.,]\d+)?").unwrap());

static SEVERE_WORDS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(error|errors|failed|failing|failure|fails|fail|fatal|panic|panicked|exception|traceback|stacktrace|refused|denied|unauthorized|forbidden|timeout|timed out|cannot|could not|no such|not found|undefined|undeclared|unresolved|unmet|unexpected|invalid|illegal|missing|abort|aborted|killed|segmentation fault|core dumped|exit status|exit code|rejected|conflict|mismatch|FAIL|ERR)\b").unwrap()
});
static SEVERE_MARKS: Lazy<Regex> = Lazy::new(|| Regex::new("[✕✗✖❌×⨯]").unwrap());
static DIAGNOSTIC: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*\S+:\d+(:\d+)?:\s").unwrap());

/// A line reporting a failure is never noise, however often it repeats. A build
/// that breaks the same way every day is still the answer to "what happened".
pub fn is_severe(line: &str) -> bool {
    is_severe_text(line) || DIAGNOSTIC.is_match(line)
}

/// Severity carried by the words or marks a line uses, ignoring the structural
/// `path:line:col:` shape. A grouping compressor already accounts for that
/// shape by keeping a count and an example, so it asks this narrower question.
pub fn is_severe_text(line: &str) -> bool {
    SEVERE_WORDS.is_match(line) || SEVERE_MARKS.is_match(line)
}

/// Commands differ by their arguments; what repeats is the program and shape.
pub fn signature(command: &str) -> String {
    let s = QUOTED.replace_all(command, "S");
    let s = HEXISH.replace_all(&s, "H");
    let s = INTEGER.replace_all(&s, "N");
    let s = SPACES.replace_all(&s, " ");
    s.trim().chars().take(200).collect()
}

fn digest(text: &str) -> String {
    let mut h = Sha1::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())[..16].to_string()
}

fn full_digest(text: &str) -> String {
    let mut h = Sha1::new();
    h.update(text.as_bytes());
    format!("{:x}", h.finalize())
}

/// Shape identity: numbers and hashes collapse, so a duration or a counter does
/// not make every run look new. Used to decide what is noise.
pub fn line_hash(line: &str) -> String {
    let t = line.trim();
    let t = HEX_RUN.replace_all(t, "H");
    let t = NUMERIC.replace_all(&t, "N");
    digest(&t)
}

/// Exact identity, numbers included. Used to decide what is new: "module 3
/// failed" and "module 7 failed" are different events sharing one shape.
pub fn exact_hash(line: &str) -> String {
    digest(line.trim())
}

pub fn open(file: &Path) -> rusqlite::Result<Connection> {
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let db = Connection::open(file)?;
    db.execute_batch(
        "PRAGMA busy_timeout = 5000;
         PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS runs (sig TEXT PRIMARY KEY, n INTEGER NOT NULL, last_hash TEXT);
         CREATE TABLE IF NOT EXISTS lines (sig TEXT, h TEXT, df INTEGER NOT NULL, PRIMARY KEY (sig, h));
         CREATE TABLE IF NOT EXISTS last (sig TEXT PRIMARY KEY, body TEXT);
         CREATE TABLE IF NOT EXISTS exact (sig TEXT, shape TEXT, h TEXT, PRIMARY KEY (sig, h));",
    )?;
    Ok(db)
}

pub struct Prior {
    pub n: i64,
    pub last_hash: Option<String>,
    pub body: Option<String>,
}

pub fn run_count(db: &Connection, sig: &str) -> rusqlite::Result<Prior> {
    let row: Option<(i64, Option<String>)> = db
        .query_row("SELECT n, last_hash FROM runs WHERE sig = ?", params![sig], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .ok();
    let Some((n, last_hash)) = row else {
        return Ok(Prior { n: 0, last_hash: None, body: None });
    };
    let body = db
        .query_row("SELECT body FROM last WHERE sig = ?", params![sig], |r| r.get::<_, String>(0))
        .ok();
    Ok(Prior { n, last_hash, body })
}

pub struct Observed {
    pub sig: String,
    pub hash: String,
    pub runs: i64,
    pub unchanged: bool,
    pub previous: Option<String>,
}

/// Records one run: how many times this command shape has been seen, and how
/// many of those runs each line appeared in.
pub fn observe(db: &mut Connection, command: &str, output: &str) -> rusqlite::Result<Observed> {
    let sig = signature(command);
    let hash = full_digest(output);
    let all: Vec<&str> = output.split('\n').collect();
    let lines: &[&str] = if all.len() > MAX_LINES { &all[..MAX_LINES] } else { &all };

    let prior = run_count(db, &sig)?;
    let uniq: HashSet<String> =
        lines.iter().filter(|l| !l.trim().is_empty()).map(|l| line_hash(l)).collect();

    let tx = db.transaction()?;
    tx.execute(
        "INSERT INTO runs(sig, n, last_hash) VALUES (?, 1, ?)
         ON CONFLICT(sig) DO UPDATE SET n = n + 1, last_hash = ?",
        params![sig, hash, hash],
    )?;
    {
        let mut bump = tx.prepare(
            "INSERT INTO lines(sig, h, df) VALUES (?, ?, 1)
             ON CONFLICT(sig, h) DO UPDATE SET df = df + 1",
        )?;
        for h in &uniq {
            bump.execute(params![sig, h])?;
        }
        let mut seen = tx.prepare("INSERT OR IGNORE INTO exact(sig, shape, h) VALUES (?, ?, ?)")?;
        for line in lines {
            if !line.trim().is_empty() {
                seen.execute(params![sig, line_hash(line), exact_hash(line)])?;
            }
        }
    }
    tx.execute(
        "INSERT INTO last(sig, body) VALUES (?, ?) ON CONFLICT(sig) DO UPDATE SET body = ?",
        params![sig, output, output],
    )?;
    tx.commit()?;

    Ok(Observed {
        sig,
        hash: hash.clone(),
        runs: prior.n + 1,
        unchanged: prior.last_hash.as_deref() == Some(hash.as_str()) && prior.n > 0,
        previous: prior.body,
    })
}

pub struct Denoised {
    pub text: String,
    pub learned: bool,
    pub runs: i64,
    pub dropped: usize,
}

/// Drops lines that carry no information: those seen in most previous runs of
/// the same command. Below `min_runs` there is not enough history to judge.
pub fn denoise(db: &Connection, command: &str, output: &str, min_runs: i64, ceiling: f64) -> rusqlite::Result<Denoised> {
    let sig = signature(command);
    let prior = run_count(db, &sig)?;
    let n = prior.n;
    if n < min_runs {
        return Ok(Denoised { text: output.to_string(), learned: false, runs: n, dropped: 0 });
    }

    let mut df: HashMap<String, i64> = HashMap::new();
    {
        let mut st = db.prepare("SELECT h, df FROM lines WHERE sig = ?")?;
        let rows = st.query_map(params![sig], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (h, d) = row?;
            df.insert(h, d);
        }
    }
    let mut exact: HashSet<String> = HashSet::new();
    {
        let mut st = db.prepare("SELECT h FROM exact WHERE sig = ?")?;
        for row in st.query_map(params![sig], |r| r.get::<_, String>(0))? {
            exact.insert(row?);
        }
    }
    let mut spread: HashMap<String, i64> = HashMap::new();
    {
        let mut st = db.prepare("SELECT shape, count(*) c FROM exact WHERE sig = ? GROUP BY shape")?;
        let rows = st.query_map(params![sig], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        for row in rows {
            let (s, c) = row?;
            spread.insert(s, c);
        }
    }

    let mut dropped = 0usize;
    let mut kept: Vec<&str> = Vec::new();
    for line in output.split('\n') {
        if line.trim().is_empty() {
            kept.push(line);
            continue;
        }
        if is_severe(line) {
            kept.push(line);
            continue;
        }
        let shape = line_hash(line);
        let seen = *df.get(&shape).unwrap_or(&0);
        let distinct = *spread.get(&shape).unwrap_or(&0);
        let volatile = distinct as f64 / n.max(1) as f64 > 0.5;
        if !volatile && !exact.contains(&exact_hash(line)) {
            kept.push(line);
            continue;
        }
        if seen as f64 / n as f64 > ceiling {
            dropped += 1;
            continue;
        }
        kept.push(line);
    }

    Ok(Denoised { text: kept.join("\n").trim().to_string(), learned: true, runs: n, dropped })
}

/// Jaccard overlap between two outputs, on normalised line identity.
pub fn similarity(previous: &str, current: &str) -> f64 {
    let set = |t: &str| -> HashSet<String> {
        t.split('\n').filter(|l| !l.trim().is_empty()).map(line_hash).collect()
    };
    let a = set(previous);
    let b = set(current);
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let shared = b.iter().filter(|h| a.contains(*h)).count();
    let union = a.len() + b.len() - shared;
    if union == 0 {
        return 1.0;
    }
    shared as f64 / union as f64
}

/// Lines present now that were not in the previous run of the same command.
pub fn novelty(previous: &str, current: &str) -> Vec<String> {
    let before: HashSet<String> = previous.split('\n').map(exact_hash).collect();
    current
        .split('\n')
        .filter(|l| !l.trim().is_empty() && !before.contains(&exact_hash(l)))
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn db() -> Connection {
        let dir = std::env::temp_dir().join(format!("bilro-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let file: PathBuf = dir.join(format!("{}.db", uniq_name()));
        open(&file).unwrap()
    }

    fn uniq_name() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!("t{}", N.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn signature_collapses_variable_argument() {
        assert_eq!(signature("git show abc1234def5678"), signature("git show 9876543fedcba0"));
        assert_eq!(signature("kubectl logs pod-12"), signature("kubectl logs pod-99"));
        assert_ne!(signature("git status"), signature("git diff"));
    }

    #[test]
    fn shape_collapses_number_but_exact_does_not() {
        assert_eq!(line_hash("module 3 failed"), line_hash("module 7 failed"));
        assert_ne!(exact_hash("module 3 failed"), exact_hash("module 7 failed"));
    }

    #[test]
    fn duration_with_suffix_counts_as_a_number() {
        assert_eq!(line_hash("compiled in 150ms"), line_hash("compiled in 2300ms"));
    }

    #[test]
    fn failure_line_is_never_dropped_no_matter_how_much_it_repeats() {
        let mut d = db();
        let out = "Running suite\nFAIL src/auth.test.ts  token expiry\nDone";
        for _ in 0..50 {
            observe(&mut d, "npm test", out).unwrap();
        }
        let r = denoise(&d, "npm test", out, 3, 0.8).unwrap();
        assert!(r.text.contains("FAIL src/auth.test.ts"));
    }

    #[test]
    fn error_with_variable_pid_survives_even_with_a_volatile_shape() {
        let mut d = db();
        for i in 0..10 {
            let out = format!("ERROR: connection refused at 10.0.0.{}:5432 pid={}", i, 9000 + i);
            observe(&mut d, "connect.sh", &out).unwrap();
        }
        let new_out = "ERROR: connection refused at 10.0.0.999:5432 pid=12345";
        let r = denoise(&d, "connect.sh", new_out, 3, 0.8).unwrap();
        assert!(r.text.contains("connection refused"));
    }

    #[test]
    fn benign_noise_still_gets_dropped() {
        let mut d = db();
        for i in 0..10 {
            let out = format!("> build\nwebpack compiled in {}ms\nasset main.js 2.1 MiB", 100 + i);
            observe(&mut d, "npm run build", &out).unwrap();
        }
        let r = denoise(&d, "npm run build", "> build\nwebpack compiled in 999ms\nasset main.js 2.1 MiB", 3, 0.8).unwrap();
        assert!(r.dropped > 0);
    }

    #[test]
    fn below_min_runs_does_not_judge() {
        let mut d = db();
        let out = "common line\nanother line";
        observe(&mut d, "cmd", out).unwrap();
        observe(&mut d, "cmd", out).unwrap();
        let r = denoise(&d, "cmd", out, 3, 0.8).unwrap();
        assert!(!r.learned);
        assert_eq!(r.text, out);
    }

    #[test]
    fn line_always_repeated_gets_dropped() {
        let mut d = db();
        let routine = "compiling\nmodule 3 ready\ndone";
        for _ in 0..5 {
            observe(&mut d, "cmd", routine).unwrap();
        }
        assert_eq!(denoise(&d, "cmd", routine, 3, 0.8).unwrap().text, "");
    }

    #[test]
    fn failure_in_a_new_module_survives_with_a_known_shape() {
        let mut d = db();
        let routine = "compiling\nmodule 3 ready\ndone";
        for _ in 0..5 {
            observe(&mut d, "cmd", routine).unwrap();
        }
        let r = denoise(&d, "cmd", "compiling\nmodule 3 ready\nmodule 7 ready\ndone", 3, 0.8).unwrap();
        assert!(r.text.contains("module 7 ready"));
    }

    #[test]
    fn observe_counts_runs_and_detects_identical_output() {
        let mut d = db();
        let a = observe(&mut d, "cmd", "same").unwrap();
        assert_eq!(a.runs, 1);
        assert!(!a.unchanged);
        let b = observe(&mut d, "cmd", "same").unwrap();
        assert_eq!(b.runs, 2);
        assert!(b.unchanged);
        let c = observe(&mut d, "cmd", "different").unwrap();
        assert!(!c.unchanged);
    }

    #[test]
    fn jaccard_similarity() {
        assert_eq!(similarity("a\nb", "a\nb"), 1.0);
        assert_eq!(similarity("a\nb", "c\nd"), 0.0);
        let mid = similarity("a\nb", "a\nc");
        assert!(mid > 0.3 && mid < 0.4, "got {mid}");
    }

    #[test]
    fn novelty_lists_only_what_did_not_exist_before() {
        let n = novelty("a\nb", "a\nb\nc");
        assert_eq!(n, vec!["c".to_string()]);
    }

    #[test]
    fn compiler_diagnostic_is_severe_in_any_language() {
        for l in [
            "./main.go:10:2: undefined: fooBar",
            "src/lib.rs:42:9: expected semicolon",
            "app/models.py:8:1: E402 import not at top",
            "src/App.tsx:15:3: Type error",
        ] {
            assert!(is_severe(l), "diagnostic should be severe: {l}");
        }
        for l in ["compiling module", "asset main.js 2.1 MiB", "Done in 3s", "web-1 1/1 Running"] {
            assert!(!is_severe(l), "should not be severe: {l}");
        }
    }

    #[test]
    fn is_severe_catches_an_error_that_does_not_use_the_word_error() {
        for l in ["npm ERR! code ELIFECYCLE", "Killed", "exit status 1", "Segmentation fault (core dumped)", "ld: undefined reference to foo"] {
            assert!(is_severe(l), "should be severe: {l}");
        }
    }

    #[test]
    fn is_severe_catches_failure_conjugations() {
        for l in ["connection fails", "test fails intermittently", "the module fails", "two tests fail", "build fail"] {
            assert!(is_severe(l), "should be severe: {l}");
        }
    }

    #[test]
    fn is_severe_recognizes_failure_across_phrasings_and_symbols() {
        for l in ["ERROR: x", "module failed", "Traceback (most recent call last)", "connection refused", "FAIL auth", "✗ test"] {
            assert!(is_severe(l), "should be severe: {l}");
        }
        for l in ["compiling", "asset main.js 2.1 MiB", "ok"] {
            assert!(!is_severe(l), "should not be severe: {l}");
        }
    }
}
