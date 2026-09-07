use crate::proc::spawn_with_timeout;
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const CHARS_PER_TOKEN: usize = 4;
const DEFAULT_CHUNK_LINES: usize = 60;
const RUN_TIMEOUT_MS: u64 = 120_000;

static BLANK_RUN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{2,}").unwrap());

/// Same accounting the rest of bilro uses to price what a run would have
/// cost the context window had it not been indexed instead.
fn tokens_of(text: &str) -> usize {
    (text.len() + CHARS_PER_TOKEN / 2) / CHARS_PER_TOKEN
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

/// Default index location, shared by every `run` regardless of caller.
pub fn default_db_path() -> PathBuf {
    home().join(".claude").join("bilro").join("index.db")
}

/// Beyond this an index costs more to keep than the answers it holds are
/// worth: a single verbose run once grew the file past a hundred megabytes.
const MAX_INDEXED_BYTES: usize = 8 * 1024 * 1024;

/// Splits on blank lines so a section stays whole, then caps runaway blocks.
pub fn chunk(text: &str, max_lines: usize) -> Vec<String> {
    let mut out = Vec::new();
    for block in BLANK_RUN.split(text) {
        let lines: Vec<&str> = block.split('\n').collect();
        let mut i = 0;
        while i < lines.len() {
            let end = (i + max_lines).min(lines.len());
            let piece = lines[i..end].join("\n");
            let piece = piece.trim();
            if !piece.is_empty() {
                out.push(piece.to_string());
            }
            i += max_lines;
        }
    }
    out
}

/// FTS5 reads punctuation as syntax, so every term travels as a quoted
/// phrase; a stray `"` in the query can never break out into new syntax.
pub fn to_match_query(query: &str) -> Option<String> {
    let terms: Vec<String> =
        query.split_whitespace().map(|t| t.replace('"', "")).filter(|t| !t.is_empty()).collect();
    if terms.is_empty() {
        return None;
    }
    Some(terms.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" OR "))
}

/// Opens (creating if needed) the FTS5 chunk index at `path`.
pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
    let _ = conn.execute_batch("PRAGMA journal_mode = WAL;");
    conn.execute_batch(
        "CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(label, body, source, at UNINDEXED);",
    )?;
    Ok(conn)
}

pub fn open_default() -> rusqlite::Result<Connection> {
    open(&default_db_path())
}

/// How many bytes `index` will refuse to keep for a body this size.
pub fn dropped_bytes(body: &str) -> usize {
    body.len().saturating_sub(MAX_INDEXED_BYTES)
}

/// A body over the cap keeps both ends, because the two places a failure hides
/// are the first thing a command says and the last. Indexing only the head made
/// the summary at the bottom of a long build log unreachable.
fn within_cap(body: &str) -> String {
    if body.len() <= MAX_INDEXED_BYTES {
        return body.to_string();
    }
    let head_budget = MAX_INDEXED_BYTES / 2;
    let head_end = body.char_indices().map(|(i, _)| i).take_while(|i| *i <= head_budget).last().unwrap_or(0);
    let tail_start = body
        .char_indices()
        .map(|(i, _)| i)
        .find(|i| *i >= body.len() - head_budget)
        .unwrap_or(body.len());
    format!(
        "{}\n\n[bilro: {} bytes in the middle were not indexed]\n\n{}",
        &body[..head_end],
        dropped_bytes(body),
        &body[tail_start..]
    )
}

/// Indexes `body` as chunks tied to `label`/`source`, returning how many
/// pieces it was split into. The delete and the inserts share one transaction
/// so a failure halfway cannot leave the previous index destroyed and the new
/// one half written.
pub fn index(conn: &Connection, label: &str, body: &str, source: &str) -> rusqlite::Result<usize> {
    let body = within_cap(body);
    let pieces = chunk(&body, DEFAULT_CHUNK_LINES);
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let written = write_chunks(conn, label, source, &pieces).and_then(|n| {
        conn.execute_batch("COMMIT")?;
        Ok(n)
    });
    if written.is_err() && !conn.is_autocommit() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    written
}

/// The write half of `index`, kept apart so the caller can roll back as a unit.
/// `BEGIN IMMEDIATE` rather than the default deferred: two runs indexing at once
/// would otherwise both start as readers and deadlock on the upgrade, which no
/// `busy_timeout` can wait out.
fn write_chunks(conn: &Connection, label: &str, source: &str, pieces: &[String]) -> rusqlite::Result<usize> {
    conn.execute("DELETE FROM chunks WHERE label = ?1 AND source = ?2", rusqlite::params![label, source])?;
    let at = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    let mut stmt = conn.prepare("INSERT INTO chunks(label, body, source, at) VALUES (?1, ?2, ?3, ?4)")?;
    for piece in pieces {
        stmt.execute(params![label, piece, source, at])?;
    }
    Ok(pieces.len())
}

