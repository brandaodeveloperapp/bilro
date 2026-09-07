use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}

fn bilro_dir(home: &Path) -> PathBuf {
    home.join(".claude").join("bilro")
}

fn learn_db_path(home: &Path) -> PathBuf {
    bilro_dir(home).join("learn.db")
}

fn index_db_path(home: &Path) -> PathBuf {
    bilro_dir(home).join("index.db")
}

fn journal_db_path(home: &Path) -> PathBuf {
    bilro_dir(home).join("journal.db")
}

fn sessions_dir_path(home: &Path) -> PathBuf {
    bilro_dir(home).join("sessions")
}

fn settings_path(home: &Path) -> PathBuf {
    home.join(".claude").join("settings.json")
}

fn claude_json_path(home: &Path) -> PathBuf {
    home.join(".claude.json")
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Session files bilro owns under a sessions directory, ignoring the lock
/// files a concurrent write leaves behind.
fn session_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .collect()
}

fn total_bytes(paths: &[PathBuf]) -> u64 {
    paths.iter().map(|p| file_len(p)).sum()
}

fn count_table(db: &Connection, table: &str) -> usize {
    db.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
}

/// What bilro has learned so far, and what a denoise pass would cut today.
#[derive(Debug)]
pub struct Stats {
    pub learned_commands: usize,
    pub learned_mature: usize,
    pub denoise_savings_bytes: u64,
    pub indexed_chunks: usize,
    pub journal_events_total: usize,
    pub journal_events_by_kind: Vec<(String, usize)>,
    pub sessions: usize,
    pub learn_db_bytes: u64,
    pub index_db_bytes: u64,
    pub journal_db_bytes: u64,
    pub sessions_bytes: u64,
}

/// Replays `denoise` over every command shape's last known body, so the
/// savings figure is measured against the real corpus instead of guessed.
fn measure_denoise_savings(db: &Connection) -> u64 {
    let Ok(mut stmt) = db.prepare("SELECT sig, body FROM last") else { return 0 };
    let Ok(rows) = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))) else {
        return 0;
    };
    rows.flatten()
        .filter_map(|(sig, body)| {
            crate::compress::learn::denoise(db, &sig, &body, 3, 0.8)
                .ok()
                .map(|d| body.len().saturating_sub(d.text.len()) as u64)
        })
        .sum()
}

fn stats_at(home: &Path) -> Stats {
    let learn_path = learn_db_path(home);
    let (learned_commands, learned_mature, denoise_savings_bytes) = learn_path
        .exists()
        .then(|| crate::compress::learn::open(&learn_path).ok())
        .flatten()
        .map(|db| {
            let total: i64 = db.query_row("SELECT count(*) FROM runs", [], |r| r.get(0)).unwrap_or(0);
            let mature: i64 =
                db.query_row("SELECT count(*) FROM runs WHERE n >= 3", [], |r| r.get(0)).unwrap_or(0);
            (total as usize, mature as usize, measure_denoise_savings(&db))
        })
        .unwrap_or((0, 0, 0));

    let index_path = index_db_path(home);
    let indexed_chunks = index_path
        .exists()
        .then(|| crate::store::sandbox::open(&index_path).ok())
        .flatten()
        .and_then(|db| db.query_row("SELECT count(*) FROM chunks", [], |r| r.get::<_, i64>(0)).ok())
        .unwrap_or(0) as usize;

    let journal_path = journal_db_path(home);
    let (journal_events_total, journal_events_by_kind) = journal_path
        .exists()
        .then(|| crate::store::journal::open(&journal_path).ok())
        .flatten()
        .map(|db| {
            let total: i64 = db.query_row("SELECT count(*) FROM events", [], |r| r.get(0)).unwrap_or(0);
            let mut by_kind = Vec::new();
            if let Ok(mut st) = db.prepare("SELECT kind, count(*) FROM events GROUP BY kind ORDER BY kind") {
                if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as usize))) {
                    by_kind.extend(rows.flatten());
                }
            }
            (total as usize, by_kind)
        })
        .unwrap_or((0, Vec::new()));

    let sessions = session_files(&sessions_dir_path(home));

    Stats {
        learned_commands,
        learned_mature,
        denoise_savings_bytes,
        indexed_chunks,
        journal_events_total,
        journal_events_by_kind,
        sessions: sessions.len(),
        learn_db_bytes: file_len(&learn_path),
        index_db_bytes: file_len(&index_path),
        journal_db_bytes: file_len(&journal_path),
        sessions_bytes: total_bytes(&sessions),
    }
}

/// How much context bilro has saved so far, measured against what it holds
/// right now: learned command shapes, the chunk index, the journal, sessions.
pub fn stats() -> Stats {
    stats_at(&home())
}

/// One diagnostic result. A failing check always says how to fix it.
#[derive(Debug)]
pub struct Check {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

fn check(name: &str, ok: bool, detail: impl Into<String>) -> Check {
    Check { name: name.to_string(), ok, detail: detail.into() }
}

fn which(program: &str) -> Option<PathBuf> {
    let out = Command::new("which").arg(program).stdout(Stdio::piped()).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let found = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!found.is_empty()).then(|| PathBuf::from(found))
}

fn check_binary_on_path() -> Check {
    let Ok(current) = std::env::current_exe() else {
        return check(
            "binary on PATH",
            false,
            "std::env::current_exe() failed. Run the binary by its absolute path to diagnose.",
        );
    };
    let Some(found) = which("bilro") else {
        return check(
            "binary on PATH",
            false,
            format!(
                "`bilro` is not on PATH. Run `{} install` or create a symlink in a directory on PATH.",
                current.display()
            ),
        );
    };
    let same = std::fs::canonicalize(&found).ok() == std::fs::canonicalize(&current).ok();
    if same {
        check("binary on PATH", true, format!("{} -> {}", found.display(), current.display()))
    } else {
        check(
            "binary on PATH",
            false,
            format!(
                "`bilro` on PATH points to {}, but the current executable is {}. Run `bilro install` again to update the symlink.",
                found.display(),
                current.display()
            ),
        )
    }
}

fn check_hooks(home: &Path) -> Check {
    let path = settings_path(home);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return check(
            "hooks in settings.json",
            false,
            format!("could not find {}. Run `bilro install` to register the hooks.", path.display()),
        );
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return check(
            "hooks in settings.json",
            false,
            format!("{} is not valid JSON. Fix it or delete it and run `bilro install` again.", path.display()),
        );
    };
    let is_bilro = |cmd: &str| -> bool {
        cmd.split_whitespace()
            .next()
            .and_then(|bin| Path::new(bin).file_name())
            .and_then(|f| f.to_str())
            == Some("bilro")
    };

    let commands: Vec<String> = value
        .get("hooks")
        .and_then(|h| h.as_object())
        .into_iter()
        .flat_map(|groups| groups.values())
        .filter_map(|list| list.as_array())
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(|h| h.as_array()))
        .flatten()
        .filter_map(|h| h.get("command").and_then(|c| c.as_str()))
        .map(str::to_string)
        .filter(|cmd| is_bilro(cmd))
        .collect();

    if commands.is_empty() {
        return check(
            "hooks in settings.json",
            false,
            format!("no bilro hook registered in {}. Run `bilro install`.", path.display()),
        );
    }
    let missing: Vec<&String> = commands
        .iter()
        .filter(|cmd| {
            let bin = cmd.split_whitespace().next().unwrap_or("");
            bin.is_empty() || !Path::new(bin).exists()
        })
        .collect();
    if missing.is_empty() {
        check("hooks in settings.json", true, format!("{} hook(s), all pointing to an existing binary", commands.len()))
    } else {
        check(
            "hooks in settings.json",
            false,
            format!(
                "hook(s) pointing to a binary that is gone: {missing:?}. Run `bilro install` again to fix the path."
            ),
        )
    }
}

fn check_mcp(home: &Path) -> Check {
    let path = claude_json_path(home);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return check("MCP server in .claude.json", false, format!("could not find {}. Run `bilro install`.", path.display()));
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return check("MCP server in .claude.json", false, format!("{} is not valid JSON.", path.display()));
    };
    let Some(cmd) = value
        .get("mcpServers")
        .and_then(|m| m.get("bilro"))
        .and_then(|b| b.get("command"))
        .and_then(|c| c.as_str())
    else {
        return check(
            "MCP server in .claude.json",
            false,
            "mcpServers.bilro is not registered or has no \"command\". Run `bilro install`.",
        );
    };
    if Path::new(cmd).exists() {
        check("MCP server in .claude.json", true, format!("points to {cmd}"))
    } else {
        check(
            "MCP server in .claude.json",
            false,
            format!("points to {cmd}, which no longer exists. Run `bilro install` again."),
        )
    }
}