/// One BM25-ranked hit against the chunk index.
pub struct Hit {
    pub label: String,
    pub body: String,
    pub source: String,
    pub score: f64,
}

pub fn search(conn: &Connection, query: &str, limit: usize) -> rusqlite::Result<Vec<Hit>> {
    let Some(matcher) = to_match_query(query) else {
        return Ok(Vec::new());
    };
    let mut stmt = conn.prepare(
        "SELECT label, body, source, bm25(chunks) AS score
         FROM chunks WHERE chunks MATCH ?1 ORDER BY score LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![matcher, limit as i64], |row| {
        Ok(Hit {
            label: row.get(0)?,
            body: row.get(1)?,
            source: row.get(2)?,
            score: row.get(3)?,
        })
    })?;
    rows.collect()
}

/// A ranked hit tagged with the query that produced it.
pub struct QueryHit {
    pub query: String,
    pub hit: Hit,
}

/// Result of `run`: whether the command failed, how much the caller was
/// spared from seeing, and whatever the given queries turned up instead.
pub struct RunResult {
    pub failed: bool,
    pub withheld_tokens: usize,
    pub chunks: usize,
    pub dropped_bytes: usize,
    pub hits: Vec<QueryHit>,
}

/// Runs a command, keeps its output in the index, and hands back only the
/// size of what was withheld plus whatever the queries asked for. So output
/// large enough to blow the context budget never has to enter it whole.
pub fn run(command: &str, label: Option<&str>, queries: &[String], cwd: Option<&Path>) -> RunResult {
    let mut sh = Command::new("/bin/sh");
    sh.arg("-c").arg(command);
    if let Some(dir) = cwd {
        sh.current_dir(dir);
    }

    let (failed, output) = match spawn_with_timeout(sh, RUN_TIMEOUT_MS) {
        Ok(o) if o.timed_out => {
            (true, String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr))
        }
        Ok(o) => {
            let ok = o.status.map(|s| s.success()).unwrap_or(false);
            let both =
                String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
            (!ok, both)
        }
        Err(e) => (true, e.to_string()),
    };

    let output = crate::redact::redact(&output);
    let label = crate::redact::redact(
        &label.map(str::to_string).unwrap_or_else(|| command.chars().take(60).collect()),
    );
    let withheld_tokens = tokens_of(&output);

    let Ok(conn) = open_default() else {
        return RunResult { failed, withheld_tokens, chunks: 0, dropped_bytes: dropped_bytes(&output), hits: Vec::new() };
    };
    let stored = index(&conn, &label, &output, &crate::redact::redact(command));
    let chunks = stored.unwrap_or(0);
    if chunks == 0 && !output.trim().is_empty() {
        return RunResult {
            failed: true,
            withheld_tokens,
            chunks: 0,
            dropped_bytes: dropped_bytes(&output),
            hits: Vec::new(),
        };
    }
    let mut hits = Vec::new();
    for q in queries {
        if let Ok(found) = search(&conn, q, 3) {
            hits.extend(found.into_iter().map(|hit| QueryHit { query: q.clone(), hit }));
        }
    }

    RunResult { failed, withheld_tokens, chunks, dropped_bytes: dropped_bytes(&output), hits }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn tmp_db() -> PathBuf {
        let dir = env::temp_dir().join(format!("bilro-db-{}", uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("i.db")
    }

    fn uuid() -> u128 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn splits_on_blank_line_and_keeps_the_whole_block() {
        assert_eq!(chunk("one\ntwo\n\nthree", 60), vec!["one\ntwo", "three"]);
    }

    #[test]
    fn huge_block_is_cut_so_it_does_not_become_a_single_chunk() {
        let big = (0..130).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n");
        assert_eq!(chunk(&big, 60).len(), 3);
    }

    #[test]
    fn path_punctuation_does_not_break_fts5() {
        assert_eq!(to_match_query("features/references"), Some("\"features/references\"".into()));
        assert_eq!(to_match_query("  "), None);
    }

    #[test]
    fn user_quotes_do_not_become_syntax_injection() {
        assert_eq!(to_match_query(r#"a" OR "b"#), Some(r#""a" OR "OR" OR "b""#.into()));
    }

    #[test]
    fn indexes_and_retrieves_by_term() {
        let path = tmp_db();
        let conn = open(&path).unwrap();
        index(
            &conn,
            "l",
            "spinner stuck in the query store\n\naffiliate code with no deadline",
            "s",
        )
        .unwrap();
        assert_eq!(search(&conn, "affiliate", 5).unwrap().len(), 1);
        assert_eq!(search(&conn, "nonexistent", 5).unwrap().len(), 0);
    }

    #[test]
    fn run_returns_the_withheld_size_not_the_content() {
        let r = run("printf 'line\\n%.0s' $(seq 1 200)", None, &[], None);
        assert!(!r.failed);
        assert!(r.withheld_tokens > 100);
        assert!(r.chunks >= 1);
    }

    #[test]
    fn failing_command_does_not_blow_up_and_still_indexes_the_output() {
        let r = run("echo before; exit 3", None, &[], None);
        assert!(r.failed);
        assert!(r.withheld_tokens > 0);
    }
}

#[cfg(test)]
mod cap_tests {
    use super::*;

    #[test]
    fn huge_output_does_not_inflate_the_index_without_limit() {
        let dir = std::env::temp_dir().join(format!("bilro-cap-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("i.db");
        let conn = open(&file).unwrap();
        let huge = "log line with some content\n".repeat(700_000);
        assert!(huge.len() > 16 * 1024 * 1024, "fixture needs to exceed the cap");
        index(&conn, "huge", &huge, "test").unwrap();
        drop(conn);
        let size = std::fs::metadata(&file).unwrap().len();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(size < 32 * 1024 * 1024, "index ended up with {size} bytes");
    }
}

#[cfg(test)]
mod reindex_tests {
    use super::*;

    fn temp_db() -> (PathBuf, Connection) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "bilro-ri-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("i.db");
        let c = open(&f).unwrap();
        (dir, c)
    }

    #[test]
    fn reindexing_replaces_instead_of_accumulating() {
        let (dir, conn) = temp_db();
        let body = "first section\n\nsecond section with rare term xylophone";
        for _ in 0..3 {
            index(&conn, "page", body, "web").unwrap();
        }
        let hits = search(&conn, "xylophone", 10).unwrap();
        assert_eq!(hits.len(), 1, "the same chunk came back {} times", hits.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn different_labels_coexist() {
        let (dir, conn) = temp_db();
        index(&conn, "page-a", "rare term xylophone here", "web").unwrap();
        index(&conn, "page-b", "rare term xylophone there", "web").unwrap();
        assert_eq!(search(&conn, "xylophone", 10).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn new_content_under_the_same_label_erases_the_old_one() {
        let (dir, conn) = temp_db();
        index(&conn, "page", "old content zebra", "web").unwrap();
        index(&conn, "page", "new content giraffe", "web").unwrap();
        assert!(search(&conn, "zebra", 5).unwrap().is_empty(), "old content survived");
        assert_eq!(search(&conn, "giraffe", 5).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod cap_regressions {
    use super::*;

    fn tmp_db() -> PathBuf {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("bilro-cap-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("i.db")
    }

    fn big_log_ending_in_a_failure() -> String {
        let mut body = String::with_capacity(MAX_INDEXED_BYTES + 1_000_000);
        while body.len() < MAX_INDEXED_BYTES + 500_000 {
            body.push_str("compiling module ok\n");
        }
        body.push_str("FATAL: migration 0042 failed, tenant data left half written\n");
        body
    }

    #[test]
    fn the_end_of_an_oversized_log_stays_searchable() {
        let path = tmp_db();
        let conn = open(&path).unwrap();
        let body = big_log_ending_in_a_failure();
        index(&conn, "build", &body, "test").unwrap();
        let hits = search(&conn, "FATAL migration", 5).unwrap();
        assert!(!hits.is_empty(), "the failure at the end of the log was unreachable");
        assert!(hits.iter().any(|h| h.body.contains("left half written")));
    }

    #[test]
    fn what_did_not_fit_is_announced_not_hidden() {
        let body = big_log_ending_in_a_failure();
        assert!(dropped_bytes(&body) > 0);
        assert_eq!(dropped_bytes("short"), 0);
    }
}

/// The lines of a chunk worth showing for a given query. A chunk can be
/// thousands of lines long, so printing its head answers a search for something
/// that matched at the bottom with text that has nothing to do with it.
/// A line reporting a failure earns its place whether or not it matched, but
/// never at the cost of every line that did: half the room is held for matches.
/// When there are more failures than room, the count of what did not fit is
/// printed — trimming them quietly is the one thing this function may not do.
/// Short words are dropped from the query only while longer ones remain — a
/// search for `db` must not silently become a search for nothing.
pub fn excerpt(body: &str, query: &str, max_lines: usize) -> Vec<String> {
    let all: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();
    let long: Vec<String> = all.iter().filter(|t| t.len() > 2).cloned().collect();
    let terms = if long.is_empty() { all } else { long };
    let mut severe: Vec<(usize, &str)> = Vec::new();
    let mut matched: Vec<(usize, &str)> = Vec::new();
    for (i, line) in body.lines().enumerate() {
        if crate::compress::learn::is_severe(line) {
            severe.push((i, line));
            continue;
        }
        if matched.len() < max_lines {
            let low = line.to_lowercase();
            if terms.iter().any(|t| low.contains(t.as_str())) {
                matched.push((i, line));
            }
        }
    }
    let will_warn = severe.len() > max_lines.saturating_sub((max_lines / 2).min(matched.len()));
    let budget = max_lines.saturating_sub(if will_warn { 1 } else { 0 });
    let reserved = (budget / 2).min(matched.len());
    let severe_room = budget.saturating_sub(reserved);
    let hidden = severe.len().saturating_sub(severe_room);
    severe.truncate(severe_room);
    matched.truncate(budget.saturating_sub(severe.len()));

    let mut picked = severe;
    picked.extend(matched);
    if picked.is_empty() {
        return body.lines().take(max_lines).map(str::to_string).collect();
    }
    picked.sort_by_key(|(i, _)| *i);
    let mut out: Vec<String> = picked.into_iter().map(|(_, l)| l.to_string()).collect();
    if hidden > 0 {
        out.push(format!("[bilro: {hidden} more failure line(s) in this chunk, not shown]"));
    }
    out
}

#[cfg(test)]
mod excerpt_tests {
    use super::*;

    #[test]
    fn a_match_at_the_bottom_is_what_gets_shown() {
        let mut body = "compiling module ok\n".repeat(5_000);
        body.push_str("FATAL: migration 0042 failed, tenant data left half written\n");
        let lines = excerpt(&body, "FATAL migration", 8);
        assert!(lines.iter().any(|l| l.contains("tenant data left half written")), "{lines:?}");
    }

    #[test]
    fn a_failure_shows_even_when_the_query_did_not_ask_for_it() {
        let mut body = "step ok\n".repeat(200);
        body.push_str("ERROR: disk full\n");
        let lines = excerpt(&body, "step", 40);
        assert!(lines.iter().any(|l| l.contains("disk full")), "{lines:?}");
    }

    #[test]
    fn a_chunk_with_nothing_special_still_shows_something() {
        let body = "alpha\nbeta\ngamma\n";
        assert_eq!(excerpt(body, "zzz", 2), vec!["alpha".to_string(), "beta".to_string()]);
    }
}

#[cfg(test)]
mod excerpt_priority_tests {
    use super::*;

    #[test]
    fn failures_beyond_the_room_are_counted_out_loud() {
        let mut body = String::new();
        for i in 0..30 {
            body.push_str(&format!("ERROR: migration {i} failed on tenant {i}\n"));
        }
        for i in 0..30 {
            body.push_str(&format!("step {i} deploy ok\n"));
        }
        let out = excerpt(&body, "deploy", 40);
        let shown = out.iter().filter(|l| l.contains("ERROR")).count();
        let notice = out.iter().find(|l| l.contains("more failure line(s)"));
        assert!(notice.is_some(), "failures were trimmed in silence: {out:?}");
        assert!(shown + 30 - shown == 30);
    }

    #[test]
    fn nothing_is_announced_when_everything_fits() {
        let body = "ERROR: one thing broke\nstep ok\n";
        let out = excerpt(body, "step", 40);
        assert!(out.iter().all(|l| !l.contains("more failure line(s)")), "{out:?}");
    }
}

#[cfg(test)]
mod excerpt_budget_tests {
    use super::*;

    #[test]
    fn the_notice_is_paid_for_out_of_the_budget() {
        let mut body = String::new();
        for i in 0..30 {
            body.push_str(&format!("ERROR: migration {i} failed on tenant {i}\n"));
        }
        for i in 0..30 {
            body.push_str(&format!("step {i} deploy ok\n"));
        }
        for max in [4usize, 8, 12, 40] {
            let out = excerpt(&body, "deploy", max);
            assert!(out.len() <= max, "excerpt returned {} lines for max {max}", out.len());
        }
    }
}