fn check_db(name: &str, path: &Path, opener: impl Fn(&Path) -> rusqlite::Result<Connection>) -> Check {
    match opener(path) {
        Ok(_) => check(name, true, format!("opens at {}", path.display())),
        Err(e) => check(
            name,
            false,
            format!(
                "{} does not open ({e}). If the file is corrupted, rename it and let bilro recreate it.",
                path.display()
            ),
        ),
    }
}

fn check_fts5() -> Check {
    match Connection::open_in_memory() {
        Ok(conn) => match conn.execute_batch("CREATE VIRTUAL TABLE bilro_doctor_probe USING fts5(x);") {
            Ok(()) => check("FTS5 available", true, "CREATE VIRTUAL TABLE ... USING fts5 worked in memory"),
            Err(e) => check(
                "FTS5 available",
                false,
                format!(
                    "sqlite without FTS5 ({e}). Recompile with rusqlite's \"bundled\" feature (already the default for this project)."
                ),
            ),
        },
        Err(e) => check("FTS5 available", false, format!("could not open sqlite in memory: {e}")),
    }
}

fn check_scripts() -> Check {
    let missing: Vec<&'static str> = crate::run::script::languages()
        .into_iter()
        .filter_map(crate::run::script::runtime_for)
        .filter(|rt| !crate::run::script::available(rt))
        .map(|rt| rt.program)
        .collect();
    if missing.is_empty() {
        check(
            "script interpreters",
            true,
            format!("available: {}", crate::run::script::languages().join(", ")),
        )
    } else {
        check(
            "script interpreters",
            false,
            format!("missing: {}. Install them for these languages to work in `bilro run`.", missing.join(", ")),
        )
    }
}

fn check_curl() -> Check {
    match which("curl") {
        Some(p) => check("curl present", true, format!("{}", p.display())),
        None => check("curl present", false, "curl is not on PATH. Install curl, used by fetch."),
    }
}

fn check_disk_usage(home: &Path) -> Check {
    let learn = file_len(&learn_db_path(home));
    let index = file_len(&index_db_path(home));
    let journal = file_len(&journal_db_path(home));
    let sessions = total_bytes(&session_files(&sessions_dir_path(home)));
    let total = learn + index + journal + sessions;
    check(
        "database disk usage",
        true,
        format!(
            "total {total} bytes (learn.db: {learn}, index.db: {index}, journal.db: {journal}, sessions: {sessions}). Above tens of MB, run `bilro purge`."
        ),
    )
}

fn doctor_at(home: &Path) -> Vec<Check> {
    vec![
        check_binary_on_path(),
        check_hooks(home),
        check_mcp(home),
        check_db("learn.db opens", &learn_db_path(home), crate::compress::learn::open),
        check_db("index.db opens", &index_db_path(home), crate::store::sandbox::open),
        check_db("journal.db opens", &journal_db_path(home), crate::store::journal::open),
        check_fts5(),
        check_scripts(),
        check_curl(),
        check_disk_usage(home),
    ]
}

/// Every self-check bilro can run: installation, indexes, and the external
/// tools it shells out to.
pub fn doctor() -> Vec<Check> {
    doctor_at(&home())
}

/// What to erase. `All` clears every table bilro keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purge {
    Index,
    Journal,
    Learn,
    Sessions,
    All,
}

/// Rows and bytes one purge target held before (and whether it was actually
/// cleared, which only happens when the caller confirmed).
#[derive(Debug)]
pub struct TargetReport {
    pub name: String,
    pub rows: usize,
    pub bytes: u64,
    pub purged: bool,
}

#[derive(Debug)]
pub struct PurgeReport {
    pub confirmed: bool,
    pub targets: Vec<TargetReport>,
}

fn purge_index_at(home: &Path, confirmed: bool) -> TargetReport {
    let path = index_db_path(home);
    let bytes = file_len(&path);
    let db = path.exists().then(|| crate::store::sandbox::open(&path).ok()).flatten();
    let rows = db.as_ref().map(|d| count_table(d, "chunks")).unwrap_or(0);
    if confirmed {
        if let Some(d) = &db {
            let _ = d.execute("DELETE FROM chunks", []);
            let _ = d.execute_batch("VACUUM;");
        }
    }
    TargetReport { name: "index".to_string(), rows, bytes, purged: confirmed }
}

fn purge_journal_at(home: &Path, confirmed: bool) -> TargetReport {
    let path = journal_db_path(home);
    let bytes = file_len(&path);
    let db = path.exists().then(|| crate::store::journal::open(&path).ok()).flatten();
    let rows = db.as_ref().map(|d| count_table(d, "events")).unwrap_or(0);
    if confirmed {
        if let Some(d) = &db {
            let _ = d.execute("DELETE FROM events", []);
            let _ = d.execute_batch("VACUUM;");
        }
    }
    TargetReport { name: "journal".to_string(), rows, bytes, purged: confirmed }
}

fn purge_learn_at(home: &Path, confirmed: bool) -> TargetReport {
    let path = learn_db_path(home);
    let bytes = file_len(&path);
    let db = path.exists().then(|| crate::compress::learn::open(&path).ok()).flatten();
    let rows = db
        .as_ref()
        .map(|d| count_table(d, "runs") + count_table(d, "lines") + count_table(d, "last") + count_table(d, "exact"))
        .unwrap_or(0);
    if confirmed {
        if let Some(d) = &db {
            let _ = d.execute_batch("DELETE FROM runs; DELETE FROM lines; DELETE FROM last; DELETE FROM exact; VACUUM;");
        }
    }
    TargetReport { name: "learn".to_string(), rows, bytes, purged: confirmed }
}

fn purge_sessions_at(home: &Path, confirmed: bool) -> TargetReport {
    let files = session_files(&sessions_dir_path(home));
    let rows = files.len();
    let bytes = total_bytes(&files);
    if confirmed {
        for f in &files {
            let _ = std::fs::remove_file(f);
        }
    }
    TargetReport { name: "sessions".to_string(), rows, bytes, purged: confirmed }
}

fn purge_at(home: &Path, what: Purge, confirmed: bool) -> PurgeReport {
    let targets = match what {
        Purge::Index => vec![purge_index_at(home, confirmed)],
        Purge::Journal => vec![purge_journal_at(home, confirmed)],
        Purge::Learn => vec![purge_learn_at(home, confirmed)],
        Purge::Sessions => vec![purge_sessions_at(home, confirmed)],
        Purge::All => vec![
            purge_index_at(home, confirmed),
            purge_journal_at(home, confirmed),
            purge_learn_at(home, confirmed),
            purge_sessions_at(home, confirmed),
        ],
    };
    PurgeReport { confirmed, targets }
}

/// Deletes stored data for `what`. Without `confirmed` nothing is erased:
/// the report still says what would have been, in rows and bytes.
pub fn purge(what: Purge, confirmed: bool) -> PurgeReport {
    purge_at(&home(), what, confirmed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_home() -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir()
            .join(format!("bilro-ops-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    #[test]
    fn stats_in_empty_environment_does_not_break_and_returns_zeros() {
        let home = temp_home();
        let s = stats_at(&home);
        assert_eq!(s.learned_commands, 0);
        assert_eq!(s.learned_mature, 0);
        assert_eq!(s.denoise_savings_bytes, 0);
        assert_eq!(s.indexed_chunks, 0);
        assert_eq!(s.journal_events_total, 0);
        assert!(s.journal_events_by_kind.is_empty());
        assert_eq!(s.sessions, 0);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn stats_counts_learned_and_mature_commands() {
        let home = temp_home();
        let mut db = crate::compress::learn::open(&learn_db_path(&home)).unwrap();
        for _ in 0..5 {
            crate::compress::learn::observe(&mut db, "npm test", "line 1\nline 2").unwrap();
        }
        crate::compress::learn::observe(&mut db, "npm run build", "other output").unwrap();
        drop(db);

        let s = stats_at(&home);
        assert_eq!(s.learned_commands, 2, "expected 2 distinct signatures");
        assert_eq!(s.learned_mature, 1, "only npm test passed 3 runs");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn check_hooks_ignores_hook_from_another_tool_and_fails_only_bilros() {
        let home = temp_home();
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            settings_path(&home),
            serde_json::json!({
                "hooks": {
                    "PreToolUse": [
                        { "hooks": [{ "type": "command", "command": "node \"/does/not/exist/other-tool.mjs\"" }] },
                        { "hooks": [{ "type": "command", "command": "/does/not/exist/bilro hook bash" }] }
                    ]
                }
            })
            .to_string(),
        )
        .unwrap();

        let c = check_hooks(&home);
        assert!(!c.ok, "should fail because bilro is missing, not because of the other tool");
        assert!(c.detail.contains("bilro"), "detail: {}", c.detail);
        assert!(!c.detail.contains("other-tool"), "leaked a hook from another tool: {}", c.detail);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn doctor_returns_one_check_per_item_and_failures_have_detail() {
        let home = temp_home();
        let checks = doctor_at(&home);
        assert_eq!(checks.len(), 10);

        let failing: Vec<&Check> = checks.iter().filter(|c| !c.ok).collect();
        assert!(!failing.is_empty(), "empty home should fail at least one check");
        for c in &failing {
            assert!(!c.detail.is_empty(), "check {} failed without saying how to fix it", c.name);
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn purge_without_confirmed_does_not_delete_but_reports() {
        let home = temp_home();
        let mut db = crate::compress::learn::open(&learn_db_path(&home)).unwrap();
        crate::compress::learn::observe(&mut db, "cmd", "line").unwrap();
        drop(db);

        let r = purge_at(&home, Purge::Learn, false);
        assert!(!r.confirmed);
        assert!(!r.targets[0].purged);
        assert!(r.targets[0].rows > 0, "should report what it would delete");

        let db2 = crate::compress::learn::open(&learn_db_path(&home)).unwrap();
        assert_eq!(count_table(&db2, "runs"), 1, "purge without confirmed deleted anyway");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn purge_with_confirmed_deletes_only_the_requested_target() {
        let home = temp_home();
        let mut learn_db = crate::compress::learn::open(&learn_db_path(&home)).unwrap();
        crate::compress::learn::observe(&mut learn_db, "cmd", "line").unwrap();
        drop(learn_db);
        let idx = crate::store::sandbox::open(&index_db_path(&home)).unwrap();
        crate::store::sandbox::index(&idx, "l", "indexed body", "source").unwrap();
        drop(idx);

        let r = purge_at(&home, Purge::Learn, true);
        assert!(r.targets[0].purged);

        let learn_db2 = crate::compress::learn::open(&learn_db_path(&home)).unwrap();
        assert_eq!(count_table(&learn_db2, "runs"), 0, "learn was not cleared");
        drop(learn_db2);

        let idx2 = crate::store::sandbox::open(&index_db_path(&home)).unwrap();
        assert_eq!(count_table(&idx2, "chunks"), 1, "index was deleted without being requested");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn purge_all_clears_everything_and_stats_afterward_does_not_break() {
        let home = temp_home();
        let mut learn_db = crate::compress::learn::open(&learn_db_path(&home)).unwrap();
        crate::compress::learn::observe(&mut learn_db, "cmd", "line").unwrap();
        drop(learn_db);
        let idx = crate::store::sandbox::open(&index_db_path(&home)).unwrap();
        crate::store::sandbox::index(&idx, "l", "body", "source").unwrap();
        drop(idx);
        let jr = crate::store::journal::open(&journal_db_path(&home)).unwrap();
        crate::store::journal::record(&jr, "prompt", "subject", "body", "s1", "project").unwrap();
        drop(jr);
        std::fs::create_dir_all(sessions_dir_path(&home)).unwrap();
        std::fs::write(sessions_dir_path(&home).join("s1.json"), "{}").unwrap();

        let r = purge_at(&home, Purge::All, true);
        assert_eq!(r.targets.len(), 4);
        assert!(r.targets.iter().all(|t| t.purged));

        let s = stats_at(&home);
        assert_eq!(s.learned_commands, 0);
        assert_eq!(s.indexed_chunks, 0);
        assert_eq!(s.journal_events_total, 0);
        assert_eq!(s.sessions, 0);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    #[ignore = "manual diagnostic: runs against the real machine, not an isolated home"]
    fn real_machine_diagnostics() {
        println!("{:#?}", stats());
        for c in doctor() {
            println!("[{}] {} - {}", if c.ok { "ok" } else { "FAIL" }, c.name, c.detail);
        }
    }
}
